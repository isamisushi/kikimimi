//! Isolated real web router for the browser E2E; does not start collectors.
use std::path::PathBuf;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let data_dir = PathBuf::from(std::env::args().nth(1).expect("data directory"));
    kikimimi_sink::ensure_schema_stub(&data_dir)?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    println!("http://{}", listener.local_addr()?);
    let app = kikimimi_cli::web::router(kikimimi_cli::web::WebAppState {
        token: "usage-e2e-token".into(),
        data_dir,
    });
    axum::serve(listener, app).await?;
    Ok(())
}
