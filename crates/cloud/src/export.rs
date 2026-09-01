//! `GET /v1/export` (API contract, architecture.md §6 "エクスポート (pull)").
//!
//! Reads `events` for the caller's org (RLS-scoped, same as query.rs) over
//! `[dt_from, dt_to]` and streams back one zstd-compressed Parquet file whose
//! column order matches `kikimimi_schema::COLUMNS` exactly. This crate builds its
//! own Arrow `RecordBatch` (mirroring the pattern in `crates/sink`, not
//! reusing it) reading straight from `PgRow`s rather than through
//! `kikimimi_schema::Event`, since the DB round trip already gives typed columns.

use std::sync::Arc;

use arrow_array::builder::{BooleanBuilder, Float64Builder, Int64Builder, StringBuilder};
use arrow_array::{ArrayRef, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use axum::extract::{Query, State};
use axum::http::header;
use axum::response::IntoResponse;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use sqlx::postgres::PgRow;
use sqlx::Row;

use kikimimi_schema::COLUMNS;

use crate::auth::AuthContext;
use crate::error::AppError;
use crate::query::DateRangeParams;
use crate::state::AppState;

pub async fn export(
    State(state): State<AppState>,
    auth: AuthContext,
    Query(params): Query<DateRangeParams>,
) -> Result<impl IntoResponse, AppError> {
    let dt_from = params.dt_from.unwrap_or_else(|| "0001-01-01".to_string());
    let dt_to = params.dt_to.unwrap_or_else(|| "9999-12-31".to_string());

    let sql = export_sql();
    let mut tx = state.pools.org_scoped_tx(auth.org_id).await?;
    let pg_rows: Vec<PgRow> = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(&dt_from)
        .bind(&dt_to)
        .fetch_all(&mut *tx)
        .await
        .map_err(anyhow::Error::from)?;
    tx.commit().await.map_err(anyhow::Error::from)?;

    let bytes = build_parquet(&pg_rows)?;

    Ok((
        [(header::CONTENT_TYPE, "application/vnd.apache.parquet")],
        bytes,
    ))
}

/// `kikimimi_schema::COLUMNS` in order, except `org_id` is cast to text (it's
/// UUID in the DB — see migrations/0001_core.sql — but text in `kikimimi.v1`).
fn export_sql() -> String {
    let cols: Vec<String> = COLUMNS
        .iter()
        .map(|&c| {
            if c == "org_id" {
                "org_id::text AS org_id".to_string()
            } else {
                c.to_string()
            }
        })
        .collect();
    format!(
        "SELECT {} FROM events WHERE dt BETWEEN $1 AND $2 ORDER BY ts",
        cols.join(", ")
    )
}

fn column_data_type(col: &str) -> DataType {
    match col {
        "ts" | "duration_ms" | "input_tokens" | "output_tokens" | "cache_read_tokens"
        | "cache_write_tokens" | "reasoning_tokens" => DataType::Int64,
        "cost_usd" => DataType::Float64,
        "success" | "thinking" | "redaction_applied" => DataType::Boolean,
        _ => DataType::Utf8,
    }
}

/// Mirrors kikimimi_schema's non-null columns (event_id/ts/dt/host_id/agent/
/// source/event_type); `org_id` is additionally non-null server-side (it's
/// `UUID NOT NULL` in the DB) but stays in the same "non-null" bucket here.
fn column_nullable(col: &str) -> bool {
    !matches!(
        col,
        "event_id" | "ts" | "dt" | "org_id" | "host_id" | "agent" | "source" | "event_type"
    )
}

fn build_parquet(rows: &[PgRow]) -> Result<Vec<u8>, AppError> {
    let fields: Vec<Field> = COLUMNS
        .iter()
        .map(|&name| Field::new(name, column_data_type(name), column_nullable(name)))
        .collect();
    let schema = Arc::new(Schema::new(fields));

    let mut arrays: Vec<ArrayRef> = Vec::with_capacity(COLUMNS.len());
    for (idx, &col) in COLUMNS.iter().enumerate() {
        let array: ArrayRef = match column_data_type(col) {
            DataType::Int64 => {
                let mut b = Int64Builder::with_capacity(rows.len());
                for row in rows {
                    b.append_option(row.try_get::<Option<i64>, _>(idx).map_err(pg_err)?);
                }
                Arc::new(b.finish())
            }
            DataType::Float64 => {
                let mut b = Float64Builder::with_capacity(rows.len());
                for row in rows {
                    b.append_option(row.try_get::<Option<f64>, _>(idx).map_err(pg_err)?);
                }
                Arc::new(b.finish())
            }
            DataType::Boolean => {
                let mut b = BooleanBuilder::with_capacity(rows.len());
                for row in rows {
                    b.append_option(row.try_get::<Option<bool>, _>(idx).map_err(pg_err)?);
                }
                Arc::new(b.finish())
            }
            _ => {
                let mut b = StringBuilder::new();
                for row in rows {
                    match row.try_get::<Option<String>, _>(idx).map_err(pg_err)? {
                        Some(v) => b.append_value(v),
                        None => b.append_null(),
                    }
                }
                Arc::new(b.finish())
            }
        };
        arrays.push(array);
    }

    let batch = RecordBatch::try_new(schema.clone(), arrays).map_err(|e| {
        AppError::Internal(anyhow::Error::new(e).context("building export record batch"))
    })?;

    let mut buf: Vec<u8> = Vec::new();
    {
        let props = WriterProperties::builder()
            .set_compression(Compression::ZSTD(Default::default()))
            .build();
        let mut writer = ArrowWriter::try_new(&mut buf, schema, Some(props)).map_err(|e| {
            AppError::Internal(anyhow::Error::new(e).context("creating parquet arrow writer"))
        })?;
        writer.write(&batch).map_err(|e| {
            AppError::Internal(anyhow::Error::new(e).context("writing record batch"))
        })?;
        writer.close().map_err(|e| {
            AppError::Internal(anyhow::Error::new(e).context("closing parquet writer"))
        })?;
    }
    Ok(buf)
}

fn pg_err(e: sqlx::Error) -> AppError {
    AppError::Internal(e.into())
}
