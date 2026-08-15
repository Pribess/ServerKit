use std::{
    fmt,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use crate::{FromRequest, IntoResponse, Request, Response, openapi::Operation};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebSocketMessage {
    Text(String),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    Close { code: Option<u16>, reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSocketError {
    message: String,
}

impl WebSocketError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for WebSocketError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for WebSocketError {}

impl IntoResponse for WebSocketError {
    fn into_response(self) -> Response {
        Response::error(500, self.message)
    }
}

pub struct WebSocket {
    inner: Box<dyn WebSocketIo>,
}

impl WebSocket {
    #[doc(hidden)]
    pub fn from_io(inner: impl WebSocketIo + 'static) -> Self {
        Self {
            inner: Box::new(inner),
        }
    }

    pub async fn next(&mut self) -> Option<Result<WebSocketMessage, WebSocketError>> {
        std::future::poll_fn(|context| self.inner.poll_next(context)).await
    }

    pub async fn send(&mut self, message: WebSocketMessage) -> Result<(), WebSocketError> {
        std::future::poll_fn(|context| self.inner.poll_ready(context)).await?;
        self.inner.start_send(message)?;
        std::future::poll_fn(|context| self.inner.poll_flush(context)).await
    }

    pub async fn send_text(&mut self, text: impl Into<String>) -> Result<(), WebSocketError> {
        self.send(WebSocketMessage::Text(text.into())).await
    }

    pub async fn send_binary(&mut self, bytes: impl Into<Vec<u8>>) -> Result<(), WebSocketError> {
        self.send(WebSocketMessage::Binary(bytes.into())).await
    }

    pub async fn close(
        &mut self,
        code: Option<u16>,
        reason: impl Into<String>,
    ) -> Result<(), WebSocketError> {
        self.send(WebSocketMessage::Close {
            code,
            reason: reason.into(),
        })
        .await?;
        std::future::poll_fn(|context| self.inner.poll_close(context)).await
    }
}

#[doc(hidden)]
pub trait WebSocketIo {
    fn poll_next(
        &mut self,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<WebSocketMessage, WebSocketError>>>;

    fn poll_ready(&mut self, context: &mut Context<'_>) -> Poll<Result<(), WebSocketError>>;

    fn start_send(&mut self, message: WebSocketMessage) -> Result<(), WebSocketError>;

    fn poll_flush(&mut self, context: &mut Context<'_>) -> Poll<Result<(), WebSocketError>>;

    fn poll_close(&mut self, context: &mut Context<'_>) -> Poll<Result<(), WebSocketError>>;
}

pub struct WebSocketUpgrade {
    key: String,
    requested_protocols: Vec<String>,
    selected_protocol: Option<String>,
}

impl WebSocketUpgrade {
    pub fn protocols(&self) -> impl Iterator<Item = &str> {
        self.requested_protocols.iter().map(String::as_str)
    }

    pub fn protocol(mut self, protocol: impl Into<String>) -> Result<Self, WebSocketUpgradeError> {
        let protocol = protocol.into();

        if !self
            .requested_protocols
            .iter()
            .any(|requested| requested == &protocol)
        {
            return Err(WebSocketUpgradeError::Protocol);
        }

        self.selected_protocol = Some(protocol);
        Ok(self)
    }

    pub fn on_upgrade<F, Fut, Output>(self, handler: F) -> Response
    where
        F: FnOnce(WebSocket) -> Fut + 'static,
        Fut: Future<Output = Output> + 'static,
        Output: 'static,
    {
        let task = Box::new(move |socket| {
            Box::pin(async move {
                drop(handler(socket).await);
            }) as WebSocketFuture
        });
        let selected_protocol = self.selected_protocol.clone();
        let plan = WebSocketPlan {
            key: self.key,
            selected_protocol,
            task,
        };

        Response::websocket(plan)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebSocketUpgradeError {
    NotWebSocket,
    Version,
    MissingKey,
    Protocol,
}

impl IntoResponse for WebSocketUpgradeError {
    fn into_response(self) -> Response {
        match self {
            Self::NotWebSocket => Response::error(400, "request is not a WebSocket upgrade"),
            Self::Version => {
                let mut response = Response::error(426, "unsupported WebSocket version");
                response.set_header("Sec-WebSocket-Version", "13");
                response
            }
            Self::MissingKey => Response::error(400, "WebSocket key is missing"),
            Self::Protocol => Response::error(400, "WebSocket protocol was not requested"),
        }
    }
}

impl<'request> FromRequest<(&'request Request, &'request [u8])> for WebSocketUpgrade {
    type Error = WebSocketUpgradeError;

    async fn from_request(input: (&'request Request, &'request [u8])) -> Result<Self, Self::Error> {
        let headers = input.0.headers();
        let upgrade = header_text(headers.get("upgrade"));
        let connection = header_text(headers.get("connection"));

        if !upgrade.is_some_and(|value| value.eq_ignore_ascii_case("websocket"))
            || !connection.is_some_and(|value| {
                value
                    .split(',')
                    .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
            })
        {
            return Err(WebSocketUpgradeError::NotWebSocket);
        }

        if header_text(headers.get("sec-websocket-version")) != Some("13") {
            return Err(WebSocketUpgradeError::Version);
        }

        let key = header_text(headers.get("sec-websocket-key"))
            .filter(|key| !key.is_empty())
            .ok_or(WebSocketUpgradeError::MissingKey)?
            .to_owned();
        let requested_protocols = header_text(headers.get("sec-websocket-protocol"))
            .into_iter()
            .flat_map(|protocols| protocols.split(','))
            .map(str::trim)
            .filter(|protocol| !protocol.is_empty())
            .map(str::to_owned)
            .collect();

        Ok(Self {
            key,
            requested_protocols,
            selected_protocol: None,
        })
    }

    fn openapi(operation: &mut Operation) {
        operation.response(101, "WebSocket protocol upgrade", None, None);
        operation.response(400, "Invalid WebSocket upgrade request", None, None);
        operation.response(426, "Unsupported WebSocket version", None, None);
    }
}

fn header_text(value: Option<&[u8]>) -> Option<&str> {
    value.and_then(|value| std::str::from_utf8(value).ok())
}

type WebSocketFuture = Pin<Box<dyn Future<Output = ()> + 'static>>;
type WebSocketTask = Box<dyn FnOnce(WebSocket) -> WebSocketFuture + 'static>;

#[doc(hidden)]
pub struct WebSocketPlan {
    key: String,
    selected_protocol: Option<String>,
    task: WebSocketTask,
}

impl WebSocketPlan {
    #[doc(hidden)]
    pub fn key(&self) -> &str {
        &self.key
    }

    #[doc(hidden)]
    pub fn selected_protocol(&self) -> Option<&str> {
        self.selected_protocol.as_deref()
    }

    #[doc(hidden)]
    pub async fn run(self, socket: WebSocket) {
        (self.task)(socket).await;
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::RefCell,
        collections::VecDeque,
        future::Future,
        rc::Rc,
        task::{Context, Poll, Waker},
    };

    use super::{WebSocket, WebSocketError, WebSocketIo, WebSocketMessage};

    struct MockSocket {
        incoming: VecDeque<WebSocketMessage>,
        sent: Rc<RefCell<Vec<WebSocketMessage>>>,
    }

    impl WebSocketIo for MockSocket {
        fn poll_next(
            &mut self,
            _context: &mut Context<'_>,
        ) -> Poll<Option<Result<WebSocketMessage, WebSocketError>>> {
            Poll::Ready(self.incoming.pop_front().map(Ok))
        }

        fn poll_ready(&mut self, _context: &mut Context<'_>) -> Poll<Result<(), WebSocketError>> {
            Poll::Ready(Ok(()))
        }

        fn start_send(&mut self, message: WebSocketMessage) -> Result<(), WebSocketError> {
            self.sent.borrow_mut().push(message);
            Ok(())
        }

        fn poll_flush(&mut self, _context: &mut Context<'_>) -> Poll<Result<(), WebSocketError>> {
            Poll::Ready(Ok(()))
        }

        fn poll_close(&mut self, _context: &mut Context<'_>) -> Poll<Result<(), WebSocketError>> {
            Poll::Ready(Ok(()))
        }
    }

    fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = std::pin::pin!(future);
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);

        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(output) => return output,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    #[test]
    fn sends_and_receives_portable_messages() {
        let sent = Rc::new(RefCell::new(Vec::new()));
        let mut socket = WebSocket::from_io(MockSocket {
            incoming: VecDeque::from([WebSocketMessage::Text("hello".to_owned())]),
            sent: Rc::clone(&sent),
        });

        assert_eq!(
            block_on(socket.next()).unwrap().unwrap(),
            WebSocketMessage::Text("hello".to_owned()),
        );
        block_on(socket.send_binary([1, 2, 3])).unwrap();
        assert_eq!(
            sent.borrow().as_slice(),
            &[WebSocketMessage::Binary(vec![1, 2, 3])],
        );
    }
}
