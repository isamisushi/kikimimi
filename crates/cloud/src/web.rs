//! Hosted web UI: `POST /web/login` / `POST /web/logout` / `GET /web/me`
//! (WEB API CONTRACT) plus serving the built SPA (`web/dist`) — the
//! multi-tenant, invite-code-gated sibling of `kikimimi agent`'s local web UI
//! (`crates/cli/src/web.rs`, single-secret `kikimimi_local` cookie, no login
//! flow). Static-asset embedding/serving here mirrors that file's shape
//! (there is no shared crate to hang a common `RustEmbed` type off, so it's
//! duplicated rather than reused — see task notes); auth is entirely
//! different (real login, server-side sessions) since this deployment has no
//! single trusted local user.
//!
//! `/web/q/*` (the actual data endpoints) live in `web_query.rs`; this module
//! owns login/logout/me, [`WebSessionContext`] (the `/web/q/*` auth
//! extractor), and static asset serving.
//!
//! SESSION MODEL: `POST /web/login` creates a `web_sessions` row (migration
//! `0006_web_sessions.sql`) — `token_hash = sha256(token)`, `account_id`,
//! `org_id` pinned at login time (an account's personal org never changes,
//! see `device::ensure_personal_org`), `expires_at = now() + 30d`. The
//! plaintext `token` is handed to the browser exactly once, as the
//! `kikimimi_session` cookie value (HttpOnly + Secure + SameSite=Lax, task
//! spec), and never stored. Every `/web/q/*` request and `GET /web/me`
//! resolves that cookie back to `(account_id, org_id, email)` via
//! [`WebSessionContext`], which — like `auth::AuthContext` for bearer
//! tokens — runs on the SUPERUSER pool (`web_sessions`/`accounts` are never
//! touched by the RLS-scoped `kikimimi_app` pool) and rejects a revoked or
//! expired row with 401.

use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use rust_embed::RustEmbed;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth::{generate_token, hash_token};
use crate::device::{constant_time_eq, ensure_account, ensure_personal_org};
use crate::error::AppError;
use crate::state::AppState;

#[derive(RustEmbed)]
#[folder = "../../web/dist"]
struct WebAssets;

/// WEB API CONTRACT: "set-cookie kikimimi_session". Distinct from `kikimimi agent`'s
/// local-mode `kikimimi_local` cookie (`crates/cli/src/web.rs`) — the two auth
/// schemes must never be confused with each other even in a build that
/// somehow links both.
pub const SESSION_COOKIE_NAME: &str = "kikimimi_session";
/// WEB API CONTRACT: "expires_at 30d". `pub(crate)`: `github.rs`'s OAuth
/// callback sets the same cookie with the same TTL.
pub(crate) const SESSION_TTL_DAYS: i64 = 30;

// ---------------------------------------------------------------------------
// WebSessionContext — the /web/q/*, /web/me, /activate, orgs.rs auth extractor
// ---------------------------------------------------------------------------

/// Identity attached to a request once the `kikimimi_session` cookie has been
/// resolved to a live (non-revoked, non-expired) `web_sessions` row. Mirrors
/// `auth::AuthContext`'s shape/role for bearer tokens.
///
/// `org_id` is the session's *active* org (account-model contract: `POST
/// /web/active-org` can change which org a session points at after login —
/// see `orgs::set_active_org`), not fixed for the session's whole lifetime
/// the way it was before that endpoint existed. Every `/web/q/*` handler
/// still just reads it fresh per-request, so a mid-session org switch takes
/// effect on the very next request with no extra plumbing.
#[derive(Debug, Clone)]
pub struct WebSessionContext {
    pub session_id: Uuid,
    pub account_id: Uuid,
    pub org_id: Uuid,
    pub email: String,
}

impl FromRequestParts<AppState> for WebSessionContext {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let token = cookie_value(&parts.headers, SESSION_COOKIE_NAME)
            .ok_or(AppError::Unauthorized("missing session cookie"))?;
        if token.is_empty() {
            return Err(AppError::Unauthorized("empty session cookie"));
        }

