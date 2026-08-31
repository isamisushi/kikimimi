//! guru-cloud — minimal guru cloud server (architecture.md §12 Stage 0: "最小
//! guru cloud"). Library crate so integration tests (`tests/`) can build a
//! real `Router` against the live dev Postgres without going through a
//! spawned OS process.

pub mod auth;
pub mod config;
pub mod db;
pub mod device;
pub mod error;
pub mod export;
pub mod ingest;
pub mod logging;
pub mod query;
pub mod query_sql;
pub mod rate_limit;
pub mod state;
pub mod web;
pub mod web_query;
pub mod web_query_sql;

use std::time::Duration;

use axum::error_handling::HandleErrorLayer;
use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{BoxError, Router};
use tower::ServiceBuilder;

use state::AppState;

/// Anything above `ingest::MAX_COMPRESSED_BYTES` (5 MB) plus generous
/// slack for headers/framing; the ingest handler itself enforces the exact
/// 5 MB contract boundary and returns the JSON-shaped 413 body. This layer
/// is just the outer backstop against a wildly oversized request.
const GLOBAL_BODY_LIMIT: usize = 8 * 1024 * 1024;

/// Per-request processing timeout (security review: "no request/connection
/// timeouts on the HTTP server" — a slow-loris-style client, or many of them,
/// could otherwise hold a request open indefinitely; the `ingest_semaphore`
/// bounds concurrent *ingest* processing but nothing previously bounded how
/// long any single request — including reading a slow body — was allowed to
/// run). Generous relative to real handler work (export can stream a lot of
/// rows) but still a hard backstop.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/device/code", post(device::device_code))
        .route("/v1/device/token", post(device::device_token))
        .route("/v1/device/revoke", post(device::revoke))
        .route(
            "/activate",
            get(device::activate_get).post(device::activate_post),
        )
        .route("/v1/events", post(ingest::ingest))
        .route("/v1/query/{name}", get(query::named_query))
        .route("/v1/export", get(export::export))
        // Hosted web UI (WEB API CONTRACT): login/logout/me + the /web/q/*
        // data endpoints, RLS-scoped the same way /v1/query/* is (see
        // web.rs / web_query.rs module docs). Additive only -- every /v1/*
        // route above is untouched.
        .route("/web/login", post(web::login))
        .route("/web/logout", post(web::logout))
        .route("/web/me", get(web::me))
        .route("/web/q/overview", get(web_query::overview))
        .route("/web/q/machines", get(web_query::machines))
        .route("/web/q/tools", get(web_query::tools))
        .route("/web/q/mcp", get(web_query::mcp))
        .route("/web/q/sessions", get(web_query::sessions))
        // Anything else (including "/") serves the built SPA (web.rs) --
        // registered last so it never shadows a route matched above.
        .fallback(get(web::serve_spa))
        .layer(DefaultBodyLimit::max(GLOBAL_BODY_LIMIT))
        .layer(axum::middleware::from_fn(logging::log_requests))
        .layer(
            // `.timeout()` (`tower::timeout::Timeout`) turns an elapsed
            // timeout into a `BoxError`, which isn't a valid axum error type
            // on its own (axum requires `Infallible`) — `HandleErrorLayer`
            // converts it into a real 408 response so the outer `Router`
            // stays infallible. Canonical axum recipe (see `axum::error_
            // handling::HandleErrorLayer` docs) — order matters: the error
            // handler must be the *outer* layer so it can see errors coming
            // from the timeout beneath it.
            ServiceBuilder::new()
                .layer(HandleErrorLayer::new(handle_timeout_error))
                .timeout(REQUEST_TIMEOUT),
        )
        .with_state(state)
}

async fn handle_timeout_error(err: BoxError) -> (StatusCode, &'static str) {
    if err.is::<tower::timeout::error::Elapsed>() {
        (StatusCode::REQUEST_TIMEOUT, "request timed out")
    } else {
        (StatusCode::INTERNAL_SERVER_ERROR, "internal error")
    }
}

async fn healthz() -> &'static str {
    "ok"
}
