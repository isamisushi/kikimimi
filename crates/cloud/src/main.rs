use anyhow::Context;

use guru_cloud::config::Config;
use guru_cloud::db::Pools;
use guru_cloud::state::AppState;
use guru_cloud::build_router;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(
            |_| tracing_subscriber::EnvFilter::new("info"),
        ))
        .init();

    let config = Config::from_env();
    tracing::info!(bind_addr = %config.bind_addr, "starting guru-cloud");

    let pools = Pools::connect(&config.database_url, &config.app_db_password)
        .await
        .context("connecting to Postgres (includes running migrations)")?;

    let bind_addr = config.bind_addr.clone();
    let state = AppState::new(pools, config);
    let app = build_router(state);

    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("binding {bind_addr}"))?;
    tracing::info!(addr = %bind_addr, "guru-cloud listening");
    axum::serve(listener, app)
        .await
        .context("serving")?;
    Ok(())
}
