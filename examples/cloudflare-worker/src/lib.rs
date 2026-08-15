use std::sync::LazyLock;

use serverkit::{
    Config, Response as ServerResponse, RouteMethods, Router, WebSocketMessage, WebSocketUpgrade,
};
use serverkit_worker::{WorkerContext, from_request, into_response};
use worker::{Context, Env, Request, Response, Result, event};

static ROUTER: LazyLock<Router> = LazyLock::new(router);

fn router() -> Router {
    Router::new(Config::new(), (
        "/health".GET(health),
        "/colo".GET(colo),
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
