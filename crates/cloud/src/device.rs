//! Device authorization grant flow (API contract, architecture.md §6):
//!
//! `POST /v1/device/code` → `GET /activate?code=<user_code>` (HTML) →
//! `POST /activate {code, email}` (approves) → `POST /v1/device/token`
//! (polled by the CLI; materializes the account/org/device + mints the
//! bearer token exactly once, the moment it's first observed approved).
//!
//! All DB access here is on the SUPERUSER pool: accounts/orgs/org_members/
//! devices/device_codes are never touched by the RLS-scoped `guru_app` pool.
//!
//! SECURITY NOTE (reviewed, accepted risk for Stage 0, not fixed by a code
//! patch here): `POST /activate` trusts the caller-supplied `email` with no
//! proof of mailbox ownership (no magic link / OTP) — this is the API
//! contract's literal, frozen shape ("POST /activate {code, email} approves
//! ... issues token"), and matches architecture.md §11's explicit Stage 0
//! scope ("cloud 側の運用は第三者検証が整うまで自己申告に留まる"). Anyone who
//! can complete their own device-code flow can therefore mint a token bound
//! to any email's existing account/org by typing that email into the approve
//! form. Closing this for real needs an email-confirmation round trip (new
//! infra, and a different `/activate` response shape/timing than the
//! contract specifies) — out of scope for a contract-compatible patch; flag
//! for a follow-up decision before Stage 1 opens this up beyond trusted
//! dev/pilot use. What *is* fixed here: `POST /v1/device/revoke` (below) so
//! a compromised token can be killed server-side via `guru logout`, instead
//! of only ever being deleted from the local config file.
//!
//! INVITE CODE GATE (public deployment): with `GURU_INVITE_CODE` set,
//! `POST /activate` additionally requires a matching `invite_code` field
//! (constant-time compared — never `==` — so a network observer/timing
//! attacker can't learn the code one byte at a time). This is the only line
//! of defense against open self-registration once the server is reachable
//! from the public internet, so the server *fails closed*: if neither
//! `GURU_INVITE_CODE` nor `GURU_DEV_AUTOAPPROVE` is configured, `POST
//! /activate` refuses outright with 503 rather than silently running open
//! registration. A `user_code`'s wrong-invite-code attempts are counted
//! (`device_codes.invite_attempts`); at the threshold the row is expired
//! early so a brute-force script can't sit there guessing forever.

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse};
use axum::{body::Bytes, Json};
use chrono::{DateTime, Utc};
use rand::RngExt as _;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::auth::{generate_token, AuthContext};
use crate::error::AppError;
use crate::state::AppState;

const CODE_TTL_MINUTES: i64 = 10;
const POLL_INTERVAL_SECS: u64 = 2;
const USER_CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789"; // no 0/O/1/I/L
/// Wrong-invite-code attempts a single `user_code` tolerates before its
/// `device_codes` row is expired early (brute-force backstop).
const MAX_INVITE_ATTEMPTS: i32 = 5;

/// Constant-time byte comparison for the invite code (manual fold-over-bytes
/// — deliberately not `==`/`PartialEq`, which short-circuits on the first
/// differing byte and would leak a timing side-channel an attacker could use
/// to guess the invite code one byte at a time). Comparing every byte
/// unconditionally and only combining results with `|=` keeps the number of
/// operations independent of *where* the two inputs first differ.
pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn generate_device_code() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    hex::encode(bytes)
}

fn generate_user_code() -> String {
    let mut rng = rand::rng();
    let part = |rng: &mut rand::rngs::ThreadRng| -> String {
        (0..4)
            .map(|_| {
                let idx = rng.random_range(0..USER_CODE_ALPHABET.len());
                USER_CODE_ALPHABET[idx] as char
            })
            .collect()
    };
    format!("{}-{}", part(&mut rng), part(&mut rng))
}

// ---------------------------------------------------------------------------
// POST /v1/device/code
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct DeviceCodeRequest {
    host_id: String,
    #[serde(default)]
    hostname: Option<String>,
}

