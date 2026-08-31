//! One line per request: method, path, status, elapsed ms — never headers or
//! bodies, so a bearer token or event payload can never end up in a log line
//! (architecture.md §6 auth model; task instructions: "NEVER log tokens or
//! event payloads").

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use std::time::Instant;

pub async fn log_requests(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let start = Instant::now();
    let response = next.run(req).await;
    let status = response.status().as_u16();
    let elapsed_ms = start.elapsed().as_millis();
    tracing::info!(%method, %path, status, elapsed_ms, "request");
    response
}
