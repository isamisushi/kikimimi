//! Device authorization grant flow (API contract, architecture.md §6.1):
//!
//! `POST /v1/device/code` → `GET /activate?code=<user_code>` (HTML, requires
//! an authenticated web session — see [`crate::web::WebSessionContext`]) →
//! `POST /activate {code, org_slug}` (approves, binding the device to the
//! session's account + the chosen org) → `POST /v1/device/token` (polled by
//! the CLI; materializes the device + mints the bearer token exactly once,
//! the moment it's first observed approved).
//!
//! All DB access here is on the SUPERUSER pool: accounts/orgs/memberships/
//! devices/device_codes are never touched by the RLS-scoped `kikimimi_app`
//! pool.
//!
//! ACCOUNT-MODEL CONTRACT CHANGE (architecture.md §6.1, 2026-09-01): device
//! activation used to trust a caller-supplied `email` typed into the approve
//! form with no proof of mailbox ownership. That whole shape is gone now —
//! `POST /activate` no longer takes an email at all. Approving a code
//! requires an already-authenticated web session (obtained via GitHub OAuth
//! or the legacy email+invite `POST /web/login`, both in `web.rs`/
//! `github.rs`), and the approval binds the device to *that session's*
//! account and a `org_slug` the account is actually a member of (an org
//! dropdown on the `/activate` page, pre-selected from `org_hint` if the CLI
//! passed `--org <slug>`). This closes the old finding for real: minting a
//! token for account X now requires already being logged in as account X,
//! not just knowing X's email address.
//!
//! `KIKIMIMI_DEV_AUTOAPPROVE=1` (tests/CI, architecture.md §12 Stage 0) skips
//! the browser step entirely: `POST /v1/device/code` immediately resolves
//! and stores an `account_id`/`org_id` for `KIKIMIMI_DEV_EMAIL`, so the very
//! next `/v1/device/token` poll materializes a token — see
//! [`resolve_dev_account_and_org`].

use axum::extract::{Path, State};
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
use crate::web::WebSessionContext;

const CODE_TTL_MINUTES: i64 = 10;
const POLL_INTERVAL_SECS: u64 = 2;
const USER_CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789"; // no 0/O/1/I/L

/// Constant-time byte comparison (manual fold-over-bytes — deliberately not
/// `==`/`PartialEq`, which short-circuits on the first differing byte and
/// would leak a timing side-channel). Used by `web.rs`'s legacy `/web/login`
/// invite-code check and `github.rs`'s oauth-state-cookie check. Comparing
/// every byte unconditionally and only combining results with `|=` keeps the
/// number of operations independent of *where* the two inputs first differ.
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
    /// `kikimimi login --org <slug>` (account-model contract): a hint the
    /// server uses to pre-select an org in the `/activate` dropdown, and (in
    /// the `KIKIMIMI_DEV_AUTOAPPROVE` path only, since there is no dropdown to
    /// pre-select) to actually pick which of the dev account's orgs to bind
    /// to. Never trusted on its own — `activate_post` and
    /// [`resolve_dev_account_and_org`] both re-check that the resolved
    /// account is actually a member of the hinted org before using it.
    #[serde(default)]
    org_hint: Option<String>,
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
    // `kikimimi login`) bounds the table without needing a separate background
    // job (security review finding #7).
    sqlx::query("DELETE FROM device_codes WHERE expires_at < now()")
        .execute(&state.pools.superuser)
        .await
        .map_err(anyhow::Error::from)?;

    let device_code = generate_device_code();
    let user_code = generate_user_code();
    let expires_at = Utc::now() + chrono::Duration::minutes(CODE_TTL_MINUTES);

    // KIKIMIMI_DEV_AUTOAPPROVE=1: pre-approve right away, for tests/CI
    // (architecture.md §12 Stage 0). Materialization (device/token) still
    // only happens on the first POST /v1/device/token poll, same as the real
    // approval path — see module docs.
    let (approved, account_id, org_id) = if state.config.dev_autoapprove {
        let mut tx = state
            .pools
            .superuser
            .begin()
            .await
            .map_err(anyhow::Error::from)?;
        let account_id = ensure_account(&mut tx, &state.config.dev_email).await?;
        let org_id = resolve_dev_account_and_org(
            &mut tx,
            account_id,
            &state.config.dev_email,
            body.org_hint.as_deref(),
        )
        .await?;
        tx.commit().await.map_err(anyhow::Error::from)?;
        (true, Some(account_id), Some(org_id))
    } else {
        (false, None, None)
    };

    sqlx::query(
        "INSERT INTO device_codes (device_code, user_code, host_id, hostname, approved, account_id, org_id, org_hint, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(&device_code)
    .bind(&user_code)
    .bind(&body.host_id)
    .bind(&body.hostname)
    .bind(approved)
    .bind(account_id)
    .bind(org_id)
    .bind(&body.org_hint)
    .bind(expires_at)
    .execute(&state.pools.superuser)
    .await
    .map_err(anyhow::Error::from)?;

    Ok(Json(DeviceCodeResponse {
        device_code,
        verification_url: format!(
            "{}/activate?code={}",
            state.config.public_base_url, user_code
        ),
        user_code,
        interval_secs: POLL_INTERVAL_SECS,
    }))
}

