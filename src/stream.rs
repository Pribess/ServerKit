use std::{
    sync::Arc,
    task::{Context, Poll},
};

use crate::{IntoResponse, Response};

pub trait RequestStream {
    fn poll_next(
        &mut self,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Vec<u8>, StreamError>>>;
}

#[derive(Debug)]
pub struct StreamError {
    message: String,
}

impl StreamError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl IntoResponse for StreamError {
    fn into_response(self) -> Response {
        Response::error(400, self.message)
    }
}

pub(crate) async fn collect_stream(stream: &mut dyn RequestStream) -> Result<Vec<u8>, StreamError> {
    let mut buffered = Vec::new();

    while let Some(chunk) = std::future::poll_fn(|context| stream.poll_next(context)).await {
        buffered.extend_from_slice(&chunk?);
    }

    Ok(buffered)
}

pub(crate) struct BufferedRequestStream {
    buffered: Arc<Vec<u8>>,
    cursor: usize,
}

impl BufferedRequestStream {
    pub(crate) fn new(buffered: Arc<Vec<u8>>) -> Self {
        Self {
            buffered,
            cursor: 0,
        }
    }
}

impl RequestStream for BufferedRequestStream {
    fn poll_next(
        &mut self,
        _context: &mut Context<'_>,
    ) -> Poll<Option<Result<Vec<u8>, StreamError>>> {
        if self.cursor >= self.buffered.len() {
            return Poll::Ready(None);
        }

        let chunk = self.buffered[self.cursor..].to_vec();
        self.cursor = self.buffered.len();

        Poll::Ready(Some(Ok(chunk)))
    }
}
