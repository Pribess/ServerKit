use serverkit_hyper::*;

async fn health() -> &'static str {
    "ok"
}

fn main() -> std::io::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .build()?;

    runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await?;

        Router::new(Config::new(), ("/health".GET(health),))
            .run(listener)
            .await
    })
}
