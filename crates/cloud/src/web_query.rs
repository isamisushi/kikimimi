//! `GET /web/q/*` — the hosted web UI's Postgres-backed query endpoints
//! (WEB API CONTRACT; contract types: `web/src/api/types.ts`; reference
//! impl: `web/mock/server.mjs`; local/DuckDB sibling: `crates/cli/src/
//! web_query.rs`, whose SQL `web_query_sql.rs` ports to Postgres).
//!
//! Auth + tenant scoping is [`crate::web::WebSessionContext`] (cookie →
//! `web_sessions` row → `org_id`) followed by the exact same RLS transaction
//! pattern `/v1/query/{name}` uses (`Pools::org_scoped_tx`, `query.rs`):
//! `SET LOCAL app.org_id` on the `kikimimi_app` pool, so a session can only ever
//! see its own org's rows no matter what the SQL text says.
//!
//! Every handler returns exactly `{"columns":[...],"rows":[[...]]}` with the
//! contract's column names/order — read straight off the prepared
//! statement, not hand-listed, so a wrong `AS` alias in `web_query_sql.rs`
//! surfaces as a wrong `columns` entry rather than silently reshuffling data
//! (`query.rs`'s `columns_and_rows_to_json`, shared with `/v1/query/*`).

use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::Value;
use sqlx::postgres::PgRow;
use sqlx::{Column, Executor, SqlStr, Statement};

use crate::error::AppError;
use crate::query::columns_and_rows_to_json;
use crate::roles::role_at_least;
use crate::state::AppState;
use crate::web::WebSessionContext;
use crate::web_query_sql::{
    MACHINES_SQL, MCP_SQL, MEMBERS_SQL, OVERVIEW_SQL, SESSIONS_SQL, SESSIONS_SQL_SELF, SKILLS_SQL,
    TOOLS_SQL, UNUSED_MCP_SQL,
};

