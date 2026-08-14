use std::sync::LazyLock;

use serverkit::{
    App, Response as ServerResponse, RouteMethods, WebSocketMessage, WebSocketUpgrade,
    cloudflare::{self, WorkerContext},
};
use worker::{Context, Env, Request, Response, Result, event};

static APP: LazyLock<App> = LazyLock::new(application);

fn application() -> App {
    App::new((
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
    cloudflare::into_response(
        APP.handle(cloudflare::from_request(request, env, context)?)
            .await,
    )
}