        let hash = hash_token(&token);
        let row: Option<(Uuid, Uuid, Uuid, String, bool, DateTime<Utc>)> = sqlx::query_as(
            "SELECT s.id, s.account_id, s.org_id, a.email, s.revoked, s.expires_at \
             FROM web_sessions s JOIN accounts a ON a.id = s.account_id \
             WHERE s.token_hash = $1",
        )
        .bind(&hash)
        .fetch_optional(&state.pools.superuser)
        .await
        .map_err(anyhow::Error::from)?;

        let (session_id, account_id, org_id, email, revoked, expires_at) =
            row.ok_or(AppError::Unauthorized("invalid or unknown session"))?;
        if revoked {
            return Err(AppError::Unauthorized("session revoked"));
        }
        if Utc::now() > expires_at {
            return Err(AppError::Unauthorized("session expired"));
        }

        Ok(WebSessionContext { session_id, account_id, org_id, email })
    }
}

/// Shared by `POST /web/login` (below) and `github.rs`'s OAuth callback:
/// inserts a `web_sessions` row and returns the plaintext token (handed to
/// the browser exactly once, as the cookie value, and never stored).
pub(crate) async fn create_web_session(
    conn: &mut sqlx::PgConnection,
    account_id: Uuid,
    org_id: Uuid,
) -> Result<String, AppError> {
    let (token, hash) = generate_token();
    let expires_at = Utc::now() + chrono::Duration::days(SESSION_TTL_DAYS);
    sqlx::query(
        "INSERT INTO web_sessions (account_id, org_id, token_hash, expires_at) VALUES ($1, $2, $3, $4)",
    )
    .bind(account_id)
    .bind(org_id)
    .bind(&hash)
    .bind(expires_at)
    .execute(&mut *conn)
    .await
    .map_err(anyhow::Error::from)?;
    Ok(token)
}

// ---------------------------------------------------------------------------
// GET /web/config
// ---------------------------------------------------------------------------

/// `GET /web/config` (additive glue for the SPA, unauthenticated -- the
/// login page needs this *before* there's any session): tells the SPA
/// which login paths are actually live on this deployment, mirroring the
/// exact same gate `login()` (below) and `github::require_github_config`
/// enforce server-side, so the SPA never has to guess or hardcode it.
///
/// `github_oauth`: `GET /auth/github` will do something other than 503
/// (both `GITHUB_CLIENT_ID` and `_SECRET` configured).
/// `legacy_login`: `POST /web/login` will do something other than 404 (the
/// account-model contract's gate: unset `GITHUB_CLIENT_ID`, or
/// `KIKIMIMI_LEGACY_INVITE=1`).
pub async fn config(State(state): State<AppState>) -> Json<Value> {
    let github_oauth = state.config.github_client_id.is_some() && state.config.github_client_secret.is_some();
    let legacy_login = state.config.github_client_id.is_none() || state.config.legacy_invite;
    Json(json!({ "github_oauth": github_oauth, "legacy_login": legacy_login }))
}

// ---------------------------------------------------------------------------
// POST /web/login
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct WebLoginRequest {
    email: String,
    #[serde(default)]
    invite_code: String,
}

