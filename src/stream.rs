use std::{
    fmt,
    task::{Context, Poll},
};

use crate::{IntoResponse, Response};

pub trait RequestStream {
    /// Advances to the next chunk without transferring its allocation.
    ///
    /// After returning `Poll::Ready(Some(Ok(())))`, `chunk` must expose the
    /// new chunk until the next mutable call to this stream.
    fn poll_next(&mut self, context: &mut Context<'_>) -> Poll<Option<Result<(), StreamError>>>;

    /// Borrows the chunk produced by the latest successful `poll_next` call.
    fn chunk(&self) -> &[u8];
}

pub trait ResponseStream {
    fn poll_next(
        &mut self,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Vec<u8>, StreamError>>>;
}

#[derive(Debug)]
pub struct StreamError {
    status: u16,
    message: String,
}

impl StreamError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            status: 400,
            message: message.into(),
        }
    }

    pub fn payload_too_large(limit: usize) -> Self {
        Self {
            status: 413,
            message: format!("request body exceeds the {limit}-byte limit"),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for StreamError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for StreamError {}

impl IntoResponse for StreamError {
    fn into_response(self) -> Response {
        Response::error(self.status, self.message)
    }
}

pub(crate) async fn collect_stream(
    stream: &mut dyn RequestStream,
    limit: Option<usize>,
) -> Result<Vec<u8>, StreamError> {
    let mut buffered = Vec::new();

    while let Some(result) = std::future::poll_fn(|context| stream.poll_next(context)).await {
        result?;
        let chunk = stream.chunk();

        if let Some(limit) = limit
            && buffered.len().saturating_add(chunk.len()) > limit
        {
            return Err(StreamError::payload_too_large(limit));
        }

        buffered.extend_from_slice(chunk);
    }

    Ok(buffered)
}

pub(crate) struct LimitedRequestStream {
    stream: Box<dyn RequestStream>,
    limit: usize,
    read: usize,
    exhausted: bool,
}

impl LimitedRequestStream {
    pub(crate) fn new(stream: Box<dyn RequestStream>, limit: usize) -> Self {
        Self {
            stream,
            limit,
            read: 0,
            exhausted: false,
        }
    }
}

impl RequestStream for LimitedRequestStream {
    fn poll_next(&mut self, context: &mut Context<'_>) -> Poll<Option<Result<(), StreamError>>> {
        if self.exhausted {
            return Poll::Ready(None);
        }

        match self.stream.poll_next(context) {
            Poll::Ready(Some(Ok(()))) => {
                self.read = self.read.saturating_add(self.stream.chunk().len());

                if self.read > self.limit {
                    self.exhausted = true;
                    Poll::Ready(Some(Err(StreamError::payload_too_large(self.limit))))
                } else {
                    Poll::Ready(Some(Ok(())))
                }
            }
            Poll::Ready(Some(Err(error))) => {
                self.exhausted = true;
                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(None) => {
                self.exhausted = true;
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn chunk(&self) -> &[u8] {
        self.stream.chunk()
    }
}

pub(crate) struct BufferedRequestStream {
    buffered: Vec<u8>,
    consumed: bool,
}

impl BufferedRequestStream {
    pub(crate) fn new(buffered: Vec<u8>) -> Self {
        Self {
            buffered,
            consumed: false,
        }
    }
}

impl RequestStream for BufferedRequestStream {
    fn poll_next(&mut self, _context: &mut Context<'_>) -> Poll<Option<Result<(), StreamError>>> {
        if self.consumed || self.buffered.is_empty() {
            return Poll::Ready(None);
        }

        self.consumed = true;
        Poll::Ready(Some(Ok(())))
    }

    fn chunk(&self) -> &[u8] {
        &self.buffered
    }
}
