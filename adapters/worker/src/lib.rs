#![forbid(unsafe_code)]
#![cfg(target_arch = "wasm32")]

use std::{
    future::Future,
    pin::Pin,
    rc::Rc,
    task::{Context as TaskContext, Poll},
};

#[cfg(feature = "websocket")]
use std::{cell::RefCell, collections::VecDeque, task::Waker};

use futures_core::Stream;
use worker::{Cf, Context, Env};

use serverkit::{
    FromRequest, Headers, IntoResponse, Method, Request, RequestStream, Response, ResponseBody,
    ResponseStream, StreamError,
};

#[cfg(feature = "websocket")]
use serverkit::{
    WebSocket, WebSocketError, WebSocketMessage,
    adapter::{WebSocketIo, WebSocketPlan},
};

struct WorkerContextInner {
    env: Env,
    context: Context,
    cf: Option<Cf>,
}

#[derive(Clone)]
pub struct WorkerContext {
    inner: Rc<WorkerContextInner>,
}

#[derive(Debug)]
pub struct WorkerContextError;

impl IntoResponse for WorkerContextError {
    fn into_response(self) -> Response {
        Response::text(500, "Cloudflare worker context is unavailable")
    }
}

impl WorkerContext {
    fn new(env: Env, context: Context, cf: Option<Cf>) -> Self {
        Self {
            inner: Rc::new(WorkerContextInner { env, context, cf }),
        }
    }

    pub fn env(&self) -> &Env {
        &self.inner.env
    }

    pub fn context(&self) -> &Context {
        &self.inner.context
    }

    pub fn cf(&self) -> Option<&Cf> {
        self.inner.cf.as_ref()
    }

    pub fn wait_until<F: Future<Output = ()> + 'static>(&self, future: F) {
        self.inner.context.wait_until(future);
    }
}

impl<'request> FromRequest<(&'request Request, &'request [u8])> for WorkerContext {
    type Error = WorkerContextError;

    async fn from_request(input: (&'request Request, &'request [u8])) -> Result<Self, Self::Error> {
        input
            .0
            .extension::<Self>()
            .cloned()
            .ok_or(WorkerContextError)
    }
}

struct WorkerRequestStream {
    stream: worker::ByteStream,
    current: Vec<u8>,
}

impl WorkerRequestStream {
    fn new(stream: worker::ByteStream) -> Self {
        Self {
            stream,
            current: Vec::new(),
        }
    }
}

