use std::{
    pin::Pin,
    task::{Context, Poll},
};

use hyper::body::{Body as HyperBody, Bytes, Frame, Incoming, SizeHint};
use serverkit::{Chunk, RequestStream, ResponseBody, StreamError};

pub(crate) struct HyperRequestStream {
    body: Pin<Box<Incoming>>,
    current: Option<Bytes>,
}

impl HyperRequestStream {
    pub(crate) fn new(body: Incoming) -> Self {
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

pub(crate) struct HyperResponseBody {
    body: ResponseBody,
}

pub(crate) struct HyperChunk(Chunk);

impl hyper::body::Buf for HyperChunk {
    fn remaining(&self) -> usize {
        self.0.remaining()
    }

    fn chunk(&self) -> &[u8] {
        self.0.bytes()
    }

    fn advance(&mut self, count: usize) {
        self.0.advance(count);
    }
}

impl HyperResponseBody {
    pub(crate) fn new(body: ResponseBody) -> Self {
        Self { body }
    }
}

impl HyperBody for HyperResponseBody {
    type Data = HyperChunk;
    type Error = StreamError;

    fn poll_frame(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        match &mut self.get_mut().body {
            ResponseBody::Buffered(bytes) if bytes.is_empty() => Poll::Ready(None),
            ResponseBody::Buffered(bytes) => {
                let chunk = HyperChunk(Chunk::from(std::mem::take(bytes)));
                Poll::Ready(Some(Ok(Frame::data(chunk))))
            }
            ResponseBody::Streaming(stream) => match stream.poll_next(context) {
                Poll::Ready(Some(Ok(chunk))) => {
                    Poll::Ready(Some(Ok(Frame::data(HyperChunk(chunk)))))
                }
                Poll::Ready(Some(Err(error))) => Poll::Ready(Some(Err(error))),
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Pending => Poll::Pending,
            },
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
