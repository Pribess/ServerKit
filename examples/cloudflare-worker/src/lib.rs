use std::{
    sync::LazyLock,
    task::{Context as TaskContext, Poll},
};

use serverkit::{
    Chunk, Config, Multipart, MultipartError, Response as ServerResponse, ResponseStream,
    RouteMethods, Router, StreamError, WebSocketMessage, WebSocketUpgrade,
};
use serverkit_worker::{WorkerContext, from_request, into_response};
use worker::{Context, Env, Request, Response, Result, event};

static ROUTER: LazyLock<Router> = LazyLock::new(router);

fn router() -> Router {
    Router::new(Config::new(), (
        "/health".GET(health),
        "/colo".GET(colo),
        "/stream".GET(stream),
        "/upload".POST(upload),
        "/ws".GET(websocket),
    ))
}

async fn health() -> &'static str {
    "ok"
}

async fn colo(context: WorkerContext) -> String {
    context
        .cf()
        .map_or_else(|| "unknown".to_owned(), |cf| cf.colo())
}

struct TestStream {
    remaining: usize,
}

impl ResponseStream for TestStream {
    fn poll_next(
        &mut self,
        _context: &mut TaskContext<'_>,
    ) -> Poll<Option<Result<Chunk, StreamError>>> {
        if self.remaining == 0 {
            return Poll::Ready(None);
        }

        self.remaining -= 1;
        Poll::Ready(Some(Ok(Chunk::from(vec![
            self.remaining as u8;
            64 * 1024
        ]))))
    }
}

async fn stream() -> ServerResponse {
    let mut response = ServerResponse::stream(200, TestStream { remaining: 16 });
    response
        .headers()
        .set("Content-Type", "application/octet-stream")
        .unwrap();
    response
}

async fn upload(mut multipart: Multipart) -> Result<String, MultipartError> {
    let mut fields = 0;
    let mut chunks = 0;
    let mut bytes = 0;

    while let Some(field) = multipart.next().await {
        let mut field = field?;
        fields += 1;

        while let Some(chunk) = field.next().await {
            chunks += 1;
            bytes += chunk?.len();
        }
    }

    Ok(format!("fields={fields},chunks={chunks},bytes={bytes}"))
}

async fn websocket(upgrade: WebSocketUpgrade) -> ServerResponse {
    upgrade.on_upgrade(|mut socket| async move {
        while let Some(message) = socket.next().await {
            match message {
                Ok(WebSocketMessage::Text(text)) => {
                    if socket.send_text(text).await.is_err() {
                        break;
                    }
                }
                Ok(WebSocketMessage::Binary(bytes)) => {
                    if socket.send_binary(bytes).await.is_err() {
                        break;
                    }
                }
                Ok(WebSocketMessage::Close { .. }) | Err(_) => break,
                Ok(WebSocketMessage::Ping(_) | WebSocketMessage::Pong(_)) => {}
            }
        }
    })
}

#[event(fetch)]
async fn fetch(request: Request, env: Env, context: Context) -> Result<Response> {
    into_response(ROUTER.handle(from_request(request, env, context)?).await)
}
