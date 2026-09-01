//! kikimimi-cloud — minimal kikimimi cloud server (architecture.md §12 Stage 0: "最小
//! kikimimi cloud"). Library crate so integration tests (`tests/`) can build a
//! real `Router` against the live dev Postgres without going through a
//! spawned OS process.

pub mod auth;
pub mod config;
pub mod db;
pub mod device;
pub mod error;
pub mod export;
pub mod github;
pub mod ingest;
pub mod logging;
pub mod orgs;
pub mod query;
pub mod query_sql;
pub mod rate_limit;
pub mod roles;
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
        // Bearer-token device management for the CLI (`kikimimi devices` /
        // `kikimimi devices revoke <id>`) -- the session-cookie counterpart
        // to `/web/devices*` below, see device.rs module docs.
        .route("/v1/devices", get(device::list_devices_v1))
        .route("/v1/devices/{id}/revoke", post(device::revoke_device_v1))
        // Bearer-token counterpart to `GET /web/me`'s `orgs` list, for the
        // CLI's `kikimimi orgs` (crates/cli/src/orgs_cmd.rs, orgs.rs module
        // docs).
        .route("/v1/orgs", get(orgs::list_orgs_v1))
        .route("/v1/events", post(ingest::ingest))
        .route("/v1/query/{name}", get(query::named_query))
        .route("/v1/export", get(export::export))
        // Hosted web UI (WEB API CONTRACT): login/logout/me + the /web/q/*
        // data endpoints, RLS-scoped the same way /v1/query/* is (see
        // web.rs / web_query.rs module docs). Additive only -- every /v1/*
        // route above is untouched.
        .route("/web/config", get(web::config))
        .route("/web/login", post(web::login))
        .route("/web/logout", post(web::logout))
        .route("/web/me", get(web::me))
        .route("/web/q/overview", get(web_query::overview))
        .route("/web/q/machines", get(web_query::machines))
        .route("/web/q/tools", get(web_query::tools))
        .route("/web/q/mcp", get(web_query::mcp))
        .route("/web/q/skills", get(web_query::skills))
        .route("/web/q/sessions", get(web_query::sessions))
        .route("/web/q/members", get(web_query::members))
        // GitHub OAuth (account-model contract, architecture.md §6.1):
        // primary login path once GITHUB_CLIENT_ID/_SECRET are configured.
        .route("/auth/github", get(github::github_login))
        .route("/auth/github/callback", get(github::github_callback))
        // Org/team management (account-model contract "Org/team API"):
        // create a team org, switch active org, per-org invite links, and
        // device listing/revocation across an org. All session-authed +
        // role-enforced (orgs.rs / roles.rs).
        .route("/web/orgs", post(orgs::create_org))
        .route("/web/active-org", post(orgs::set_active_org))
        .route(
            "/web/orgs/{slug}/invites",
            get(orgs::list_invites).post(orgs::create_invite),
        )
        .route(
            "/web/orgs/{slug}/invites/{id}",
            axum::routing::delete(orgs::revoke_invite),
        )
        .route("/web/orgs/{slug}/members", get(orgs::list_members))
        // GET here is the SPA shell (React owns the confirmation view via
        // GET /web/invites/:token below), not a handler in orgs.rs -- see
        // that module's "GET /join/:token" doc comment.
        .route("/join/{token}", get(web::serve_spa).post(orgs::join_post))
        .route("/web/invites/{token}", get(orgs::invite_info))
        .route("/web/devices", get(orgs::list_devices))
        .route("/web/devices/{id}/revoke", post(orgs::revoke_device))
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
