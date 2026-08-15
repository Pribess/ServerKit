use std::convert::Infallible;

use crate::{
    Cookie, Headers, InvalidHeader, ResponseStream,
    openapi::Operation,
    schemaval::{SchemaKind, SchemaMetadata},
};

#[cfg(feature = "websocket")]
use crate::websocket::WebSocketPlan;

pub enum ResponseBody {
    Buffered(Vec<u8>),
    Streaming(Box<dyn ResponseStream>),
    #[cfg(feature = "websocket")]
    WebSocket(WebSocketPlan),
}

impl ResponseBody {
    pub fn buffered(&self) -> Option<&[u8]> {
        match self {
            Self::Buffered(bytes) => Some(bytes),
            Self::Streaming(_) => None,
            #[cfg(feature = "websocket")]
            Self::WebSocket(_) => None,
        }
    }

    pub fn is_streaming(&self) -> bool {
        matches!(self, Self::Streaming(_))
    }
}

pub struct Response {
    status: u16,
    headers: Headers,
    body: ResponseBody,
}

impl Response {
    pub fn new(status: u16) -> Self {
        Self {
            status,
            headers: Headers::new(),
            body: ResponseBody::Buffered(Vec::new()),
        }
    }

    pub fn empty() -> Self {
        Self::new(204)
    }

    pub fn text(status: u16, text: impl Into<String>) -> Self {
        let mut response = Self {
            status,
            headers: Headers::new(),
            body: ResponseBody::Buffered(text.into().into_bytes()),
        };
        response
            .headers
            .set_unchecked("Content-Type", "text/plain; charset=utf-8");
        response
    }

    pub fn bytes(status: u16, bytes: impl Into<Vec<u8>>) -> Self {
        let mut response = Self {
            status,
            headers: Headers::new(),
            body: ResponseBody::Buffered(bytes.into()),
        };
        response
            .headers
            .set_unchecked("Content-Type", "application/octet-stream");
        response
    }

    pub fn stream(status: u16, stream: impl ResponseStream + 'static) -> Self {
        Self {
            status,
            headers: Headers::new(),
            body: ResponseBody::Streaming(Box::new(stream)),
        }
    }

    #[cfg(feature = "websocket")]
    pub(crate) fn websocket(plan: WebSocketPlan) -> Self {
        let selected_protocol = plan.selected_protocol().map(str::to_owned);
        let mut response = Self {
            status: 101,
            headers: Headers::new(),
            body: ResponseBody::WebSocket(plan),
        };

        if let Some(protocol) = selected_protocol {
            response
                .headers
                .set_unchecked("Sec-WebSocket-Protocol", protocol);
        }

        response
    }

    pub(crate) fn error(status: u16, message: impl Into<String>) -> Self {
        Self::text(status, message)
    }

    pub fn status(&self) -> u16 {
        self.status
    }

    pub fn content_type(&self) -> Option<&str> {
        self.headers
            .get("content-type")
            .and_then(|value| std::str::from_utf8(value).ok())
    }

    pub fn body(&self) -> &[u8] {
        self.body.buffered().unwrap_or_default()
    }

    pub fn is_streaming(&self) -> bool {
        self.body.is_streaming()
    }

    pub fn headers(&mut self) -> &mut Headers {
        &mut self.headers
    }

    pub fn set_cookie(&mut self, cookie: Cookie) -> Result<(), InvalidHeader> {
        self.headers.append("Set-Cookie", cookie.header_value())
    }

    pub(crate) fn set_header(&mut self, name: impl Into<String>, value: impl Into<Vec<u8>>) {
        self.headers.set_unchecked(name, value);
    }

    pub(crate) fn without_body(mut self) -> Self {
        if let ResponseBody::Buffered(body) = &mut self.body {
            let content_length = body.len().to_string();
            body.clear();
            self.set_header("Content-Length", content_length);
        }
        self
    }

    pub fn into_parts(self) -> (u16, Headers, ResponseBody) {
        (self.status, self.headers, self.body)
    }
}

