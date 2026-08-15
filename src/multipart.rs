use crate::{
    Body, FromRequest, Headers, IntoResponse, Request, Response, StreamError,
    extract::content_type, openapi::Operation,
};

pub struct Multipart {
    boundary: Vec<u8>,
    body: Body,
    buffer: Vec<u8>,
    started: bool,
    done: bool,
}

impl Multipart {
    pub async fn next(&mut self) -> Option<Result<MultipartField, MultipartError>> {
        if self.done {
            return None;
        }

        match self.next_field().await {
            Ok(field) => field.map(Ok),
            Err(error) => {
                self.done = true;
                Some(Err(error))
            }
        }
    }

    async fn next_field(&mut self) -> Result<Option<MultipartField>, MultipartError> {
        if !self.started {
            let boundary_length = self.boundary.len();
            self.ensure(boundary_length).await?;

            if !self.buffer.starts_with(&self.boundary) {
                return Err(MultipartError::Malformed);
            }

            self.buffer.drain(..boundary_length);
            self.started = true;
        }

        self.ensure(2).await?;

        if self.buffer.starts_with(b"--") {
            self.buffer.drain(..2);
            self.done = true;
            return Ok(None);
        }

        if !self.buffer.starts_with(b"\r\n") {
            return Err(MultipartError::Malformed);
        }
        self.buffer.drain(..2);

        let header_end = loop {
            if let Some(index) = find_bytes(&self.buffer, b"\r\n\r\n") {
                break index;
            }

            if self.buffer.len() > 64 * 1024 {
                return Err(MultipartError::Malformed);
            }

            self.pull().await?;
        };
        let header_bytes = self.buffer[..header_end].to_vec();
        self.buffer.drain(..header_end + 4);
        let headers = parse_headers(&header_bytes)?;
        let mut delimiter = Vec::with_capacity(self.boundary.len() + 2);
        delimiter.extend_from_slice(b"\r\n");
        delimiter.extend_from_slice(&self.boundary);
        let field_end = loop {
            if let Some(index) = find_bytes(&self.buffer, &delimiter) {
                break index;
            }

            self.pull().await?;
        };
        let bytes = self.buffer[..field_end].to_vec();
        self.buffer.drain(..field_end + delimiter.len());

        Ok(Some(multipart_field(headers, bytes)))
    }

    async fn ensure(&mut self, length: usize) -> Result<(), MultipartError> {
        while self.buffer.len() < length {
            self.pull().await?;
        }

        Ok(())
    }

    async fn pull(&mut self) -> Result<(), MultipartError> {
        loop {
            match self.body.next().await {
                Some(Ok(chunk)) => {
                    if !chunk.is_empty() {
                        self.buffer.extend_from_slice(chunk);
                        return Ok(());
                    }
                }
                Some(Err(error)) => return Err(MultipartError::Stream(error)),
                None => return Err(MultipartError::Malformed),
            }
        }
    }
}

pub struct MultipartField {
    headers: Headers,
    name: Option<String>,
    file_name: Option<String>,
    content_type: Option<String>,
    bytes: Vec<u8>,
}

impl MultipartField {
    pub fn headers(&self) -> &Headers {
        &self.headers
    }

    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub fn file_name(&self) -> Option<&str> {
        self.file_name.as_deref()
    }

    pub fn content_type(&self) -> Option<&str> {
        self.content_type.as_deref()
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    pub fn text(&self) -> Result<&str, std::str::Utf8Error> {
        std::str::from_utf8(&self.bytes)
    }
}

#[derive(Debug)]
pub enum MultipartError {
    ContentType,
    Boundary,
    Malformed,
    Stream(StreamError),
}

impl IntoResponse for MultipartError {
    fn into_response(self) -> Response {
        match self {
            Self::ContentType => Response::error(415, "expected multipart/form-data request body"),
            Self::Boundary => Response::error(400, "multipart boundary is missing or invalid"),
            Self::Malformed => Response::error(400, "multipart request body is malformed"),
            Self::Stream(error) => error.into_response(),
        }
    }
}

impl FromRequest<Request> for Multipart {
    type Error = MultipartError;

