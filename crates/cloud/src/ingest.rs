//! `POST /v1/events` (API contract, architecture.md §6 "取り込み API の仕様").
//!
//! Bearer + `Content-Encoding: gzip` required. Limits: 5000 events / 5 MB
//! compressed / 32 MB decompressed (413 otherwise). `org_id`/`user_id` are
//! always set from the token, never trusted from the client; every event's
//! `host_id` must equal the token's `host_id` (422 on any mismatch, whole
//! batch rejected). Body columns are defensively NULLed — Stage 0 cloud
//! accepts metadata only (architecture.md §5.2), regardless of what a
//! misbehaving/older client sent. `ON CONFLICT (event_id) DO NOTHING` makes
//! re-sends of an already-accepted batch a no-op (`deduped`, not an error).

use std::io::Read;

use axum::extract::State;
use axum::http::{header, HeaderMap};
use axum::Json;
use flate2::read::GzDecoder;
use guru_schema::{Event, COLUMNS};
use serde::{Deserialize, Serialize};
use sqlx::SqlSafeStr as _;

use crate::auth::AuthContext;
use crate::error::AppError;
use crate::state::AppState;

pub const MAX_EVENTS: usize = 5000;
pub const MAX_COMPRESSED_BYTES: usize = 5 * 1024 * 1024;
pub const MAX_DECOMPRESSED_BYTES: usize = 32 * 1024 * 1024;

#[derive(Deserialize)]
struct IngestBody {
    schema: String,
    events: Vec<Event>,
}

#[derive(Serialize)]
pub struct IngestResponse {
    accepted: i64,
    deduped: i64,
}

