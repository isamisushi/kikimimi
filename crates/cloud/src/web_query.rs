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
use crate::state::AppState;
use crate::web::WebSessionContext;
use crate::web_query_sql::{MACHINES_SQL, MCP_SQL, OVERVIEW_SQL, SESSIONS_SQL, TOOLS_SQL};

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
    let columns: Vec<String> = stmt.columns().iter().map(|c| c.name().to_string()).collect();
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
    let columns: Vec<String> = stmt.columns().iter().map(|c| c.name().to_string()).collect();
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
    let columns: Vec<String> = stmt.columns().iter().map(|c| c.name().to_string()).collect();
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
    let columns: Vec<String> = stmt.columns().iter().map(|c| c.name().to_string()).collect();
    let pg_rows: Vec<PgRow> = sqlx::query(MCP_SQL)
        .bind(&from_dt)
        .fetch_all(&mut *tx)
        .await
        .map_err(anyhow::Error::from)?;
    tx.commit().await.map_err(anyhow::Error::from)?;

    Ok(Json(columns_and_rows_to_json(&columns, &pg_rows)?))
}

pub async fn sessions(
    State(state): State<AppState>,
    session: WebSessionContext,
    Query(q): Query<DaysLimitQuery>,
) -> Result<Json<Value>, AppError> {
    let days = validate_range(q.days, 14, 1, 365, "days")?;
    let limit = validate_range(q.limit, 50, 1, 500, "limit")?;
    let from_dt = today_minus_days(days.saturating_sub(1));

    let mut tx = state.pools.org_scoped_tx(session.org_id).await?;
    let stmt = (&mut *tx)
        .prepare(SqlStr::from_static(SESSIONS_SQL))
        .await
        .map_err(anyhow::Error::from)?;
    let columns: Vec<String> = stmt.columns().iter().map(|c| c.name().to_string()).collect();
    let pg_rows: Vec<PgRow> = sqlx::query(SESSIONS_SQL)
        .bind(&from_dt)
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await
        .map_err(anyhow::Error::from)?;
    tx.commit().await.map_err(anyhow::Error::from)?;

    Ok(Json(columns_and_rows_to_json(&columns, &pg_rows)?))
}

/// `value`, defaulted to `default` when absent, must fall in `min..=max`
/// (WEB API CONTRACT: "days 1..=365, limit 1..=500").
fn validate_range(value: Option<u32>, default: u32, min: u32, max: u32, name: &str) -> Result<u32, AppError> {
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
        assert!(matches!(validate_range(Some(365), 14, 1, 365, "days"), Ok(365)));
        assert!(matches!(validate_range(Some(500), 50, 1, 500, "limit"), Ok(500)));
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
