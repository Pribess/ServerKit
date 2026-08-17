use std::{
    task::{Context, Poll},
    time::Duration,
};

use crate::{Chunk, IntoResponse, Response, ResponseStream, StreamError, openapi::Operation};

pub trait SseStream {
    fn poll_next(
        &mut self,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<SseEvent, StreamError>>>;
}

pub struct Sse<S> {
    stream: S,
}

impl<S> Sse<S> {
    pub fn new(stream: S) -> Self {
        Self { stream }
    }
}

impl<S: SseStream + 'static> IntoResponse for Sse<S> {
    fn into_response(self) -> Response {
        let mut response = Response::stream(
            200,
            EncodedSseStream {
                stream: self.stream,
            },
        );
        response
            .headers()
            .set("Content-Type", "text/event-stream; charset=utf-8")
            .expect("the built-in SSE content type is valid");
        response
            .headers()
            .set("Cache-Control", "no-cache")
            .expect("the built-in SSE cache policy is valid");
        response
    }

    fn openapi(operation: &mut Operation) {
        operation.response(
            200,
            "Server-sent event stream",
            Some("text/event-stream"),
            None,
        );
    }
}

pub struct SseEvent {
    data: String,
    event: Option<String>,
    id: Option<String>,
    retry: Option<Duration>,
}

impl SseEvent {
    pub fn data(data: impl Into<String>) -> Self {
        Self {
            data: data.into(),
            event: None,
            id: None,
            retry: None,
        }
    }

    pub fn event(mut self, event: impl Into<String>) -> Self {
        self.event = Some(event.into());
        self
    }

    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn retry(mut self, retry: Duration) -> Self {
        self.retry = Some(retry);
        self
    }

    #[cfg(test)]
    fn encode(self) -> Vec<u8> {
        let mut encoded = Vec::new();
        self.encode_into(&mut encoded);
        encoded
    }

    fn encode_into(self, encoded: &mut Vec<u8>) {
        encoded.clear();

        if let Some(event) = self.event {
            encoded.extend_from_slice(b"event: ");
            extend_sanitized(encoded, &event);
            encoded.push(b'\n');
        }

        if let Some(id) = self.id {
            encoded.extend_from_slice(b"id: ");
            extend_sanitized(encoded, &id);
            encoded.push(b'\n');
        }

        if let Some(retry) = self.retry {
            encoded.extend_from_slice(b"retry: ");
            encoded.extend_from_slice(retry.as_millis().to_string().as_bytes());
            encoded.push(b'\n');
        }

        for line in self.data.lines() {
            encoded.extend_from_slice(b"data: ");
            encoded.extend_from_slice(line.as_bytes());
            encoded.push(b'\n');
        }

        if self.data.is_empty() {
            encoded.extend_from_slice(b"data:\n");
        }

        encoded.push(b'\n');
    }
}

struct EncodedSseStream<S> {
    stream: S,
}

impl<S: SseStream> ResponseStream for EncodedSseStream<S> {
    fn poll_next(&mut self, context: &mut Context<'_>) -> Poll<Option<Result<Chunk, StreamError>>> {
        match self.stream.poll_next(context) {
            Poll::Ready(Some(Ok(event))) => {
                let mut encoded = Vec::new();
                event.encode_into(&mut encoded);
                Poll::Ready(Some(Ok(Chunk::from(encoded))))
            }
            Poll::Ready(Some(Err(error))) => Poll::Ready(Some(Err(error))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

fn extend_sanitized(encoded: &mut Vec<u8>, value: &str) {
    encoded.extend(value.bytes().filter(|byte| !matches!(byte, b'\r' | b'\n')));
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::SseEvent;

    #[test]
    fn encodes_an_event() {
        let event = SseEvent::data("first\nsecond")
            .event("update")
            .id("42")
            .retry(Duration::from_secs(1));

        assert_eq!(
            event.encode(),
            b"event: update\nid: 42\nretry: 1000\ndata: first\ndata: second\n\n",
        );
    }
}