pub async fn ingest(
    State(state): State<AppState>,
    auth: AuthContext,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<IngestResponse>, AppError> {
    // Debug_assert-free runtime guard: `try_acquire` never blocks, so a
    // saturated server fails fast with 429 instead of queuing requests
    // behind each other (architecture.md §6 "輻輳時は 429 + Retry-After").
    let _permit = state
        .ingest_semaphore
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::TooManyRequests { retry_after_secs: 2 })?;

    let content_encoding = headers
        .get(header::CONTENT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if content_encoding != "gzip" {
        return Err(AppError::BadRequest(
            "Content-Encoding: gzip is required".into(),
        ));
    }

    if body.len() > MAX_COMPRESSED_BYTES {
        return Err(AppError::PayloadTooLarge(format!(
            "compressed body {} bytes exceeds {MAX_COMPRESSED_BYTES}",
            body.len()
        )));
    }

    let decompressed = decode_gzip_capped(&body, MAX_DECOMPRESSED_BYTES)?;

    let parsed: IngestBody = serde_json::from_slice(&decompressed)
        .map_err(|e| AppError::BadRequest(format!("invalid JSON body: {e}")))?;
    if parsed.schema != guru_schema::SCHEMA_VERSION {
        return Err(AppError::BadRequest(format!(
            "unsupported schema {:?}, expected {:?}",
            parsed.schema,
            guru_schema::SCHEMA_VERSION
        )));
    }
    if parsed.events.len() > MAX_EVENTS {
        return Err(AppError::PayloadTooLarge(format!(
            "batch has {} events, max {MAX_EVENTS}",
            parsed.events.len()
        )));
    }
    if let Some(bad) = parsed.events.iter().find(|e| e.host_id != auth.host_id) {
        return Err(AppError::UnprocessableEntity(format!(
            "event.host_id {:?} does not match token host_id {:?}",
            bad.host_id, auth.host_id
        )));
    }

    let user_id = auth.account_id.to_string();
    // Built once from `AssertSqlSafe` (the SQL text is assembled at runtime
    // from `COLUMNS`, not literal, so it needs the audit-bypass wrapper —
    // see sqlx::SqlSafeStr); `SqlStr::clone()` is Arc-cheap after the first
    // clone, so this doesn't re-copy the string on every one of up to 5000
    // iterations below.
    let sql: sqlx::SqlStr = sqlx::AssertSqlSafe(insert_sql()).into_sql_str();
    let mut tx = state.pools.org_scoped_tx(auth.org_id).await?;
    let mut accepted: i64 = 0;
    for ev in &parsed.events {
        let result = insert_event(&mut tx, sql.clone(), ev, auth.org_id, &user_id)
            .await
            .map_err(anyhow::Error::from)?;
        if result.rows_affected() > 0 {
            accepted += 1;
        }
    }
    tx.commit().await.map_err(anyhow::Error::from)?;

    let deduped = parsed.events.len() as i64 - accepted;
    Ok(Json(IngestResponse { accepted, deduped }))
}

/// `event_id` first, then every other `COLUMNS` entry in order, with `org_id`
/// / `user_id` bound from the token (never the client-supplied values) and
/// the body columns (`tool_input_json` / `tool_output_excerpt` /
/// `prompt_text`) always bound as NULL regardless of what the client sent.
///
/// `ON CONFLICT (org_id, event_id)` — not just `(event_id)` — matches the
/// `events_org_event_pkey` composite primary key (migrations/
/// 0004_events_org_scoped_pk.sql): a client-supplied `event_id` is only a
/// dedup key *within* its own org, so it can never collide with (and thus
/// silently block/dedupe against) another org's row of the same id.
fn insert_sql() -> String {
    let cols: Vec<&str> = COLUMNS.to_vec();
    let placeholders: Vec<String> = (1..=cols.len()).map(|i| format!("${i}")).collect();
    format!(
        "INSERT INTO events ({}) VALUES ({}) ON CONFLICT (org_id, event_id) DO NOTHING",
        cols.join(", "),
        placeholders.join(", ")
    )
}

async fn insert_event<'t>(
    tx: &mut sqlx::Transaction<'t, sqlx::Postgres>,
    sql: sqlx::SqlStr,
    ev: &Event,
    org_id: uuid::Uuid,
    user_id: &str,
) -> Result<sqlx::postgres::PgQueryResult, sqlx::Error> {
    sqlx::query(sql)
        .bind(&ev.event_id) // event_id
        .bind(ev.ts) // ts
        .bind(&ev.dt) // dt
        .bind(org_id) // org_id (from token, not ev.org_id)
        .bind(&ev.team_id) // team_id
        .bind(user_id) // user_id (from token, not ev.user_id)
        .bind(&ev.user_id_source) // user_id_source
        .bind(&ev.host_id) // host_id
        .bind(&ev.env_kind) // env_kind
        .bind(&ev.os) // os
        .bind(&ev.agent) // agent
        .bind(&ev.agent_version) // agent_version
        .bind(&ev.session_id) // session_id
        .bind(&ev.parent_session_id) // parent_session_id
        .bind(&ev.turn_id) // turn_id
        .bind(&ev.cwd_hash) // cwd_hash
        .bind(&ev.repo) // repo
        .bind(&ev.source) // source
        .bind(&ev.correlation_key) // correlation_key
        .bind(&ev.correlation_confidence) // correlation_confidence
        .bind(&ev.event_type) // event_type
        .bind(&ev.tool_name) // tool_name
        .bind(&ev.tool_kind) // tool_kind
        .bind(&ev.mcp_server) // mcp_server
        .bind(&ev.mcp_tool) // mcp_tool
        .bind(ev.duration_ms) // duration_ms
        .bind(ev.success) // success
        .bind(&ev.error_type) // error_type
        .bind(&ev.decision) // decision
        .bind(&ev.decision_source) // decision_source
        .bind(&ev.provider) // provider
        .bind(&ev.model) // model
        .bind(&ev.effort) // effort
        .bind(ev.thinking) // thinking
        .bind(ev.input_tokens) // input_tokens
        .bind(ev.output_tokens) // output_tokens
        .bind(ev.cache_read_tokens) // cache_read_tokens
        .bind(ev.cache_write_tokens) // cache_write_tokens
        .bind(ev.reasoning_tokens) // reasoning_tokens
        .bind(ev.cost_usd) // cost_usd
        .bind(&ev.usage_source) // usage_source
        .bind(None::<String>) // tool_input_json — defensively NULLed (§5.2)
        .bind(None::<String>) // tool_output_excerpt — defensively NULLed
        .bind(None::<String>) // prompt_text — defensively NULLed
        .bind(ev.redaction_applied) // redaction_applied
        .execute(&mut **tx)
        .await
}

/// Decodes gzip, aborting as soon as more than `cap` bytes have come out —
/// never inflates an unbounded "gzip bomb" fully into memory first.
fn decode_gzip_capped(compressed: &[u8], cap: usize) -> Result<Vec<u8>, AppError> {
    let mut decoder = GzDecoder::new(compressed).take(cap as u64 + 1);
    let mut out = Vec::new();
    decoder
        .read_to_end(&mut out)
        .map_err(|e| AppError::BadRequest(format!("invalid gzip body: {e}")))?;
    if out.len() > cap {
        return Err(AppError::PayloadTooLarge(format!(
            "decompressed body exceeds {cap} bytes"
        )));
    }
    Ok(out)
}
