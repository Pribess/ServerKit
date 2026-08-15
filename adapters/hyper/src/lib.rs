#![forbid(unsafe_code)]

use std::{
    convert::Infallible,
    future::Future,
    io,
    net::{TcpListener, ToSocketAddrs},
    pin::Pin,
    rc::Rc,
    task::{Context, Poll},
};

use hyper::{
    Request as HyperRequest, Response as HyperResponse,
    body::{Body as HyperBody, Bytes, Frame, Incoming, SizeHint},
    header::{CONTENT_LENGTH, HeaderName, HeaderValue},
    rt::Executor,
    service::service_fn,
};
use hyper_util::{rt::TokioIo, server::conn::auto};

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

use serverkit::{
    Headers, Listener, Method, Request, RequestStream, Response, ResponseBody, Router, StreamError,
};

#[cfg(feature = "websocket")]
use serverkit::{
    WebSocket, WebSocketError, WebSocketMessage,
    adapter::{WebSocketIo, WebSocketPlan},
};

pub struct Http {
    listener: TcpListener,
}

impl Http {
    pub fn bind(address: impl ToSocketAddrs) -> io::Result<Self> {
        TcpListener::bind(address).map(Self::from_listener)
    }

    pub fn from_listener(listener: TcpListener) -> Self {
        Self { listener }
    }
}

impl Listener for Http {
    type Output = io::Result<()>;

    fn serve(self, router: Router) -> Self::Output {
        serve(router, self.listener)
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

    let builder = auto::Builder::new(LocalExecutor);

    #[cfg(feature = "websocket")]
    let _result = builder
        .serve_connection_with_upgrades(TokioIo::new(connection), service)
        .await;

    #[cfg(not(feature = "websocket"))]
    let _result = builder
        .serve_connection(TokioIo::new(connection), service)
        .await;
}

#[derive(Clone, Copy)]
struct LocalExecutor;

impl<F: Future<Output = ()> + 'static> Executor<F> for LocalExecutor {
    fn execute(&self, future: F) {
        tokio::task::spawn_local(future);
    }
}

async fn handle_request(
    router: Rc<Router>,
    request: HyperRequest<Incoming>,
    address: std::net::SocketAddr,
) -> HyperResponse<HyperResponseBody> {
    #[cfg(feature = "websocket")]
    let mut request = request;
    #[cfg(feature = "websocket")]
    let on_upgrade = hyper::upgrade::on(&mut request);
    let (parts, body) = request.into_parts();
    let mut headers = Headers::new();

    for (name, value) in &parts.headers {
        headers
            .append(name.as_str(), value.as_bytes())
            .expect("Hyper supplied an invalid request header");
    }

    let mut request = Request::from_parts(
        Method::new(parts.method.as_str()),
        parts.uri.path(),
        parts.uri.query().map(str::to_owned),
        headers,
        Box::new(HyperRequestStream::new(body)),
    );
    request.insert_extension(address);
    let response = router.handle(request).await;

    into_hyper_response(
        response,
        #[cfg(feature = "websocket")]
        on_upgrade,
    )
}

struct HyperRequestStream {
    body: Pin<Box<Incoming>>,
    current: Option<Bytes>,
}

impl HyperRequestStream {
    fn new(body: Incoming) -> Self {
        Self {
            body: Box::pin(body),
            current: None,
        }
    }
}

