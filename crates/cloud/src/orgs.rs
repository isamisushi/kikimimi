//! Org/team management: create a team org, switch a session's active org,
//! per-org invite links (create/list/revoke + join), and device listing/
//! revocation across an org (account-model contract §"Org/team API"). All
//! session-authed (`WebSessionContext`) and role-enforced via `roles.rs`'s
//! `require_role_at_least`; every query runs on the SUPERUSER pool (these
//! tables are never touched by the RLS-scoped `kikimimi_app` pool — same club
//! as `accounts`/`orgs`/`memberships`/`devices`/`device_codes`).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth::{generate_token, hash_token, AuthContext};
use crate::error::AppError;
use crate::roles::{membership_role, require_role_at_least, role_at_least, role_rank};
use crate::state::AppState;
use crate::web::WebSessionContext;

/// `id`/`name`/`kind` for `slug`, or 404 — shared by every `:slug`-addressed
/// handler below.
async fn org_by_slug(pool: &sqlx::PgPool, slug: &str) -> Result<(Uuid, String, String), AppError> {
    let row: Option<(Uuid, String, String)> = sqlx::query_as("SELECT id, name, kind FROM orgs WHERE slug = $1")
        .bind(slug)
        .fetch_optional(pool)
        .await
        .map_err(anyhow::Error::from)?;
    row.ok_or_else(|| AppError::NotFound(format!("org {slug:?} not found")))
}

fn is_valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 63
        && slug.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !slug.starts_with('-')
        && !slug.ends_with('-')
}

const VALID_ROLES: &[&str] = &["owner", "admin", "member", "viewer"];

// ---------------------------------------------------------------------------
// POST /web/orgs — create a team org, caller becomes owner.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct CreateOrgRequest {
    name: String,
    slug: String,
}

pub async fn create_org(
    State(state): State<AppState>,
    session: WebSessionContext,
    Json(body): Json<CreateOrgRequest>,
) -> Result<Json<Value>, AppError> {
    let name = body.name.trim().to_string();
    let slug = body.slug.trim().to_lowercase();
    if name.is_empty() {
        return Err(AppError::BadRequest("name must not be empty".into()));
    }
    if !is_valid_slug(&slug) {
        return Err(AppError::BadRequest(
            "slug must be lowercase alphanumeric/hyphen, not starting or ending with a hyphen".into(),
        ));
    }

    let org_id = Uuid::new_v4();
    let mut tx = state.pools.superuser.begin().await.map_err(anyhow::Error::from)?;
    let inserted = sqlx::query(
        "INSERT INTO orgs (id, name, personal, kind, slug) VALUES ($1, $2, false, 'team', $3) \
         ON CONFLICT (slug) DO NOTHING",
    )
    .bind(org_id)
    .bind(&name)
    .bind(&slug)
    .execute(&mut *tx)
    .await
    .map_err(anyhow::Error::from)?;
    if inserted.rows_affected() == 0 {
        return Err(AppError::BadRequest(format!("slug {slug:?} is already taken")));
    }
    sqlx::query("INSERT INTO memberships (account_id, org_id, role) VALUES ($1, $2, 'owner')")
        .bind(session.account_id)
        .bind(org_id)
        .execute(&mut *tx)
        .await
        .map_err(anyhow::Error::from)?;
    tx.commit().await.map_err(anyhow::Error::from)?;

    Ok(Json(json!({ "slug": slug, "name": name, "kind": "team", "role": "owner" })))
}

// ---------------------------------------------------------------------------
// POST /web/active-org — switch the session's active org.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ActiveOrgRequest {
    slug: String,
}

pub async fn set_active_org(
    State(state): State<AppState>,
    session: WebSessionContext,
    Json(body): Json<ActiveOrgRequest>,
) -> Result<Json<Value>, AppError> {
    let (org_id, _name, _kind) = org_by_slug(&state.pools.superuser, &body.slug).await?;
    membership_role(&state.pools.superuser, session.account_id, org_id)
        .await?
        .ok_or_else(|| AppError::Forbidden("not a member of that org".into()))?;

    sqlx::query("UPDATE web_sessions SET org_id = $1 WHERE id = $2")
        .bind(org_id)
        .bind(session.session_id)
        .execute(&state.pools.superuser)
        .await
        .map_err(anyhow::Error::from)?;

    Ok(Json(json!({ "active_org": body.slug })))
}

