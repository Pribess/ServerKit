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

    pub async fn next(&mut self) -> Option<Result<Vec<u8>, StreamError>> {
        std::future::poll_fn(|context| self.stream.poll_next(context)).await
    }
}