#[derive(Serialize)]
pub struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_url: String,
    interval_secs: u64,
}

pub async fn device_code(
    State(state): State<AppState>,
    Json(body): Json<DeviceCodeRequest>,
) -> Result<Json<DeviceCodeResponse>, AppError> {
    if body.host_id.trim().is_empty() {
        return Err(AppError::BadRequest("host_id must not be empty".into()));
    }

    // Opportunistic garbage collection: a `device_codes` row that expires
    // without ever being polled again previously lived forever (only the
    // *polled* code's row is deleted, in device_token below, and only once
    // materialized or seen expired). Piggybacking a cheap sweep onto every
    // `POST /v1/device/code` call (itself already infrequent — one per
    // `guru login`) bounds the table without needing a separate background
    // job (security review finding #7).
    sqlx::query("DELETE FROM device_codes WHERE expires_at < now()")
        .execute(&state.pools.superuser)
        .await
        .map_err(anyhow::Error::from)?;

    let device_code = generate_device_code();
    let user_code = generate_user_code();
    let expires_at = Utc::now() + chrono::Duration::minutes(CODE_TTL_MINUTES);

    // GURU_DEV_AUTOAPPROVE=1: pre-approve with GURU_DEV_EMAIL right away, for
    // tests/CI (architecture.md §12 Stage 0). Materialization (account/org/
    // device/token) still only happens on the first POST /v1/device/token
    // poll, same as the real approval path — see module docs.
    let (approved, account_email) = if state.config.dev_autoapprove {
        (true, Some(state.config.dev_email.clone()))
    } else {
        (false, None)
    };

    sqlx::query(
        "INSERT INTO device_codes (device_code, user_code, host_id, hostname, approved, account_email, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(&device_code)
    .bind(&user_code)
    .bind(&body.host_id)
    .bind(&body.hostname)
    .bind(approved)
    .bind(&account_email)
    .bind(expires_at)
    .execute(&state.pools.superuser)
    .await
    .map_err(anyhow::Error::from)?;

    Ok(Json(DeviceCodeResponse {
        device_code,
        verification_url: format!("{}/activate?code={}", state.config.public_base_url, user_code),
        user_code,
        interval_secs: POLL_INTERVAL_SECS,
    }))
}

// ---------------------------------------------------------------------------
// POST /v1/device/token
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct DeviceTokenRequest {
    device_code: String,
}

pub async fn device_token(
    State(state): State<AppState>,
    Json(body): Json<DeviceTokenRequest>,
) -> Result<axum::response::Response, AppError> {
    let mut tx = state
        .pools
        .superuser
        .begin()
        .await
        .map_err(anyhow::Error::from)?;

    // FOR UPDATE: serializes concurrent polls of the same device_code so
    // exactly one of them ever materializes the device + token below.
    let row: Option<(String, String, Option<String>, bool, Option<String>, DateTime<Utc>)> =
        sqlx::query_as(
            "SELECT device_code, host_id, hostname, approved, account_email, expires_at \
             FROM device_codes WHERE device_code = $1 FOR UPDATE",
        )
        .bind(&body.device_code)
        .fetch_optional(&mut *tx)
        .await
        .map_err(anyhow::Error::from)?;

    let Some((device_code, host_id, hostname, approved, account_email, expires_at)) = row else {
        return Ok(expired_response());
    };

    if !approved {
        if Utc::now() > expires_at {
            sqlx::query("DELETE FROM device_codes WHERE device_code = $1")
                .bind(&device_code)
                .execute(&mut *tx)
                .await
                .map_err(anyhow::Error::from)?;
            tx.commit().await.map_err(anyhow::Error::from)?;
            return Ok(expired_response());
        }
        tx.commit().await.map_err(anyhow::Error::from)?;
        return Ok((StatusCode::OK, Json(json!({ "status": "pending" }))).into_response());
    }

    let email = account_email.ok_or_else(|| {
        AppError::Internal(anyhow::anyhow!(
            "device_codes row approved=true but account_email is NULL"
        ))
    })?;

    let account_id = ensure_account(&mut tx, &email).await?;
    let org_id = ensure_personal_org(&mut tx, account_id, &email).await?;
    let (token, _device_id) =
        create_device(&mut tx, org_id, account_id, &host_id, hostname.as_deref()).await?;

    // Single-use: once materialized, the device_codes row is gone, so a
    // second poll (or replay) for the same device_code hits the `None` arm
    // above and gets 410, never a second token.
    sqlx::query("DELETE FROM device_codes WHERE device_code = $1")
        .bind(&device_code)
        .execute(&mut *tx)
        .await
        .map_err(anyhow::Error::from)?;

    tx.commit().await.map_err(anyhow::Error::from)?;

    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "ok",
            "token": token,
            "org_id": org_id,
            "user_id": account_id,
            "email": email,
        })),
    )
        .into_response())
}