impl RequestStream for HyperRequestStream {
    fn poll_next(&mut self, context: &mut Context<'_>) -> Poll<Option<Result<(), StreamError>>> {
        loop {
            match self.body.as_mut().poll_frame(context) {
                Poll::Ready(Some(Ok(frame))) => match frame.into_data() {
                    Ok(data) => {
                        self.current = Some(data);
                        return Poll::Ready(Some(Ok(())));
                    }
                    Err(_) => continue,
                },
                Poll::Ready(Some(Err(error))) => {
                    self.current = None;
                    return Poll::Ready(Some(Err(StreamError::new(error.to_string()))));
                }
                Poll::Ready(None) => {
                    self.current = None;
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }

    fn chunk(&self) -> &[u8] {
        self.current.as_deref().unwrap_or_default()
    }
}

struct HyperResponseBody {
    body: ResponseBody,
}

impl HyperResponseBody {
    fn new(body: ResponseBody) -> Self {
        Self { body }
    }
}

impl HyperBody for HyperResponseBody {
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

fn into_hyper_response(
    response: Response,
    #[cfg(feature = "websocket")] on_upgrade: hyper::upgrade::OnUpgrade,
) -> HyperResponse<HyperResponseBody> {
    let (status, headers, body) = response.into_parts();

    #[cfg(feature = "websocket")]
    let body = match body {
        ResponseBody::WebSocket(plan) => {
            return into_hyper_websocket_response(headers, plan, on_upgrade);
        }
        body => body,
    };

    let length = body.buffered().map(<[u8]>::len);
    let has_content_length = headers.contains("content-length");
    let mut response = HyperResponse::new(HyperResponseBody::new(body));

    match hyper::StatusCode::from_u16(status) {
        Ok(status) => *response.status_mut() = status,
        Err(_) => *response.status_mut() = hyper::StatusCode::INTERNAL_SERVER_ERROR,
    }

    append_headers(&mut response, headers);

    if !has_content_length && let Some(length) = length {
        response.headers_mut().insert(CONTENT_LENGTH, length.into());
    }

    response
}

fn append_headers(response: &mut HyperResponse<HyperResponseBody>, headers: Headers) {
    for (name, value) in headers.iter() {
        let name = HeaderName::from_bytes(name.as_bytes())
            .expect("ServerKit generated an invalid response header name");
        let value = HeaderValue::from_bytes(value)
            .expect("ServerKit generated an invalid response header value");

        response.headers_mut().append(name, value);
    }
}

#[cfg(feature = "websocket")]
fn into_hyper_websocket_response(
    headers: Headers,
    plan: WebSocketPlan,
    on_upgrade: hyper::upgrade::OnUpgrade,
) -> HyperResponse<HyperResponseBody> {
    let accept_key = derive_accept_key(plan.key().as_bytes());

    tokio::task::spawn_local(async move {
        let Ok(upgraded) = on_upgrade.await else {
            return;
        };
        let stream =
            WebSocketStream::from_raw_socket(TokioIo::new(upgraded), Role::Server, None).await;
        plan.run(WebSocket::from_io(HyperWebSocket { stream }))
            .await;
    });

    let mut response =
        HyperResponse::new(HyperResponseBody::new(ResponseBody::Buffered(Vec::new())));
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
    append_headers(&mut response, headers);

    response
}

#[cfg(feature = "websocket")]
struct HyperWebSocket {
    stream: WebSocketStream<TokioIo<hyper::upgrade::Upgraded>>,
}

#[cfg(feature = "websocket")]
impl WebSocketIo for HyperWebSocket {
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
        Sink::start_send(Pin::new(&mut self.stream), into_hyper_message(message))
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
fn into_hyper_message(message: WebSocketMessage) -> Message {
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

#[cfg(test)]
mod tests {
    use std::{net::TcpListener, rc::Rc};

    use http_body_util::Empty;
    use hyper::{Request, body::Bytes, client::conn::http2};
    use hyper_util::rt::TokioIo;
    use serverkit::{Config, RouteMethods, Router};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::{LocalExecutor, serve_connection};

    fn router() -> Router {
        Router::new(Config::new(), "/health".GET(|| async { "ok" }))
    }

    fn listener() -> (TcpListener, std::net::SocketAddr) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        (listener, address)
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .build()
            .unwrap()
    }

    fn request_http_1(version: &str) -> String {
        let (listener, address) = listener();
        let runtime = runtime();

        tokio::task::LocalSet::new().block_on(&runtime, async move {
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            let server = tokio::task::spawn_local(async move {
                let (connection, peer) = listener.accept().await.unwrap();
                serve_connection(Rc::new(router()), connection, peer).await;
            });
            let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
            let request =
                format!("GET /health {version}\r\nHost: localhost\r\nConnection: close\r\n\r\n");
            client.write_all(request.as_bytes()).await.unwrap();
            let mut response = Vec::new();
            client.read_to_end(&mut response).await.unwrap();
            server.await.unwrap();

            String::from_utf8(response).unwrap()
        })
    }

    #[test]
    fn serves_http_1_0_and_http_1_1() {
        assert!(request_http_1("HTTP/1.0").starts_with("HTTP/1.0 200 OK"));
        assert!(request_http_1("HTTP/1.1").starts_with("HTTP/1.1 200 OK"));
    }

    #[test]
    fn serves_http_2() {
        let (listener, address) = listener();
        let runtime = runtime();

        tokio::task::LocalSet::new().block_on(&runtime, async move {
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            tokio::task::spawn_local(async move {
                let (connection, peer) = listener.accept().await.unwrap();
                serve_connection(Rc::new(router()), connection, peer).await;
            });
            let client = tokio::net::TcpStream::connect(address).await.unwrap();
            let (mut sender, connection) = http2::Builder::new(LocalExecutor)
                .handshake(TokioIo::new(client))
                .await
                .unwrap();
            tokio::task::spawn_local(async move {
                connection.await.unwrap();
            });
            let request = Request::builder()
                .uri("http://localhost/health")
                .body(Empty::<Bytes>::new())
                .unwrap();
            let response = sender.send_request(request).await.unwrap();

            assert_eq!(response.version(), hyper::Version::HTTP_2);
            assert_eq!(response.status(), hyper::StatusCode::OK);
        });
    }
}
