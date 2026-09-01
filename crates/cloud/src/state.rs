use std::sync::Arc;

use tokio::sync::Semaphore;

use crate::config::Config;
use crate::db::Pools;
use crate::rate_limit::LoginRateLimiter;

#[derive(Clone)]
pub struct AppState {
    pub pools: Pools,
    pub config: Arc<Config>,
    /// Crude overload guard for `POST /v1/events` (architecture.md §6
    /// "輻輳時は 429 + Retry-After"). A bounded number of ingest requests may
    /// run at once; anything beyond that gets 429 immediately instead of
    /// queueing, so CI's "everyone flushes at once" case degrades predictably.
    pub ingest_semaphore: Arc<Semaphore>,
    /// `POST /web/login` brute-force guard (10 failures / 10 min / email —
    /// see `rate_limit.rs` module docs for the single-instance caveat).
    pub login_rate_limiter: Arc<LoginRateLimiter>,
    /// Shared client for outbound GitHub OAuth calls (`github.rs`) —
    /// `GITHUB_OAUTH_BASE`/`GITHUB_API_BASE` in `config` decide where these
    /// actually go, so tests point it at a local mock instead of real
    /// GitHub. One client is reused across requests (connection pooling)
    /// rather than built per-request.
    pub http_client: reqwest::Client,
}

impl AppState {
    pub const INGEST_CONCURRENCY: usize = 64;

    pub fn new(pools: Pools, config: Config) -> Self {
        Self {
            pools,
            config: Arc::new(config),
            ingest_semaphore: Arc::new(Semaphore::new(Self::INGEST_CONCURRENCY)),
            login_rate_limiter: Arc::new(LoginRateLimiter::default()),
            http_client: reqwest::Client::builder()
                .user_agent("kikimimi-cloud")
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }
}