// ---------------------------------------------------------------------------
// POST /web/orgs/:slug/invites (admin+) — mint an invite link.
// GET  /web/orgs/:slug/invites (admin+) — list them.
// DELETE /web/orgs/:slug/invites/:id (admin+) — revoke one.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct CreateInviteRequest {
    role: String,
    #[serde(default)]
    expires_hours: Option<i64>,
    #[serde(default)]
    max_uses: Option<i32>,
}

pub async fn create_invite(
    State(state): State<AppState>,
    session: WebSessionContext,
    Path(slug): Path<String>,
    Json(body): Json<CreateInviteRequest>,
) -> Result<Json<Value>, AppError> {
    let (org_id, _name, _kind) = org_by_slug(&state.pools.superuser, &slug).await?;
    let caller_role = require_role_at_least(&state.pools.superuser, session.account_id, org_id, "admin").await?;

    if !VALID_ROLES.contains(&body.role.as_str()) {
        return Err(AppError::BadRequest(format!(
            "role must be one of {}",
            VALID_ROLES.join("/")
        )));
    }
    // An admin can't mint an invite for a role above their own (e.g. an
    // admin handing out "owner") — floor is the caller's own rank.
    if role_rank(&body.role) > role_rank(&caller_role) {
        return Err(AppError::Forbidden(
            "cannot create an invite for a role higher than your own".into(),
        ));
    }

    let expires_hours = body.expires_hours.unwrap_or(24 * 7).clamp(1, 24 * 90);
    // Hashed exactly like device bearer tokens (auth::generate_token /
    // hash_token) -- the plaintext only ever exists in this response.
    let (token, hash) = generate_token();
    let expires_at = Utc::now() + chrono::Duration::hours(expires_hours);
    sqlx::query(
        "INSERT INTO org_invites (org_id, role, token_hash, expires_at, max_uses, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(org_id)
    .bind(&body.role)
    .bind(&hash)
    .bind(expires_at)
    .bind(body.max_uses)
    .bind(session.account_id)
    .execute(&state.pools.superuser)
    .await
    .map_err(anyhow::Error::from)?;

    Ok(Json(json!({ "url": format!("/join/{token}") })))
}

pub async fn list_invites(
    State(state): State<AppState>,
    session: WebSessionContext,
    Path(slug): Path<String>,
) -> Result<Json<Value>, AppError> {
    let (org_id, _name, _kind) = org_by_slug(&state.pools.superuser, &slug).await?;
    require_role_at_least(&state.pools.superuser, session.account_id, org_id, "admin").await?;

    let rows: Vec<(Uuid, String, DateTime<Utc>, Option<i32>, i32, bool, DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, role, expires_at, max_uses, uses, revoked, created_at \
         FROM org_invites WHERE org_id = $1 ORDER BY created_at DESC",
    )
    .bind(org_id)
    .fetch_all(&state.pools.superuser)
    .await
    .map_err(anyhow::Error::from)?;

    let invites: Vec<Value> = rows
        .into_iter()
        .map(|(id, role, expires_at, max_uses, uses, revoked, created_at)| {
            json!({
                "id": id, "role": role, "expires_at": expires_at, "max_uses": max_uses,
                "uses": uses, "revoked": revoked, "created_at": created_at,
            })
        })
        .collect();
    Ok(Json(json!({ "invites": invites })))
}

pub async fn revoke_invite(
    State(state): State<AppState>,
    session: WebSessionContext,
    Path((slug, id)): Path<(String, Uuid)>,
) -> Result<Json<Value>, AppError> {
    let (org_id, _name, _kind) = org_by_slug(&state.pools.superuser, &slug).await?;
    require_role_at_least(&state.pools.superuser, session.account_id, org_id, "admin").await?;

    let result = sqlx::query("UPDATE org_invites SET revoked = true WHERE id = $1 AND org_id = $2")
        .bind(id)
        .bind(org_id)
        .execute(&state.pools.superuser)
        .await
        .map_err(anyhow::Error::from)?;
    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("invite not found".into()));
    }
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// GET /join/:token — SPA shell (see lib.rs: registered as `get(web::serve_
// spa)`, not a handler in this module — the confirmation view is a React
// route, `web/src/routes/Join.tsx`, that calls GET /web/invites/:token
// below for the data). POST /join/:token — joins (still JSON, called by
// that same React route).
// ---------------------------------------------------------------------------