impl RequestStream for WorkerRequestStream {
    fn poll_next(
        &mut self,
        context: &mut TaskContext<'_>,
    ) -> Poll<Option<Result<(), StreamError>>> {
        match Pin::new(&mut self.stream).poll_next(context) {
            Poll::Ready(Some(Ok(chunk))) => {
                self.current = chunk;
                Poll::Ready(Some(Ok(())))
            }
            Poll::Ready(Some(Err(error))) => {
                self.current.clear();
                Poll::Ready(Some(Err(StreamError::new(error.to_string()))))
            }
            Poll::Ready(None) => {
                self.current.clear();
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn chunk(&self) -> &[u8] {
        &self.current
    }
}

struct EmptyRequestStream;

impl RequestStream for EmptyRequestStream {
    fn poll_next(
        &mut self,
        _context: &mut TaskContext<'_>,
    ) -> Poll<Option<Result<(), StreamError>>> {
        Poll::Ready(None)
    }

    fn chunk(&self) -> &[u8] {
        &[]
    }
}

pub fn from_request(
    mut source: worker::Request,
    env: Env,
    context: Context,
) -> worker::Result<Request> {
    let method = Method::new(source.method().to_string());
    let path = source.path();
    let query = source.url()?.query().map(str::to_owned);
    let cf = source.cf().cloned();
    let mut headers = Headers::new();

    for (name, value) in source.headers().entries() {
        headers
            .append(name, value)
            .expect("Workers supplied an invalid request header");
    }

    let body: Box<dyn RequestStream> = if source.inner().body().is_some() {
        Box::new(WorkerRequestStream::new(source.stream()?))
    } else {
        Box::new(EmptyRequestStream)
    };

    let mut request = Request::from_parts(method, path, query, headers, body);
    request.insert_extension(WorkerContext::new(env, context, cf));

    Ok(request)
}

pub fn into_response(response: Response) -> worker::Result<worker::Response> {
    let (status, headers, body) = response.into_parts();
    let mut response = match body {
        ResponseBody::Buffered(body) if body.is_empty() => worker::Response::empty()?,
        ResponseBody::Buffered(body) => worker::Response::from_bytes(body)?,
        ResponseBody::Streaming(stream) => {
            worker::Response::from_stream(WorkerResponseStream { stream })?
        }
        #[cfg(feature = "websocket")]
        ResponseBody::WebSocket(plan) => worker_websocket_response(plan)?,
    }
    .with_status(status);

    for (name, value) in headers.iter() {
        let value = std::str::from_utf8(value)
            .map_err(|error| worker::Error::RustError(error.to_string()))?;

        response.headers_mut().append(name, value)?;
    }

    Ok(response)
}

#[cfg(feature = "websocket")]
fn worker_websocket_response(plan: WebSocketPlan) -> worker::Result<worker::Response> {
    let pair = worker::WebSocketPair::new()?;
    let server = pair.server;
    server.accept()?;
    let queue = Rc::new(RefCell::new(WorkerWebSocketQueue::default()));
    let event_queue = Rc::clone(&queue);
    let event_socket = server.clone();

    worker::wasm_bindgen_futures::spawn_local(async move {
        let mut events = match event_socket.events() {
            Ok(events) => events,
            Err(error) => {
                push_worker_event(&event_queue, Err(WebSocketError::new(error.to_string())));
                close_worker_events(&event_queue);
                return;
            }
        };

        while let Some(event) =
            std::future::poll_fn(|context| Pin::new(&mut events).poll_next(context)).await
        {
            let event = match event {
                Ok(worker::WebsocketEvent::Message(message)) => {
                    if let Some(text) = message.text() {
                        Ok(WebSocketMessage::Text(text))
                    } else if let Some(bytes) = message.bytes() {
                        Ok(WebSocketMessage::Binary(bytes))
                    } else {
                        Err(WebSocketError::new("unsupported Worker WebSocket message"))
                    }
                }
                Ok(worker::WebsocketEvent::Close(close)) => Ok(WebSocketMessage::Close {
                    code: Some(close.code()),
                    reason: close.reason(),
                }),
                Err(error) => Err(WebSocketError::new(error.to_string())),
            };
            let closed = matches!(event, Ok(WebSocketMessage::Close { .. }));
            push_worker_event(&event_queue, event);

            if closed {
                break;
            }
        }

        close_worker_events(&event_queue);
    });

    let socket = WebSocket::from_io(WorkerWebSocket { server, queue });
    worker::wasm_bindgen_futures::spawn_local(plan.run(socket));

    worker::Response::from_websocket(pair.client)
}

#[cfg(feature = "websocket")]
#[derive(Default)]
struct WorkerWebSocketQueue {
    events: VecDeque<Result<WebSocketMessage, WebSocketError>>,
    waker: Option<Waker>,
    closed: bool,
}

#[cfg(feature = "websocket")]
fn push_worker_event(
    queue: &Rc<RefCell<WorkerWebSocketQueue>>,
    event: Result<WebSocketMessage, WebSocketError>,
) {
    let mut queue = queue.borrow_mut();
    queue.events.push_back(event);

    if let Some(waker) = queue.waker.take() {
        waker.wake();
    }
}

#[cfg(feature = "websocket")]
fn close_worker_events(queue: &Rc<RefCell<WorkerWebSocketQueue>>) {
    let mut queue = queue.borrow_mut();
    queue.closed = true;

    if let Some(waker) = queue.waker.take() {
        waker.wake();
    }
}

#[cfg(feature = "websocket")]
struct WorkerWebSocket {
    server: worker::WebSocket,
    queue: Rc<RefCell<WorkerWebSocketQueue>>,
}

#[cfg(feature = "websocket")]
impl WebSocketIo for WorkerWebSocket {
    fn poll_next(
        &mut self,
        context: &mut TaskContext<'_>,
    ) -> Poll<Option<Result<WebSocketMessage, WebSocketError>>> {
        let mut queue = self.queue.borrow_mut();

        if let Some(event) = queue.events.pop_front() {
            return Poll::Ready(Some(event));
        }

        if queue.closed {
            return Poll::Ready(None);
        }

        queue.waker = Some(context.waker().clone());
        Poll::Pending
    }

    fn poll_ready(&mut self, _context: &mut TaskContext<'_>) -> Poll<Result<(), WebSocketError>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(&mut self, message: WebSocketMessage) -> Result<(), WebSocketError> {
        let result = match message {
            WebSocketMessage::Text(text) => self.server.send_with_str(text),
            WebSocketMessage::Binary(bytes) => self.server.send_with_bytes(bytes),
            WebSocketMessage::Close { code, reason } => {
                self.server.close(code, Some(reason.as_str()))
            }
            WebSocketMessage::Ping(_) | WebSocketMessage::Pong(_) => {
                return Err(WebSocketError::new(
                    "Workers manages WebSocket ping and pong frames",
                ));
            }
        };

        result.map_err(|error| WebSocketError::new(error.to_string()))
    }

    fn poll_flush(&mut self, _context: &mut TaskContext<'_>) -> Poll<Result<(), WebSocketError>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(&mut self, _context: &mut TaskContext<'_>) -> Poll<Result<(), WebSocketError>> {
        Poll::Ready(Ok(()))
    }
}

struct WorkerResponseStream {
    stream: Box<dyn ResponseStream>,
}

impl Stream for WorkerResponseStream {
    type Item = worker::Result<Vec<u8>>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        context: &mut TaskContext<'_>,
    ) -> Poll<Option<Self::Item>> {
        match self.stream.poll_next(context) {
            Poll::Ready(Some(Ok(()))) => Poll::Ready(Some(Ok(self.stream.chunk().to_vec()))),
            Poll::Ready(Some(Err(error))) => {
                Poll::Ready(Some(Err(worker::Error::RustError(error.to_string()))))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}