/// `POST /web/login` (WEB API CONTRACT): `{email, invite_code}` → 200
/// `Set-Cookie: kikimimi_session=...` + `{email, org_id}`, or 403.
///
/// ACCOUNT-MODEL CONTRACT (architecture.md §6.1): this is the "legacy
/// email+invite login" — GitHub OAuth (`github.rs`) is the primary path now.
/// It keeps working as-is until a deployment configures GitHub OAuth: once
/// `GITHUB_CLIENT_ID` is set, this endpoint 404s (looks like it was never
/// routed at all) unless the operator explicitly opts back in with
/// `KIKIMIMI_LEGACY_INVITE=1` (e.g. to keep a CI/dev-only login path alive
/// alongside OAuth for humans). Checked before the rate limiter / credentials
/// so a disabled deployment doesn't leak any timing/blocking signal either.
///
/// Auth rule: `invite_code` must equal `KIKIMIMI_INVITE_CODE` (constant-time —
/// a network/timing attacker must not be able to learn the code one byte at
/// a time). An unconfigured `KIKIMIMI_INVITE_CODE` (`state.config.invite_code
/// == None`) fails every login the same way (nothing can equal a code that
/// doesn't exist) — fail-closed by construction.
///
/// `email` selects/creates the account + personal org exactly like device
/// activation does — `ensure_account` / `ensure_personal_org` are the same
/// functions `device.rs`'s `POST /v1/device/token` calls, reused (not
/// duplicated) here on a superuser-pool transaction of our own.
///
/// Rate-limited: `state.login_rate_limiter` blocks an email at 10 failures /
/// 10 min with 429, checked *before* looking at credentials at all (module
/// docs, `rate_limit.rs`).
pub async fn login(State(state): State<AppState>, Json(body): Json<WebLoginRequest>) -> Result<Response, AppError> {
    if state.config.github_client_id.is_some() && !state.config.legacy_invite {
        return Err(AppError::NotFound("not found".into()));
    }

    let email = body.email.trim().to_string();

    if state.login_rate_limiter.is_blocked(&email) {
        return Err(AppError::TooManyRequests { retry_after_secs: 60 });
    }

    let credentials_ok = !email.is_empty()
        && email.contains('@')
        && state
            .config
            .invite_code
            .as_deref()
            .is_some_and(|expected| constant_time_eq(body.invite_code.trim().as_bytes(), expected.as_bytes()));

    if !credentials_ok {
        state.login_rate_limiter.record_failure(&email);
        return Ok(json_response(
            StatusCode::FORBIDDEN,
            &json!({ "error": "invalid email or invite code" }),
        ));
    }
    state.login_rate_limiter.clear(&email);

    let mut tx = state.pools.superuser.begin().await.map_err(anyhow::Error::from)?;
    let account_id = ensure_account(&mut tx, &email).await?;

    // Security: once an account has a GitHub identity linked, the legacy
    // path must never be able to authenticate as it again. `invite_code` is
    // a single secret shared by the whole deployment, not proof of *this*
    // email's ownership — without this check, anyone who knows the invite
    // code could keep logging in as an arbitrary teammate's address even
    // after that teammate proved ownership via GitHub OAuth's verified
    // email (github.rs's `upsert_github_account` "link by email" branch),
    // inheriting whatever that now-verified account can see. GitHub OAuth
    // is the only way in for a linked account from here on.
    let github_linked: Option<(bool,)> =
        sqlx::query_as("SELECT github_id IS NOT NULL FROM accounts WHERE id = $1")
            .bind(account_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(anyhow::Error::from)?;
    if github_linked.is_some_and(|(linked,)| linked) {
        return Ok(json_response(
            StatusCode::FORBIDDEN,
            &json!({ "error": "this account is linked to GitHub — sign in with GitHub instead" }),
        ));
    }

    let org_id = ensure_personal_org(&mut tx, account_id, &email).await?;
    let token = create_web_session(&mut tx, account_id, org_id).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;

    let mut resp = json_response(StatusCode::OK, &json!({ "email": email, "org_id": org_id }));
    insert_set_cookie(&mut resp, &session_cookie(&token, SESSION_TTL_DAYS * 24 * 60 * 60));
    Ok(resp)
}

// ---------------------------------------------------------------------------
// POST /web/logout
// ---------------------------------------------------------------------------

/// Clears the cookie and, if it named a real session, marks that
/// `web_sessions` row `revoked` (kept, not deleted, for audit) so a copy of
/// the cookie made before logout can't be replayed afterwards.
/// No-cookie / already-invalid cookie is not an error — logging out is
/// idempotent, matching `web/mock/server.mjs`'s behavior.
pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, AppError> {
    if let Some(token) = cookie_value(&headers, SESSION_COOKIE_NAME) {
        let hash = hash_token(&token);
        sqlx::query("UPDATE web_sessions SET revoked = true WHERE token_hash = $1")
            .bind(&hash)
            .execute(&state.pools.superuser)
            .await
            .map_err(anyhow::Error::from)?;
    }
    let mut resp = json_response(StatusCode::OK, &json!({ "ok": true }));
    insert_set_cookie(&mut resp, &clear_session_cookie());
    Ok(resp)
}

// ---------------------------------------------------------------------------
// GET /web/me
// ---------------------------------------------------------------------------

/// `GET /web/me` (account-model contract) → `{email, github_login, orgs:
/// [{slug,name,kind,role}], active_org}` | 401. The 401 case never reaches
/// this handler body at all — [`WebSessionContext`]'s `FromRequestParts`
/// rejection *is* the 401 (same pattern as `auth::AuthContext`).
/// `active_org` is the active org's *slug* — a pointer into `orgs`, not a
/// duplicate of one of its entries.
pub async fn me(State(state): State<AppState>, session: WebSessionContext) -> Result<Json<Value>, AppError> {
    let (email, github_login): (String, Option<String>) =
        sqlx::query_as("SELECT email, github_login FROM accounts WHERE id = $1")
            .bind(session.account_id)
            .fetch_one(&state.pools.superuser)
            .await
            .map_err(anyhow::Error::from)?;

    let orgs: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT o.slug, o.name, o.kind, m.role FROM memberships m JOIN orgs o ON o.id = m.org_id \
         WHERE m.account_id = $1 ORDER BY (o.kind = 'personal') DESC, o.name",
    )
    .bind(session.account_id)
    .fetch_all(&state.pools.superuser)
    .await
    .map_err(anyhow::Error::from)?;

    let (active_slug,): (String,) = sqlx::query_as("SELECT slug FROM orgs WHERE id = $1")
        .bind(session.org_id)
        .fetch_one(&state.pools.superuser)
        .await
        .map_err(anyhow::Error::from)?;

    Ok(Json(json!({
        "email": email,
        "github_login": github_login,
        "orgs": orgs.into_iter().map(|(slug, name, kind, role)| json!({
            "slug": slug, "name": name, "kind": kind, "role": role,
        })).collect::<Vec<_>>(),
        "active_org": active_slug,
    })))
}