    async fn from_request(input: Request) -> Result<Self, Self::Error> {
        let content_type = content_type(&input).ok_or(MultipartError::ContentType)?;
        let mut parameters = content_type.split(';');
        let media_type = parameters.next().unwrap_or_default().trim();

        if !media_type.eq_ignore_ascii_case("multipart/form-data") {
            return Err(MultipartError::ContentType);
        }

        let boundary = parameters
            .filter_map(|parameter| parameter.trim().split_once('='))
            .find(|(name, _)| name.trim().eq_ignore_ascii_case("boundary"))
            .map(|(_, value)| value.trim().trim_matches('"'))
            .filter(|value| !value.is_empty() && value.len() <= 70)
            .ok_or(MultipartError::Boundary)?
            .as_bytes();
        let mut delimiter = Vec::with_capacity(boundary.len() + 2);
        delimiter.extend_from_slice(b"--");
        delimiter.extend_from_slice(boundary);
        let limit = input.body_limit();

        Ok(Self {
            boundary: delimiter,
            body: Body::new(input.body, limit),
            buffer: Vec::new(),
            started: false,
            done: false,
        })
    }

    fn openapi(operation: &mut Operation) {
        operation.request_body("multipart/form-data", None, true);
        operation.response(400, "Invalid multipart request body", None, None);
        operation.response(413, "Request body is too large", None, None);
        operation.response(415, "Unsupported media type", None, None);
    }
}

fn parse_headers(bytes: &[u8]) -> Result<Headers, MultipartError> {
    let text = std::str::from_utf8(bytes).map_err(|_| MultipartError::Malformed)?;
    let mut headers = Headers::new();

    for line in text.split("\r\n") {
        let (name, value) = line.split_once(':').ok_or(MultipartError::Malformed)?;
        headers.append_unchecked(name.trim(), value.trim());
    }

    Ok(headers)
}

fn multipart_field(headers: Headers, bytes: Vec<u8>) -> MultipartField {
    let disposition = headers
        .get("content-disposition")
        .and_then(|value| std::str::from_utf8(value).ok());
    let name = disposition.and_then(|value| disposition_parameter(value, "name"));
    let file_name = disposition.and_then(|value| disposition_parameter(value, "filename"));
    let content_type = headers
        .get("content-type")
        .and_then(|value| std::str::from_utf8(value).ok())
        .map(str::to_owned);

    MultipartField {
        headers,
        name,
        file_name,
        content_type,
        bytes,
    }
}

fn disposition_parameter(value: &str, expected: &str) -> Option<String> {
    value
        .split(';')
        .skip(1)
        .filter_map(|parameter| parameter.trim().split_once('='))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case(expected))
        .map(|(_, value)| value.trim().trim_matches('"').to_owned())
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    (!needle.is_empty())
        .then(|| {
            haystack
                .windows(needle.len())
                .position(|window| window == needle)
        })
        .flatten()
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        future::Future,
        task::{Context, Poll, Waker},
    };

    use super::Multipart;
    use crate::{Body, RequestStream, StreamError};

    struct Chunks {
        chunks: VecDeque<Vec<u8>>,
        current: Vec<u8>,
    }

    impl RequestStream for Chunks {
        fn poll_next(
            &mut self,
            _context: &mut Context<'_>,
        ) -> Poll<Option<Result<(), StreamError>>> {
            match self.chunks.pop_front() {
                Some(chunk) => {
                    self.current = chunk;
                    Poll::Ready(Some(Ok(())))
                }
                None => Poll::Ready(None),
            }
        }

        fn chunk(&self) -> &[u8] {
            &self.current
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
    fn parses_fields_and_files() {
        let body = b"--boundary\r\nContent-Disposition: form-data; name=\"title\"\r\n\r\nhello\r\n--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.txt\"\r\nContent-Type: text/plain\r\n\r\ncontent\r\n--boundary--\r\n";
        let chunks = body.chunks(7).map(<[u8]>::to_vec).collect::<VecDeque<_>>();
        let mut multipart = Multipart {
            boundary: b"--boundary".to_vec(),
            body: Body::new(
                Box::new(Chunks {
                    chunks,
                    current: Vec::new(),
                }),
                None,
            ),
            buffer: Vec::new(),
            started: false,
            done: false,
        };
        let first = block_on(multipart.next()).unwrap().unwrap();
        let second = block_on(multipart.next()).unwrap().unwrap();

        assert_eq!(first.name(), Some("title"));
        assert_eq!(first.bytes(), b"hello");
        assert_eq!(second.file_name(), Some("a.txt"));
        assert_eq!(second.content_type(), Some("text/plain"));
        assert!(block_on(multipart.next()).is_none());
    }
}