pub trait IntoResponse {
    fn into_response(self) -> Response;

    #[doc(hidden)]
    fn openapi(operation: &mut Operation) {
        operation.response(200, "Success", None, None);
    }
}

impl IntoResponse for Response {
    fn into_response(self) -> Response {
        self
    }
}

impl IntoResponse for () {
    fn into_response(self) -> Response {
        Response::empty()
    }

    fn openapi(operation: &mut Operation) {
        operation.response(204, "No Content", None, None);
    }
}

impl IntoResponse for String {
    fn into_response(self) -> Response {
        Response::text(200, self)
    }

    fn openapi(operation: &mut Operation) {
        operation.response(
            200,
            "Success",
            Some("text/plain; charset=utf-8"),
            Some(SchemaMetadata::new(SchemaKind::String)),
        );
    }
}

impl IntoResponse for &str {
    fn into_response(self) -> Response {
        Response::text(200, self)
    }

    fn openapi(operation: &mut Operation) {
        String::openapi(operation);
    }
}

impl IntoResponse for Vec<u8> {
    fn into_response(self) -> Response {
        Response::bytes(200, self)
    }

    fn openapi(operation: &mut Operation) {
        operation.response(
            200,
            "Success",
            Some("application/octet-stream"),
            Some(SchemaMetadata::new(SchemaKind::Bytes)),
        );
    }
}

impl IntoResponse for Infallible {
    fn into_response(self) -> Response {
        match self {}
    }
}

impl<T: IntoResponse, E: IntoResponse> IntoResponse for Result<T, E> {
    fn into_response(self) -> Response {
        match self {
            Ok(value) => value.into_response(),
            Err(error) => error.into_response(),
        }
    }

    fn openapi(operation: &mut Operation) {
        T::openapi(operation);
        E::openapi(operation);
    }
}

#[cfg(test)]
mod tests {
    use std::task::{Context, Poll};

    use crate::{Cookie, IntoResponse, Redirect, Response, ResponseStream, StreamError};

    struct OneChunk(Option<Vec<u8>>);

    impl ResponseStream for OneChunk {
        fn poll_next(
            &mut self,
            _context: &mut Context<'_>,
        ) -> Poll<Option<Result<Vec<u8>, StreamError>>> {
            Poll::Ready(self.0.take().map(Ok))
        }
    }

    #[test]
    fn headers_set_append_and_remove_case_insensitively() {
        let mut response = Response::text(200, "ok");
        response.headers().set("X-Value", "first").unwrap();
        response.headers().append("x-value", "second").unwrap();

        assert_eq!(
            response.headers().get_all("X-VALUE").collect::<Vec<_>>(),
            vec![b"first".as_slice(), b"second".as_slice()],
        );

        response.headers().set("X-VALUE", "replacement").unwrap();
        assert_eq!(
            response.headers().get_all("x-value").collect::<Vec<_>>(),
            vec![b"replacement".as_slice()],
        );

        response.headers().remove("x-VaLuE");
        assert!(!response.headers().contains("x-value"));
    }

    #[test]
    fn content_type_uses_the_shared_header_collection() {
        let mut response = Response::text(200, "ok");
        response
            .headers()
            .set("Content-Type", "application/custom")
            .unwrap();

        assert_eq!(response.content_type(), Some("application/custom"));
    }

    #[test]
    fn appends_set_cookie_headers() {
        let mut response = Response::new(200);
        response.set_cookie(Cookie::new("a", "1")).unwrap();
        response.set_cookie(Cookie::new("b", "2")).unwrap();

        assert_eq!(response.headers().get_all("set-cookie").count(), 2);
    }

    #[test]
    fn creates_streaming_responses_and_redirects() {
        let response = Response::stream(200, OneChunk(Some(b"chunk".to_vec())));
        assert!(response.is_streaming());

        let mut redirect = Redirect::temporary("/next").into_response();
        assert_eq!(redirect.status(), 307);
        assert_eq!(
            redirect.headers().get("location"),
            Some(b"/next".as_slice())
        );
    }
}