// ---------------------------------------------------------------------------
// Cookie helpers -- `pub(crate)`: also used by `github.rs`'s OAuth callback
// (same `kikimimi_session` cookie) and `orgs.rs`'s `POST /web/active-org`.
// ---------------------------------------------------------------------------

pub(crate) fn session_cookie(token: &str, max_age_secs: i64) -> String {
    format!("{SESSION_COOKIE_NAME}={token}; HttpOnly; Secure; Path=/; SameSite=Lax; Max-Age={max_age_secs}")
}

fn clear_session_cookie() -> String {
    format!("{SESSION_COOKIE_NAME}=; HttpOnly; Secure; Path=/; SameSite=Lax; Max-Age=0")
}

pub(crate) fn insert_set_cookie(resp: &mut Response, cookie: &str) {
    if let Ok(value) = header::HeaderValue::from_str(cookie) {
        resp.headers_mut().insert(header::SET_COOKIE, value);
    }
}

/// Like [`insert_set_cookie`] but *adds* a `Set-Cookie` header instead of
/// replacing any existing one — `HeaderMap::insert` on a multi-value header
/// like `Set-Cookie` silently drops whatever was there before, which would
/// be wrong for `github.rs`'s callback (sets the session cookie *and* clears
/// the oauth-state cookie in the same response). Use this for every
/// `Set-Cookie` after the first one on a given response.
pub(crate) fn append_set_cookie(resp: &mut Response, cookie: &str) {
    if let Ok(value) = header::HeaderValue::from_str(cookie) {
        resp.headers_mut().append(header::SET_COOKIE, value);
    }
}

