use anyhow::Context;

use kikimimi_cloud::build_router;
use kikimimi_cloud::config::Config;
use kikimimi_cloud::db::Pools;
use kikimimi_cloud::state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config = Config::from_env();
    tracing::info!(bind_addr = %config.bind_addr, "starting kikimimi-cloud");

    let pools = Pools::connect(&config.database_url, &config.app_db_password)
        .await
        .context("connecting to Postgres (includes running migrations)")?;

    let bind_addr = config.bind_addr.clone();
    let state = AppState::new(pools, config);
    let app = build_router(state);

    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("binding {bind_addr}"))?;
    tracing::info!(addr = %bind_addr, "kikimimi-cloud listening");
    axum::serve(listener, app).await.context("serving")?;
    Ok(())
}
