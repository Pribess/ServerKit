use std::convert::Infallible;

pub struct Response {
    status: u16,
    content_type: Option<&'static str>,
    body: Vec<u8>,
}

impl Response {
    pub fn empty() -> Self {
        Self {
            status: 204,
            content_type: None,
            body: Vec::new(),
        }
    }

    pub fn text(status: u16, text: impl Into<String>) -> Self {
        Self {
            status,
            content_type: Some("text/plain; charset=utf-8"),
            body: text.into().into_bytes(),
        }
    }

    pub fn bytes(status: u16, bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            content_type: Some("application/octet-stream"),
            body: bytes.into(),
        }
    }

    pub(crate) fn error(status: u16, message: impl Into<String>) -> Self {
        Self::text(status, message)
    }

    pub fn status(&self) -> u16 {
        self.status
    }

    pub fn content_type(&self) -> Option<&str> {
        self.content_type
    }

    pub fn body(&self) -> &[u8] {
        &self.body
    }

    pub(crate) fn into_parts(self) -> (u16, Option<&'static str>, Vec<u8>) {
        (self.status, self.content_type, self.body)
    }
}

pub trait IntoResponse {
    fn into_response(self) -> Response;
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
}

impl IntoResponse for String {
    fn into_response(self) -> Response {
        Response::text(200, self)
    }
}

impl IntoResponse for &str {
    fn into_response(self) -> Response {
        Response::text(200, self)
    }
}

impl IntoResponse for Vec<u8> {
    fn into_response(self) -> Response {
        Response::bytes(200, self)
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
}
