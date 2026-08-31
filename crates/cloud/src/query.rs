//! `GET /v1/query/<name>` (API contract, architecture.md §8).
//!
//! Runs one of the fixed named queries in `query_sql.rs` on the RLS-scoped
//! `guru_app` pool, so a caller's token can only ever see its own org's rows
//! no matter what — the query text itself has no `org_id` filter at all,
//! Postgres adds it via the `events` row-security policy.

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::Value;
use sqlx::postgres::PgRow;
use sqlx::{Column, Executor, Row, SqlStr, Statement, TypeInfo};

use crate::auth::AuthContext;
use crate::error::AppError;
use crate::query_sql::NAMED_QUERIES;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct DateRangeParams {
    pub dt_from: Option<String>,
    pub dt_to: Option<String>,
}

pub async fn named_query(
    State(state): State<AppState>,
    auth: AuthContext,
    Path(name): Path<String>,
    Query(params): Query<DateRangeParams>,
) -> Result<Json<Value>, AppError> {
    let sql: &'static str = NAMED_QUERIES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, s)| *s)
        .ok_or_else(|| {
            let names: Vec<&str> = NAMED_QUERIES.iter().map(|(n, _)| *n).collect();
            AppError::NotFound(format!(
                "unknown named query {name:?}; available: {}",
                names.join(", ")
            ))
        })?;

    // Wide-open defaults when the caller omits the range, rather than 400ing —
    // `dt` is lexicographically-ordered "YYYY-MM-DD" text so this comparison
    // is safe.
    let dt_from = params.dt_from.unwrap_or_else(|| "0001-01-01".to_string());
    let dt_to = params.dt_to.unwrap_or_else(|| "9999-12-31".to_string());

    let mut tx = state.pools.org_scoped_tx(auth.org_id).await?;

    // Column names/types come from preparing the statement, not from a row —
    // that way an empty result set still reports the right `columns`.
    let stmt = (&mut *tx)
        .prepare(SqlStr::from_static(sql))
        .await
        .map_err(anyhow::Error::from)?;
    let columns: Vec<String> = stmt.columns().iter().map(|c| c.name().to_string()).collect();

    let pg_rows: Vec<PgRow> = sqlx::query(sql)
        .bind(&dt_from)
        .bind(&dt_to)
        .fetch_all(&mut *tx)
        .await
        .map_err(anyhow::Error::from)?;
    tx.commit().await.map_err(anyhow::Error::from)?;

    Ok(Json(columns_and_rows_to_json(&columns, &pg_rows)?))
}

/// Shared with `web_query.rs`'s `/web/q/*` handlers: same "prepare for
/// column names, decode each row generically by Postgres type" shape as
/// above, just factored out so both places get it (and its null/HUGEINT-ish
/// edge cases) from one implementation instead of two.
pub(crate) fn columns_and_rows_to_json(columns: &[String], pg_rows: &[PgRow]) -> Result<Value, AppError> {
    let mut rows: Vec<Vec<Value>> = Vec::with_capacity(pg_rows.len());
    for row in pg_rows {
        let mut out = Vec::with_capacity(columns.len());
        for idx in 0..columns.len() {
            out.push(pg_value_to_json(row, idx)?);
        }
        rows.push(out);
    }
    Ok(serde_json::json!({ "columns": columns, "rows": rows }))
}

/// Decodes one column of one row to JSON based on the Postgres type name.
/// `query_sql.rs` casts every named-query output column to one of
/// TEXT/INT8/FLOAT8/BOOL, so those four arms cover everything the fixed
/// queries can ever produce; anything else falls back to a best-effort text
/// decode so a future named query doesn't hard-fail, just loses precision.
fn pg_value_to_json(row: &PgRow, idx: usize) -> Result<Value, AppError> {
    let type_name = row.column(idx).type_info().name();
    let to_err = |e: sqlx::Error| AppError::Internal(e.into());
    let value = match type_name {
        "INT8" => row.try_get::<Option<i64>, _>(idx).map_err(to_err)?.map(Value::from),
        "INT4" => row.try_get::<Option<i32>, _>(idx).map_err(to_err)?.map(Value::from),
        "INT2" => row.try_get::<Option<i16>, _>(idx).map_err(to_err)?.map(Value::from),
        "FLOAT8" => row.try_get::<Option<f64>, _>(idx).map_err(to_err)?.map(Value::from),
        "FLOAT4" => row
            .try_get::<Option<f32>, _>(idx)
            .map_err(to_err)?
            .map(|v| Value::from(v as f64)),
        "BOOL" => row.try_get::<Option<bool>, _>(idx).map_err(to_err)?.map(Value::from),
        _ => row.try_get::<Option<String>, _>(idx).map_err(to_err)?.map(Value::from),
    };
    Ok(value.unwrap_or(Value::Null))
}
