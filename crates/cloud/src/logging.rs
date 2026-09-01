//! One line per request: method, path, status, elapsed ms — never headers or
//! bodies, so a bearer token or event payload can never end up in a log line
//! (architecture.md §6 auth model; task instructions: "NEVER log tokens or
//! event payloads").
//!
//! Bearer/session/device tokens never appear in a URL at all (Authorization
//! header / cookie only), so logging the raw path is normally safe. Org
//! invite links (account-model contract) are the exception: both `GET`/
//! `POST /join/<token>` (`orgs::join_post`) and `GET /web/invites/<token>`
//! (`orgs::invite_info`, the SPA's pre-join preview call — same plaintext,
//! membership-granting token, just a different route) put the actual secret
//! directly in the path (same shape as GitHub's own invite links), so both
//! are redacted before they ever reach `%path` below — never logged, same
//! rule as every other token in this codebase. (Security review: the SPA's
//! preview call was originally missed here — `/join/<token>` was redacted
//! but `/web/invites/<token>` carries the identical secret and was not.)

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use std::time::Instant;

pub async fn log_requests(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    // Owned (not borrowed from `req`) so it outlives the `next.run(req)` move
    // below.
    let path = redact_join_token(req.uri().path());
    let start = Instant::now();
    let response = next.run(req).await;
    let status = response.status().as_u16();
    let elapsed_ms = start.elapsed().as_millis();
    tracing::info!(%method, %path, status, elapsed_ms, "request");
    response
}

/// `/join/<token>` -> `/join/<redacted>`, `/web/invites/<token>` ->
/// `/web/invites/<redacted>`. Any other path is returned unchanged (still an
/// owned `String` — the caller needs one that outlives the request it was
/// read from).
fn redact_join_token(path: &str) -> String {
    for prefix in ["/join/", "/web/invites/"] {
        if let Some(rest) = path.strip_prefix(prefix) {
            if !rest.is_empty() {
                return format!("{prefix}<redacted>");
            }
        }
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_the_invite_token_in_a_join_path() {
        assert_eq!(
            redact_join_token("/join/abcdef0123456789"),
            "/join/<redacted>"
        );
    }

    /// Security review: `GET /web/invites/:token` (the SPA's pre-join
    /// preview call, `orgs::invite_info`) carries the exact same plaintext
    /// invite secret as `/join/:token` and must be redacted identically —
    /// this was the gap (route existed, redaction didn't cover it).
    #[test]
    fn redacts_the_invite_token_in_a_web_invites_preview_path() {
        assert_eq!(
            redact_join_token("/web/invites/abcdef0123456789"),
            "/web/invites/<redacted>"
        );
    }

    #[test]
    fn leaves_other_paths_untouched() {
        assert_eq!(redact_join_token("/web/me"), "/web/me");
        assert_eq!(redact_join_token("/activate"), "/activate");
        assert_eq!(redact_join_token("/join/"), "/join/");
        assert_eq!(redact_join_token("/web/invites/"), "/web/invites/");
        assert_eq!(
            redact_join_token("/web/orgs/acme/invites"),
            "/web/orgs/acme/invites"
        );
    }
}
