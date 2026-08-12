use crate::{RequestStream, StreamError};

pub struct Body {
    stream: Box<dyn RequestStream>,
}

impl Body {
    pub(crate) fn new(stream: Box<dyn RequestStream>) -> Self {
        Self { stream }
    }

    pub async fn next(&mut self) -> Option<Result<Vec<u8>, StreamError>> {
        std::future::poll_fn(|context| self.stream.poll_next(context)).await
    }
}