#[derive(Debug, Deserialize)]
pub struct DaysQuery {
    days: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct DaysLimitQuery {
    days: Option<u32>,
    limit: Option<u32>,
}

pub async fn overview(
    State(state): State<AppState>,
    session: WebSessionContext,
    Query(q): Query<DaysQuery>,
) -> Result<Json<Value>, AppError> {
    let days = validate_range(q.days, 14, 1, 365, "days")?;
    let from_dt = today_minus_days(days.saturating_sub(1));

    let mut tx = state.pools.org_scoped_tx(session.org_id).await?;
    let stmt = (&mut *tx)
        .prepare(SqlStr::from_static(OVERVIEW_SQL))
        .await
        .map_err(anyhow::Error::from)?;
    let columns: Vec<String> = stmt
        .columns()
        .iter()
        .map(|c| c.name().to_string())
        .collect();
    let pg_rows: Vec<PgRow> = sqlx::query(OVERVIEW_SQL)
        .bind(&from_dt)
        .fetch_all(&mut *tx)
        .await
        .map_err(anyhow::Error::from)?;
    tx.commit().await.map_err(anyhow::Error::from)?;

    Ok(Json(columns_and_rows_to_json(&columns, &pg_rows)?))
}

/// No `days` param (contract: `GET /web/q/machines` takes no query params) —
/// `events_30d` uses a fixed trailing-30-day window regardless.
pub async fn machines(
    State(state): State<AppState>,
    session: WebSessionContext,
) -> Result<Json<Value>, AppError> {
    let from_30d = today_minus_days(29);

    let mut tx = state.pools.org_scoped_tx(session.org_id).await?;
    let stmt = (&mut *tx)
        .prepare(SqlStr::from_static(MACHINES_SQL))
        .await
        .map_err(anyhow::Error::from)?;
    let columns: Vec<String> = stmt
        .columns()
        .iter()
        .map(|c| c.name().to_string())
        .collect();
    let pg_rows: Vec<PgRow> = sqlx::query(MACHINES_SQL)
        .bind(&from_30d)
        .fetch_all(&mut *tx)
        .await
        .map_err(anyhow::Error::from)?;
    tx.commit().await.map_err(anyhow::Error::from)?;

    Ok(Json(columns_and_rows_to_json(&columns, &pg_rows)?))
}

pub async fn tools(
    State(state): State<AppState>,
    session: WebSessionContext,
    Query(q): Query<DaysQuery>,
) -> Result<Json<Value>, AppError> {
    let days = validate_range(q.days, 14, 1, 365, "days")?;
    let from_dt = today_minus_days(days.saturating_sub(1));

    let mut tx = state.pools.org_scoped_tx(session.org_id).await?;
    let stmt = (&mut *tx)
        .prepare(SqlStr::from_static(TOOLS_SQL))
        .await
        .map_err(anyhow::Error::from)?;
    let columns: Vec<String> = stmt
        .columns()
        .iter()
        .map(|c| c.name().to_string())
        .collect();
    let pg_rows: Vec<PgRow> = sqlx::query(TOOLS_SQL)
        .bind(&from_dt)
        .fetch_all(&mut *tx)
        .await
        .map_err(anyhow::Error::from)?;
    tx.commit().await.map_err(anyhow::Error::from)?;

    Ok(Json(columns_and_rows_to_json(&columns, &pg_rows)?))
}

pub async fn mcp(
    State(state): State<AppState>,
    session: WebSessionContext,
    Query(q): Query<DaysQuery>,
) -> Result<Json<Value>, AppError> {
    let days = validate_range(q.days, 14, 1, 365, "days")?;
    let from_dt = today_minus_days(days.saturating_sub(1));

    let mut tx = state.pools.org_scoped_tx(session.org_id).await?;
    let stmt = (&mut *tx)
        .prepare(SqlStr::from_static(MCP_SQL))
        .await
        .map_err(anyhow::Error::from)?;
    let columns: Vec<String> = stmt
        .columns()
        .iter()
        .map(|c| c.name().to_string())
        .collect();
    let pg_rows: Vec<PgRow> = sqlx::query(MCP_SQL)
        .bind(&from_dt)
        .fetch_all(&mut *tx)
        .await
        .map_err(anyhow::Error::from)?;
    tx.commit().await.map_err(anyhow::Error::from)?;

    Ok(Json(columns_and_rows_to_json(&columns, &pg_rows)?))
}

pub async fn skills(
    State(state): State<AppState>,
    session: WebSessionContext,
    Query(q): Query<DaysQuery>,
) -> Result<Json<Value>, AppError> {
    let days = validate_range(q.days, 14, 1, 365, "days")?;
    let from_dt = today_minus_days(days.saturating_sub(1));

    let mut tx = state.pools.org_scoped_tx(session.org_id).await?;
    let stmt = (&mut *tx)
        .prepare(SqlStr::from_static(SKILLS_SQL))
        .await
        .map_err(anyhow::Error::from)?;
    let columns: Vec<String> = stmt
        .columns()
        .iter()
        .map(|c| c.name().to_string())
        .collect();
    let pg_rows: Vec<PgRow> = sqlx::query(SKILLS_SQL)
        .bind(&from_dt)
        .fetch_all(&mut *tx)
        .await
        .map_err(anyhow::Error::from)?;
    tx.commit().await.map_err(anyhow::Error::from)?;

    Ok(Json(columns_and_rows_to_json(&columns, &pg_rows)?))
}

/// `/web/q/unused-mcp?days=N` (architecture.md §7.1/§7.2) — viewer-accessible
/// like [`mcp`]/[`skills`] (no role gate, unlike [`members`]): every
/// membership can see which of its org's configured MCP servers are going
/// unused. See [`UNUSED_MCP_SQL`]'s own doc comment for the column shape
/// and the `configured_from_snapshot` fallback.
pub async fn unused_mcp(
    State(state): State<AppState>,
    session: WebSessionContext,
    Query(q): Query<DaysQuery>,
) -> Result<Json<Value>, AppError> {
    let days = validate_range(q.days, 14, 1, 365, "days")?;
    let from_dt = today_minus_days(days.saturating_sub(1));

    let mut tx = state.pools.org_scoped_tx(session.org_id).await?;
    let stmt = (&mut *tx)
        .prepare(SqlStr::from_static(UNUSED_MCP_SQL))
        .await
        .map_err(anyhow::Error::from)?;
    let columns: Vec<String> = stmt
        .columns()
        .iter()
        .map(|c| c.name().to_string())
        .collect();
    let pg_rows: Vec<PgRow> = sqlx::query(UNUSED_MCP_SQL)
        .bind(&from_dt)
        .fetch_all(&mut *tx)
        .await
        .map_err(anyhow::Error::from)?;
    tx.commit().await.map_err(anyhow::Error::from)?;

    Ok(Json(columns_and_rows_to_json(&columns, &pg_rows)?))
}

/// Role/purpose-limited (account-model contract): in a `team` org, a
/// `member`/`viewer` sees only their own sessions ([`SESSIONS_SQL_SELF`]);
/// `owner`/`admin` see every session ([`SESSIONS_SQL`]) and that drilldown
/// gets one `audit_log` row per request (architecture.md §11 "admin の
/// ドリルダウンはロールで制限し、閲覧を監査ログに残す"). A `personal` org has no
/// "other members" to scope away from, so it always behaves like the
/// unscoped/unaudited admin path.
pub async fn sessions(
    State(state): State<AppState>,
    session: WebSessionContext,
    Query(q): Query<DaysLimitQuery>,
) -> Result<Json<Value>, AppError> {
    let days = validate_range(q.days, 14, 1, 365, "days")?;
    let limit = validate_range(q.limit, 50, 1, 500, "limit")?;
    let from_dt = today_minus_days(days.saturating_sub(1));

    let (role, org_kind): (String, String) = sqlx::query_as(
        "SELECT m.role, o.kind FROM memberships m JOIN orgs o ON o.id = m.org_id \
         WHERE m.account_id = $1 AND m.org_id = $2",
    )
    .bind(session.account_id)
    .bind(session.org_id)
    .fetch_one(&state.pools.superuser)
    .await
    .map_err(anyhow::Error::from)?;

    let is_team = org_kind == "team";
    let is_admin_plus = role_at_least(&role, "admin");
    let scope_to_self = is_team && !is_admin_plus;

    if is_team && is_admin_plus {
        sqlx::query("INSERT INTO audit_log (actor, org_id, action, target) VALUES ($1, $2, 'sessions_drilldown', NULL)")
            .bind(session.account_id)
            .bind(session.org_id)
            .execute(&state.pools.superuser)
            .await
            .map_err(anyhow::Error::from)?;
    }

    let sql = if scope_to_self {
        SESSIONS_SQL_SELF
    } else {
        SESSIONS_SQL
    };
    let mut tx = state.pools.org_scoped_tx(session.org_id).await?;
    let stmt = (&mut *tx)
        .prepare(SqlStr::from_static(sql))
        .await
        .map_err(anyhow::Error::from)?;
    let columns: Vec<String> = stmt
        .columns()
        .iter()
        .map(|c| c.name().to_string())
        .collect();
    let mut query = sqlx::query(sql).bind(&from_dt);
    if scope_to_self {
        query = query.bind(session.account_id.to_string());
    }
    let pg_rows: Vec<PgRow> = query
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await
        .map_err(anyhow::Error::from)?;
    tx.commit().await.map_err(anyhow::Error::from)?;

    Ok(Json(columns_and_rows_to_json(&columns, &pg_rows)?))
}

/// Admin/owner-only per-member usage view (WEB API CONTRACT `/web/q/members`)
/// -- an *explanatory* aggregation ("what's driving this member's usage",
/// loops/cache re-reads), never a spending leaderboard, so unlike
/// [`sessions`] there is no self-scoped fallback: in a `team` org, a role
/// below `admin` gets a 403, full stop, not their own row. `personal` org
/// has no "other members" to gate away from, so it's always allowed --
/// same reasoning as [`sessions`]'s personal-org path -- and, also like
/// [`sessions`], only the `team`-org admin/owner path writes one `audit_log`
/// row per request (action `members_usage`, architecture.md §11).
pub async fn members(
    State(state): State<AppState>,
    session: WebSessionContext,
    Query(q): Query<DaysQuery>,
) -> Result<Json<Value>, AppError> {
    let days = validate_range(q.days, 30, 1, 365, "days")?;
    let from_dt = today_minus_days(days.saturating_sub(1));

    let (role, org_kind): (String, String) = sqlx::query_as(
        "SELECT m.role, o.kind FROM memberships m JOIN orgs o ON o.id = m.org_id \
         WHERE m.account_id = $1 AND m.org_id = $2",
    )
    .bind(session.account_id)
    .bind(session.org_id)
    .fetch_one(&state.pools.superuser)
    .await
    .map_err(anyhow::Error::from)?;

    let is_team = org_kind == "team";
    let is_admin_plus = role_at_least(&role, "admin");

    if is_team && !is_admin_plus {
        return Err(AppError::Forbidden(format!(
            "member usage is limited to admins/owners, caller has {role}"
        )));
    }

    if is_team {
        sqlx::query("INSERT INTO audit_log (actor, org_id, action, target) VALUES ($1, $2, 'members_usage', NULL)")
            .bind(session.account_id)
            .bind(session.org_id)
            .execute(&state.pools.superuser)
            .await
            .map_err(anyhow::Error::from)?;
    }

    let mut tx = state.pools.org_scoped_tx(session.org_id).await?;
    let stmt = (&mut *tx)
        .prepare(SqlStr::from_static(MEMBERS_SQL))
        .await
        .map_err(anyhow::Error::from)?;
    let columns: Vec<String> = stmt
        .columns()
        .iter()
        .map(|c| c.name().to_string())
        .collect();
    let pg_rows: Vec<PgRow> = sqlx::query(MEMBERS_SQL)
        .bind(&from_dt)
        .fetch_all(&mut *tx)
        .await
        .map_err(anyhow::Error::from)?;
    tx.commit().await.map_err(anyhow::Error::from)?;

    Ok(Json(columns_and_rows_to_json(&columns, &pg_rows)?))
}

/// `value`, defaulted to `default` when absent, must fall in `min..=max`
/// (WEB API CONTRACT: "days 1..=365, limit 1..=500").
fn validate_range(
    value: Option<u32>,
    default: u32,
    min: u32,
    max: u32,
    name: &str,
) -> Result<u32, AppError> {
    let v = value.unwrap_or(default);
    if (min..=max).contains(&v) {
        Ok(v)
    } else {
        Err(AppError::BadRequest(format!(
            "{name} must be between {min} and {max}, got {v}"
        )))
    }
}

fn today_minus_days(n: u32) -> String {
    (chrono::Utc::now().date_naive() - chrono::Duration::days(i64::from(n)))
        .format("%Y-%m-%d")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_range_defaults_when_absent() {
        assert!(matches!(validate_range(None, 14, 1, 365, "days"), Ok(14)));
        assert!(matches!(validate_range(None, 50, 1, 500, "limit"), Ok(50)));
    }

    #[test]
    fn validate_range_accepts_the_boundaries() {
        assert!(matches!(validate_range(Some(1), 14, 1, 365, "days"), Ok(1)));
        assert!(matches!(
            validate_range(Some(365), 14, 1, 365, "days"),
            Ok(365)
        ));
        assert!(matches!(
            validate_range(Some(500), 50, 1, 500, "limit"),
            Ok(500)
        ));
    }

    #[test]
    fn validate_range_rejects_out_of_range() {
        assert!(matches!(
            validate_range(Some(0), 14, 1, 365, "days"),
            Err(AppError::BadRequest(_))
        ));
        assert!(matches!(
            validate_range(Some(366), 14, 1, 365, "days"),
            Err(AppError::BadRequest(_))
        ));
        assert!(matches!(
            validate_range(Some(501), 50, 1, 500, "limit"),
            Err(AppError::BadRequest(_))
        ));
    }

    #[test]
    fn today_minus_days_zero_is_today() {
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        assert_eq!(today_minus_days(0), today);
    }
}
