use std::{
    convert::Infallible,
    io,
    net::TcpListener,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll},
};

use hyper::{
    Request as HyperRequest, Response as HyperResponse,
    body::{Body as HyperBody, Bytes, Frame, Incoming, SizeHint},
    header::{CONTENT_LENGTH, HeaderName, HeaderValue},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;

#[cfg(feature = "websocket")]
use futures_util::{Sink, Stream};
#[cfg(feature = "websocket")]
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{
        Message,
        handshake::derive_accept_key,
        protocol::{CloseFrame, Role, frame::coding::CloseCode},
    },
};

use crate::{
    Headers, Listener, Method, Request, RequestStream, Response, ResponseBody, Router, StreamError,
};

#[cfg(feature = "websocket")]
use crate::{
    WebSocket, WebSocketError, WebSocketMessage,
    websocket::{NativeUpgrade, WebSocketIo, WebSocketPlan},
};

impl Listener for TcpListener {
    type Output = io::Result<()>;

    fn serve(self, router: Router) -> Self::Output {
        serve(router, self)
    }
}

fn serve(router: Router, listener: TcpListener) -> io::Result<()> {
    listener.set_nonblocking(true)?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .build()?;
    let tasks = tokio::task::LocalSet::new();

    tasks.block_on(&runtime, serve_connections(router, listener))
}

async fn serve_connections(router: Router, listener: TcpListener) -> io::Result<()> {
    let listener = tokio::net::TcpListener::from_std(listener)?;
    let router = Rc::new(router);

    loop {
        let (connection, address) = listener.accept().await?;
        let router = Rc::clone(&router);

        tokio::task::spawn_local(async move {
            serve_connection(router, connection, address).await;
        });
    }
}

async fn serve_connection(
    router: Rc<Router>,
    connection: tokio::net::TcpStream,
    address: std::net::SocketAddr,
) {
    let service = service_fn(move |request| {
        let router = Rc::clone(&router);

        async move { Ok::<_, Infallible>(handle_request(router, request, address).await) }
    });

    let _result = http1::Builder::new()
        .serve_connection(TokioIo::new(connection), service)
        .await;
}

async fn handle_request(
    router: Rc<Router>,
    request: HyperRequest<Incoming>,
    address: std::net::SocketAddr,
) -> HyperResponse<NativeResponseBody> {
    #[cfg(feature = "websocket")]
    let mut request = request;
    #[cfg(feature = "websocket")]
    let native_upgrade = NativeUpgrade::new(hyper::upgrade::on(&mut request));
    let (parts, body) = request.into_parts();
    let mut headers = Headers::new();

    for (name, value) in &parts.headers {
        headers.append_unchecked(name.as_str(), value.as_bytes());
    }

    let mut request = Request::new(
        Method::new(parts.method.as_str()),
        parts.uri.path(),
        parts.uri.query().map(str::to_owned),
        headers,
        Box::new(NativeRequestStream::new(body)),
    );
    request.insert_extension(address);
    #[cfg(feature = "websocket")]
    request.insert_extension(native_upgrade);
    let response = router.handle(request).await;

    into_native_response(response)
}

struct NativeRequestStream {
    body: Pin<Box<Incoming>>,
}

impl NativeRequestStream {
    fn new(body: Incoming) -> Self {
        Self {
            body: Box::pin(body),
        }
    }
}

