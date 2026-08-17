use std::{
    fmt,
    sync::Arc,
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

#[derive(Debug)]
pub struct Chunk {
    bytes: ChunkBytes,
    position: usize,
}

#[derive(Debug)]
enum ChunkBytes {
    Owned(Vec<u8>),
    Shared(Arc<Vec<u8>>),
}

impl Chunk {
    pub fn shared(bytes: Arc<Vec<u8>>) -> Self {
        Self {
            bytes: ChunkBytes::Shared(bytes),
            position: 0,
        }
    }

    pub fn remaining(&self) -> usize {
        let length = match &self.bytes {
            ChunkBytes::Owned(bytes) => bytes.len(),
            ChunkBytes::Shared(bytes) => bytes.len(),
        };

        length - self.position
    }

    pub fn bytes(&self) -> &[u8] {
        let bytes = match &self.bytes {
            ChunkBytes::Owned(bytes) => bytes,
            ChunkBytes::Shared(bytes) => bytes,
        };

        &bytes[self.position..]
    }

    pub fn advance(&mut self, count: usize) {
        assert!(count <= self.remaining(), "cannot advance past the chunk");
        self.position += count;
    }

    pub fn into_vec(self) -> Vec<u8> {
        let mut bytes = match self.bytes {
            ChunkBytes::Owned(bytes) => bytes,
            ChunkBytes::Shared(bytes) => match Arc::try_unwrap(bytes) {
                Ok(bytes) => bytes,
                Err(bytes) => return bytes[self.position..].to_vec(),
            },
        };

        if self.position != 0 {
            let remaining = bytes.len() - self.position;
            bytes.copy_within(self.position.., 0);
            bytes.truncate(remaining);
        }

        bytes
    }
}

impl From<Vec<u8>> for Chunk {
    fn from(bytes: Vec<u8>) -> Self {
        Self {
            bytes: ChunkBytes::Owned(bytes),
            position: 0,
        }
    }
}

impl AsRef<[u8]> for Chunk {
    fn as_ref(&self) -> &[u8] {
        self.bytes()
    }
}

pub trait ResponseStream {
    fn poll_next(&mut self, context: &mut Context<'_>) -> Poll<Option<Result<Chunk, StreamError>>>;
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::Chunk;

    #[test]
    fn owned_chunks_move_their_allocation() {
        let bytes = b"chunk".to_vec();
        let pointer = bytes.as_ptr();
        let chunk = Chunk::from(bytes);
        let bytes = chunk.into_vec();

        assert_eq!(bytes.as_ptr(), pointer);
        assert_eq!(bytes, b"chunk");
    }

    #[test]
    fn chunks_support_multiple_partial_advances() {
        let mut chunk = Chunk::from(b"abcdef".to_vec());

        assert_eq!(chunk.remaining(), 6);
        assert_eq!(chunk.bytes(), b"abcdef");

        chunk.advance(2);
        assert_eq!(chunk.remaining(), 4);
        assert_eq!(chunk.bytes(), b"cdef");

        chunk.advance(1);
        assert_eq!(chunk.remaining(), 3);
        assert_eq!(chunk.bytes(), b"def");

        chunk.advance(3);
        assert_eq!(chunk.remaining(), 0);
        assert_eq!(chunk.bytes(), b"");
    }

    #[test]
    fn shared_chunks_copy_only_when_converted_back_to_a_vec() {
        let shared = Arc::new(b"abcdef".to_vec());
        let mut chunk = Chunk::shared(Arc::clone(&shared));
        chunk.advance(2);

        assert_eq!(chunk.into_vec(), b"cdef");
        assert_eq!(shared.as_slice(), b"abcdef");
    }
}
