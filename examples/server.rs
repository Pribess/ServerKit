use std::net::TcpListener;

use serverkit::prelude::*;

async fn health() -> &'static str {
    "ok"
}

fn main() -> std::io::Result<()> {
    Router::new(Config::new(), ("/health".GET(health),)).run(TcpListener::bind("127.0.0.1:3000")?)
}
