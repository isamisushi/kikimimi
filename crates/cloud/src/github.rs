//! GitHub OAuth login (architecture.md §6.1 "主認証は GitHub OAuth。メール検証
//! 済み ID が得られる").
//!
//! `GET /auth/github` → 302 to GitHub's authorize endpoint, with a random
//! `state` value stashed in an HttpOnly cookie (CSRF defense: the callback
//! only accepts a `state` that round-trips through *this browser's* cookie,
//! so a state value alone — e.g. leaked via a Referer header — can't be
//! replayed by an attacker who hasn't also stolen the cookie). 503 when
//! `GITHUB_CLIENT_ID`/`GITHUB_CLIENT_SECRET` aren't both configured.
//!
//! `GET /auth/github/callback` → exchanges `code` for an access token, reads
//! `GET /user` + `GET /user/emails`, requires a primary *verified* email
//! (GitHub lets an account hide its email or leave it unverified — neither
//! is acceptable as an account identity here), upserts the account (by
//! `github_id`, falling back to linking an existing account by verified
//! email — see [`upsert_github_account`]'s doc comment for why that branch
//! also revokes every pre-existing `web_sessions` row on the account it
//! links to, and `web.rs`'s `login` for the matching legacy-login-side half
//! of that fix), ensures a personal org + owner membership, and sets the
//! same `kikimimi_session` cookie `POST /web/login` does, then 302s to `/`.
//!
//! `GITHUB_API_BASE`/`GITHUB_OAUTH_BASE` (`config.rs`) are overridable so
//! tests point this at a local mock instead of the real `github.com`/
//! `api.github.com` — task contract: no real GitHub calls in tests.

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use rand::RngExt as _;
use serde::Deserialize;

use crate::device::{constant_time_eq, ensure_personal_org};
use crate::error::AppError;
use crate::state::AppState;
use crate::web::{
    append_set_cookie, cookie_value, create_web_session, insert_set_cookie, session_cookie,
    SESSION_TTL_DAYS,
};

const OAUTH_STATE_COOKIE_NAME: &str = "kikimimi_oauth_state";
const OAUTH_STATE_TTL_SECS: i64 = 600;

fn generate_oauth_state() -> String {
    let mut bytes = [0u8; 24];
    rand::rng().fill(&mut bytes);
    hex::encode(bytes)
}

fn oauth_state_cookie(value: &str) -> String {
    format!("{OAUTH_STATE_COOKIE_NAME}={value}; HttpOnly; Secure; Path=/; SameSite=Lax; Max-Age={OAUTH_STATE_TTL_SECS}")
}

fn clear_oauth_state_cookie() -> String {
    format!("{OAUTH_STATE_COOKIE_NAME}=; HttpOnly; Secure; Path=/; SameSite=Lax; Max-Age=0")
}

/// 302 with an explicit `Location` header (not axum's `Redirect::to`, which
/// issues 303 See Other — architecture.md §6.1 literally specifies "302").
fn redirect_302(location: &str) -> Response {
    axum::response::Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, location)
        .body(axum::body::Body::empty())
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn require_github_config(state: &AppState) -> Result<(String, String), AppError> {
    match (&state.config.github_client_id, &state.config.github_client_secret) {
        (Some(id), Some(secret)) if !id.is_empty() && !secret.is_empty() => Ok((id.clone(), secret.clone())),
        _ => Err(AppError::ServiceUnavailable(
            "GitHub OAuth is not configured on this server (GITHUB_CLIENT_ID / GITHUB_CLIENT_SECRET)".into(),
        )),
    }
}

// ---------------------------------------------------------------------------
// GET /auth/github
// ---------------------------------------------------------------------------

pub async fn github_login(State(state): State<AppState>) -> Result<Response, AppError> {
    let (client_id, _secret) = require_github_config(&state)?;

    let oauth_state = generate_oauth_state();
    let redirect_uri = format!("{}/auth/github/callback", state.config.public_base_url);
    let query = serde_urlencoded::to_string([
        ("client_id", client_id.as_str()),
        ("redirect_uri", redirect_uri.as_str()),
        ("scope", "user:email"),
        ("state", oauth_state.as_str()),
    ])
    .map_err(|e| AppError::Internal(anyhow::anyhow!("encoding github authorize query: {e}")))?;

    let mut resp = redirect_302(&format!(
        "{}/login/oauth/authorize?{query}",
        state.config.github_oauth_base
    ));
    insert_set_cookie(&mut resp, &oauth_state_cookie(&oauth_state));
    Ok(resp)
}

// ---------------------------------------------------------------------------
// GET /auth/github/callback
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct GithubCallbackQuery {
    code: Option<String>,
    state: Option<String>,
}

#[derive(Deserialize)]
struct TokenExchangeResponse {
    access_token: Option<String>,
}

#[derive(Deserialize)]
struct GithubUser {
    id: i64,
    login: String,
}

#[derive(Deserialize)]
struct GithubEmail {
    email: String,
    primary: bool,
    verified: bool,
}