fn expired_response() -> axum::response::Response {
    (StatusCode::GONE, Json(json!({ "status": "expired" }))).into_response()
}

/// `INSERT ... ON CONFLICT (email) DO UPDATE` (not `DO NOTHING`) so this is
/// both idempotent *and* always returns the row's id in one round trip.
///
/// `pub(crate)`: also called from `web.rs`'s `POST /web/login` (task spec:
/// "email selects/creates the account + personal org exactly like device
/// activation does (reuse that logic)") -- shared, not duplicated.
pub(crate) async fn ensure_account(conn: &mut PgConnection, email: &str) -> Result<Uuid, AppError> {
    let (id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO accounts (email) VALUES ($1) \
         ON CONFLICT (email) DO UPDATE SET email = EXCLUDED.email \
         RETURNING id",
    )
    .bind(email)
    .fetch_one(&mut *conn)
    .await
    .map_err(anyhow::Error::from)?;
    Ok(id)
}

/// architecture.md §"POST /activate...": "creates a personal org (\"<email>
/// (personal)\") if missing". One personal org per account, reused across
/// devices/hosts for the same account.
///
/// `pub(crate)`: shared with `web.rs`'s `POST /web/login` -- see
/// `ensure_account`'s doc comment.
pub(crate) async fn ensure_personal_org(
    conn: &mut PgConnection,
    account_id: Uuid,
    email: &str,
) -> Result<Uuid, AppError> {
    let existing: Option<(Uuid,)> = sqlx::query_as(
        "SELECT o.id FROM orgs o \
         JOIN org_members m ON m.org_id = o.id \
         WHERE m.account_id = $1 AND o.personal = true \
         LIMIT 1",
    )
    .bind(account_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(anyhow::Error::from)?;
    if let Some((org_id,)) = existing {
        return Ok(org_id);
    }

    let org_name = format!("{email} (personal)");
    let (org_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO orgs (name, personal) VALUES ($1, true) RETURNING id",
    )
    .bind(&org_name)
    .fetch_one(&mut *conn)
    .await
    .map_err(anyhow::Error::from)?;

    sqlx::query(
        "INSERT INTO org_members (org_id, account_id, role) VALUES ($1, $2, 'owner')",
    )
    .bind(org_id)
    .bind(account_id)
    .execute(&mut *conn)
    .await
    .map_err(anyhow::Error::from)?;

    Ok(org_id)
}

async fn create_device(
    conn: &mut PgConnection,
    org_id: Uuid,
    account_id: Uuid,
    host_id: &str,
    hostname: Option<&str>,
) -> Result<(String, Uuid), AppError> {
    let (token, hash) = generate_token();
    let (device_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO devices (org_id, account_id, host_id, hostname, token_hash) \
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(org_id)
    .bind(account_id)
    .bind(host_id)
    .bind(hostname)
    .bind(&hash)
    .fetch_one(&mut *conn)
    .await
    .map_err(anyhow::Error::from)?;
    Ok((token, device_id))
}

// ---------------------------------------------------------------------------
// POST /v1/device/revoke
// ---------------------------------------------------------------------------

/// Self-revokes the bearer token that authenticated this request (sets
/// `devices.revoked = true` for exactly the `devices` row `AuthContext`
/// resolved from). architecture.md §6 documents the cloud token as
/// "`guru logout` / Web から失効可" (revocable via `guru logout` or the web)
/// — `guru logout` calls this so a forgotten/leaked token stops working
/// server-side immediately, not just locally (see `crates/cli/src/login_cmd.rs`).
/// Not in the frozen API contract's explicit endpoint list, but additive
/// (no existing endpoint's shape changes) and required by the architecture
/// doc's own auth table, so it's in scope here.
pub async fn revoke(
    State(state): State<AppState>,
    auth: AuthContext,
) -> Result<axum::response::Response, AppError> {
    sqlx::query("UPDATE devices SET revoked = true WHERE id = $1")
        .bind(auth.device_id)
        .execute(&state.pools.superuser)
        .await
        .map_err(anyhow::Error::from)?;
    Ok((StatusCode::OK, Json(json!({ "status": "revoked" }))).into_response())
}

// ---------------------------------------------------------------------------
// GET /activate — plain HTML page. POST /activate — approves.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ActivateQuery {
    code: Option<String>,
}

pub async fn activate_get(
    State(state): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<ActivateQuery>,
) -> Html<String> {
    let Some(code) = q.code.filter(|c| !c.is_empty()) else {
        return Html(activate_error_page("Missing ?code=."));
    };

    let row: Option<(String, Option<String>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT host_id, hostname, expires_at FROM device_codes WHERE user_code = $1",
    )
    .bind(&code)
    .fetch_optional(&state.pools.superuser)
    .await
    .unwrap_or(None);

    let Some((host_id, hostname, expires_at)) = row else {
        return Html(activate_error_page("This code is invalid or has already been used."));
    };
    if Utc::now() > expires_at {
        return Html(activate_error_page("This code has expired. Run `guru login` again."));
    }

    let invite_field = if state.config.invite_code.is_some() {
        r#"<label for="invite_code">Invite code</label><br>
  <input id="invite_code" name="invite_code" type="text" required placeholder="invite code" style="padding:.5rem; width:100%; box-sizing:border-box;"><br><br>
"#
    } else {
        ""
    };

    Html(format!(
        r#"<!doctype html>
<html><head><title>guru — approve device</title></head>
<body style="font-family: system-ui, sans-serif; max-width: 32rem; margin: 4rem auto;">
<h1>Approve this device</h1>
<p>Host: <code>{host}</code>{hostname_line}</p>
<form method="post" action="/activate">
  <input type="hidden" name="code" value="{code}">
  <label for="email">Email</label><br>
  <input id="email" name="email" type="email" required placeholder="you@example.com" style="padding:.5rem; width:100%; box-sizing:border-box;"><br><br>
  {invite_field}<button type="submit" style="padding:.5rem 1.5rem;">Approve</button>
</form>
</body></html>"#,
        host = html_escape(&host_id),
        hostname_line = hostname
            .map(|h| format!("<br>Hostname: <code>{}</code>", html_escape(&h)))
            .unwrap_or_default(),
        code = html_escape(&code),
        invite_field = invite_field,
    ))
}

#[derive(Deserialize)]
struct ActivateBody {
    code: String,
    email: String,
    #[serde(default)]
    invite_code: Option<String>,
}

pub async fn activate_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<axum::response::Response, AppError> {
    // Fail closed: an invite code is the only gate standing between a public
    // deployment and open self-registration. If the operator hasn't set
    // either it or the (loudly-logged) dev/CI escape hatch, refuse outright
    // rather than ever running open registration by accident.
    if state.config.invite_code.is_none() && !state.config.dev_autoapprove {
        return Ok((
            StatusCode::SERVICE_UNAVAILABLE,
            Html(activate_error_page(
                "Activation is not configured on this server. Contact the operator.",
            )),
        )
            .into_response());
    }

    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let parsed: ActivateBody = if content_type.starts_with("application/json") {
        serde_json::from_slice(&body)
            .map_err(|e| AppError::BadRequest(format!("invalid JSON body: {e}")))?
    } else {
        serde_urlencoded::from_bytes(&body)
            .map_err(|e| AppError::BadRequest(format!("invalid form body: {e}")))?
    };

    if parsed.email.trim().is_empty() || !parsed.email.contains('@') {
        return Err(AppError::BadRequest("a valid email is required".into()));
    }

    if let Some(expected) = &state.config.invite_code {
        let supplied = parsed.invite_code.clone().unwrap_or_default();
        if !constant_time_eq(supplied.as_bytes(), expected.as_bytes()) {
            return reject_invite_code(&state, &parsed.code).await;
        }
    }

    let result = sqlx::query(
        "UPDATE device_codes SET approved = true, account_email = $2 \
         WHERE user_code = $1 AND expires_at > now()",
    )
    .bind(&parsed.code)
    .bind(&parsed.email)
    .execute(&state.pools.superuser)
    .await
    .map_err(anyhow::Error::from)?;

    if result.rows_affected() == 0 {
        return Ok(Html(activate_error_page(
            "This code is invalid or has expired.",
        ))
        .into_response());
    }

    Ok(Html(format!(
        r#"<!doctype html>
<html><head><title>guru — approved</title></head>
<body style="font-family: system-ui, sans-serif; max-width: 32rem; margin: 4rem auto;">
<h1>Device approved</h1>
<p>You can return to your terminal — <code>guru login</code> will pick this up automatically.</p>
</body></html>"#,
    ))
    .into_response())
}

/// Records a wrong-invite-code attempt against `user_code`'s `device_codes`
/// row and returns the 403 the caller should see. If `user_code` doesn't
/// resolve to a live (unexpired) row at all, this is indistinguishable from
/// an already-invalid/expired code, so it gets that same friendly message
/// instead of leaking whether the code itself was ever valid. Once a row's
/// attempt count reaches [`MAX_INVITE_ATTEMPTS`], its `expires_at` is moved
/// into the past — the existing lazy-expiry path in `device_token` then
/// deletes it and returns 410 on the CLI's next poll, same as any other
/// expired code.
async fn reject_invite_code(state: &AppState, user_code: &str) -> Result<axum::response::Response, AppError> {
    let row: Option<(i32,)> = sqlx::query_as(
        "UPDATE device_codes SET invite_attempts = invite_attempts + 1 \
         WHERE user_code = $1 AND expires_at > now() \
         RETURNING invite_attempts",
    )
    .bind(user_code)
    .fetch_optional(&state.pools.superuser)
    .await
    .map_err(anyhow::Error::from)?;

    let Some((attempts,)) = row else {
        return Ok(Html(activate_error_page(
            "This code is invalid or has expired.",
        ))
        .into_response());
    };

    if attempts >= MAX_INVITE_ATTEMPTS {
        sqlx::query(
            "UPDATE device_codes SET expires_at = now() - interval '1 second' \
             WHERE user_code = $1",
        )
        .bind(user_code)
        .execute(&state.pools.superuser)
        .await
        .map_err(anyhow::Error::from)?;
        tracing::warn!(user_code, attempts, "device activation: too many wrong invite code attempts, expiring code");
    }

    Ok((
        StatusCode::FORBIDDEN,
        Html(activate_error_page("Wrong invite code.")),
    )
        .into_response())
}

fn activate_error_page(message: &str) -> String {
    format!(
        r#"<!doctype html>
<html><head><title>guru — activate</title></head>
<body style="font-family: system-ui, sans-serif; max-width: 32rem; margin: 4rem auto;">
<h1>guru</h1>
<p>{}</p>
</body></html>"#,
        html_escape(message)
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
