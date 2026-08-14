use std::{
    task::{Context, Poll},
    time::Duration,
};

use crate::{IntoResponse, Response, ResponseStream, StreamError, openapi::Operation};

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

    fn encode(self) -> Vec<u8> {
        let mut encoded = String::new();

        if let Some(event) = self.event {
            encoded.push_str("event: ");
            encoded.push_str(&sanitize(&event));
            encoded.push('\n');
        }

        if let Some(id) = self.id {
            encoded.push_str("id: ");
            encoded.push_str(&sanitize(&id));
            encoded.push('\n');
        }

        if let Some(retry) = self.retry {
            encoded.push_str("retry: ");
            encoded.push_str(&retry.as_millis().to_string());
            encoded.push('\n');
        }

        for line in self.data.lines() {
            encoded.push_str("data: ");
            encoded.push_str(line);
            encoded.push('\n');
        }

        if self.data.is_empty() {
            encoded.push_str("data:\n");
        }

        encoded.push('\n');
        encoded.into_bytes()
    }
}

struct EncodedSseStream<S> {
    stream: S,
}

impl<S: SseStream> ResponseStream for EncodedSseStream<S> {
    fn poll_next(
        &mut self,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Vec<u8>, StreamError>>> {
        self.stream
            .poll_next(context)
            .map(|next| next.map(|event| event.map(SseEvent::encode)))
    }
}

fn sanitize(value: &str) -> String {
    value.replace(['\r', '\n'], "")
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