impl RequestStream for NativeRequestStream {
    fn poll_next(
        &mut self,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Vec<u8>, StreamError>>> {
        loop {
            match self.body.as_mut().poll_frame(context) {
                Poll::Ready(Some(Ok(frame))) => match frame.into_data() {
                    Ok(data) => return Poll::Ready(Some(Ok(data.to_vec()))),
                    Err(_) => continue,
                },
                Poll::Ready(Some(Err(error))) => {
                    return Poll::Ready(Some(Err(StreamError::new(error.to_string()))));
                }
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

struct NativeResponseBody {
    body: ResponseBody,
}

impl NativeResponseBody {
    fn new(body: ResponseBody) -> Self {
        Self { body }
    }
}

impl HyperBody for NativeResponseBody {
    type Data = Bytes;
    type Error = StreamError;

    fn poll_frame(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        match &mut self.get_mut().body {
            ResponseBody::Buffered(bytes) if bytes.is_empty() => Poll::Ready(None),
            ResponseBody::Buffered(bytes) => {
                let bytes = Bytes::from(std::mem::take(bytes));
                Poll::Ready(Some(Ok(Frame::data(bytes))))
            }
            ResponseBody::Streaming(stream) => stream
                .poll_next(context)
                .map(|next| next.map(|chunk| chunk.map(Bytes::from).map(Frame::data))),
            #[cfg(feature = "websocket")]
            ResponseBody::WebSocket(_) => Poll::Ready(None),
        }
    }

    fn is_end_stream(&self) -> bool {
        if matches!(&self.body, ResponseBody::Buffered(bytes) if bytes.is_empty()) {
            return true;
        }

        #[cfg(feature = "websocket")]
        if matches!(&self.body, ResponseBody::WebSocket(_)) {
            return true;
        }

        false
    }

    fn size_hint(&self) -> SizeHint {
        let mut hint = SizeHint::new();

        if let ResponseBody::Buffered(bytes) = &self.body {
            hint.set_exact(bytes.len() as u64);
        }

        hint
    }
}

fn into_native_response(response: Response) -> HyperResponse<NativeResponseBody> {
    let (status, headers, body) = response.into_parts();

    #[cfg(feature = "websocket")]
    let body = match body {
        ResponseBody::WebSocket(plan) => {
            return into_native_websocket_response(headers, plan);
        }
        body => body,
    };

    let length = body.buffered().map(<[u8]>::len);
    let has_content_length = headers.contains("content-length");
    let mut response = HyperResponse::new(NativeResponseBody::new(body));

    match hyper::StatusCode::from_u16(status) {
        Ok(status) => *response.status_mut() = status,
        Err(_) => *response.status_mut() = hyper::StatusCode::INTERNAL_SERVER_ERROR,
    }

    for (name, value) in headers.iter() {
        let name = HeaderName::from_bytes(name.as_bytes())
            .expect("ServerKit generated an invalid response header name");
        let value = HeaderValue::from_bytes(value)
            .expect("ServerKit generated an invalid response header value");

        response.headers_mut().append(name, value);
    }

    if !has_content_length && let Some(length) = length {
        response.headers_mut().insert(CONTENT_LENGTH, length.into());
    }

    response
}

#[cfg(feature = "websocket")]
fn into_native_websocket_response(
    headers: Headers,
    plan: WebSocketPlan,
) -> HyperResponse<NativeResponseBody> {
    let Some(on_upgrade) = plan.take_native_upgrade() else {
        return into_native_response(Response::error(
            500,
            "WebSocket runtime upgrade is unavailable",
        ));
    };
    let accept_key = derive_accept_key(plan.key().as_bytes());

    tokio::task::spawn_local(async move {
        let Ok(upgraded) = on_upgrade.await else {
            return;
        };
        let stream =
            WebSocketStream::from_raw_socket(TokioIo::new(upgraded), Role::Server, None).await;
        plan.run(WebSocket::new(NativeWebSocket { stream })).await;
    });

    let mut response =
        HyperResponse::new(NativeResponseBody::new(ResponseBody::Buffered(Vec::new())));
    *response.status_mut() = hyper::StatusCode::SWITCHING_PROTOCOLS;
    response
        .headers_mut()
        .insert("connection", HeaderValue::from_static("Upgrade"));
    response
        .headers_mut()
        .insert("upgrade", HeaderValue::from_static("websocket"));
    response.headers_mut().insert(
        "sec-websocket-accept",
        HeaderValue::from_str(&accept_key)
            .expect("a derived WebSocket accept key is a valid header value"),
    );

    for (name, value) in headers.iter() {
        let name = HeaderName::from_bytes(name.as_bytes())
            .expect("ServerKit generated an invalid response header name");
        let value = HeaderValue::from_bytes(value)
            .expect("ServerKit generated an invalid response header value");
        response.headers_mut().append(name, value);
    }

    response
}

#[cfg(feature = "websocket")]
struct NativeWebSocket {
    stream: WebSocketStream<TokioIo<hyper::upgrade::Upgraded>>,
}

#[cfg(feature = "websocket")]
impl WebSocketIo for NativeWebSocket {
    fn poll_next(
        &mut self,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<WebSocketMessage, WebSocketError>>> {
        loop {
            let next = match Stream::poll_next(Pin::new(&mut self.stream), context) {
                Poll::Ready(next) => next,
                Poll::Pending => return Poll::Pending,
            };

            return Poll::Ready(match next {
                Some(Ok(Message::Text(text))) => Some(Ok(WebSocketMessage::Text(text.to_string()))),
                Some(Ok(Message::Binary(bytes))) => {
                    Some(Ok(WebSocketMessage::Binary(bytes.to_vec())))
                }
                Some(Ok(Message::Ping(bytes))) => Some(Ok(WebSocketMessage::Ping(bytes.to_vec()))),
                Some(Ok(Message::Pong(bytes))) => Some(Ok(WebSocketMessage::Pong(bytes.to_vec()))),
                Some(Ok(Message::Close(frame))) => Some(Ok(WebSocketMessage::Close {
                    code: frame.as_ref().map(|frame| u16::from(frame.code)),
                    reason: frame.map_or_else(String::new, |frame| frame.reason.to_string()),
                })),
                Some(Ok(Message::Frame(_))) => continue,
                Some(Err(error)) => Some(Err(WebSocketError::new(error.to_string()))),
                None => None,
            });
        }
    }

    fn poll_ready(&mut self, context: &mut Context<'_>) -> Poll<Result<(), WebSocketError>> {
        Sink::poll_ready(Pin::new(&mut self.stream), context)
            .map_err(|error| WebSocketError::new(error.to_string()))
    }

    fn start_send(&mut self, message: WebSocketMessage) -> Result<(), WebSocketError> {
        Sink::start_send(Pin::new(&mut self.stream), into_native_message(message))
            .map_err(|error| WebSocketError::new(error.to_string()))
    }

    fn poll_flush(&mut self, context: &mut Context<'_>) -> Poll<Result<(), WebSocketError>> {
        Sink::poll_flush(Pin::new(&mut self.stream), context)
            .map_err(|error| WebSocketError::new(error.to_string()))
    }

    fn poll_close(&mut self, context: &mut Context<'_>) -> Poll<Result<(), WebSocketError>> {
        Sink::poll_close(Pin::new(&mut self.stream), context)
            .map_err(|error| WebSocketError::new(error.to_string()))
    }
}

#[cfg(feature = "websocket")]
fn into_native_message(message: WebSocketMessage) -> Message {
    match message {
        WebSocketMessage::Text(text) => Message::Text(text.into()),
        WebSocketMessage::Binary(bytes) => Message::Binary(bytes.into()),
        WebSocketMessage::Ping(bytes) => Message::Ping(bytes.into()),
        WebSocketMessage::Pong(bytes) => Message::Pong(bytes.into()),
        WebSocketMessage::Close { code, reason } => Message::Close(code.map(|code| CloseFrame {
            code: CloseCode::from(code),
            reason: reason.into(),
        })),
    }
}