struct InviteLookup {
    org_name: String,
    role: String,
    expires_at: DateTime<Utc>,
    max_uses: Option<i32>,
    uses: i32,
    revoked: bool,
}

impl InviteLookup {
    fn expired(&self) -> bool {
        Utc::now() > self.expires_at
    }

    fn exhausted(&self) -> bool {
        self.max_uses.is_some_and(|m| self.uses >= m)
    }

    fn usable(&self) -> bool {
        !self.revoked && !self.expired() && !self.exhausted()
    }
}

async fn lookup_invite(pool: &sqlx::PgPool, token: &str) -> Result<Option<InviteLookup>, AppError> {
    let hash = hash_token(token);
    let row: Option<(Uuid, Uuid, String, String, DateTime<Utc>, Option<i32>, i32, bool)> = sqlx::query_as(
        "SELECT i.id, i.org_id, o.name, i.role, i.expires_at, i.max_uses, i.uses, i.revoked \
         FROM org_invites i JOIN orgs o ON o.id = i.org_id WHERE i.token_hash = $1",
    )
    .bind(&hash)
    .fetch_optional(pool)
    .await
    .map_err(anyhow::Error::from)?;
    Ok(row.map(|(_invite_id, _org_id, org_name, role, expires_at, max_uses, uses, revoked)| InviteLookup {
        org_name,
        role,
        expires_at,
        max_uses,
        uses,
        revoked,
    }))
}

/// `GET /web/invites/:token` (session-authed, additive glue for the SPA's
/// `/join/:token` confirmation view — the account-model contract only
/// specifies the HTML `GET /join/:token`, which is now the SPA shell
/// itself, so the SPA needs a JSON way to preview an invite before
/// `POST /join/:token` commits it). 404 when the token doesn't resolve to
/// any invite at all (never distinguishes "wrong token" from "revoked" at
/// that point — nothing to distinguish); a *found* invite always 200s, with
/// `usable` plus the individual `revoked`/`expired`/`exhausted` reasons so
/// the UI can say which.
pub async fn invite_info(
    State(state): State<AppState>,
    session: WebSessionContext,
    Path(token): Path<String>,
) -> Result<Json<Value>, AppError> {
    let _ = session; // just needs to be authenticated
    let invite = lookup_invite(&state.pools.superuser, &token)
        .await?
        .ok_or_else(|| AppError::NotFound("invite not found".into()))?;
    Ok(Json(json!({
        "org_name": invite.org_name,
        "role": invite.role,
        "usable": invite.usable(),
        "revoked": invite.revoked,
        "expired": invite.expired(),
        "exhausted": invite.exhausted(),
    })))
}

