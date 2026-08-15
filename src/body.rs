use crate::{RequestStream, StreamError, stream::LimitedRequestStream};

pub struct Body {
    stream: Box<dyn RequestStream>,
}

impl Body {
    pub(crate) fn new(stream: Box<dyn RequestStream>, limit: Option<usize>) -> Self {
        let stream = match limit {
            Some(limit) => {
                Box::new(LimitedRequestStream::new(stream, limit)) as Box<dyn RequestStream>
            }
            None => stream,
        };

        Self { stream }
    }

    pub async fn next(&mut self) -> Option<Result<&[u8], StreamError>> {
        let result = std::future::poll_fn(|context| self.stream.poll_next(context)).await?;

        match result {
            Ok(()) => Some(Ok(self.stream.chunk())),
            Err(error) => Some(Err(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        future::Future,
        task::{Context, Poll, Waker},
    };

    use super::Body;
    use crate::{RequestStream, StreamError};

    struct OneChunk {
        bytes: Vec<u8>,
        sent: bool,
    }

    impl RequestStream for OneChunk {
        fn poll_next(
            &mut self,
            _context: &mut Context<'_>,
        ) -> Poll<Option<Result<(), StreamError>>> {
            if self.sent {
                Poll::Ready(None)
            } else {
                self.sent = true;
                Poll::Ready(Some(Ok(())))
            }
        }

        fn chunk(&self) -> &[u8] {
            &self.bytes
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
    fn borrows_the_adapter_chunk_without_copying() {
        let bytes = b"chunk".to_vec();
        let pointer = bytes.as_ptr();
        let mut body = Body::new(Box::new(OneChunk { bytes, sent: false }), None);
        let chunk = block_on(body.next()).unwrap().unwrap();

        assert_eq!(chunk, b"chunk");
        assert_eq!(chunk.as_ptr(), pointer);
    }
}