/// Finds `name`'s value in the `Cookie` header (`k=v; k2=v2` pairs). No
/// percent-decoding: our own cookie value is a plain base64url string
/// (`auth::generate_token`), never encoded — same as `crates/cli/src/
/// web.rs`'s `cookie_value`.
pub(crate) fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';').find_map(|part| {
        part.trim()
            .strip_prefix(name)
            .and_then(|rest| rest.strip_prefix('='))
            .map(str::to_string)
    })
}

fn json_response(status: StatusCode, body: &Value) -> Response {
    axum::response::Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

// ---------------------------------------------------------------------------
// Static SPA serving (mirrors crates/cli/src/web.rs's serve_spa/serve_asset)
// ---------------------------------------------------------------------------

/// Fallback for every path not matched by an explicit route: `"/"` and every
/// other unrecognized path serve `index.html` so the SPA's client-side
/// router can render it (task spec: '"/" serves the app (no ?t= token flow
/// in hosted mode — login page handles auth)' — hosted mode has no tokened-
/// URL flow at all, so unlike the local `kikimimi agent` UI's `handle_root`,
/// there is nothing special about `"/"` here). Real embedded assets
/// (`/assets/x.js`) serve themselves first since axum matches this only when
/// nothing more specific did.
pub async fn serve_spa(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    serve_asset(if path.is_empty() { "index.html" } else { path })
}

fn serve_asset(path: &str) -> Response {
    if let Some(file) = WebAssets::get(path) {
        return asset_response(file.metadata.mimetype(), file.data);
    }
    // A path whose last segment has no "." is a client-side route (no real
    // asset could ever match it), not a typo'd asset request -- fall back to
    // index.html. A path that does look like an asset request
    // ("/assets/x.js") and still isn't embedded is a genuine 404.
    let looks_like_asset = path.rsplit('/').next().is_some_and(|last| last.contains('.'));
    if !looks_like_asset {
        if let Some(file) = WebAssets::get("index.html") {
            return asset_response("text/html; charset=utf-8", file.data);
        }
    }
    (StatusCode::NOT_FOUND, "not found").into_response()
}

fn asset_response(mime: &str, data: std::borrow::Cow<'static, [u8]>) -> Response {
    axum::response::Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .body(axum::body::Body::from(data.into_owned()))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_value_finds_the_named_cookie_among_several() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            "foo=bar; kikimimi_session=abc123; other=1".parse().unwrap(),
        );
        assert_eq!(cookie_value(&headers, SESSION_COOKIE_NAME), Some("abc123".to_string()));
        assert_eq!(cookie_value(&headers, "missing"), None);
    }

    #[test]
    fn cookie_value_does_not_prefix_match_a_longer_cookie_name() {
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, "kikimimi_session_extra=x".parse().unwrap());
        assert_eq!(cookie_value(&headers, SESSION_COOKIE_NAME), None);
    }

    #[test]
    fn cookie_value_none_when_no_cookie_header() {
        assert_eq!(cookie_value(&HeaderMap::new(), SESSION_COOKIE_NAME), None);
    }

    #[test]
    fn session_cookie_has_the_documented_attributes() {
        let c = session_cookie("tok", 2_592_000);
        assert!(c.starts_with("kikimimi_session=tok;"));
        assert!(c.contains("HttpOnly"));
        assert!(c.contains("Secure"));
        assert!(c.contains("SameSite=Lax"));
        assert!(c.contains("Max-Age=2592000"));
    }

    #[test]
    fn clear_session_cookie_expires_immediately() {
        assert!(clear_session_cookie().contains("Max-Age=0"));
    }
}
