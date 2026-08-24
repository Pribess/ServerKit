use serverkit_hyper::*;

async fn health() -> &'static str {
    "ok"
}

fn main() -> std::io::Result<()> {
    let listener = std::net::TcpListener::bind("127.0.0.1:3000")?;

    Router::new(Config::new(), ("/health".GET(health),)).run(listener)
}