/// `KIKIMIMI_DEV_AUTOAPPROVE` path only: resolves which org to bind the
/// pre-approved device to. Tries `hint` first (only if the dev account is
/// actually a member of that org slug), otherwise falls back to the
/// account's personal org — same fallback shape `activate_get` uses to
/// pre-select the dropdown, just with no human to ask.
async fn resolve_dev_account_and_org(
    conn: &mut PgConnection,
    account_id: Uuid,
    email: &str,
    hint: Option<&str>,
) -> Result<Uuid, AppError> {
    if let Some(slug) = hint {
        if let Some(org_id) = org_id_for_member_slug(conn, account_id, slug).await? {
            return Ok(org_id);
        }
    }
    ensure_personal_org(conn, account_id, email).await
}

async fn org_id_for_member_slug(
    conn: &mut PgConnection,
    account_id: Uuid,
    slug: &str,
) -> Result<Option<Uuid>, AppError> {
    let row: Option<(Uuid,)> = sqlx::query_as(
        "SELECT o.id FROM orgs o JOIN memberships m ON m.org_id = o.id \
         WHERE o.slug = $1 AND m.account_id = $2",
    )
    .bind(slug)
    .bind(account_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(anyhow::Error::from)?;
    Ok(row.map(|(id,)| id))
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
    let row: Option<(
        String,
        String,
        Option<String>,
        bool,
        Option<Uuid>,
        Option<Uuid>,
        DateTime<Utc>,
    )> = sqlx::query_as(
        "SELECT device_code, host_id, hostname, approved, account_id, org_id, expires_at \
             FROM device_codes WHERE device_code = $1 FOR UPDATE",
    )
    .bind(&body.device_code)
    .fetch_optional(&mut *tx)
    .await
    .map_err(anyhow::Error::from)?;

    let Some((device_code, host_id, hostname, approved, account_id, org_id, expires_at)) = row
    else {
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

    let account_id = account_id.ok_or_else(|| {
        AppError::Internal(anyhow::anyhow!(
            "device_codes row approved=true but account_id is NULL"
        ))
    })?;
    let org_id = org_id.ok_or_else(|| {
        AppError::Internal(anyhow::anyhow!(
            "device_codes row approved=true but org_id is NULL"
        ))
    })?;

    let (token, _device_id) =
        create_device(&mut tx, org_id, account_id, &host_id, hostname.as_deref()).await?;

    // account-model contract: "/v1/device/token response gains org_slug +
    // org_kind" — fetched fresh here rather than trusted from whatever the
    // approver's browser sent, same "server is the source of truth" posture
    // as the rest of this handler.
    let (email, org_slug, org_kind): (String, String, String) = sqlx::query_as(
        "SELECT a.email, o.slug, o.kind FROM accounts a, orgs o WHERE a.id = $1 AND o.id = $2",
    )
    .bind(account_id)
    .bind(org_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(anyhow::Error::from)?;

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
            "org_slug": org_slug,
            "org_kind": org_kind,
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
/// `pub(crate)`: also called from `web.rs`'s legacy `POST /web/login` --
/// shared, not duplicated.
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

/// architecture.md §6.1: "personal org はアカウント作成時に自動生成". One personal
/// org per account, reused across devices/hosts/logins for the same account.
///
/// `pub(crate)`: shared with `web.rs`'s legacy `POST /web/login` and
/// `github.rs`'s OAuth callback -- see `ensure_account`'s doc comment.
pub(crate) async fn ensure_personal_org(
    conn: &mut PgConnection,
    account_id: Uuid,
    email: &str,
) -> Result<Uuid, AppError> {
    let existing: Option<(Uuid,)> = sqlx::query_as(
        "SELECT o.id FROM orgs o \
         JOIN memberships m ON m.org_id = o.id \
         WHERE m.account_id = $1 AND o.kind = 'personal' \
         LIMIT 1",
    )
    .bind(account_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(anyhow::Error::from)?;
    if let Some((org_id,)) = existing {
        return Ok(org_id);
    }

    // `id` is generated client-side (not `DEFAULT gen_random_uuid()`) so the
    // slug's uniqueness suffix can be derived from it in the same INSERT --
    // see `slugify_personal_org`.
    let org_id = Uuid::new_v4();
    let org_name = format!("{email} (personal)");
    let slug = slugify_personal_org(email, org_id);
    sqlx::query(
        "INSERT INTO orgs (id, name, personal, kind, slug) VALUES ($1, $2, true, 'personal', $3)",
    )
    .bind(org_id)
    .bind(&org_name)
    .bind(&slug)
    .execute(&mut *conn)
    .await
    .map_err(anyhow::Error::from)?;

    sqlx::query("INSERT INTO memberships (account_id, org_id, role) VALUES ($1, $2, 'owner')")
        .bind(account_id)
        .bind(org_id)
        .execute(&mut *conn)
        .await
        .map_err(anyhow::Error::from)?;

    Ok(org_id)
}

/// Rust-side mirror of the SQL backfill in `migrations/0007_account_model.sql`
/// (kept in sync deliberately, not shared code — one's SQL text, one's Rust):
/// the email's local-part, sanitized to url-safe (runs of non-alphanumeric
/// collapse to one hyphen, trimmed, lowercased, `"org"` if that leaves
/// nothing), plus an 8-hex-char suffix from `org_id` for uniqueness.
fn slugify_personal_org(email: &str, org_id: Uuid) -> String {
    let local = email.split('@').next().unwrap_or("");
    let mut cleaned = String::with_capacity(local.len());
    let mut last_was_dash = true; // suppresses a leading hyphen
    for ch in local.chars() {
        if ch.is_ascii_alphanumeric() {
            cleaned.push(ch.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash {
            cleaned.push('-');
            last_was_dash = true;
        }
    }
    while cleaned.ends_with('-') {
        cleaned.pop();
    }
    if cleaned.is_empty() {
        cleaned.push_str("org");
    }
    let suffix = org_id.simple().to_string();
    format!("{cleaned}-{}", &suffix[..8])
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
/// resolved from). architecture.md §6.1 documents the cloud token as
/// "`kikimimi devices revoke <id>` + Web でデバイス一覧・失効" -- `kikimimi logout`
/// calls this so a forgotten/leaked token stops working server-side
/// immediately, not just locally (see `crates/cli/src/login_cmd.rs`).
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
// GET /v1/devices / POST /v1/devices/:id/revoke — Bearer-token counterpart
// to `orgs.rs`'s session-cookie `GET/POST /web/devices*`, for `kikimimi
// devices` / `kikimimi devices revoke <id>` (crates/cli/src/devices_cmd.rs).
// A CLI device authenticates with its own device bearer token, not a
// session cookie, so it can't call the `/web/*` surface at all -- these are
// new, additive `/v1/*` routes on the same `AuthContext` extractor every
// other `/v1/*` endpoint uses. Not admin-aware (a device token carries no
// role): always exactly "every device belonging to `AuthContext::
// account_id`, across all that account's orgs", never "the whole org's
// devices" -- that stays a `/web/devices` (session + admin role) capability.
// See crates/cli/src/devices_cmd.rs's module docs for the full contract
// this was written against.
// ---------------------------------------------------------------------------

pub async fn list_devices_v1(
    State(state): State<AppState>,
    auth: AuthContext,
) -> Result<Json<serde_json::Value>, AppError> {
    let rows: Vec<(Uuid, String, Option<String>, String, String, DateTime<Utc>, Option<DateTime<Utc>>, bool)> =
        sqlx::query_as(
            "SELECT d.id, d.host_id, d.hostname, o.slug, o.kind, d.created_at, d.last_seen_at, d.revoked \
             FROM devices d JOIN orgs o ON o.id = d.org_id \
             WHERE d.account_id = $1 ORDER BY d.created_at DESC",
        )
        .bind(auth.account_id)
        .fetch_all(&state.pools.superuser)
        .await
        .map_err(anyhow::Error::from)?;

    let devices: Vec<serde_json::Value> = rows
        .into_iter()
        .map(
            |(id, host_id, hostname, org_slug, org_kind, created_at, last_seen_at, revoked)| {
                json!({
                    "id": id, "host_id": host_id, "hostname": hostname,
                    "org_slug": org_slug, "org_kind": org_kind,
                    "created_at": created_at, "last_seen_at": last_seen_at, "revoked": revoked,
                    "current": id == auth.device_id,
                })
            },
        )
        .collect();
    Ok(Json(json!({ "devices": devices })))
}

pub async fn revoke_device_v1(
    State(state): State<AppState>,
    auth: AuthContext,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, AppError> {
    // `account_id = $2`, never just `id = $1`: a device id belonging to a
    // *different* account must 404, not silently revoke someone else's
    // token (same "never confirm existence to a non-owner" rule
    // `orgs::revoke_device` follows).
    let result = sqlx::query("UPDATE devices SET revoked = true WHERE id = $1 AND account_id = $2")
        .bind(id)
        .bind(auth.account_id)
        .execute(&state.pools.superuser)
        .await
        .map_err(anyhow::Error::from)?;
    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("device not found".into()));
    }
    Ok(Json(json!({ "status": "revoked" })))
}

// ---------------------------------------------------------------------------
// GET /activate — plain HTML page (org dropdown). POST /activate — approves.
// Both require an authenticated web session (WebSessionContext) -- see
// module docs for why the old "type an email in" shape is gone.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ActivateQuery {
    code: Option<String>,
}

pub async fn activate_get(
    State(state): State<AppState>,
    session: WebSessionContext,
    axum::extract::Query(q): axum::extract::Query<ActivateQuery>,
) -> Result<Html<String>, AppError> {
    let Some(code) = q.code.filter(|c| !c.is_empty()) else {
        return Ok(Html(activate_error_page("Missing ?code=.")));
    };

    let row: Option<(String, Option<String>, DateTime<Utc>, Option<String>)> = sqlx::query_as(
        "SELECT host_id, hostname, expires_at, org_hint FROM device_codes WHERE user_code = $1",
    )
    .bind(&code)
    .fetch_optional(&state.pools.superuser)
    .await
    .map_err(anyhow::Error::from)?;

    let Some((host_id, hostname, expires_at, org_hint)) = row else {
        return Ok(Html(activate_error_page(
            "This code is invalid or has already been used.",
        )));
    };
    if Utc::now() > expires_at {
        return Ok(Html(activate_error_page(
            "This code has expired. Run `kikimimi login` again.",
        )));
    }

    let orgs: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT o.slug, o.name, o.kind FROM memberships m JOIN orgs o ON o.id = m.org_id \
         WHERE m.account_id = $1 ORDER BY (o.kind = 'personal') DESC, o.name",
    )
    .bind(session.account_id)
    .fetch_all(&state.pools.superuser)
    .await
    .map_err(anyhow::Error::from)?;

    let options: String = orgs
        .iter()
        .map(|(slug, name, kind)| {
            let selected = if org_hint.as_deref() == Some(slug.as_str()) {
                " selected"
            } else {
                ""
            };
            format!(
                r#"<option value="{slug}"{selected}>{name} ({kind})</option>"#,
                slug = html_escape(slug),
                name = html_escape(name),
                kind = html_escape(kind),
                selected = selected,
            )
        })
        .collect();

    Ok(Html(format!(
        r#"<!doctype html>
<html><head><title>kikimimi — approve device</title></head>
<body style="font-family: system-ui, sans-serif; max-width: 32rem; margin: 4rem auto;">
<h1>Approve this device</h1>
<p>Signed in as <code>{email}</code></p>
<p>Host: <code>{host}</code>{hostname_line}</p>
<form method="post" action="/activate">
  <input type="hidden" name="code" value="{code}">
  <label for="org_slug">Organization</label><br>
  <select id="org_slug" name="org_slug" required style="padding:.5rem; width:100%; box-sizing:border-box;">{options}</select><br><br>
  <button type="submit" style="padding:.5rem 1.5rem;">Approve</button>
</form>
</body></html>"#,
        email = html_escape(&session.email),
        host = html_escape(&host_id),
        hostname_line = hostname
            .map(|h| format!("<br>Hostname: <code>{}</code>", html_escape(&h)))
            .unwrap_or_default(),
        code = html_escape(&code),
        options = options,
    )))
}

#[derive(Deserialize)]
struct ActivateBody {
    code: String,
    org_slug: String,
}

pub async fn activate_post(
    State(state): State<AppState>,
    session: WebSessionContext,
    headers: HeaderMap,
    body: Bytes,
) -> Result<axum::response::Response, AppError> {
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

    if parsed.org_slug.trim().is_empty() {
        return Err(AppError::BadRequest("org_slug is required".into()));
    }

    // The session's account must actually be a member of the chosen org —
    // never trust the slug on its own (someone could type an arbitrary
    // other org's slug into the form).
    let mut conn = state
        .pools
        .superuser
        .acquire()
        .await
        .map_err(anyhow::Error::from)?;
    let org_id = org_id_for_member_slug(&mut conn, session.account_id, &parsed.org_slug)
        .await?
        .ok_or_else(|| AppError::Forbidden(format!("not a member of org {:?}", parsed.org_slug)))?;

    let result = sqlx::query(
        "UPDATE device_codes SET approved = true, account_id = $2, org_id = $3 \
         WHERE user_code = $1 AND expires_at > now()",
    )
    .bind(&parsed.code)
    .bind(session.account_id)
    .bind(org_id)
    .execute(&state.pools.superuser)
    .await
    .map_err(anyhow::Error::from)?;

    if result.rows_affected() == 0 {
        return Ok(
            Html(activate_error_page("This code is invalid or has expired.")).into_response(),
        );
    }

    Ok(Html(format!(
        r#"<!doctype html>
<html><head><title>kikimimi — approved</title></head>
<body style="font-family: system-ui, sans-serif; max-width: 32rem; margin: 4rem auto;">
<h1>Device approved</h1>
<p>You can return to your terminal — <code>kikimimi login</code> will pick this up automatically.</p>
</body></html>"#,
    ))
    .into_response())
}

fn activate_error_page(message: &str) -> String {
    format!(
        r#"<!doctype html>
<html><head><title>kikimimi — activate</title></head>
<body style="font-family: system-ui, sans-serif; max-width: 32rem; margin: 4rem auto;">
<h1>kikimimi</h1>
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_personal_org_sanitizes_and_suffixes() {
        let id = Uuid::nil();
        let slug = slugify_personal_org("Alice.Smith+test@example.com", id);
        assert!(slug.starts_with("alice-smith-test-"), "{slug}");
        assert_eq!(
            slug,
            format!("alice-smith-test-{}", &id.simple().to_string()[..8])
        );
    }

    #[test]
    fn slugify_personal_org_falls_back_to_org_when_local_part_is_empty() {
        let id = Uuid::nil();
        let slug = slugify_personal_org("+++@example.com", id);
        assert!(slug.starts_with("org-"), "{slug}");
    }
}
