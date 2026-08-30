use crate::{
    Body, Error, FromRequest, Headers, IntoResponse, Request, Response, StreamError,
    extract::content_type, openapi::Operation,
};

pub struct Multipart {
    boundary: Vec<u8>,
    body: Body,
    buffer: Vec<u8>,
    started: bool,
    field_open: bool,
    done: bool,
}

impl Multipart {
    pub async fn next(&mut self) -> Option<Result<MultipartField<'_>, MultipartError>> {
        if self.done {
            return None;
        }

        match self.prepare_field().await {
            Ok(Some(headers)) => Some(Ok(multipart_field(headers, self))),
            Ok(None) => None,
            Err(error) => {
                self.done = true;
                Some(Err(error))
            }
        }
    }

    async fn prepare_field(&mut self) -> Result<Option<Headers>, MultipartError> {
        if self.field_open {
            self.discard_field().await?;
        }

        if !self.started {
            let boundary_length = self.boundary.len() - 2;
            self.ensure(boundary_length).await?;

            if !self.buffer.starts_with(&self.boundary[2..]) {
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
        self.field_open = true;

        Ok(Some(headers))
    }

    async fn next_field_chunk(&mut self) -> Result<Option<Vec<u8>>, MultipartError> {
        if !self.field_open {
            return Ok(None);
        }

        loop {
            if let Some(index) = find_bytes(&self.buffer, &self.boundary) {
                let bytes = self.buffer.drain(..index).collect::<Vec<_>>();
                self.buffer.drain(..self.boundary.len());
                self.field_open = false;

                return if bytes.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(bytes))
                };
            }

            let retained = self.boundary.len() - 1;

            if self.buffer.len() > retained {
                let available = self.buffer.len() - retained;
                return Ok(Some(self.buffer.drain(..available).collect()));
            }

            self.pull().await?;
        }
    }

    async fn discard_field(&mut self) -> Result<(), MultipartError> {
        while self.next_field_chunk().await?.is_some() {}
        Ok(())
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

pub struct MultipartField<'multipart> {
    multipart: &'multipart mut Multipart,
    headers: Headers,
    name: Option<String>,
    file_name: Option<String>,
    content_type: Option<String>,
}

impl MultipartField<'_> {
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

    pub async fn next(&mut self) -> Option<Result<Vec<u8>, MultipartError>> {
        match self.multipart.next_field_chunk().await {
            Ok(Some(chunk)) => Some(Ok(chunk)),
            Ok(None) => None,
            Err(error) => {
                self.multipart.done = true;
                self.multipart.field_open = false;
                Some(Err(error))
            }
        }
    }

    pub async fn bytes(mut self) -> Result<Vec<u8>, MultipartError> {
        let mut bytes = Vec::new();

        while let Some(chunk) = self.next().await {
            bytes.extend_from_slice(&chunk?);
        }

        Ok(bytes)
    }

    pub async fn text(self) -> Result<String, MultipartError> {
        String::from_utf8(self.bytes().await?).map_err(MultipartError::Text)
    }
}

#[derive(Debug)]
pub enum MultipartError {
    ContentType,
    Boundary,
    Malformed,
    Stream(StreamError),
    Text(std::string::FromUtf8Error),
}

impl IntoResponse for MultipartError {
    fn into_response(self) -> Response {
        match self {
            Self::ContentType => Error::new(
                415,
                "request.content_type.unsupported",
                "expected multipart/form-data request body",
            ),
            Self::Boundary => Error::new(
                400,
                "request.multipart.invalid_boundary",
                "multipart boundary is missing or invalid",
            ),
            Self::Malformed => Error::new(
                400,
                "request.multipart.invalid",
                "multipart request body is malformed",
            ),
            Self::Stream(error) => return error.into_response(),
            Self::Text(error) => {
                Error::new(400, "request.multipart.invalid_text", error.to_string())
            }
        }
        .into_response()
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
        let mut delimiter = Vec::with_capacity(boundary.len() + 4);
        delimiter.extend_from_slice(b"\r\n--");
        delimiter.extend_from_slice(boundary);
        let limit = input.body_limit();

        Ok(Self {
            boundary: delimiter,
            body: Body::new(input.body, limit),
            buffer: Vec::new(),
            started: false,
            field_open: false,
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

fn multipart_field(headers: Headers, multipart: &mut Multipart) -> MultipartField<'_> {
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
        multipart,
        headers,
        name,
        file_name,
        content_type,
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

    fn multipart(body: &[u8], chunk_size: usize) -> Multipart {
        let chunks = body
            .chunks(chunk_size)
            .map(<[u8]>::to_vec)
            .collect::<VecDeque<_>>();

        Multipart {
            boundary: b"\r\n--boundary".to_vec(),
            body: Body::new(
                Box::new(Chunks {
                    chunks,
                    current: Vec::new(),
                }),
                None,
            ),
            buffer: Vec::new(),
            started: false,
            field_open: false,
            done: false,
        }
    }

    #[test]
    fn parses_fields_and_files() {
        let body = b"--boundary\r\nContent-Disposition: form-data; name=\"title\"\r\n\r\nhello\r\n--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.txt\"\r\nContent-Type: text/plain\r\n\r\ncontent\r\n--boundary--\r\n";
        let mut multipart = multipart(body, 7);
        let first = block_on(multipart.next()).unwrap().unwrap();

        assert_eq!(first.name(), Some("title"));
        assert_eq!(block_on(first.text()).unwrap(), "hello");

        let second = block_on(multipart.next()).unwrap().unwrap();
        assert_eq!(second.file_name(), Some("a.txt"));
        assert_eq!(second.content_type(), Some("text/plain"));
        assert_eq!(block_on(second.bytes()).unwrap(), b"content");
        assert!(block_on(multipart.next()).is_none());
    }

    #[test]
    fn streams_large_fields_in_bounded_chunks() {
        let content = vec![7; 256 * 1024];
        let mut body = b"--boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"large.bin\"\r\n\r\n".to_vec();
        body.extend_from_slice(&content);
        body.extend_from_slice(b"\r\n--boundary--\r\n");
        let mut multipart = multipart(&body, 1024);
        let mut field = block_on(multipart.next()).unwrap().unwrap();
        let mut received = Vec::new();
        let mut largest = 0;

        while let Some(chunk) = block_on(field.next()) {
            let chunk = chunk.unwrap();
            largest = largest.max(chunk.len());
            received.extend_from_slice(&chunk);
        }

        assert_eq!(received, content);
        assert!(largest <= 2048);
        drop(field);
        assert!(block_on(multipart.next()).is_none());
    }

    #[test]
    fn discards_an_unread_field_before_parsing_the_next_one() {
        let content = vec![7; 32 * 1024];
        let mut body =
            b"--boundary\r\nContent-Disposition: form-data; name=\"file\"\r\n\r\n".to_vec();
        body.extend_from_slice(&content);
        body.extend_from_slice(b"\r\n--boundary\r\nContent-Disposition: form-data; name=\"title\"\r\n\r\nafter\r\n--boundary--\r\n");
        let mut multipart = multipart(&body, 128);

        {
            let mut first = block_on(multipart.next()).unwrap().unwrap();
            assert_eq!(first.name(), Some("file"));
            assert!(block_on(first.next()).unwrap().unwrap().len() <= 256);
        }

        let second = block_on(multipart.next()).unwrap().unwrap();
        assert_eq!(second.name(), Some("title"));
        assert_eq!(block_on(second.text()).unwrap(), "after");
        assert!(block_on(multipart.next()).is_none());
    }
}
