use serverkit::prelude::*;
use serverkit_hyper::Http1;

async fn health() -> &'static str {
    "ok"
}

fn main() -> std::io::Result<()> {
    Router::new(Config::new(), ("/health".GET(health),)).run(Http1::bind("127.0.0.1:3000")?)
}