pub async fn github_callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<GithubCallbackQuery>,
) -> Result<Response, AppError> {
    let (client_id, client_secret) = require_github_config(&state)?;

    let code = q
        .code
        .filter(|c| !c.is_empty())
        .ok_or_else(|| AppError::BadRequest("missing code".into()))?;
    let given_state = q
        .state
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::BadRequest("missing state".into()))?;
    let cookie_state = cookie_value(&headers, OAUTH_STATE_COOKIE_NAME)
        .ok_or(AppError::Unauthorized("missing oauth state cookie"))?;
    if !constant_time_eq(given_state.as_bytes(), cookie_state.as_bytes()) {
        return Err(AppError::Unauthorized("oauth state mismatch"));
    }

    let redirect_uri = format!("{}/auth/github/callback", state.config.public_base_url);
    let token_body: TokenExchangeResponse = state
        .http_client
        .post(format!(
            "{}/login/oauth/access_token",
            state.config.github_oauth_base
        ))
        .header(header::ACCEPT, "application/json")
        .form(&[
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("code", code.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
        ])
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| AppError::Internal(anyhow::anyhow!("github token exchange failed: {e}")))?
        .json()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("github token exchange response: {e}")))?;
    let access_token = token_body.access_token.ok_or(AppError::Unauthorized(
        "github did not return an access token",
    ))?;

    let user: GithubUser = state
        .http_client
        .get(format!("{}/user", state.config.github_api_base))
        .bearer_auth(&access_token)
        .header(header::USER_AGENT, "kikimimi-cloud")
        .header(header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| AppError::Internal(anyhow::anyhow!("github GET /user failed: {e}")))?
        .json()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("github GET /user response: {e}")))?;

    let emails: Vec<GithubEmail> = state
        .http_client
        .get(format!("{}/user/emails", state.config.github_api_base))
        .bearer_auth(&access_token)
        .header(header::USER_AGENT, "kikimimi-cloud")
        .header(header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| AppError::Internal(anyhow::anyhow!("github GET /user/emails failed: {e}")))?
        .json()
        .await
        .map_err(|e| {
            AppError::Internal(anyhow::anyhow!("github GET /user/emails response: {e}"))
        })?;

    // Never fall back to an unverified/unprimary email — architecture.md
    // §6.1's whole rationale for GitHub OAuth over the legacy flow is "メール
    // 検証済み ID が得られる".
    let email = emails
        .into_iter()
        .find(|e| e.primary && e.verified)
        .map(|e| e.email)
        .ok_or_else(|| {
            AppError::UnprocessableEntity("GitHub account has no primary verified email".into())
        })?;

    let mut tx = state
        .pools
        .superuser
        .begin()
        .await
        .map_err(anyhow::Error::from)?;
    let account_id = upsert_github_account(&mut tx, user.id, &user.login, &email).await?;
    let org_id = ensure_personal_org(&mut tx, account_id, &email).await?;
    let token = create_web_session(&mut tx, account_id, org_id).await?;
    tx.commit().await.map_err(anyhow::Error::from)?;

    let mut resp = redirect_302("/");
    insert_set_cookie(
        &mut resp,
        &session_cookie(&token, SESSION_TTL_DAYS * 24 * 60 * 60),
    );
    append_set_cookie(&mut resp, &clear_oauth_state_cookie());
    Ok(resp)
}

/// Upsert order (account-model contract): by `github_id` first (an account
/// that has logged in via GitHub before), else link an existing account by
/// verified email (a pre-existing legacy email+invite account signing in
/// with GitHub for the first time), else create a brand-new account.
async fn upsert_github_account(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    github_id: i64,
    github_login: &str,
    email: &str,
) -> Result<uuid::Uuid, AppError> {
    if let Some((id,)) =
        sqlx::query_as::<_, (uuid::Uuid,)>("SELECT id FROM accounts WHERE github_id = $1")
            .bind(github_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(anyhow::Error::from)?
    {
        sqlx::query("UPDATE accounts SET github_login = $2, email = $3 WHERE id = $1")
            .bind(id)
            .bind(github_login)
            .bind(email)
            .execute(&mut **tx)
            .await
            .map_err(anyhow::Error::from)?;
        return Ok(id);
    }

    if let Some((id,)) =
        sqlx::query_as::<_, (uuid::Uuid,)>("SELECT id FROM accounts WHERE email = $1")
            .bind(email)
            .fetch_optional(&mut **tx)
            .await
            .map_err(anyhow::Error::from)?
    {
        sqlx::query("UPDATE accounts SET github_id = $2, github_login = $3 WHERE id = $1")
            .bind(id)
            .bind(github_id)
            .bind(github_login)
            .execute(&mut **tx)
            .await
            .map_err(anyhow::Error::from)?;

        // Security: this branch links a *pre-existing* account (created via
        // the legacy email+invite-code login, which — unlike this GitHub
        // callback — never proves mailbox ownership; see web.rs's `login`)
        // to a GitHub identity whose email GitHub itself just verified.
        // Anyone who knew the deployment's shared invite code could have
        // logged into this exact account, under this exact email, at any
        // point before the real owner ever touched GitHub, and may still be
        // holding a live `kikimimi_session` cookie for it right now. The
        // instant we have real proof of ownership, revoke every browser
        // session minted before that proof existed (this same transaction
        // mints a brand new one for the just-authenticated caller right
        // after this function returns, in `github_callback`) so an
        // unverified party's access never silently carries over onto the
        // now-verified account — the OAuth login rotates the session
        // outright rather than merely adding to whatever was already there.
        sqlx::query(
            "UPDATE web_sessions SET revoked = true WHERE account_id = $1 AND revoked = false",
        )
        .bind(id)
        .execute(&mut **tx)
        .await
        .map_err(anyhow::Error::from)?;

        return Ok(id);
    }

    let (id,): (uuid::Uuid,) = sqlx::query_as(
        "INSERT INTO accounts (email, github_id, github_login) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(email)
    .bind(github_id)
    .bind(github_login)
    .fetch_one(&mut **tx)
    .await
    .map_err(anyhow::Error::from)?;
    Ok(id)
}