pub async fn join_post(
    State(state): State<AppState>,
    session: WebSessionContext,
    Path(token): Path<String>,
) -> Result<Json<Value>, AppError> {
    let hash = hash_token(&token);
    let mut tx = state.pools.superuser.begin().await.map_err(anyhow::Error::from)?;

    // FOR UPDATE: serializes concurrent joins against the same invite so a
    // `max_uses` check-then-increment race can't let more than `max_uses`
    // people through.
    let row: Option<(Uuid, Uuid, String, DateTime<Utc>, Option<i32>, i32, bool)> = sqlx::query_as(
        "SELECT id, org_id, role, expires_at, max_uses, uses, revoked \
         FROM org_invites WHERE token_hash = $1 FOR UPDATE",
    )
    .bind(&hash)
    .fetch_optional(&mut *tx)
    .await
    .map_err(anyhow::Error::from)?;

    let Some((invite_id, org_id, role, expires_at, max_uses, uses, revoked)) = row else {
        return Err(AppError::NotFound("invite not found".into()));
    };
    if revoked {
        return Err(AppError::BadRequest("invite has been revoked".into()));
    }
    if Utc::now() > expires_at {
        return Err(AppError::BadRequest("invite has expired".into()));
    }
    if max_uses.is_some_and(|m| uses >= m) {
        return Err(AppError::BadRequest("invite has reached its use limit".into()));
    }

    // A re-join (already a member) doesn't downgrade an existing higher
    // role -- ON CONFLICT DO NOTHING, not DO UPDATE.
    sqlx::query(
        "INSERT INTO memberships (account_id, org_id, role) VALUES ($1, $2, $3) \
         ON CONFLICT (account_id, org_id) DO NOTHING",
    )
    .bind(session.account_id)
    .bind(org_id)
    .bind(&role)
    .execute(&mut *tx)
    .await
    .map_err(anyhow::Error::from)?;
    sqlx::query("UPDATE org_invites SET uses = uses + 1 WHERE id = $1")
        .bind(invite_id)
        .execute(&mut *tx)
        .await
        .map_err(anyhow::Error::from)?;

    let (org_slug,): (String,) = sqlx::query_as("SELECT slug FROM orgs WHERE id = $1")
        .bind(org_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(anyhow::Error::from)?;
    tx.commit().await.map_err(anyhow::Error::from)?;

    Ok(Json(json!({ "joined": true, "org_slug": org_slug, "role": role })))
}

// ---------------------------------------------------------------------------
// GET /web/orgs/:slug/members (admin+) — list members with their roles, for
// the SPA's Members & Invites page. Not in the account-model contract's
// literal endpoint list, but required by the SPA task spec ("Members &
// Invites page (admin: list members with roles, ...)") and there is no
// other way to get this — additive glue, same auth/role shape as the invite
// endpoints right above.
// ---------------------------------------------------------------------------

pub async fn list_members(
    State(state): State<AppState>,
    session: WebSessionContext,
    Path(slug): Path<String>,
) -> Result<Json<Value>, AppError> {
    let (org_id, _name, _kind) = org_by_slug(&state.pools.superuser, &slug).await?;
    require_role_at_least(&state.pools.superuser, session.account_id, org_id, "admin").await?;

    let rows: Vec<(Uuid, String, Option<String>, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT a.id, a.email, a.github_login, m.role, m.created_at \
         FROM memberships m JOIN accounts a ON a.id = m.account_id \
         WHERE m.org_id = $1 ORDER BY m.created_at ASC",
    )
    .bind(org_id)
    .fetch_all(&state.pools.superuser)
    .await
    .map_err(anyhow::Error::from)?;

    let members: Vec<Value> = rows
        .into_iter()
        .map(|(account_id, email, github_login, role, created_at)| {
            json!({
                "account_id": account_id, "email": email, "github_login": github_login,
                "role": role, "created_at": created_at,
            })
        })
        .collect();
    Ok(Json(json!({ "members": members })))
}

// ---------------------------------------------------------------------------
// GET /web/devices / POST /web/devices/:id/revoke
// ---------------------------------------------------------------------------

#[allow(clippy::type_complexity)]
pub async fn list_devices(State(state): State<AppState>, session: WebSessionContext) -> Result<Json<Value>, AppError> {
    let role = membership_role(&state.pools.superuser, session.account_id, session.org_id)
        .await?
        .unwrap_or_default();

    // Joined to `accounts`/`orgs` in both branches: the admin branch lists
    // devices across every member of the org (the SPA's Devices page needs
    // to say *whose* device each row is), and the non-admin branch lists
    // devices across every org the account belongs to (see query below --
    // not scoped to the active org), so it needs an org label per row too.
    let rows: Vec<(Uuid, String, Option<String>, DateTime<Utc>, Option<DateTime<Utc>>, bool, String, String, String)> =
        if role_at_least(&role, "admin") {
            sqlx::query_as(
                "SELECT d.id, d.host_id, d.hostname, d.created_at, d.last_seen_at, d.revoked, \
                        a.email, o.slug, o.kind \
                 FROM devices d JOIN accounts a ON a.id = d.account_id JOIN orgs o ON o.id = d.org_id \
                 WHERE d.org_id = $1 ORDER BY d.created_at DESC",
            )
            .bind(session.org_id)
            .fetch_all(&state.pools.superuser)
            .await
            .map_err(anyhow::Error::from)?
        } else {
            sqlx::query_as(
                "SELECT d.id, d.host_id, d.hostname, d.created_at, d.last_seen_at, d.revoked, \
                        a.email, o.slug, o.kind \
                 FROM devices d JOIN accounts a ON a.id = d.account_id JOIN orgs o ON o.id = d.org_id \
                 WHERE d.account_id = $1 ORDER BY d.created_at DESC",
            )
            .bind(session.account_id)
            .fetch_all(&state.pools.superuser)
            .await
            .map_err(anyhow::Error::from)?
        };

    let devices: Vec<Value> = rows
        .into_iter()
        .map(|(id, host_id, hostname, created_at, last_seen_at, revoked, account_email, org_slug, org_kind)| {
            json!({
                "id": id, "host_id": host_id, "hostname": hostname,
                "created_at": created_at, "last_seen_at": last_seen_at, "revoked": revoked,
                "account_email": account_email, "org_slug": org_slug, "org_kind": org_kind,
            })
        })
        .collect();
    Ok(Json(json!({ "devices": devices })))
}

pub async fn revoke_device(
    State(state): State<AppState>,
    session: WebSessionContext,
    Path(id): Path<Uuid>,
) -> Result<Response, AppError> {
    let role = membership_role(&state.pools.superuser, session.account_id, session.org_id)
        .await?
        .unwrap_or_default();

    let result = if role_at_least(&role, "admin") {
        sqlx::query("UPDATE devices SET revoked = true WHERE id = $1 AND org_id = $2")
            .bind(id)
            .bind(session.org_id)
    } else {
        sqlx::query("UPDATE devices SET revoked = true WHERE id = $1 AND account_id = $2")
            .bind(id)
            .bind(session.account_id)
    }
    .execute(&state.pools.superuser)
    .await
    .map_err(anyhow::Error::from)?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("device not found".into()));
    }
    Ok((StatusCode::OK, Json(json!({ "ok": true }))).into_response())
}

