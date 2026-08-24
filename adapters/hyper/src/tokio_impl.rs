use std::{convert::Infallible, future::Future, io, pin::Pin, rc::Rc};

use hyper::{body::Incoming, rt::Executor, service::service_fn};
use hyper_util::{rt::TokioIo, server::conn::auto};
use serverkit::Router;

use crate::{
    Run,
    bridge::{HandledRequest, handle_request},
};

#[cfg(not(feature = "websocket"))]
use crate::bridge::into_hyper_response;

#[cfg(feature = "websocket")]
use futures_util::{Sink, Stream};
#[cfg(feature = "websocket")]
use hyper::{Response as HyperResponse, header::HeaderValue};
#[cfg(feature = "websocket")]
use serverkit::{
    ResponseBody, WebSocket, WebSocketError, WebSocketMessage,
    adapter::{WebSocketIo, WebSocketPlan},
};
#[cfg(feature = "websocket")]
use std::task::{Context, Poll};
#[cfg(feature = "websocket")]
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{
        Message,
        handshake::derive_accept_key,
        protocol::{CloseFrame, Role, frame::coding::CloseCode},
    },
};

#[cfg(feature = "websocket")]
use crate::{
    body::HyperResponseBody,
    bridge::{append_headers, into_hyper_response_parts},
};

impl Run<tokio::net::TcpListener> for Router {
    type Output = Pin<Box<dyn Future<Output = io::Result<()>>>>;

    fn run(self, listener: tokio::net::TcpListener) -> Self::Output {
        Box::pin(serve(self, listener))
    }
}

async fn serve(router: Router, listener: tokio::net::TcpListener) -> io::Result<()> {
    tokio::task::LocalSet::new()
        .run_until(accept_connections(router, listener))
        .await
}

async fn accept_connections(router: Router, listener: tokio::net::TcpListener) -> io::Result<()> {
    let router = Rc::new(router);

    loop {
        let (connection, address) = listener.accept().await?;
        let router = Rc::clone(&router);

        tokio::task::spawn_local(async move {
            serve_connection(router, connection, address).await;
        });
    }
}

pub(crate) async fn serve_connection(
    router: Rc<Router>,
    connection: tokio::net::TcpStream,
    address: std::net::SocketAddr,
) {
    let service = service_fn(move |request: hyper::Request<Incoming>| {
        let router = Rc::clone(&router);

        async move {
            let handled = handle_request(router.as_ref(), request, address).await;
            Ok::<_, Infallible>(into_tokio_response(handled))
        }
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
pub(crate) struct LocalExecutor;

impl<F: Future<Output = ()> + 'static> Executor<F> for LocalExecutor {
    fn execute(&self, future: F) {
        tokio::task::spawn_local(future);
    }
}

#[cfg(not(feature = "websocket"))]
fn into_tokio_response(handled: HandledRequest) -> hyper::Response<crate::body::HyperResponseBody> {
    into_hyper_response(handled.response)
}

#[cfg(feature = "websocket")]
fn into_tokio_response(handled: HandledRequest) -> HyperResponse<HyperResponseBody> {
    let (status, headers, body) = handled.response.into_parts();

    match body {
        ResponseBody::WebSocket(plan) => {
            into_hyper_websocket_response(headers, plan, handled.on_upgrade)
        }
        body => into_hyper_response_parts(status, headers, body),
    }
}

#[cfg(feature = "websocket")]
fn into_hyper_websocket_response(
    headers: serverkit::Headers,
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
        plan.run(WebSocket::from_io(TokioWebSocket { stream }))
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
struct TokioWebSocket {
    stream: WebSocketStream<TokioIo<hyper::upgrade::Upgraded>>,
}

#[cfg(feature = "websocket")]
impl WebSocketIo for TokioWebSocket {
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
    use std::{
        rc::Rc,
        task::{Context, Poll},
    };

    use http_body_util::{BodyExt, Empty};
    use hyper::{Request, body::Bytes, client::conn::http2};
    use hyper_util::rt::TokioIo;
    use serverkit::{Chunk, Config, Response, ResponseStream, RouteMethods, Router, StreamError};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::{LocalExecutor, serve_connection};

    struct LargeStream {
        sent: bool,
    }

    impl ResponseStream for LargeStream {
        fn poll_next(
            &mut self,
            _context: &mut Context<'_>,
        ) -> Poll<Option<Result<Chunk, StreamError>>> {
            if self.sent {
                Poll::Ready(None)
            } else {
                self.sent = true;
                Poll::Ready(Some(Ok(Chunk::from(vec![7; 1024 * 1024]))))
            }
        }
    }

    fn router() -> Router {
        Router::new(
            Config::new(),
            (
                "/health".GET(|| async { "ok" }),
                "/stream".GET(|| async { Response::stream(200, LargeStream { sent: false }) }),
            ),
        )
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .build()
            .unwrap()
    }

    fn request_http_1(version: &str) -> String {
        let runtime = runtime();

        tokio::task::LocalSet::new().block_on(&runtime, async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
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
        let runtime = runtime();

        tokio::task::LocalSet::new().block_on(&runtime, async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
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

            let request = Request::builder()
                .uri("http://localhost/stream")
                .body(Empty::<Bytes>::new())
                .unwrap();
            let response = sender.send_request(request).await.unwrap();
            let body = response.into_body().collect().await.unwrap().to_bytes();

            assert_eq!(body.len(), 1024 * 1024);
            assert!(body.iter().all(|byte| *byte == 7));
        });
    }
}