// ---------------------------------------------------------------------------
// GET /v1/orgs — Bearer-token counterpart to `GET /web/me`'s `orgs` list, for
// `kikimimi orgs` (crates/cli/src/orgs_cmd.rs). A CLI device authenticates
// with its own device bearer token, not a session cookie, so it can't call
// `/web/me` at all -- this is a new, additive `/v1/*` route on the same
// `AuthContext` extractor every other `/v1/*` endpoint uses. Always exactly
// `AuthContext::account_id`'s own memberships, across every org the account
// belongs to (not scoped to the org the calling device happens to be bound
// to) -- no `active_org` field, unlike `/web/me`: a device's active org is
// fixed at approval time into the token itself (architecture.md §6.1 "1 マ
// シン = 1 アクティブ org"), so there's no per-request session state to
// report back here. See crates/cli/src/orgs_cmd.rs's module docs for the
// full contract this was written against.
// ---------------------------------------------------------------------------

pub async fn list_orgs_v1(State(state): State<AppState>, auth: AuthContext) -> Result<Json<Value>, AppError> {
    let rows: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT o.slug, o.name, o.kind, m.role FROM memberships m JOIN orgs o ON o.id = m.org_id \
         WHERE m.account_id = $1 ORDER BY (o.kind = 'personal') DESC, o.name",
    )
    .bind(auth.account_id)
    .fetch_all(&state.pools.superuser)
    .await
    .map_err(anyhow::Error::from)?;

    let orgs: Vec<Value> = rows
        .into_iter()
        .map(|(slug, name, kind, role)| json!({ "slug": slug, "name": name, "kind": kind, "role": role }))
        .collect();
    Ok(Json(json!({ "orgs": orgs })))
}
