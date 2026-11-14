//! `/web/q/*` — the local web UI's DuckDB-backed query endpoints
//! (architecture.md §8; contract: `web/src/api/types.ts`, reference impl:
//! `web/mock/server.mjs`; SQL/CLI pattern reused from `query_cmd.rs`'s
//! `duckdb -c` shell-out and `kikimimi_schema::paths::events_glob_sql*`).
//!
//! Every handler here returns exactly `{"columns":[...],"rows":[[...]]}`
//! with the contract's column names and order, numeric nulls preserved.
//! Unlike `query_cmd.rs` (a one-shot CLI command, sync `std::process`), these
//! run inside the daemon's axum server, so the DuckDB subprocess is async
//! (`tokio::process`), bounded by a 10s timeout, and killed on timeout
//! (`kill_on_drop`) rather than left to leak.
//!
//! DuckDB gotcha this file works around: `sum()` over a `BIGINT` column
//! widens to `HUGEINT`, and DuckDB's `-json` output serializes `HUGEINT` as a
//! JSON *string* (it doesn't fit a JS-safe double) -- silently breaking the
//! contract's `number | null` types. Every such `sum(...)` is wrapped in
//! `CAST(... AS BIGINT)` to force it back to a plain JSON number (verified
//! against the real `duckdb` CLI, not just reasoned about).
//!
//! `tool.result` double-counting (architecture.md §4, same reasoning as
//! `query_cmd.rs`'s module doc and `crates/cloud/src/query_sql.rs`'s): the
//! Claude Code hook and OTel exporter both emit a `tool.result` row for the
//! same `tool_use_id` when `kikimimi init` enables both, so counting/measuring
//! `tool.result` rows naively double-counts every such pair. [`TOOL_RESULTS_CTE`]
//! is the shared dedup fragment every affected handler below (`overview`,
//! `tools`, `mcp`, `skills`, `sessions`) embeds into its own `WITH` clause,
//! right after that handler's own `e AS (...)` — same rule as the cloud
//! Postgres port: keep `source='otel'` when present for a
//! `(session_id, correlation_key)` pair, else `source='hook'`; a `NULL`
//! `correlation_key` is never merged into another row.

use std::path::Path;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::web::WebAppState;

/// architecture.md §8 task spec: "run with a 10s timeout".
const DUCKDB_TIMEOUT: Duration = Duration::from_secs(10);

/// Resolve verified content links at read time. Raw API events retain their
/// stable identity; late history arrival cannot duplicate token usage. Conflicting
/// links abstain, and the metadata-only link records do not inflate event counts.
fn linked_event_source(glob: &str) -> String {
    crate::activity_filter::source(&kikimimi_sink::store::resolved_source(&format!(
        "read_parquet('{glob}', union_by_name=true, hive_partitioning=false)"
    )))
}

#[cfg(test)]
#[tokio::test]
async fn linked_history_preserves_usage_and_abstains_on_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("links.parquet").display().to_string();
    let sql=format!("COPY (SELECT *, 'claude' AS agent, 'hook' AS source FROM (VALUES \
      ('host','claude-plugin:sdk','msg_one',NULL,NULL,'api.request','claude_desktop','exact',12), \
      ('host','chat','msg_one','claude-plugin:sdk','message-one','message.usage.link','claude_desktop','content_match',NULL), \
      ('host','chat','msg_one','claude-plugin:sdk','message-one','message.usage.link','claude_desktop','content_match',NULL), \
      ('host','claude-plugin:sdk','msg_two',NULL,NULL,'api.request','claude_desktop','exact',18), \
      ('host','chat','msg_two','claude-plugin:sdk','message-two','message.usage.link','claude_desktop','content_match',NULL), \
      ('host','other-chat','msg_two','claude-plugin:sdk','other-message','message.usage.link','claude_desktop','content_match',NULL)) \
      t(host_id,session_id,correlation_key,parent_session_id,turn_id,event_type,client,correlation_confidence,output_tokens)) \
      TO '{path}' (FORMAT PARQUET, COMPRESSION ZSTD)");
    run_duckdb_json(&format!("{sql}; SELECT 1 AS created"))
        .await
        .unwrap();
    let source = linked_event_source(&path);
    let rows = run_duckdb_json(&format!("SELECT * FROM {source} ORDER BY correlation_key"))
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["session_id"], "chat");
    assert_eq!(rows[0]["turn_id"], "message-one");
    assert_eq!(rows[0]["output_tokens"], 12);
    assert_eq!(rows[1]["session_id"], "claude-plugin:sdk");
    assert_eq!(rows[1]["output_tokens"], 18);
    // A later duplicate response invalidates a previously unique match.
    let rejected = dir.path().join("rejected.parquet").display().to_string();
    let sql=format!("COPY (SELECT 'host' AS host_id,'claude-plugin:sdk' AS session_id,'msg_one' AS correlation_key,'message.usage.link.rejected' AS event_type) TO '{rejected}' (FORMAT PARQUET,COMPRESSION ZSTD); SELECT 1 AS created");
    run_duckdb_json(&sql).await.unwrap();
    let source = linked_event_source(&format!("{}/*.parquet", dir.path().display()));
    let rows = run_duckdb_json(&format!("SELECT * FROM {source} ORDER BY correlation_key"))
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["session_id"], "claude-plugin:sdk");
    assert_eq!(rows[0]["output_tokens"], 12);
}

const OVERVIEW_COLUMNS: &[&str] = &[
    "dt",
    "events",
    "tool_calls",
    "failures",
    "input_tokens",
    "output_tokens",
    "cost_usd",
];
const MACHINES_COLUMNS: &[&str] = &["host_id", "env_kind", "os", "last_event_ts", "events_30d"];
const TOOLS_COLUMNS: &[&str] = &[
    "tool_name",
    "tool_kind",
    "calls",
    "failures",
    "p50_duration_ms",
    "p95_duration_ms",
];
const MCP_COLUMNS: &[&str] = &[
    "mcp_server",
    "calls",
    "failures",
    "distinct_sessions",
    "last_called_dt",
];
const SKILLS_COLUMNS: &[&str] = &[
    "skill_name",
    "calls",
    "failures",
    "distinct_sessions",
    "last_used_dt",
];
const UNUSED_MCP_COLUMNS: &[&str] = &[
    "mcp_server",
    "configured",
    "calls",
    "distinct_sessions",
    "last_called_dt",
    "sessions_configured",
    "configured_from_snapshot",
];
const UNUSED_SKILLS_COLUMNS: &[&str] = &[
    "skill_name",
    "configured",
    "sessions_configured",
    "calls",
    "distinct_sessions",
    "last_used_dt",
];
const COVERAGE_COLUMNS: &[&str] = &[
    "events",
    "events_user_id_null",
    "sessions",
    "sessions_without_usage",
    "tool_results_hook",
    "tool_results_otel",
    "tool_results_matched",
    "tool_results_raw",
    "tool_results_deduped",
    "subagents",
    "subagents_with_usage",
    "hosts",
    "hosts_silent_24h",
    "last_event_ts",
];
const SUBAGENTS_COLUMNS: &[&str] = &[
    "session_id",
    "started_at",
    "subagents",
    "agent_types",
    "subagent_tool_calls",
    "tool_calls",
    "subagent_duration_ms",
    "session_duration_ms",
    "duration_share",
    "subagent_api_requests",
    "subagent_tokens_est",
    "session_tokens_est",
    "token_share",
    "subagents_with_usage",
];
const SESSIONS_COLUMNS: &[&str] = &[
    "session_id",
    "agent",
    "host_id",
    "started_at",
    "events",
    "tool_calls",
    "failures",
    "models",
    "input_tokens",
    "output_tokens",
    "cost_usd",
    "client",
    "estimated_input_tokens",
    "estimated_output_tokens",
];

/// Shared `tool.result` hook/OTel dedup CTE (module doc). Must be embedded
/// (via the `{TOOL_RESULTS_CTE}` captured-identifier format arg) into a
/// `WITH` clause right after the caller's own `e AS (...)` -- it reads `e`
/// and defines `tool_results`, one deduped row per `(session_id,
/// correlation_key)` `tool.result`.
const TOOL_RESULTS_CTE: &str = "tool_results AS ( \
    SELECT * FROM ( \
        SELECT e.*, row_number() OVER ( \
            PARTITION BY session_id, correlation_key \
            ORDER BY CASE source WHEN 'otel' THEN 0 WHEN 'hook' THEN 1 ELSE 2 END, ts \
        ) AS src_rank \
        FROM e \
        WHERE event_type = 'tool.result' AND correlation_key IS NOT NULL \
    ) d WHERE src_rank = 1 \
    UNION ALL \
    SELECT e.*, 1 AS src_rank FROM e WHERE event_type = 'tool.result' AND correlation_key IS NULL \
)";

#[derive(Debug, Deserialize)]
pub struct DaysQuery {
    days: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct DaysLimitQuery {
    days: Option<u32>,
    limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct PatternHitsQuery {
    pattern_id: String,
    subject: String,
    days: Option<u32>,
    limit: Option<u32>,
}

const PATTERNS_COLUMNS: &[&str] = &[
    "pattern_id",
    "subject",
    "sessions",
    "incidents",
    "wasted_tokens_est",
    "priced_hits",
    "hits",
    "priority",
    "first_seen_dt",
    "last_seen_dt",
];

const PATTERN_TIMELINE_COLUMNS: &[&str] = &[
    "dt",
    "sessions_total",
    "sessions_hit",
    "rate_pct",
    "incidents",
    "wasted_tokens_est",
];

#[derive(Debug, Deserialize)]
pub struct PatternSubjectQuery {
    #[serde(default)]
    pattern_id: String,
    #[serde(default)]
    subject: String,
    days: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct CreateMarkRequest {
    pattern_id: String,
    subject: String,
    marked_dt: String,
    #[serde(default)]
    note: String,
}

/// One improvement mark as stored in `<KIKIMIMI_DIR>/marks.json` (KKM-12).
/// The local daemon is single-user, so there is no `created_by`; the file
/// is the whole store and is rewritten atomically on every change.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Mark {
    pub id: String,
    pub pattern_id: String,
    pub subject: String,
    pub marked_dt: String,
    #[serde(default)]
    pub note: String,
    pub created_at: String,
    #[serde(default)]
    pub scope: String,
}

fn marks_path(state: &WebAppState) -> std::path::PathBuf {
    // data_dir is <kikimimi_dir>/data/events; marks live next to config.json.
    state
        .data_dir
        .parent()
        .and_then(|p| p.parent())
        .map(|d| d.join("marks.json"))
        .unwrap_or_else(|| kikimimi_schema::paths::kikimimi_dir().join("marks.json"))
}

// Serialize read/modify/write within the local process, and never overwrite corrupt records.
static MARKS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn load_marks(path: &Path) -> std::io::Result<Vec<Mark>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

fn save_marks(path: &Path, marks: &[Mark]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() { std::fs::create_dir_all(dir)?; }
    crate::state::write_atomic(path, &serde_json::to_vec_pretty(marks)?)
        .map_err(std::io::Error::other)
}

const PATTERN_HITS_COLUMNS: &[&str] = &[
    "dt",
    "session_id",
    "first_ts",
    "last_ts",
    "incidents",
    "wasted_tokens_est",
    "detail",
];

pub async fn overview(State(state): State<WebAppState>, Query(q): Query<DaysQuery>) -> Response {
    let days = match validate_range(q.days, 14, 1, 365, "days") {
        Ok(d) => d,
        Err(r) => return r,
    };
    if !any_parquet_files(&state.data_dir) {
        return query_result_response(OVERVIEW_COLUMNS, vec![]);
    }
    let glob = kikimimi_schema::paths::events_glob_sql_in(&state.data_dir);
    let event_source = linked_event_source(&glob);
    let from_dt = today_minus_days(days.saturating_sub(1));
    let sql = format!(
        "WITH e AS ( \
           SELECT * FROM {event_source} \
           WHERE dt >= '{from_dt}' \
         ), \
         {TOOL_RESULTS_CTE}, \
         fails AS ( \
           SELECT dt, count(*) AS failures \
           FROM ( \
             SELECT dt, success FROM e WHERE event_type <> 'tool.result' \
             UNION ALL \
             SELECT dt, success FROM tool_results \
           ) u \
           WHERE success = false \
           GROUP BY dt \
         ) \
         SELECT e.dt AS dt, \
           count(*) AS events, \
           count(*) FILTER (WHERE e.event_type = 'tool.call') AS tool_calls, \
           coalesce(max(fails.failures), 0) AS failures, \
           CAST(sum(e.input_tokens) AS BIGINT) AS input_tokens, \
           CAST(sum(e.output_tokens) AS BIGINT) AS output_tokens, \
           sum(e.cost_usd) AS cost_usd \
         FROM e \
         LEFT JOIN fails ON fails.dt = e.dt \
         GROUP BY e.dt \
         ORDER BY e.dt;"
    );
    respond(OVERVIEW_COLUMNS, run_duckdb_json(&sql).await)
}

pub async fn machines(State(state): State<WebAppState>) -> Response {
    if !any_parquet_files(&state.data_dir) {
        return query_result_response(MACHINES_COLUMNS, vec![]);
    }
    let glob = kikimimi_schema::paths::events_glob_sql_in(&state.data_dir);
    let event_source = linked_event_source(&glob);
    // events_30d is a fixed trailing 30-day window, independent of any caller
    // param -- machines has none (contract: `GET /web/q/machines` takes no
    // query params).
    let from_30d = today_minus_days(29);
    let sql = format!(
        "SELECT host_id, \
           max(env_kind) AS env_kind, \
           max(os) AS os, \
           strftime(epoch_ms(max(ts)), '%Y-%m-%dT%H:%M:%SZ') AS last_event_ts, \
           count(*) FILTER (WHERE dt >= '{from_30d}') AS events_30d \
         FROM {event_source} \
         GROUP BY host_id \
         ORDER BY max(ts) DESC NULLS LAST;"
    );
    respond(MACHINES_COLUMNS, run_duckdb_json(&sql).await)
}

pub async fn tools(State(state): State<WebAppState>, Query(q): Query<DaysQuery>) -> Response {
    let days = match validate_range(q.days, 14, 1, 365, "days") {
        Ok(d) => d,
        Err(r) => return r,
    };
    if !any_parquet_files(&state.data_dir) {
        return query_result_response(TOOLS_COLUMNS, vec![]);
    }
    let glob = kikimimi_schema::paths::events_glob_sql_in(&state.data_dir);
    let event_source = linked_event_source(&glob);
    let from_dt = today_minus_days(days.saturating_sub(1));
    let sql = format!(
        "WITH e AS ( \
           SELECT * FROM {event_source} \
           WHERE tool_name IS NOT NULL AND dt >= '{from_dt}' \
         ), \
         {TOOL_RESULTS_CTE}, \
         results AS ( \
           SELECT \
             tool_name, \
             count(*) FILTER (WHERE success = false) AS failures, \
             approx_quantile(duration_ms, 0.5)  AS p50_duration_ms, \
             approx_quantile(duration_ms, 0.95) AS p95_duration_ms \
           FROM tool_results \
           GROUP BY tool_name \
         ) \
         SELECT e.tool_name AS tool_name, \
           max(e.tool_kind) AS tool_kind, \
           count(*) FILTER (WHERE e.event_type = 'tool.call') AS calls, \
           coalesce(max(results.failures), 0) AS failures, \
           max(results.p50_duration_ms) AS p50_duration_ms, \
           max(results.p95_duration_ms) AS p95_duration_ms \
         FROM e \
         LEFT JOIN results ON results.tool_name = e.tool_name \
         GROUP BY e.tool_name \
         ORDER BY calls DESC;"
    );
    respond(TOOLS_COLUMNS, run_duckdb_json(&sql).await)
}

pub async fn mcp(State(state): State<WebAppState>, Query(q): Query<DaysQuery>) -> Response {
    let days = match validate_range(q.days, 14, 1, 365, "days") {
        Ok(d) => d,
        Err(r) => return r,
    };
    if !any_parquet_files(&state.data_dir) {
        return query_result_response(MCP_COLUMNS, vec![]);
    }
    let glob = kikimimi_schema::paths::events_glob_sql_in(&state.data_dir);
    let event_source = linked_event_source(&glob);
    let from_dt = today_minus_days(days.saturating_sub(1));
    let sql = format!(
        "WITH e AS ( \
           SELECT * FROM {event_source} \
           WHERE mcp_server IS NOT NULL AND dt >= '{from_dt}' \
         ), \
         {TOOL_RESULTS_CTE}, \
         results AS ( \
           SELECT mcp_server, count(*) FILTER (WHERE success = false) AS failures \
           FROM tool_results \
           GROUP BY mcp_server \
         ) \
         SELECT e.mcp_server AS mcp_server, \
           count(*) FILTER (WHERE e.event_type = 'tool.call') AS calls, \
           coalesce(max(results.failures), 0) AS failures, \
           count(DISTINCT e.session_id) AS distinct_sessions, \
           max(e.dt) FILTER (WHERE e.event_type = 'tool.call') AS last_called_dt \
         FROM e \
         LEFT JOIN results ON results.mcp_server = e.mcp_server \
         GROUP BY e.mcp_server \
         ORDER BY calls DESC;"
    );
    respond(MCP_COLUMNS, run_duckdb_json(&sql).await)
}

pub async fn skills(State(state): State<WebAppState>, Query(q): Query<DaysQuery>) -> Response {
    let days = match validate_range(q.days, 14, 1, 365, "days") {
        Ok(d) => d,
        Err(r) => return r,
    };
    if !any_parquet_files(&state.data_dir) {
        return query_result_response(SKILLS_COLUMNS, vec![]);
    }
    let glob = kikimimi_schema::paths::events_glob_sql_in(&state.data_dir);
    let event_source = linked_event_source(&glob);
    let from_dt = today_minus_days(days.saturating_sub(1));
    let sql = format!(
        "WITH e AS ( \
           SELECT * FROM {event_source} \
           WHERE skill_name IS NOT NULL AND dt >= '{from_dt}' \
         ), \
         {TOOL_RESULTS_CTE}, \
         results AS ( \
           SELECT skill_name, count(*) FILTER (WHERE success = false) AS failures \
           FROM tool_results \
           GROUP BY skill_name \
         ) \
         SELECT e.skill_name AS skill_name, \
           count(*) FILTER (WHERE e.event_type = 'tool.call') AS calls, \
           coalesce(max(results.failures), 0) AS failures, \
           count(DISTINCT e.session_id) AS distinct_sessions, \
           max(e.dt) AS last_used_dt \
         FROM e \
         LEFT JOIN results ON results.skill_name = e.skill_name \
         GROUP BY e.skill_name \
         ORDER BY calls DESC;"
    );
    respond(SKILLS_COLUMNS, run_duckdb_json(&sql).await)
}

/// `/web/q/unused-mcp?days=N` (architecture.md §7.1 「導入されているのに呼ばれない
/// サーバー」, §7.2 `unused_mcp_server`) → `[mcp_server, configured, calls,
/// distinct_sessions, last_called_dt, sessions_configured,
/// configured_from_snapshot]` — same shape as the cloud
/// `web_query_sql::UNUSED_MCP_SQL`. Unlike cloud, the local daemon runs on
/// the same machine as `~/.claude*`/`.mcp.json`, so "configured" comes
/// straight from those files (`mcp_config.rs`, no `cwd` — the local web UI
/// has no per-session cwd context) rather than a `session.start` snapshot;
/// `configured_from_snapshot` is therefore always `true` (this is always
/// the real, current config, never the cloud's 30-day-observed proxy).
/// `sessions_configured` has no local equivalent of the cloud's per-session
/// snapshot count, so it's always `0`.
pub async fn s3_unused_mcp(
    State(state): State<WebAppState>,
    Query(q): Query<DaysQuery>,
) -> Response {
    let days = match validate_range(q.days, 14, 1, 365, "days") {
        Ok(d) => d,
        Err(r) => return r,
    };
    let glob = kikimimi_schema::paths::events_glob_sql_in(&state.data_dir);
    let event_source = linked_event_source(&glob);
    let from_dt = today_minus_days(days.saturating_sub(1));
    // Only the exported session snapshots describe a remote machine's config.
    // Never mix this viewer's ~/.claude settings into team S3 results.
    let sql = format!(
        r#"WITH e AS (
      SELECT * FROM {event_source} WHERE dt >= '{from_dt}'
    ), configured_sessions AS (
      SELECT DISTINCT session_id, unnest(from_json(configured_mcp_servers, '["VARCHAR"]')) AS mcp_server
      FROM e WHERE configured_mcp_servers IS NOT NULL
    ), configured AS (
      SELECT mcp_server, count(DISTINCT session_id) AS sessions_configured FROM configured_sessions GROUP BY mcp_server
    ), calls AS (
      SELECT mcp_server, count(*) AS calls, count(DISTINCT session_id) AS distinct_sessions, max(dt) AS last_called_dt
      FROM e WHERE event_type='tool.call' AND mcp_server IS NOT NULL GROUP BY mcp_server
    ) SELECT coalesce(c.mcp_server, u.mcp_server) AS mcp_server,
      c.mcp_server IS NOT NULL AS configured, coalesce(u.calls, 0) AS calls,
      coalesce(u.distinct_sessions, 0) AS distinct_sessions, u.last_called_dt,
      coalesce(c.sessions_configured, 0) AS sessions_configured,
      c.mcp_server IS NOT NULL AS configured_from_snapshot
    FROM configured c FULL OUTER JOIN calls u ON c.mcp_server=u.mcp_server
    ORDER BY calls, mcp_server;"#
    );
    respond(UNUSED_MCP_COLUMNS, run_duckdb_json(&sql).await)
}

pub async fn unused_mcp(State(state): State<WebAppState>, Query(q): Query<DaysQuery>) -> Response {
    let days = match validate_range(q.days, 14, 1, 365, "days") {
        Ok(d) => d,
        Err(r) => return r,
    };
    let configured = crate::mcp_config::configured_mcp_servers();
    if !any_parquet_files(&state.data_dir) {
        // No events flushed yet at all: DuckDB's read_parquet() on an empty
        // glob errors rather than returning an empty result (any_parquet_files'
        // own doc comment), so short-circuit straight to "every configured
        // server, zero calls" instead of shelling out to DuckDB for nothing.
        let rows: Vec<Vec<Value>> = configured
            .iter()
            .map(|s| {
                vec![
                    Value::from(s.as_str()),
                    Value::from(true),
                    Value::from(0),
                    Value::from(0),
                    Value::Null,
                    Value::from(0),
                    Value::from(true),
                ]
            })
            .collect();
        return query_result_response(UNUSED_MCP_COLUMNS, rows);
    }
    let configured_list = crate::mcp_config::mcp_configured_sql_list(&configured);
    let glob = kikimimi_schema::paths::events_glob_sql_in(&state.data_dir);
    let event_source = linked_event_source(&glob);
    let from_dt = today_minus_days(days.saturating_sub(1));
    let sql = format!(
        "WITH observed AS ( \
           SELECT mcp_server, \
             count(*) AS calls, \
             count(DISTINCT session_id) AS distinct_sessions, \
             max(dt) AS last_called_dt \
           FROM {event_source} \
           WHERE event_type = 'tool.call' AND mcp_server IS NOT NULL AND dt >= '{from_dt}' \
           GROUP BY mcp_server \
         ), \
         configured AS ( \
           SELECT DISTINCT unnest({configured_list}) AS mcp_server \
         ) \
         SELECT coalesce(c.mcp_server, observed.mcp_server) AS mcp_server, \
           (c.mcp_server IS NOT NULL) AS configured, \
           coalesce(observed.calls, 0) AS calls, \
           coalesce(observed.distinct_sessions, 0) AS distinct_sessions, \
           observed.last_called_dt AS last_called_dt, \
           0 AS sessions_configured, \
           true AS configured_from_snapshot \
         FROM configured c \
         FULL OUTER JOIN observed ON c.mcp_server = observed.mcp_server \
         ORDER BY (c.mcp_server IS NOT NULL AND coalesce(observed.calls, 0) = 0) DESC, \
           coalesce(observed.calls, 0) ASC;"
    );
    respond(UNUSED_MCP_COLUMNS, run_duckdb_json(&sql).await)
}

fn enrich_session_clients(rows: &mut [Map<String, Value>]) {
    for row in rows {
        // Older releases combined Work and Codex. Only refine that legacy value
        // from this host's observed metadata; retain genuine mixed origins.
        if row.get("client").is_some_and(|v| !v.is_null() && !v.as_str().is_some_and(|s| s.split(',').any(|c| c == "codex_desktop"))) {
            continue;
        }
        if let (Some(sid), Some(host)) = (
            row.get("session_id").and_then(Value::as_str),
            row.get("host_id").and_then(Value::as_str),
        ) {
            if let Some(client) = crate::client_source::historical(sid, host) {
                let client = crate::client_source::refine_client(row.get("client").and_then(Value::as_str), &client);
                row.insert("client".into(), Value::String(client));
            }
        }
    }
}

pub async fn sessions(
    State(state): State<WebAppState>,
    Query(q): Query<DaysLimitQuery>,
) -> Response {
    let days = match validate_range(q.days, 14, 1, 365, "days") {
        Ok(d) => d,
        Err(r) => return r,
    };
    let limit = match validate_range(q.limit, 50, 1, 500, "limit") {
        Ok(l) => l,
        Err(r) => return r,
    };
    if !any_parquet_files(&state.data_dir) {
        return query_result_response(SESSIONS_COLUMNS, vec![]);
    }
    let glob = kikimimi_schema::paths::events_glob_sql_in(&state.data_dir);
    let event_source = linked_event_source(&glob);
    let from_dt = today_minus_days(days.saturating_sub(1));
    // Keep message bodies and other large payloads out of the materialized CTE
    // and tool-result deduplication; this page only needs summary columns.
    // JSON extraction keeps optional estimates compatible with older Parquet files.
    let sql = format!(
        "WITH e AS (\
           SELECT session_id, agent, host_id, ts, event_type, source, correlation_key, \
             success, model, input_tokens, output_tokens, cost_usd, client, \
             try_cast(json_extract_string(to_json(summary_event), '$.estimated_input_tokens') AS BIGINT) AS estimated_input_tokens, \
             try_cast(json_extract_string(to_json(summary_event), '$.estimated_output_tokens') AS BIGINT) AS estimated_output_tokens \
           FROM {event_source} AS summary_event \
           WHERE session_id IS NOT NULL AND dt >= '{from_dt}' \
         ), \
         {TOOL_RESULTS_CTE}, \
         fails AS ( \
           SELECT session_id, count(*) AS failures \
           FROM ( \
             SELECT session_id, success FROM e WHERE event_type <> 'tool.result' \
             UNION ALL \
             SELECT session_id, success FROM tool_results \
           ) u \
           WHERE success = false \
           GROUP BY session_id \
         ) \
         SELECT e.session_id AS session_id, \
           max(e.agent) AS agent, \
           max(e.host_id) AS host_id, \
           strftime(epoch_ms(min(e.ts)), '%Y-%m-%dT%H:%M:%SZ') AS started_at, \
           count(*) AS events, \
           count(*) FILTER (WHERE e.event_type = 'tool.call') AS tool_calls, \
           coalesce(max(fails.failures), 0) AS failures, \
           coalesce(string_agg(DISTINCT e.model, ','), '') AS models, \
           CAST(sum(e.input_tokens) AS BIGINT) AS input_tokens, \
           CAST(sum(e.output_tokens) AS BIGINT) AS output_tokens, \
           sum(e.cost_usd) AS cost_usd, string_agg(DISTINCT e.client, ',' ORDER BY e.client) AS client, \
           CAST(sum(e.estimated_input_tokens) AS BIGINT) AS estimated_input_tokens, \
           CAST(sum(e.estimated_output_tokens) AS BIGINT) AS estimated_output_tokens \
         FROM e \
         LEFT JOIN fails ON fails.session_id = e.session_id \
         GROUP BY e.session_id \
         ORDER BY min(e.ts) DESC \
         LIMIT {limit};"
    );
    match run_duckdb_json(&sql).await {
        Ok(mut rows) => {
            enrich_session_clients(&mut rows);
            query_result_response(SESSIONS_COLUMNS, project(&rows, SESSIONS_COLUMNS))
        }
        Err(e) => e.into_response(),
    }
}

/// `/web/q/unused-skills?days=N` (KKM-18, local): `query_cmd::UNUSED_SKILLS_SQL`
/// over local Parquet. Columns match the cloud contract exactly.
pub async fn unused_skills(
    State(state): State<WebAppState>,
    Query(q): Query<DaysQuery>,
) -> Response {
    let days = match validate_range(q.days, 14, 1, 365, "days") {
        Ok(d) => d,
        Err(r) => return r,
    };
    if !any_parquet_files(&state.data_dir) {
        return query_result_response(UNUSED_SKILLS_COLUMNS, vec![]);
    }
    let glob = kikimimi_schema::paths::events_glob_sql_in(&state.data_dir);
    let event_source = linked_event_source(&glob);
    let from_dt = today_minus_days(days.saturating_sub(1));
    let sql = crate::query_cmd::unused_skills_sql(&glob, &from_dt);
    let sql = sql.replace(
        &format!("read_parquet('{glob}', union_by_name=true, hive_partitioning=false)"),
        &event_source,
    );
    respond(UNUSED_SKILLS_COLUMNS, run_duckdb_json(&sql).await)
}

/// `/web/q/coverage?days=N` (KKM-17, local): the same missing-data counts
/// the cloud serves (`web_query_sql::COVERAGE_SQL`; keep in sync), over
/// local Parquet. `events_user_id_null` is always `events` here — local data
/// has no account to attribute to — and the page says so.
pub async fn coverage(State(state): State<WebAppState>, Query(q): Query<DaysQuery>) -> Response {
    let days = match validate_range(q.days, 30, 1, 365, "days") {
        Ok(d) => d,
        Err(r) => return r,
    };
    if !any_parquet_files(&state.data_dir) {
        return query_result_response(COVERAGE_COLUMNS, vec![]);
    }
    let glob = kikimimi_schema::paths::events_glob_sql_in(&state.data_dir);
    let event_source = linked_event_source(&glob);
    let from_dt = today_minus_days(days.saturating_sub(1));
    let silent_before_ms = chrono::Utc::now().timestamp_millis() - 24 * 3600 * 1000;
    let sql = crate::query_cmd::coverage_sql(&glob, &from_dt, silent_before_ms);
    let sql = sql.replace(
        &format!("read_parquet('{glob}', union_by_name=true, hive_partitioning=false)"),
        &event_source,
    );
    respond(COVERAGE_COLUMNS, run_duckdb_json(&sql).await)
}

/// `/web/q/subagents?days=N&limit=M` (KKM-15, local): per-session subagent
/// fan-out over local Parquet, `query_cmd::subagents_sql` without the
/// `TOTAL` row. Columns match the cloud contract exactly.
pub async fn subagents(
    State(state): State<WebAppState>,
    Query(q): Query<DaysLimitQuery>,
) -> Response {
    let days = match validate_range(q.days, 14, 1, 365, "days") {
        Ok(d) => d,
        Err(r) => return r,
    };
    let limit = match validate_range(q.limit, 50, 1, 500, "limit") {
        Ok(l) => l,
        Err(r) => return r,
    };
    if !any_parquet_files(&state.data_dir) {
        return query_result_response(SUBAGENTS_COLUMNS, vec![]);
    }
    let glob = kikimimi_schema::paths::events_glob_sql_in(&state.data_dir);
    let event_source = linked_event_source(&glob);
    let from_dt = today_minus_days(days.saturating_sub(1));
    let sql = crate::query_cmd::subagents_sql(&glob, &from_dt, Some(limit));
    let sql = sql.replace(
        &format!("read_parquet('{glob}', union_by_name=true, hive_partitioning=false)"),
        &event_source,
    );
    respond(SUBAGENTS_COLUMNS, run_duckdb_json(&sql).await)
}

/// `/web/q/models?days=N` (KKM-34) columns: per (model, effort) usage.
const MODELS_COLUMNS: &[&str] = &[
    "model",
    "effort",
    "api_requests",
    "api_errors",
    "sessions",
    "subagent_api_requests",
    "subagent_tokens",
    "input_tokens",
    "output_tokens",
    "cache_read_tokens",
    "cache_write_tokens",
    "reasoning_tokens",
    "cost_usd",
    "reported_cost_requests",
    "complete_usage_requests",
    "complete_input_tokens",
    "complete_output_tokens",
    "complete_cache_read_tokens",
    "complete_cache_write_tokens",
];
const MODELS_DAILY_COLUMNS: &[&str] = &["dt", "model", "input_tokens", "output_tokens", "cost_usd"];

/// The `api.request` rows that count for usage (KKM-34), plus the
/// `api.error` counts per (model, effort). Needs an `e` CTE in scope. Per
/// session, OTel rows win over transcript (`log`) rows over anything else --
/// the same rule the subagents query uses for `session_tokens_est` -- so a
/// request seen by both sources is priced once. `IS NOT DISTINCT FROM`
/// keeps rows without a `session_id` (they form their own group).
const API_USAGE_CTE: &str = "pref AS ( \
    SELECT session_id, \
      min(CASE source WHEN 'otel' THEN 0 WHEN 'log' THEN 1 ELSE 2 END) AS src_rank \
    FROM e WHERE event_type = 'api.request' GROUP BY session_id \
  ), \
  api_usage AS ( \
    SELECT e.* FROM e \
    JOIN pref ON pref.session_id IS NOT DISTINCT FROM e.session_id \
    WHERE e.event_type = 'api.request' \
      AND (CASE e.source WHEN 'otel' THEN 0 WHEN 'log' THEN 1 ELSE 2 END) = pref.src_rank \
  ), \
  api_errors AS ( \
    SELECT coalesce(model, 'unknown') AS model, effort, count(*) AS api_errors \
    FROM e WHERE event_type = 'api.error' GROUP BY 1, 2 \
  )";

/// Grouped usage per (model, effort) shared by `/web/q/models` and the
/// session detail's `models` section; needs `api_usage` in scope. A row
/// counts as a subagent's when it has an `agent_id` (transcript), an
/// `agent_type` (OTel `agent.name`) or a `query_source` of `agent:...`
/// (OTel) -- OTel api.request rows never carry `agent_id`. The
/// callers FULL OUTER JOIN it with `api_errors` so a (model, effort) that
/// only ever errored still appears (0 requests, NULL usage) -- same as the
/// cloud's `MODELS_SQL`.
const MODELS_USAGE_CTE: &str = "usage AS ( \
    SELECT coalesce(model, 'unknown') AS model, effort, \
      count(*) AS api_requests, \
      count(DISTINCT session_id) AS sessions, \
      count(*) FILTER (WHERE agent_id IS NOT NULL OR agent_type IS NOT NULL OR query_source LIKE 'agent:%') AS subagent_api_requests, \
      CAST(sum(coalesce(input_tokens, 0) + coalesce(output_tokens, 0)) \
        FILTER (WHERE agent_id IS NOT NULL OR agent_type IS NOT NULL OR query_source LIKE 'agent:%') AS BIGINT) AS subagent_tokens, \
      CAST(sum(input_tokens) AS BIGINT) AS input_tokens, \
      CAST(sum(output_tokens) AS BIGINT) AS output_tokens, \
      CAST(sum(cache_read_tokens) AS BIGINT) AS cache_read_tokens, \
      CAST(sum(cache_write_tokens) AS BIGINT) AS cache_write_tokens, \
      CAST(sum(reasoning_tokens) AS BIGINT) AS reasoning_tokens, \
      sum(cost_usd) FILTER (WHERE cost_usd >= 0 AND cost_usd < 'Infinity') AS cost_usd, \
      count(*) FILTER (WHERE cost_usd >= 0 AND cost_usd < 'Infinity') AS reported_cost_requests, \
      count(*) FILTER (WHERE input_tokens >= 0 AND output_tokens >= 0 AND cache_read_tokens >= 0 AND cache_write_tokens >= 0 AND (model NOT LIKE 'gpt-%' OR input_tokens >= cache_read_tokens + cache_write_tokens)) AS complete_usage_requests, \
      CAST(sum(input_tokens) FILTER (WHERE input_tokens >= 0 AND output_tokens >= 0 AND cache_read_tokens >= 0 AND cache_write_tokens >= 0 AND (model NOT LIKE 'gpt-%' OR input_tokens >= cache_read_tokens + cache_write_tokens)) AS BIGINT) AS complete_input_tokens, \
      CAST(sum(output_tokens) FILTER (WHERE input_tokens >= 0 AND output_tokens >= 0 AND cache_read_tokens >= 0 AND cache_write_tokens >= 0 AND (model NOT LIKE 'gpt-%' OR input_tokens >= cache_read_tokens + cache_write_tokens)) AS BIGINT) AS complete_output_tokens, \
      CAST(sum(cache_read_tokens) FILTER (WHERE input_tokens >= 0 AND output_tokens >= 0 AND cache_read_tokens >= 0 AND cache_write_tokens >= 0 AND (model NOT LIKE 'gpt-%' OR input_tokens >= cache_read_tokens + cache_write_tokens)) AS BIGINT) AS complete_cache_read_tokens, \
      CAST(sum(cache_write_tokens) FILTER (WHERE input_tokens >= 0 AND output_tokens >= 0 AND cache_read_tokens >= 0 AND cache_write_tokens >= 0 AND (model NOT LIKE 'gpt-%' OR input_tokens >= cache_read_tokens + cache_write_tokens)) AS BIGINT) AS complete_cache_write_tokens \
    FROM api_usage GROUP BY 1, 2 \
  )";

/// Final select over `usage FULL OUTER JOIN api_errors` (see
/// [`MODELS_USAGE_CTE`]); `sessions` / `subagent_tokens` are projected out
/// by the session-detail caller via its column list.
const MODELS_FINAL_SQL: &str = "SELECT coalesce(usage.model, x.model) AS model, \
    coalesce(usage.effort, x.effort) AS effort, \
    coalesce(usage.api_requests, 0) AS api_requests, \
    coalesce(x.api_errors, 0) AS api_errors, \
    coalesce(usage.sessions, 0) AS sessions, \
    coalesce(usage.subagent_api_requests, 0) AS subagent_api_requests, \
    usage.subagent_tokens AS subagent_tokens, \
    usage.input_tokens AS input_tokens, usage.output_tokens AS output_tokens, \
    usage.cache_read_tokens AS cache_read_tokens, usage.cache_write_tokens AS cache_write_tokens, \
    usage.reasoning_tokens AS reasoning_tokens, usage.cost_usd AS cost_usd, usage.reported_cost_requests, usage.complete_usage_requests, usage.complete_input_tokens, usage.complete_output_tokens, usage.complete_cache_read_tokens, usage.complete_cache_write_tokens \
  FROM usage \
  FULL OUTER JOIN api_errors x ON x.model = usage.model AND x.effort IS NOT DISTINCT FROM usage.effort \
  ORDER BY coalesce(usage.input_tokens, 0) + coalesce(usage.output_tokens, 0) DESC, 1, 2;";

/// `/web/q/models?days=N` (KKM-34, local): `{models, daily, days}` --
/// per (model, effort) API requests, errors, sessions, subagent share and
/// token/cost sums over local Parquet, plus per-day per-model tokens/cost.
/// Sums are NULL (never 0) when nothing carried usage.
pub async fn models(State(state): State<WebAppState>, Query(q): Query<DaysQuery>) -> Response {
    let days = match validate_range(q.days, 14, 1, 365, "days") {
        Ok(d) => d,
        Err(r) => return r,
    };
    let empty = |columns: &[&str]| serde_json::json!({ "columns": columns, "rows": [] });
    if !any_parquet_files(&state.data_dir) {
        return Json(serde_json::json!({
            "models": empty(MODELS_COLUMNS),
            "daily": empty(MODELS_DAILY_COLUMNS),
            "days": days,
        }))
        .into_response();
    }
    let glob = kikimimi_schema::paths::events_glob_sql_in(&state.data_dir);
    let event_source = linked_event_source(&glob);
    let from_dt = today_minus_days(days.saturating_sub(1));
    let e_cte = format!(
        "e AS ( \
           SELECT * FROM {event_source} \
           WHERE dt >= '{from_dt}' AND event_type IN ('api.request', 'api.error') \
         )"
    );
    let models_sql =
        format!("WITH {e_cte}, {API_USAGE_CTE}, {MODELS_USAGE_CTE} {MODELS_FINAL_SQL}");
    let daily_sql = format!(
        "WITH {e_cte}, {API_USAGE_CTE} \
         SELECT dt, coalesce(model, 'unknown') AS model, \
           CAST(sum(input_tokens) AS BIGINT) AS input_tokens, \
           CAST(sum(output_tokens) AS BIGINT) AS output_tokens, \
           sum(cost_usd) AS cost_usd \
         FROM api_usage GROUP BY 1, 2 ORDER BY 1, 2;"
    );
    let (models, daily) = tokio::join!(run_duckdb_json(&models_sql), run_duckdb_json(&daily_sql));
    let (models, daily) = match (models, daily) {
        (Ok(m), Ok(d)) => (m, d),
        (Err(e), _) | (_, Err(e)) => return e.into_response(),
    };
    Json(serde_json::json!({
        "models": { "columns": MODELS_COLUMNS, "rows": project(&models, MODELS_COLUMNS) },
        "daily": { "columns": MODELS_DAILY_COLUMNS, "rows": project(&daily, MODELS_DAILY_COLUMNS) },
        "days": days,
    }))
    .into_response()
}

const SESSION_SUMMARY_COLUMNS: &[&str] = &[
    "session_id",
    "agent",
    "agent_version",
    "host_id",
    "repo",
    "started_at",
    "ended_at",
    "duration_ms",
    "ended",
    "events",
    "turns",
    "tool_calls",
    "failures",
    "tool_denied",
    "api_requests",
    "api_errors",
    "compactions",
    "subagents",
    "models",
    "sources",
    "input_tokens",
    "output_tokens",
    "cache_read_tokens",
    "cache_write_tokens",
    "cost_usd",
    "configured_mcp_servers",
    "configured_skills",
    "efforts",
    "client",
    "estimated_input_tokens",
    "estimated_output_tokens",
];
const SESSION_TOOLS_COLUMNS: &[&str] = &[
    "tool_name",
    "tool_kind",
    "mcp_server",
    "calls",
    "subagent_calls",
    "failures",
    "denied",
    "p50_duration_ms",
    "p95_duration_ms",
    "total_duration_ms",
];
const SESSION_SUBAGENTS_COLUMNS: &[&str] = &[
    "agent_id",
    "agent_type",
    "turn_id",
    "started_at",
    "duration_ms",
    "events",
    "tool_calls",
    "failures",
    "api_requests",
    "tokens_est",
    "tools",
    "models",
    "efforts",
    "model_source",
];
/// Per (model, effort) usage inside one session -- the session-scoped
/// sibling of [`MODELS_COLUMNS`] (KKM-34), same source-preference rule.
const SESSION_MODELS_COLUMNS: &[&str] = &[
    "model",
    "effort",
    "api_requests",
    "api_errors",
    "subagent_api_requests",
    "input_tokens",
    "output_tokens",
    "cache_read_tokens",
    "cache_write_tokens",
    "reasoning_tokens",
    "cost_usd",
    "reported_cost_requests",
    "complete_usage_requests",
    "complete_input_tokens",
    "complete_output_tokens",
    "complete_cache_read_tokens",
    "complete_cache_write_tokens",
];
const SESSION_TIMELINE_COLUMNS: &[&str] = &[
    "bucket_ts",
    "events",
    "tool_calls",
    "failures",
    "api_requests",
    "tokens",
    "subagent_events",
];
const SESSION_EVENTS_COLUMNS: &[&str] = &[
    "ts",
    "event_type",
    "source",
    "tool_name",
    "tool_kind",
    "mcp_server",
    "skill_name",
    "agent_id",
    "agent_type",
    "duration_ms",
    "success",
    "error_type",
    "decision",
    "model",
    "input_tokens",
    "output_tokens",
    "cost_usd",
    "turn_id",
    "effort",
    "estimated_input_tokens",
    "estimated_output_tokens",
    "correlation_key",
    "cache_read_tokens",
    "cache_write_tokens",
    "correlation_confidence",
];

#[derive(Debug, Deserialize)]
pub struct SessionDetailQuery {
    session_id: String,
    events_limit: Option<u32>,
}

/// Longest `session_id` accepted; same bound as the cloud handler.
const SESSION_ID_MAX_LEN: usize = 128;

/// `/web/q/session?session_id=...&events_limit=N` (local): one session,
/// drilled down — `{summary, tools, subagents, timeline, events, bucket_ms,
/// events_limit}`, each list in the usual `{columns, rows}` shape. DuckDB
/// port of the cloud's `web_query_sql::SESSION_*_SQL` (keep the two in
/// sync); one `duckdb` subprocess per section (the CLI runs one statement
/// per `-c`), the four detail queries in parallel after the summary decides
/// 404 and the timeline bucket. The local daemon is single-user, so there
/// is no self-scoping and no audit row.
pub async fn session_detail(
    State(state): State<WebAppState>,
    Query(q): Query<SessionDetailQuery>,
) -> Response {
    let session_id = q.session_id.trim();
    if session_id.is_empty() || session_id.len() > SESSION_ID_MAX_LEN {
        return json_error(
            StatusCode::BAD_REQUEST,
            &format!("session_id is required (1..={SESSION_ID_MAX_LEN} chars)"),
        );
    }
    let events_limit = match validate_range(q.events_limit, 500, 1, 2000, "events_limit") {
        Ok(l) => l,
        Err(r) => return r,
    };
    if !any_parquet_files(&state.data_dir) {
        return json_error(StatusCode::NOT_FOUND, "session not found");
    }
    let glob = kikimimi_schema::paths::events_glob_sql_in(&state.data_dir);
    let event_source = linked_event_source(&glob);
    let sid = session_id.replace('\'', "''");
    let e_cte = format!(
        "e AS ( \
           SELECT * FROM {event_source} \
           WHERE session_id = '{sid}' \
         )"
    );

    let summary_sql = format!(
        "WITH {e_cte}, {TOOL_RESULTS_CTE}, \
         fails AS ( \
           SELECT count(*) AS failures FROM ( \
             SELECT success FROM e WHERE event_type <> 'tool.result' \
             UNION ALL SELECT success FROM tool_results \
           ) u WHERE success = false \
         ) \
         SELECT max(e.session_id) AS session_id, \
           max(e.agent) AS agent, \
           max(e.agent_version) AS agent_version, \
           max(e.host_id) AS host_id, \
           max(e.repo) AS repo, \
           strftime(epoch_ms(min(e.ts)), '%Y-%m-%dT%H:%M:%SZ') AS started_at, \
           strftime(epoch_ms(max(e.ts)), '%Y-%m-%dT%H:%M:%SZ') AS ended_at, \
           CAST(max(e.ts) - min(e.ts) AS BIGINT) AS duration_ms, \
           bool_or(e.event_type = 'session.end') AS ended, \
           count(*) AS events, \
           count(*) FILTER (WHERE e.event_type = 'turn') AS turns, \
           count(*) FILTER (WHERE e.event_type = 'tool.call') AS tool_calls, \
           (SELECT failures FROM fails) AS failures, \
           count(*) FILTER (WHERE e.event_type = 'tool.denied') AS tool_denied, \
           count(*) FILTER (WHERE e.event_type = 'api.request') AS api_requests, \
           count(*) FILTER (WHERE e.event_type = 'api.error') AS api_errors, \
           count(*) FILTER (WHERE e.event_type = 'compaction') AS compactions, \
           count(DISTINCT e.agent_id) AS subagents, \
           coalesce(string_agg(DISTINCT e.model, ','), '') AS models, \
           coalesce(string_agg(DISTINCT e.source, ','), '') AS sources, \
           CAST(sum(e.input_tokens) AS BIGINT) AS input_tokens, \
           CAST(sum(e.output_tokens) AS BIGINT) AS output_tokens, \
           CAST(sum(e.cache_read_tokens) AS BIGINT) AS cache_read_tokens, \
           CAST(sum(e.cache_write_tokens) AS BIGINT) AS cache_write_tokens, \
           sum(e.cost_usd) AS cost_usd, \
           max(e.configured_mcp_servers) AS configured_mcp_servers, \
           max(e.configured_skills) AS configured_skills, \
           coalesce(string_agg(DISTINCT e.effort, ',' ORDER BY e.effort), '') AS efforts, string_agg(DISTINCT e.client, ',' ORDER BY e.client) AS client, \
           CAST(sum(try_cast(json_extract_string(to_json(e), '$.estimated_input_tokens') AS BIGINT)) AS BIGINT) AS estimated_input_tokens, \
           CAST(sum(try_cast(json_extract_string(to_json(e), '$.estimated_output_tokens') AS BIGINT)) AS BIGINT) AS estimated_output_tokens \
         FROM e HAVING count(*) > 0;"
    );
    let mut summary_rows = match run_duckdb_json(&summary_sql).await {
        Ok(rows) => rows,
        Err(e) => return e.into_response(),
    };
    enrich_session_clients(&mut summary_rows);
    let Some(summary_row) = summary_rows.first() else {
        return json_error(StatusCode::NOT_FOUND, "session not found");
    };
    let duration_ms = summary_row
        .get("duration_ms")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let bucket_ms = timeline_bucket_ms(duration_ms);

    let tools_sql = format!(
        "WITH {e_cte}, {TOOL_RESULTS_CTE}, \
         results AS ( \
           SELECT tool_name, \
             count(*) FILTER (WHERE success = false) AS failures, \
             quantile_cont(duration_ms, 0.5) AS p50_duration_ms, \
             quantile_cont(duration_ms, 0.95) AS p95_duration_ms, \
             CAST(sum(duration_ms) AS BIGINT) AS total_duration_ms \
           FROM tool_results GROUP BY tool_name \
         ) \
         SELECT e.tool_name AS tool_name, \
           max(e.tool_kind) AS tool_kind, \
           max(e.mcp_server) AS mcp_server, \
           count(*) FILTER (WHERE e.event_type = 'tool.call') AS calls, \
           count(*) FILTER (WHERE e.event_type = 'tool.call' AND e.agent_id IS NOT NULL) AS subagent_calls, \
           coalesce(max(results.failures), 0) AS failures, \
           count(*) FILTER (WHERE e.event_type = 'tool.denied') AS denied, \
           CAST(max(results.p50_duration_ms) AS DOUBLE) AS p50_duration_ms, \
           CAST(max(results.p95_duration_ms) AS DOUBLE) AS p95_duration_ms, \
           max(results.total_duration_ms) AS total_duration_ms \
         FROM e LEFT JOIN results ON results.tool_name = e.tool_name \
         WHERE e.tool_name IS NOT NULL \
         GROUP BY e.tool_name ORDER BY calls DESC, tool_name;"
    );
    // models / efforts / model_source: the agent's own rows, else the OTel
    // api.request rows of the same agent_type inside its time window (±5 s)
    // -- see the cloud's SESSION_SUBAGENTS_SQL doc for why.
    let subagents_sql = format!(
        "WITH {e_cte}, \
         agents AS ( \
           SELECT agent_id, \
             max(agent_type) AS agent_type, \
             max(turn_id) AS turn_id, \
             min(ts) AS first_ts, max(ts) AS last_ts, \
             CAST(coalesce(max(duration_ms) FILTER (WHERE event_type = 'subagent.stop'), CASE WHEN max(ts) > min(ts) THEN max(ts) - min(ts) END) AS BIGINT) AS duration_ms, \
             count(*) AS events, \
             count(*) FILTER (WHERE event_type = 'tool.call') AS tool_calls, \
             count(*) FILTER (WHERE success = false) AS failures, \
             count(*) FILTER (WHERE event_type = 'api.request') AS api_requests, \
             CAST(coalesce( \
               sum(coalesce(input_tokens, 0) + coalesce(output_tokens, 0)) \
                 FILTER (WHERE event_type = 'api.request' AND (input_tokens IS NOT NULL OR output_tokens IS NOT NULL)), \
               max(coalesce(input_tokens, 0) + coalesce(output_tokens, 0)) \
                 FILTER (WHERE event_type = 'subagent.stop' AND (input_tokens IS NOT NULL OR output_tokens IS NOT NULL)) \
             ) AS BIGINT) AS tokens_est, \
             string_agg(DISTINCT tool_name, ',' ORDER BY tool_name) AS tools, \
             string_agg(DISTINCT model, ',' ORDER BY model) AS own_models, \
             string_agg(DISTINCT effort, ',' ORDER BY effort) AS own_efforts \
           FROM e WHERE agent_id IS NOT NULL GROUP BY agent_id \
         ), \
         win AS ( \
           SELECT a.agent_id, \
             string_agg(DISTINCT o.model, ',' ORDER BY o.model) AS models, \
             string_agg(DISTINCT o.effort, ',' ORDER BY o.effort) AS efforts \
           FROM agents a JOIN e o ON o.agent_id IS NULL AND o.event_type = 'api.request' \
             AND o.agent_type = a.agent_type AND o.ts BETWEEN a.first_ts - 5000 AND a.last_ts + 5000 \
           WHERE a.agent_type IS NOT NULL AND a.agent_type <> '' \
           GROUP BY a.agent_id \
         ) \
         SELECT a.agent_id, a.agent_type, a.turn_id, \
           strftime(epoch_ms(a.first_ts), '%Y-%m-%dT%H:%M:%SZ') AS started_at, \
           a.duration_ms, a.events, a.tool_calls, a.failures, a.api_requests, a.tokens_est, a.tools, \
           coalesce(a.own_models, w.models) AS models, \
           coalesce(a.own_efforts, w.efforts) AS efforts, \
           CASE WHEN a.own_models IS NOT NULL OR a.own_efforts IS NOT NULL THEN 'agent' \
                WHEN w.models IS NOT NULL OR w.efforts IS NOT NULL THEN 'otel_window' END AS model_source \
         FROM agents a LEFT JOIN win w ON w.agent_id = a.agent_id \
         ORDER BY a.first_ts, a.agent_id;"
    );
    let models_sql =
        format!("WITH {e_cte}, {API_USAGE_CTE}, {MODELS_USAGE_CTE} {MODELS_FINAL_SQL}");
    let timeline_sql = format!(
        "WITH {e_cte}, {TOOL_RESULTS_CTE}, \
         u AS ( \
           SELECT ts, event_type, success, input_tokens, output_tokens, agent_id FROM e WHERE event_type <> 'tool.result' \
           UNION ALL \
           SELECT ts, event_type, success, input_tokens, output_tokens, agent_id FROM tool_results \
         ) \
         SELECT CAST((ts // {bucket_ms}) * {bucket_ms} AS BIGINT) AS bucket_ts, \
           count(*) AS events, \
           count(*) FILTER (WHERE event_type = 'tool.call') AS tool_calls, \
           count(*) FILTER (WHERE success = false) AS failures, \
           count(*) FILTER (WHERE event_type = 'api.request') AS api_requests, \
           CAST(sum(coalesce(input_tokens, 0) + coalesce(output_tokens, 0)) AS BIGINT) AS tokens, \
           count(*) FILTER (WHERE agent_id IS NOT NULL) AS subagent_events \
         FROM u GROUP BY 1 ORDER BY 1;"
    );
    let events_sql = format!(
        "WITH {e_cte}, {TOOL_RESULTS_CTE}, \
         u AS ( \
           SELECT e.*, 1 AS src_rank FROM e WHERE event_type <> 'tool.result' \
           UNION ALL SELECT * FROM tool_results \
         ) \
         SELECT ts, event_type, source, tool_name, tool_kind, mcp_server, skill_name, \
           agent_id, agent_type, duration_ms, success, error_type, decision, model, \
           input_tokens, output_tokens, cost_usd, turn_id, effort, \
           try_cast(json_extract_string(to_json(u), '$.estimated_input_tokens') AS BIGINT) AS estimated_input_tokens, \
           try_cast(json_extract_string(to_json(u), '$.estimated_output_tokens') AS BIGINT) AS estimated_output_tokens, u.correlation_key, u.cache_read_tokens, u.cache_write_tokens, u.correlation_confidence \
         FROM u ORDER BY ts, event_id LIMIT {events_limit};"
    );

    let (tools, subagents, models, timeline, events) = tokio::join!(
        run_duckdb_json(&tools_sql),
        run_duckdb_json(&subagents_sql),
        run_duckdb_json(&models_sql),
        run_duckdb_json(&timeline_sql),
        run_duckdb_json(&events_sql),
    );
    let section = |columns: &[&str], rows: Result<Vec<Map<String, Value>>, DuckDbError>| {
        rows.map(|rows| serde_json::json!({ "columns": columns, "rows": project(&rows, columns) }))
    };
    let (tools, subagents, models, timeline, events) = match (
        section(SESSION_TOOLS_COLUMNS, tools),
        section(SESSION_SUBAGENTS_COLUMNS, subagents),
        section(SESSION_MODELS_COLUMNS, models),
        section(SESSION_TIMELINE_COLUMNS, timeline),
        section(SESSION_EVENTS_COLUMNS, events),
    ) {
        (Ok(a), Ok(b), Ok(c), Ok(d), Ok(f)) => (a, b, c, d, f),
        (Err(e), _, _, _, _)
        | (_, Err(e), _, _, _)
        | (_, _, Err(e), _, _)
        | (_, _, _, Err(e), _)
        | (_, _, _, _, Err(e)) => return e.into_response(),
    };

    Json(serde_json::json!({
        "summary": {
            "columns": SESSION_SUMMARY_COLUMNS,
            "rows": project(&summary_rows, SESSION_SUMMARY_COLUMNS),
        },
        "tools": tools,
        "subagents": subagents,
        "models": models,
        "timeline": timeline,
        "events": events,
        "bucket_ms": bucket_ms,
        "events_limit": events_limit,
    }))
    .into_response()
}

/// Timeline bucket width for a session spanning `duration_ms`: ≥ 1 minute,
/// coarse enough for ~240 buckets, snapped to a human step. Same table as
/// the cloud's `web_query::timeline_bucket_ms` (keep in sync).
fn timeline_bucket_ms(duration_ms: i64) -> i64 {
    const MIN: i64 = 60_000;
    const STEPS: &[i64] = &[
        MIN,
        2 * MIN,
        5 * MIN,
        10 * MIN,
        15 * MIN,
        30 * MIN,
        60 * MIN,
        120 * MIN,
        360 * MIN,
        720 * MIN,
        1440 * MIN,
    ];
    let target = duration_ms.max(0) / 240;
    STEPS
        .iter()
        .copied()
        .find(|&s| s >= target)
        .unwrap_or(1440 * MIN)
}

/// `/web/q/patterns` (KKM-11, local): the same ranking the cloud serves
/// from `pattern_hits`, computed live over local Parquet with
/// `query_cmd::PATTERNS_SQL` (one machine's data — cheap enough to not
/// persist). Columns match the cloud contract exactly.
pub async fn patterns(State(state): State<WebAppState>, Query(q): Query<DaysQuery>) -> Response {
    let days = match validate_range(q.days, 30, 1, 365, "days") {
        Ok(d) => d,
        Err(r) => return r,
    };
    if !any_parquet_files(&state.data_dir) {
        return query_result_response(PATTERNS_COLUMNS, vec![]);
    }
    let glob = kikimimi_schema::paths::events_glob_sql_in(&state.data_dir);
    // Pattern detection uses native tool-session IDs. Apply display filters to
    // that same source, without repeatedly resolving chat-response links.
    let event_source = crate::activity_filter::source(&format!(
        "read_parquet('{glob}', union_by_name=true, hive_partitioning=false)"
    ));
    let from_dt = today_minus_days(days.saturating_sub(1));
    let hits = crate::query_cmd::patterns_subquery(&glob, &from_dt).replace(
        &format!("read_parquet('{glob}', union_by_name=true, hive_partitioning=false)"),
        "history",
    );
    let sql = format!(
        "WITH history AS MATERIALIZED (SELECT * FROM {event_source} WHERE dt >= '{from_dt}' AND session_id IS NOT NULL), hits AS ({hits}), \
         with_dt AS ( \
           SELECT h.*, strftime(epoch_ms(h.first_ts), '%Y-%m-%d') AS dt FROM hits h \
         ) \
         SELECT pattern_id, subject, \
           count(DISTINCT session_id)::BIGINT AS sessions, \
           sum(incidents)::BIGINT AS incidents, \
           sum(wasted_tokens_est)::BIGINT AS wasted_tokens_est, \
           count(*) FILTER (WHERE wasted_tokens_est IS NOT NULL)::BIGINT AS priced_hits, \
           count(*)::BIGINT AS hits, \
           (sum(wasted_tokens_est) * count(DISTINCT session_id))::BIGINT AS priority, \
           min(dt) AS first_seen_dt, \
           max(dt) AS last_seen_dt \
         FROM with_dt \
         GROUP BY pattern_id, subject \
         ORDER BY priority DESC NULLS LAST, sessions DESC, incidents DESC, pattern_id, subject;"
    );
    respond(PATTERNS_COLUMNS, run_duckdb_json(&sql).await)
}

/// `/web/q/pattern-hits` (KKM-11, local): the incidents behind one ranking
/// row, newest first. Single-user machine, so no role scoping here.
pub async fn pattern_hits(
    State(state): State<WebAppState>,
    Query(q): Query<PatternHitsQuery>,
) -> Response {
    let days = match validate_range(q.days, 30, 1, 365, "days") {
        Ok(d) => d,
        Err(r) => return r,
    };
    let limit = match validate_range(q.limit, 50, 1, 500, "limit") {
        Ok(l) => l,
        Err(r) => return r,
    };
    if q.pattern_id.is_empty() || q.subject.is_empty() {
        return json_error(
            StatusCode::BAD_REQUEST,
            "pattern_id and subject are required",
        );
    }
    if !any_parquet_files(&state.data_dir) {
        return query_result_response(PATTERN_HITS_COLUMNS, vec![]);
    }
    let glob = kikimimi_schema::paths::events_glob_sql_in(&state.data_dir);
    // Pattern detection uses native tool-session IDs. Apply display filters to
    // that same source, without repeatedly resolving chat-response links.
    let event_source = crate::activity_filter::source(&format!(
        "read_parquet('{glob}', union_by_name=true, hive_partitioning=false)"
    ));
    let from_dt = today_minus_days(days.saturating_sub(1));
    let hits = crate::query_cmd::pattern_subquery(&glob, &from_dt, &q.pattern_id).replace(
        &format!("read_parquet('{glob}', union_by_name=true, hive_partitioning=false)"),
        "history",
    );
    let pattern_id = q.pattern_id.replace('\'', "''");
    let subject = q.subject.replace('\'', "''");
    let sql = format!(
        "WITH history AS MATERIALIZED (SELECT * FROM {event_source} WHERE dt >= '{from_dt}' AND session_id IS NOT NULL), hits AS ({hits}) \
         SELECT strftime(epoch_ms(h.first_ts), '%Y-%m-%d') AS dt, h.session_id, h.first_ts, h.last_ts, h.incidents, h.wasted_tokens_est, h.detail \
         FROM hits h \
         WHERE h.pattern_id = '{pattern_id}' AND h.subject = '{subject}' \
         ORDER BY h.first_ts DESC \
         LIMIT {limit};"
    );
    respond(PATTERN_HITS_COLUMNS, run_duckdb_json(&sql).await)
}

/// `/web/q/pattern-timeline` (KKM-12, local): per-day sessions / hit rate /
/// cost for one (pattern, subject), same columns as the cloud.
pub async fn pattern_timeline(
    State(state): State<WebAppState>,
    Query(q): Query<PatternSubjectQuery>,
) -> Response {
    let days = match validate_range(q.days, 60, 1, 365, "days") {
        Ok(d) => d,
        Err(r) => return r,
    };
    if q.pattern_id.is_empty() || q.subject.is_empty() {
        return json_error(
            StatusCode::BAD_REQUEST,
            "pattern_id and subject are required",
        );
    }
    if !any_parquet_files(&state.data_dir) {
        return query_result_response(PATTERN_TIMELINE_COLUMNS, vec![]);
    }
    let glob = kikimimi_schema::paths::events_glob_sql_in(&state.data_dir);
    // Pattern detection uses native tool-session IDs. Apply display filters to
    // that same source, without repeatedly resolving chat-response links.
    let event_source = crate::activity_filter::source(&format!(
        "read_parquet('{glob}', union_by_name=true, hive_partitioning=false)"
    ));
    let from_dt = today_minus_days(days.saturating_sub(1));
    let hits = crate::query_cmd::pattern_subquery(&glob, &from_dt, &q.pattern_id).replace(
        &format!("read_parquet('{glob}', union_by_name=true, hive_partitioning=false)"),
        "history",
    );
    let pattern_id = q.pattern_id.replace('\'', "''");
    let subject = q.subject.replace('\'', "''");
    let eligible = match q.pattern_id.as_str() {
        "retry_spiral" | "permission_denied_loop" => format!(
            "tool_name = '{subject}' AND ((event_type = 'tool.result' AND success IS NOT NULL) OR event_type = 'tool.denied')"),
        "mcp_bypass" => format!("mcp_server = '{subject}' AND event_type = 'tool.result' AND success IS NOT NULL"),
        "long_tool_tail" => format!("mcp_server = '{subject}' AND event_type = 'tool.result' AND duration_ms IS NOT NULL"),
        "unused_mcp_server" => format!("configured_mcp_servers IS NOT NULL AND list_contains(from_json(configured_mcp_servers, '[\"VARCHAR\"]'), '{subject}')"),
        _ => "event_type = 'api.request' AND input_tokens IS NOT NULL".to_owned(),
    };
    let sql = format!(
        "WITH history AS MATERIALIZED (SELECT * FROM {event_source} WHERE dt >= '{from_dt}' AND session_id IS NOT NULL), hits AS ({hits}), \
         sess AS ( \
           SELECT DISTINCT session_id, dt \
           FROM history \
           WHERE session_id IS NOT NULL AND dt >= '{from_dt}' AND ({eligible}) \
         ), \
         days AS (SELECT dt, count(*)::BIGINT AS sessions_total FROM sess GROUP BY dt), \
         hit_days AS ( \
           SELECT s.dt, \
             count(DISTINCT h.session_id)::BIGINT AS sessions_hit, \
             sum(h.incidents)::BIGINT AS incidents, \
             sum(h.wasted_tokens_est)::BIGINT AS wasted_tokens_est \
           FROM hits h JOIN sess s ON s.session_id = h.session_id AND s.dt = strftime(epoch_ms(h.first_ts), '%Y-%m-%d') \
           WHERE h.pattern_id = '{pattern_id}' AND h.subject = '{subject}' \
           GROUP BY s.dt \
         ) \
         SELECT d.dt, d.sessions_total, \
           coalesce(hd.sessions_hit, 0)::BIGINT AS sessions_hit, \
           (100.0 * coalesce(hd.sessions_hit, 0) / NULLIF(d.sessions_total, 0))::DOUBLE AS rate_pct, \
           coalesce(hd.incidents, 0)::BIGINT AS incidents, \
           hd.wasted_tokens_est \
         FROM days d LEFT JOIN hit_days hd ON hd.dt = d.dt \
         ORDER BY d.dt;"
    );
    respond(PATTERN_TIMELINE_COLUMNS, run_duckdb_json(&sql).await)
}

/// `GET /web/marks?pattern_id=&subject=` (KKM-12, local).
pub async fn list_marks(
    State(state): State<WebAppState>,
    Query(q): Query<PatternSubjectQuery>,
) -> Response {
    let _guard = MARKS_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let marks = match load_marks(&marks_path(&state)) {
        Ok(marks) => marks,
        Err(_) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, "Cannot read change records"),
    };
    let scope = crate::activity_filter::scope_key();
    let marks: Vec<Mark> = marks.into_iter()
        .filter(|m| m.scope == scope && (q.pattern_id.is_empty() || m.pattern_id == q.pattern_id)
            && (q.subject.is_empty() || m.subject == q.subject)).collect();
    (StatusCode::OK, Json(serde_json::json!({ "marks": marks }))).into_response()
}

/// `POST /web/marks` (KKM-12, local).
pub async fn create_mark(
    State(state): State<WebAppState>,
    Json(body): Json<CreateMarkRequest>,
) -> Response {
    if body.pattern_id.is_empty() || body.subject.is_empty() {
        return json_error(
            StatusCode::BAD_REQUEST,
            "pattern_id and subject are required",
        );
    }
    if chrono::NaiveDate::parse_from_str(&body.marked_dt, "%Y-%m-%d").is_err() {
        return json_error(StatusCode::BAD_REQUEST, "marked_dt must be YYYY-MM-DD");
    }
    if body.marked_dt > chrono::Utc::now().format("%Y-%m-%d").to_string() {
        return json_error(StatusCode::BAD_REQUEST, "Change date cannot be in the future (UTC)");
    }
    if body.note.trim().is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "Describe what changed");
    }
    if body.note.chars().count() > 500 {
        return json_error(
            StatusCode::BAD_REQUEST,
            "note must be 500 characters or fewer",
        );
    }
    let path = marks_path(&state);
    let _guard = MARKS_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut marks = match load_marks(&path) {
        Ok(marks) => marks,
        Err(_) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, "Cannot read change records"),
    };
    let mark = Mark {
        id: uuid::Uuid::new_v4().to_string(),
        pattern_id: body.pattern_id,
        subject: body.subject,
        marked_dt: body.marked_dt,
        note: body.note,
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        scope: crate::activity_filter::scope_key(),
    };
    marks.push(mark.clone());
    marks.sort_by(|a, b| {
        (a.marked_dt.as_str(), a.created_at.as_str())
            .cmp(&(b.marked_dt.as_str(), b.created_at.as_str()))
    });
    if let Err(e) = save_marks(&path, &marks) {
        return json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("saving marks: {e}"),
        );
    }
    (
        StatusCode::OK,
        Json(serde_json::to_value(mark).unwrap_or(Value::Null)),
    )
        .into_response()
}

/// `DELETE /web/marks/{id}` (KKM-12, local).
pub async fn delete_mark(
    State(state): State<WebAppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    let path = marks_path(&state);
    let _guard = MARKS_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut marks = match load_marks(&path) {
        Ok(marks) => marks,
        Err(_) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, "Cannot read change records"),
    };
    let before = marks.len();
    let scope = crate::activity_filter::scope_key();
    marks.retain(|m| m.id != id || m.scope != scope);
    if marks.len() == before {
        return json_error(StatusCode::NOT_FOUND, "no such mark");
    }
    if let Err(e) = save_marks(&path, &marks) {
        return json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("saving marks: {e}"),
        );
    }
    (StatusCode::OK, Json(serde_json::json!({ "deleted": id }))).into_response()
}

fn respond(columns: &[&str], result: Result<Vec<Map<String, Value>>, DuckDbError>) -> Response {
    match result {
        Ok(rows) => query_result_response(columns, project(&rows, columns)),
        Err(e) => e.into_response(),
    }
}

/// `value`, defaulted to `default` when absent, must fall in `min..=max` or
/// this returns a ready-to-return 400 `Response` (task spec: "days/limit
/// params validated (1..=365, 1..=500)").
fn validate_range(
    value: Option<u32>,
    default: u32,
    min: u32,
    max: u32,
    name: &str,
) -> Result<u32, Response> {
    let v = value.unwrap_or(default);
    if (min..=max).contains(&v) {
        Ok(v)
    } else {
        Err(json_error(
            StatusCode::BAD_REQUEST,
            &format!("{name} must be between {min} and {max}, got {v}"),
        ))
    }
}

fn today_minus_days(n: u32) -> String {
    (chrono::Utc::now().date_naive() - chrono::Duration::days(i64::from(n)))
        .format("%Y-%m-%d")
        .to_string()
}

/// Whether `data_dir` has at least one `dt=*/*.parquet` file. A fresh install
/// (daemon running, nothing flushed yet) has none, and DuckDB's
/// `read_parquet('.../dt=*/*.parquet')` *errors* (not an empty result) when
/// the glob matches zero files -- checked against the real `duckdb` CLI, not
/// assumed. Short-circuiting here turns that into the empty-but-successful
/// `QueryResult` the SPA expects, and skips spawning DuckDB entirely.
fn any_parquet_files(data_dir: &Path) -> bool {
    if kikimimi_sink::store::registered(data_dir).is_some() {
        return true;
    }
    let Ok(entries) = std::fs::read_dir(data_dir) else {
        return false;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if entry.file_name() == kikimimi_schema::paths::SCHEMA_STUB_PARTITION {
            continue; // the zero-row schema stub is not data
        }
        let Ok(sub) = std::fs::read_dir(&path) else {
            continue;
        };
        let has_parquet = sub
            .filter_map(Result::ok)
            .any(|e| e.path().extension().and_then(|x| x.to_str()) == Some("parquet"));
        if has_parquet {
            return true;
        }
    }
    false
}

/// Picks `columns`, in order, out of each DuckDB row object by key name
/// (rather than trusting JSON object key *order*, which the JSON spec never
/// guarantees and `serde_json::Value::Object`'s default map type doesn't
/// preserve either) -- `columns` is always the same list the query's own
/// `SELECT ... AS <name>` aliases used, so this is just "read the row map by
/// name" plus `Value::Null` for anything unexpectedly absent.
fn project(rows: &[Map<String, Value>], columns: &[&str]) -> Vec<Vec<Value>> {
    rows.iter()
        .map(|row| {
            columns
                .iter()
                .map(|c| row.get(*c).cloned().unwrap_or(Value::Null))
                .collect()
        })
        .collect()
}

fn query_result_response(columns: &[&str], rows: Vec<Vec<Value>>) -> Response {
    Json(serde_json::json!({ "columns": columns, "rows": rows })).into_response()
}

fn json_error(status: StatusCode, msg: &str) -> Response {
    axum::response::Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(
            serde_json::json!({ "error": msg }).to_string(),
        ))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

#[derive(Debug)]
enum DuckDbError {
    NotFound,
    Timeout,
    Failed(String),
    BadOutput(String),
}

impl IntoResponse for DuckDbError {
    fn into_response(self) -> Response {
        match self {
            // Exact body per task spec: `503 {"error":"duckdb CLI not found"}`.
            DuckDbError::NotFound => {
                json_error(StatusCode::SERVICE_UNAVAILABLE, "duckdb CLI not found")
            }
            DuckDbError::Timeout => {
                json_error(StatusCode::INTERNAL_SERVER_ERROR, "duckdb query timed out")
            }
            DuckDbError::Failed(msg) => json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("duckdb query failed: {msg}"),
            ),
            DuckDbError::BadOutput(msg) => json_error(StatusCode::INTERNAL_SERVER_ERROR, &msg),
        }
    }
}

/// Shells out to `duckdb -json -c <sql>` (task spec pattern, same flags as
/// `query_cmd.rs`'s sync `run_duckdb`, but async + time-bounded since this
/// runs inside the daemon's request loop instead of a one-shot CLI command).
///
/// `kill_on_drop(true)` is what makes the timeout actually bound wall time:
/// `tokio::time::timeout` dropping the inner future drops the `Child` inside
/// it, and `kill_on_drop` turns that drop into a SIGKILL instead of an
/// orphaned/zombie process. `wait_with_output()` (rather than `wait()` then a
/// separate stdout read) drains stdout/stderr concurrently with waiting, so a
/// large `sessions` result can't deadlock on a full pipe buffer.
// GUI-launched processes on macOS inherit a small descriptor limit (often
// 256). Parquet scans can exceed it even though the same query works in a shell.
// Adjust only the query child, within its existing hard limit.
#[cfg(unix)]
fn configure_query_file_limit(cmd: &mut tokio::process::Command) {
    unsafe {
        cmd.pre_exec(|| {
            let mut limit = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            let target = limit.rlim_max.min(4096);
            if limit.rlim_cur < target {
                limit.rlim_cur = target;
                if libc::setrlimit(libc::RLIMIT_NOFILE, &limit) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
}

async fn run_duckdb_json(sql: &str) -> Result<Vec<Map<String, Value>>, DuckDbError> {
    let owned = sql.to_string();
    if let Some(result) =
        tokio::task::spawn_blocking(move || kikimimi_sink::store::registered_query(&owned))
            .await
            .map_err(|e| DuckDbError::Failed(e.to_string()))?
    {
        return result.map_err(|e| DuckDbError::Failed(e.to_string()));
    }
    let mut cmd = tokio::process::Command::new("duckdb");
    cmd.args(["-json", "-c", sql])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    #[cfg(unix)]
    configure_query_file_limit(&mut cmd);

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(DuckDbError::NotFound),
        Err(e) => return Err(DuckDbError::Failed(format!("spawning duckdb: {e}"))),
    };

    let output = match tokio::time::timeout(DUCKDB_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(DuckDbError::Failed(format!("running duckdb: {e}"))),
        Err(_elapsed) => return Err(DuckDbError::Timeout),
    };

    if !output.status.success() {
        return Err(DuckDbError::Failed(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }

    let rows: Vec<Value> = serde_json::from_slice(&output.stdout)
        .map_err(|e| DuckDbError::BadOutput(format!("parsing duckdb -json output: {e}")))?;

    rows.into_iter()
        .map(|row| match row {
            Value::Object(map) => Ok(map),
            other => Err(DuckDbError::BadOutput(format!(
                "expected a JSON object row from `duckdb -json`, got {other}"
            ))),
        })
        .collect()
}

/// `kikimimi status` warns when this is `false` (task spec). Separate from
/// `run_duckdb_json`'s own `NotFound` handling (which already 503s per
/// request) -- this is a cheap one-shot presence check for a CLI summary,
/// not the request path, so it stays sync (`status_cmd::run` is sync
/// end-to-end).
pub(crate) fn duckdb_available() -> bool {
    match std::process::Command::new("duckdb")
        .arg("--version")
        .output()
    {
        Ok(_) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sessions_reads_legacy_and_mixed_parquet_without_estimated_tokens() {
        if !duckdb_available() {
            eprintln!("skipping Parquet compatibility test: duckdb CLI unavailable");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let dt = today_minus_days(0);
        let partition = dir.path().join(format!("dt={dt}"));
        std::fs::create_dir_all(&partition).unwrap();
        let path = partition
            .join("legacy.parquet")
            .display()
            .to_string()
            .replace('\'', "''");
        let ts = chrono::Utc::now().timestamp_millis();
        run_duckdb_json(&format!(
            "COPY (SELECT 'session' AS session_id, 'claude' AS agent, 'host' AS host_id,
             {ts}::BIGINT AS ts, '{dt}' AS dt, 'api.request' AS event_type, 'otel' AS source,
             NULL::VARCHAR AS correlation_key, NULL::BOOLEAN AS success, 'model' AS model,
             100::BIGINT AS input_tokens, 20::BIGINT AS output_tokens, 0.5::DOUBLE AS cost_usd,
             'claude_code_cli' AS client, NULL::VARCHAR AS parent_session_id,
             NULL::VARCHAR AS turn_id, NULL::VARCHAR AS correlation_confidence)
             TO '{path}' (FORMAT PARQUET); SELECT 1 AS created"
        ))
        .await
        .unwrap();
        let response = sessions(
            State(WebAppState {
                token: "test".into(),
                data_dir: dir.path().into(),
            }),
            Query(DaysLimitQuery {
                days: Some(14),
                limit: Some(50),
            }),
        )
        .await;
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["rows"].as_array().unwrap().len(), 1);
        let row = &json["rows"][0];
        assert_eq!(row[8], 100);
        assert_eq!(row[9], 20);
        assert!(row[12].is_null());
        assert!(row[13].is_null());

        // Old and new files can coexist after an upgrade without losing usage.
        let modern = partition
            .join("modern.parquet")
            .display()
            .to_string()
            .replace('\'', "''");
        run_duckdb_json(&format!(
            "COPY (SELECT *, 3::BIGINT AS estimated_input_tokens, 7::BIGINT AS estimated_output_tokens
             FROM read_parquet('{path}')) TO '{modern}' (FORMAT PARQUET); SELECT 1 AS created"
        )).await.unwrap();
        let response = sessions(
            State(WebAppState {
                token: "test".into(),
                data_dir: dir.path().into(),
            }),
            Query(DaysLimitQuery {
                days: Some(14),
                limit: Some(50),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["rows"].as_array().unwrap().len(), 1);
        let row = &json["rows"][0];
        assert_eq!(row[8], 200);
        assert_eq!(row[9], 40);
        assert_eq!(row[12], 3);
        assert_eq!(row[13], 7);
    }

    #[tokio::test]
    async fn sessions_summary_preserves_usage_and_deduplicates_failures() {
        let dir = tempfile::tempdir().unwrap();
        let store = kikimimi_sink::store::shared(dir.path()).unwrap();
        let ts = chrono::Utc::now().timestamp_millis();
        let base = kikimimi_schema::Event {
            host_id: "sessions-test-host".into(),
            session_id: Some("session".into()),
            agent: "claude".into(),
            client: Some("claude_code_cli".into()),
            ts,
            dt: kikimimi_schema::dt_of(ts),
            // Large content must not affect summary computation.
            prompt_text: Some("content ".repeat(16_384)),
            ..Default::default()
        };
        let events = [
            kikimimi_schema::Event {
                event_id: "usage".into(),
                event_type: "api.request".into(),
                source: "otel".into(),
                input_tokens: Some(100),
                output_tokens: Some(20),
                estimated_input_tokens: Some(3),
                estimated_output_tokens: Some(7),
                ..base.clone()
            },
            kikimimi_schema::Event {
                event_id: "call".into(),
                event_type: "tool.call".into(),
                source: "hook".into(),
                ..base.clone()
            },
            kikimimi_schema::Event {
                event_id: "hook".into(),
                event_type: "tool.result".into(),
                source: "hook".into(),
                correlation_key: Some("tool-1".into()),
                success: Some(false),
                ..base.clone()
            },
            kikimimi_schema::Event {
                event_id: "otel".into(),
                event_type: "tool.result".into(),
                source: "otel".into(),
                correlation_key: Some("tool-1".into()),
                success: Some(false),
                ..base.clone()
            },
            kikimimi_schema::Event {
                event_id: "uncorrelated".into(),
                event_type: "tool.result".into(),
                source: "hook".into(),
                success: Some(false),
                ..base.clone()
            },
            kikimimi_schema::Event {
                event_id: "no-usage".into(),
                session_id: Some("empty".into()),
                event_type: "session.start".into(),
                source: "hook".into(),
                ..base
            },
        ];
        for event in events {
            store.lock().unwrap().ingest(&event, None).unwrap();
        }
        let response = sessions(
            State(WebAppState {
                token: "test".into(),
                data_dir: dir.path().into(),
            }),
            Query(DaysLimitQuery {
                days: Some(14),
                limit: Some(500),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["columns"], serde_json::json!(SESSIONS_COLUMNS));
        let rows = json["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        let row = rows.iter().find(|r| r[0] == "session").unwrap();
        assert_eq!(row[4], 5);
        assert_eq!(row[5], 1);
        assert_eq!(row[6], 2);
        assert_eq!(row[8], 100);
        assert_eq!(row[9], 20);
        assert_eq!(row[12], 3);
        assert_eq!(row[13], 7);
        let empty = rows.iter().find(|r| r[0] == "empty").unwrap();
        for index in [8, 9, 12, 13] {
            assert!(empty[index].is_null());
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn parquet_scan_recovers_from_gui_descriptor_limit() {
        if !duckdb_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let seed = dir.path().join("seed.parquet");
        let create = format!(
            "COPY (SELECT 1 AS value) TO '{}' (FORMAT PARQUET)",
            seed.display()
        );
        let created = tokio::process::Command::new("duckdb")
            .args(["-c", &create])
            .output()
            .await
            .unwrap();
        assert!(created.status.success());
        for i in 0..160 {
            std::fs::copy(&seed, dir.path().join(format!("part-{i}.parquet"))).unwrap();
        }
        let sql = format!("SELECT CAST(sum(value) AS BIGINT) AS total FROM read_parquet('{}/*.parquet', union_by_name=true)", dir.path().display());
        for repaired in [false, true] {
            let mut cmd = tokio::process::Command::new("duckdb");
            cmd.args(["-json", "-c", &sql]);
            unsafe {
                cmd.pre_exec(|| {
                    let mut limit = libc::rlimit {
                        rlim_cur: 0,
                        rlim_max: 0,
                    };
                    if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    limit.rlim_cur = 64;
                    if libc::setrlimit(libc::RLIMIT_NOFILE, &limit) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            if repaired {
                configure_query_file_limit(&mut cmd);
            }
            let output = cmd.output().await.unwrap();
            if repaired {
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                let rows: Value = serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(rows[0]["total"], 161);
            } else {
                assert!(!output.status.success());
                assert!(String::from_utf8_lossy(&output.stderr).contains("Too many open files"));
            }
        }
    }

    #[test]
    fn validate_range_defaults_when_absent() {
        assert_eq!(validate_range(None, 14, 1, 365, "days").unwrap(), 14);
        assert_eq!(validate_range(None, 50, 1, 500, "limit").unwrap(), 50);
    }

    #[test]
    fn validate_range_accepts_the_boundaries() {
        assert_eq!(validate_range(Some(1), 14, 1, 365, "days").unwrap(), 1);
        assert_eq!(validate_range(Some(365), 14, 1, 365, "days").unwrap(), 365);
        assert_eq!(validate_range(Some(500), 50, 1, 500, "limit").unwrap(), 500);
    }

    #[test]
    fn validate_range_rejects_out_of_range() {
        assert_eq!(
            validate_range(Some(0), 14, 1, 365, "days")
                .unwrap_err()
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            validate_range(Some(366), 14, 1, 365, "days")
                .unwrap_err()
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            validate_range(Some(501), 50, 1, 500, "limit")
                .unwrap_err()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn project_looks_up_by_name_and_fills_missing_with_null() {
        let mut row = Map::new();
        row.insert("b".to_string(), Value::from(2));
        row.insert("a".to_string(), Value::from(1));
        let rows = vec![row];
        // Note the reversed order vs. insertion order: this is exactly the
        // point -- project() must not depend on JSON object key order.
        let out = project(&rows, &["a", "b", "missing"]);
        assert_eq!(out, vec![vec![Value::from(1), Value::from(2), Value::Null]]);
    }

    #[test]
    fn any_parquet_files_false_for_missing_or_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!any_parquet_files(
            dir.path().join("does-not-exist").as_path()
        ));
        assert!(!any_parquet_files(dir.path()));
    }

    #[test]
    fn any_parquet_files_true_once_a_dt_partition_has_one() {
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("dt=2026-08-31");
        std::fs::create_dir_all(&part).unwrap();
        assert!(
            !any_parquet_files(dir.path()),
            "empty dt= dir doesn't count"
        );
        std::fs::write(part.join("a.parquet"), b"x").unwrap();
        assert!(any_parquet_files(dir.path()));

        let stub_only = tempfile::tempdir().unwrap();
        let stub = stub_only
            .path()
            .join(kikimimi_schema::paths::SCHEMA_STUB_PARTITION);
        std::fs::create_dir_all(&stub).unwrap();
        std::fs::write(stub.join("kikimimi.v1-1.parquet"), b"x").unwrap();
        assert!(
            !any_parquet_files(stub_only.path()),
            "the schema stub is not data"
        );
    }

    #[test]
    fn today_minus_days_zero_is_today() {
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        assert_eq!(today_minus_days(0), today);
    }

    #[tokio::test]
    async fn run_duckdb_json_parses_rows_and_preserves_null() {
        if !duckdb_available() {
            eprintln!("skipping: duckdb CLI not installed");
            return;
        }
        let rows = run_duckdb_json("SELECT 1 AS a, NULL AS b, 'x' AS c;")
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get("a"), Some(&Value::from(1)));
        assert_eq!(rows[0].get("b"), Some(&Value::Null));
        assert_eq!(rows[0].get("c"), Some(&Value::from("x")));
    }

    #[tokio::test]
    async fn run_duckdb_json_casts_bigint_sums_to_plain_numbers_not_strings() {
        if !duckdb_available() {
            eprintln!("skipping: duckdb CLI not installed");
            return;
        }
        // Regression pin for the HUGEINT-stringification gotcha documented at
        // the top of this file: without the CAST, this would come back as a
        // JSON string "18000000000000", not a number.
        let rows = run_duckdb_json(
            "SELECT CAST(sum(x) AS BIGINT) AS s \
             FROM (SELECT 9000000000000::BIGINT AS x UNION ALL SELECT 9000000000000::BIGINT) t;",
        )
        .await
        .unwrap();
        assert_eq!(rows[0].get("s"), Some(&Value::from(18_000_000_000_000i64)));
    }

    #[tokio::test]
    async fn run_duckdb_json_surfaces_syntax_errors_as_failed() {
        if !duckdb_available() {
            eprintln!("skipping: duckdb CLI not installed");
            return;
        }
        let err = run_duckdb_json("SELEKT this is not sql;")
            .await
            .unwrap_err();
        assert!(matches!(err, DuckDbError::Failed(_)));
    }

    /// End-to-end: real Parquet on disk (via `kikimimi_sink::FileSink`, the same
    /// writer `kikimimi agent` uses), real `duckdb` CLI, real handler.
    #[tokio::test]
    async fn overview_handler_reads_real_parquet_end_to_end() {
        if !duckdb_available() {
            eprintln!("skipping: duckdb CLI not installed");
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("data").join("events");

        let mut sink = kikimimi_sink::FileSink::new(
            data_dir.clone(),
            "host-web-test".to_string(),
            kikimimi_sink::FileSink::DEFAULT_MAX_ROWS,
            kikimimi_sink::FileSink::DEFAULT_MAX_AGE,
        );
        let now_ms = chrono::Utc::now().timestamp_millis();
        let today = kikimimi_schema::dt_of(now_ms);
        let ev = kikimimi_schema::Event {
            event_id: "ev-1".into(),
            ts: now_ms,
            dt: today.clone(),
            host_id: "host-web-test".into(),
            agent: "claude-code".into(),
            source: "hook".into(),
            event_type: kikimimi_schema::event_type::TOOL_CALL.to_string(),
            tool_name: Some("Bash".into()),
            tool_kind: Some("bash".into()),
            input_tokens: Some(100),
            output_tokens: Some(50),
            cost_usd: Some(0.01),
            ..Default::default()
        };
        kikimimi_sink::EventSink::push(&mut sink, ev);
        kikimimi_sink::EventSink::flush(&mut sink).unwrap();

        let state = WebAppState {
            token: "test-token".to_string(),
            data_dir,
        };
        let resp = overview(State(state), Query(DaysQuery { days: Some(14) })).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["columns"], serde_json::json!(OVERVIEW_COLUMNS));
        let rows = json["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 1, "exactly today's row: {rows:?}");
        let row = rows[0].as_array().unwrap();
        assert_eq!(row[0], Value::from(today));
        assert_eq!(row[1], Value::from(1), "events");
        assert_eq!(row[2], Value::from(1), "tool_calls");
        assert_eq!(
            row[4],
            Value::from(100),
            "input_tokens (not a HUGEINT string)"
        );
        assert_eq!(row[5], Value::from(50), "output_tokens");
    }

    #[tokio::test]
    async fn overview_handler_rejects_out_of_range_days_before_touching_duckdb() {
        // No duckdb-availability guard: this must 400 before ever shelling
        // out, so it works even without duckdb installed.
        let state = WebAppState {
            token: "t".to_string(),
            data_dir: std::env::temp_dir().join("kikimimi-web-test-never-read"),
        };
        let resp = overview(State(state), Query(DaysQuery { days: Some(9999) })).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn models_handler_returns_empty_shape_before_any_parquet_exists() {
        let dir = tempfile::tempdir().unwrap();
        let state = WebAppState {
            token: "t".to_string(),
            data_dir: dir.path().join("data").join("events"),
        };
        let resp = models(State(state), Query(DaysQuery { days: Some(7) })).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["models"]["columns"], serde_json::json!(MODELS_COLUMNS));
        assert_eq!(json["models"]["rows"], serde_json::json!([]));
        assert_eq!(
            json["daily"]["columns"],
            serde_json::json!(MODELS_DAILY_COLUMNS)
        );
        assert_eq!(json["daily"]["rows"], serde_json::json!([]));
        assert_eq!(json["days"], 7);
    }

    /// KKM-34: `/web/q/models` groups by (model, effort); per session OTel
    /// `api.request` rows win over transcript (`log`) ones so a request seen
    /// by both is priced once, a log-only session still counts, `api.error`
    /// rows are counted but never priced, NULL effort is its own group, and
    /// subagent rows (agent_id set) are split out.
    #[tokio::test]
    async fn models_handler_groups_by_model_and_effort_preferring_otel_per_session() {
        if !duckdb_available() {
            eprintln!("skipping: duckdb CLI not installed");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("data").join("events");
        let mut sink = kikimimi_sink::FileSink::new(
            data_dir.clone(),
            "host-m".to_string(),
            kikimimi_sink::FileSink::DEFAULT_MAX_ROWS,
            kikimimi_sink::FileSink::DEFAULT_MAX_AGE,
        );
        let now = chrono::Utc::now().timestamp_millis();
        let today = kikimimi_schema::dt_of(now);
        let ev = |id: &str, session: &str, source: &str, event_type: &str| kikimimi_schema::Event {
            event_id: id.into(),
            ts: now,
            dt: today.clone(),
            host_id: "host-m".into(),
            agent: "claude-code".into(),
            source: source.into(),
            session_id: Some(session.into()),
            event_type: event_type.into(),
            model: Some("claude-fable".into()),
            effort: Some("high".into()),
            ..Default::default()
        };
        let events = vec![
            // Session A: the same request via OTel and transcript -> OTel wins.
            kikimimi_schema::Event {
                input_tokens: Some(1000),
                output_tokens: Some(100),
                cost_usd: Some(0.5),
                cache_read_tokens: Some(0),
                cache_write_tokens: Some(0),
                ..ev("a-otel", "A", "otel", "api.request")
            },
            kikimimi_schema::Event {
                input_tokens: Some(1000),
                output_tokens: Some(100),
                reasoning_tokens: Some(40),
                ..ev("a-log", "A", "log", "api.request")
            },
            // Session A: a Haiku helper call with no effort reported.
            kikimimi_schema::Event {
                model: Some("claude-haiku".into()),
                effort: None,
                input_tokens: Some(10),
                output_tokens: Some(5),
                cost_usd: Some(0.01),
                ..ev("a-haiku", "A", "otel", "api.request")
            },
            // Session A: an error, counted but never priced.
            ev("a-err", "A", "otel", "api.error"),
            // Session A: a (model, effort) pair that only ever errored --
            // still listed (0 requests, NULL usage), like the cloud.
            kikimimi_schema::Event {
                model: Some("claude-opus".into()),
                effort: Some("xhigh".into()),
                ..ev("a-err-only", "A", "otel", "api.error")
            },
            // Session B: transcript only -> log rows count.
            kikimimi_schema::Event {
                input_tokens: Some(300),
                output_tokens: Some(30),
                reasoning_tokens: Some(7),
                ..ev("b-log", "B", "log", "api.request")
            },
            kikimimi_schema::Event {
                agent_id: Some("ag1".into()),
                agent_type: Some("Explore".into()),
                input_tokens: Some(200),
                output_tokens: Some(20),
                ..ev("b-sub", "B", "log", "api.request")
            },
        ];
        for e in events {
            kikimimi_sink::EventSink::push(&mut sink, e);
        }
        kikimimi_sink::EventSink::flush(&mut sink).unwrap();

        let state = WebAppState {
            token: "t".to_string(),
            data_dir,
        };
        let resp = models(State(state), Query(DaysQuery { days: None })).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["days"], 14);
        let col = |section: &str, name: &str| -> usize {
            json[section]["columns"]
                .as_array()
                .unwrap()
                .iter()
                .position(|c| c == name)
                .unwrap_or_else(|| panic!("{section} has no column {name}"))
        };
        assert_eq!(json["models"]["columns"], serde_json::json!(MODELS_COLUMNS));
        let rows = json["models"]["rows"].as_array().unwrap();
        let cost_row = rows.iter().find(|r| r[col("models", "model")] == "claude-fable").unwrap();
        assert_eq!(cost_row[col("models", "reported_cost_requests")], 1);
        assert_eq!(cost_row[col("models", "complete_usage_requests")], 1);
        assert_eq!(cost_row[col("models", "complete_input_tokens")], 1000);
        assert_eq!(cost_row[col("models", "complete_cache_write_tokens")], 0);
        assert_eq!(rows.len(), 3, "{rows:?}");

        // Last: the error-only pair, present with zero requests and no usage.
        let opus = &rows[2];
        assert_eq!(opus[col("models", "model")], "claude-opus");
        assert_eq!(opus[col("models", "effort")], "xhigh");
        assert_eq!(opus[col("models", "api_requests")], 0);
        assert_eq!(opus[col("models", "api_errors")], 1);
        assert_eq!(opus[col("models", "sessions")], 0);
        assert_eq!(opus[col("models", "input_tokens")], Value::Null);

        // Biggest first: claude-fable/high = A's OTel row + B's two log rows.
        let fable = &rows[0];
        assert_eq!(fable[col("models", "model")], "claude-fable");
        assert_eq!(fable[col("models", "effort")], "high");
        assert_eq!(
            fable[col("models", "api_requests")],
            3,
            "not 4: A's log row is dropped"
        );
        assert_eq!(fable[col("models", "api_errors")], 1);
        assert_eq!(fable[col("models", "sessions")], 2);
        assert_eq!(fable[col("models", "subagent_api_requests")], 1);
        assert_eq!(fable[col("models", "subagent_tokens")], 220);
        assert_eq!(fable[col("models", "input_tokens")], 1500);
        assert_eq!(fable[col("models", "output_tokens")], 150);
        assert_eq!(fable[col("models", "cache_read_tokens")], 0);
        assert_eq!(
            fable[col("models", "reasoning_tokens")],
            7,
            "only B's log row has it"
        );
        assert_eq!(fable[col("models", "cost_usd")], 0.5);

        let haiku = &rows[1];
        assert_eq!(haiku[col("models", "model")], "claude-haiku");
        assert_eq!(haiku[col("models", "effort")], Value::Null);
        assert_eq!(haiku[col("models", "api_requests")], 1);
        assert_eq!(haiku[col("models", "api_errors")], 0);
        assert_eq!(haiku[col("models", "subagent_api_requests")], 0);
        assert_eq!(haiku[col("models", "subagent_tokens")], Value::Null);
        assert_eq!(haiku[col("models", "input_tokens")], 10);

        assert_eq!(
            json["daily"]["columns"],
            serde_json::json!(MODELS_DAILY_COLUMNS)
        );
        let daily = json["daily"]["rows"].as_array().unwrap();
        assert_eq!(daily.len(), 2, "{daily:?}");
        assert_eq!(daily[0][col("daily", "dt")], Value::from(today.clone()));
        assert_eq!(daily[0][col("daily", "model")], "claude-fable");
        assert_eq!(daily[0][col("daily", "input_tokens")], 1500);
        assert_eq!(daily[0][col("daily", "cost_usd")], 0.5);
        assert_eq!(daily[1][col("daily", "model")], "claude-haiku");
        assert_eq!(daily[1][col("daily", "output_tokens")], 5);
    }

    /// End-to-end (real Parquet via `FileSink`, real `duckdb`, real `tools`
    /// handler): a hook row and an OTel row for the same `(session_id,
    /// correlation_key)`, both `success = false`, must report `failures: 1`,
    /// not 2 -- module doc's `tool.result` dedup, exercised through the
    /// actual production handler (not just the rendered SQL text).
    #[tokio::test]
    async fn tools_handler_dedups_a_hook_otel_tool_result_pair_end_to_end() {
        if !duckdb_available() {
            eprintln!("skipping: duckdb CLI not installed");
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("data").join("events");
        let mut sink = kikimimi_sink::FileSink::new(
            data_dir.clone(),
            "host-web-test".to_string(),
            kikimimi_sink::FileSink::DEFAULT_MAX_ROWS,
            kikimimi_sink::FileSink::DEFAULT_MAX_AGE,
        );
        let now_ms = chrono::Utc::now().timestamp_millis();
        let today = kikimimi_schema::dt_of(now_ms);
        let base = kikimimi_schema::Event {
            ts: now_ms,
            dt: today.clone(),
            host_id: "host-web-test".into(),
            agent: "claude-code".into(),
            session_id: Some("sess-1".into()),
            tool_name: Some("mcp__gh__search".into()),
            tool_kind: Some("mcp".into()),
            correlation_key: Some("tu-1".into()),
            ..Default::default()
        };
        let call = kikimimi_schema::Event {
            event_id: "ev-call".into(),
            source: "hook".into(),
            event_type: kikimimi_schema::event_type::TOOL_CALL.to_string(),
            ..base.clone()
        };
        let res_hook = kikimimi_schema::Event {
            event_id: "ev-hook".into(),
            source: "hook".into(),
            event_type: kikimimi_schema::event_type::TOOL_RESULT.to_string(),
            success: Some(false),
            ..base.clone()
        };
        let res_otel = kikimimi_schema::Event {
            event_id: "ev-otel".into(),
            source: "otel".into(),
            event_type: kikimimi_schema::event_type::TOOL_RESULT.to_string(),
            success: Some(false),
            duration_ms: Some(250),
            ..base
        };
        for ev in [call, res_hook, res_otel] {
            kikimimi_sink::EventSink::push(&mut sink, ev);
        }
        kikimimi_sink::EventSink::flush(&mut sink).unwrap();

        let state = WebAppState {
            token: "test-token".to_string(),
            data_dir,
        };
        let resp = tools(State(state), Query(DaysQuery { days: Some(14) })).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        let rows = json["rows"].as_array().unwrap();
        let row = rows
            .iter()
            .find(|r| r[0] == "mcp__gh__search")
            .unwrap_or_else(|| panic!("no mcp__gh__search row: {rows:?}"));
        assert_eq!(row[2], Value::from(1), "calls: {row:?}");
        assert_eq!(row[3], Value::from(1), "failures (deduped): {row:?}");
        assert_eq!(
            row[4],
            Value::from(250),
            "p50_duration_ms from the winning OTel row: {row:?}"
        );
    }

    /// `configured` (from `~/.claude*` files) but never called must show up
    /// with `calls: 0` — the whole point of the query, architecture.md
    /// §7.1/§7.2. `configured_from_snapshot` is always `true` locally (the
    /// daemon reads the real, current config files, never a proxy).
    #[tokio::test]
    #[serial_test::serial]
    async fn unused_mcp_handler_reports_configured_but_never_called_server() {
        if !duckdb_available() {
            eprintln!("skipping: duckdb CLI not installed");
            return;
        }

        let settings_dir = tempfile::tempdir().unwrap();
        std::env::set_var(
            "KIKIMIMI_CLAUDE_SETTINGS_PATH",
            settings_dir.path().join("settings.json"),
        );
        std::env::set_var(
            "KIKIMIMI_CLAUDE_JSON_PATH",
            settings_dir.path().join("no-claude.json"),
        );
        std::fs::write(
            settings_dir.path().join("settings.json"),
            serde_json::json!({"mcpServers": {"github": {}, "notion": {}}}).to_string(),
        )
        .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("data").join("events");
        let mut sink = kikimimi_sink::FileSink::new(
            data_dir.clone(),
            "host-web-test".to_string(),
            kikimimi_sink::FileSink::DEFAULT_MAX_ROWS,
            kikimimi_sink::FileSink::DEFAULT_MAX_AGE,
        );
        let now_ms = chrono::Utc::now().timestamp_millis();
        let today = kikimimi_schema::dt_of(now_ms);
        let ev = kikimimi_schema::Event {
            event_id: "ev-github-call".into(),
            ts: now_ms,
            dt: today,
            host_id: "host-web-test".into(),
            agent: "claude-code".into(),
            source: "hook".into(),
            session_id: Some("sess-1".into()),
            event_type: kikimimi_schema::event_type::TOOL_CALL.to_string(),
            tool_name: Some("mcp__github__search".into()),
            tool_kind: Some("mcp".into()),
            mcp_server: Some("github".into()),
            ..Default::default()
        };
        kikimimi_sink::EventSink::push(&mut sink, ev);
        kikimimi_sink::EventSink::flush(&mut sink).unwrap();

        let state = WebAppState {
            token: "test-token".to_string(),
            data_dir,
        };
        let resp = unused_mcp(State(state), Query(DaysQuery { days: Some(14) })).await;

        std::env::remove_var("KIKIMIMI_CLAUDE_SETTINGS_PATH");
        std::env::remove_var("KIKIMIMI_CLAUDE_JSON_PATH");

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["columns"], serde_json::json!(UNUSED_MCP_COLUMNS));
        let rows = json["rows"].as_array().unwrap();

        let notion = rows
            .iter()
            .find(|r| r[0] == "notion")
            .unwrap_or_else(|| panic!("no notion row: {rows:?}"));
        assert_eq!(
            notion[1],
            Value::from(true),
            "notion configured: {notion:?}"
        );
        assert_eq!(notion[2], Value::from(0), "notion calls: {notion:?}");
        assert_eq!(
            notion[6],
            Value::from(true),
            "configured_from_snapshot: {notion:?}"
        );

        let github = rows
            .iter()
            .find(|r| r[0] == "github")
            .unwrap_or_else(|| panic!("no github row: {rows:?}"));
        assert_eq!(
            github[1],
            Value::from(true),
            "github configured: {github:?}"
        );
        assert_eq!(github[2], Value::from(1), "github calls: {github:?}");

        // "notion" (never called) must sort before "github" (called once).
        let notion_idx = rows.iter().position(|r| r[0] == "notion").unwrap();
        let github_idx = rows.iter().position(|r| r[0] == "github").unwrap();
        assert!(
            notion_idx < github_idx,
            "unused-but-configured must sort first: {rows:?}"
        );
    }

    /// Before the daemon has flushed any Parquet at all, `unused_mcp` still
    /// reports every configured server (calls 0) instead of erroring —
    /// `any_parquet_files`'s doc comment explains why DuckDB itself can't be
    /// asked directly in this case.
    #[tokio::test]
    #[serial_test::serial]
    async fn unused_mcp_handler_reports_configured_servers_before_any_parquet_exists() {
        let settings_dir = tempfile::tempdir().unwrap();
        std::env::set_var(
            "KIKIMIMI_CLAUDE_SETTINGS_PATH",
            settings_dir.path().join("settings.json"),
        );
        std::env::set_var(
            "KIKIMIMI_CLAUDE_JSON_PATH",
            settings_dir.path().join("no-claude.json"),
        );
        std::fs::write(
            settings_dir.path().join("settings.json"),
            serde_json::json!({"mcpServers": {"playwright": {}}}).to_string(),
        )
        .unwrap();

        let state = WebAppState {
            token: "t".to_string(),
            data_dir: tempfile::tempdir().unwrap().path().to_path_buf(),
        };
        let resp = unused_mcp(State(state), Query(DaysQuery { days: Some(14) })).await;

        std::env::remove_var("KIKIMIMI_CLAUDE_SETTINGS_PATH");
        std::env::remove_var("KIKIMIMI_CLAUDE_JSON_PATH");

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            json["rows"],
            serde_json::json!([["playwright", true, 0, 0, null, 0, true]])
        );
    }

    #[test]
    fn timeline_bucket_ms_snaps_to_human_steps() {
        assert_eq!(timeline_bucket_ms(0), 60_000);
        assert_eq!(timeline_bucket_ms(5 * 60_000), 60_000);
        assert_eq!(
            timeline_bucket_ms(10 * 3_600_000),
            5 * 60_000,
            "10h / 240 = 2.5min -> 5min"
        );
        assert_eq!(
            timeline_bucket_ms(30 * 86_400_000),
            6 * 3_600_000,
            "30d / 240 = 3h -> 6h"
        );
        assert_eq!(
            timeline_bucket_ms(2 * 365 * 86_400_000),
            24 * 3_600_000,
            "capped at a day"
        );
    }

    /// End-to-end (real Parquet via `FileSink`, real `duckdb`, real
    /// `session_detail` handler): the same fixture the cloud's
    /// `web_q_session_drills_into_one_session` uses, so both ports agree.
    #[tokio::test]
    async fn session_detail_handler_reads_real_parquet_end_to_end() {
        if !duckdb_available() {
            eprintln!("skipping: duckdb CLI not installed");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("data").join("events");
        let mut sink = kikimimi_sink::FileSink::new(
            data_dir.clone(),
            "host-sd".to_string(),
            kikimimi_sink::FileSink::DEFAULT_MAX_ROWS,
            kikimimi_sink::FileSink::DEFAULT_MAX_AGE,
        );
        // Minute-aligned so the events 0..9 s after t0 stay in one bucket.
        let t0 = (chrono::Utc::now().timestamp_millis() - 10 * 60_000) / 60_000 * 60_000;
        let ev = |id: &str, ts: i64, event_type: &str| kikimimi_schema::Event {
            event_id: id.into(),
            ts,
            dt: kikimimi_schema::dt_of(ts),
            host_id: "host-sd".into(),
            agent: "claude-code".into(),
            source: "hook".into(),
            session_id: Some("sess-detail".into()),
            event_type: event_type.into(),
            ..Default::default()
        };
        let events = vec![
            kikimimi_schema::Event {
                agent_version: Some("2.1.0".into()),
                configured_mcp_servers: Some(r#"["github"]"#.into()),
                estimated_input_tokens: Some(3),
                estimated_output_tokens: Some(7),
                ..ev("sd-start", t0, "session.start")
            },
            kikimimi_schema::Event {
                tool_name: Some("Bash".into()),
                tool_kind: Some("bash".into()),
                correlation_key: Some("tu1".into()),
                ..ev("sd-call", t0 + 1_000, "tool.call")
            },
            kikimimi_schema::Event {
                tool_name: Some("Bash".into()),
                tool_kind: Some("bash".into()),
                correlation_key: Some("tu1".into()),
                success: Some(false),
                duration_ms: Some(100),
                ..ev("sd-res-hook", t0 + 2_000, "tool.result")
            },
            kikimimi_schema::Event {
                source: "otel".into(),
                tool_name: Some("Bash".into()),
                tool_kind: Some("bash".into()),
                correlation_key: Some("tu1".into()),
                success: Some(false),
                duration_ms: Some(120),
                ..ev("sd-res-otel", t0 + 2_100, "tool.result")
            },
            kikimimi_schema::Event {
                source: "otel".into(),
                model: Some("claude-sonnet".into()),
                input_tokens: Some(1000),
                output_tokens: Some(200),
                cache_read_tokens: Some(30),
                cache_write_tokens: Some(0),
                cost_usd: Some(0.05),
                effort: Some("high".into()),
                ..ev("sd-api", t0 + 3_000, "api.request")
            },
            kikimimi_schema::Event {
                agent_id: Some("ag1".into()),
                agent_type: Some("Explore".into()),
                tool_name: Some("Read".into()),
                tool_kind: Some("builtin".into()),
                correlation_key: Some("tu2".into()),
                ..ev("sd-sub-call", t0 + 4_000, "tool.call")
            },
            // What OTel emits for a subagent's request: agent.name but no
            // agent_id, inside ag1's window -> attributed to ag1 by type + time.
            kikimimi_schema::Event {
                source: "otel".into(),
                agent_type: Some("Explore".into()),
                model: Some("claude-sonnet".into()),
                effort: Some("medium".into()),
                ..ev("sd-sub-api", t0 + 6_000, "api.request")
            },
            kikimimi_schema::Event {
                agent_id: Some("ag1".into()),
                agent_type: Some("Explore".into()),
                duration_ms: Some(5_000),
                ..ev("sd-sub-stop", t0 + 9_000, "subagent.stop")
            },
            ev("sd-end", t0 + 5 * 60_000, "session.end"),
        ];
        for e in events {
            kikimimi_sink::EventSink::push(&mut sink, e);
        }
        kikimimi_sink::EventSink::flush(&mut sink).unwrap();

        let state = WebAppState {
            token: "t".to_string(),
            data_dir,
        };
        let resp = session_detail(
            State(state.clone()),
            Query(SessionDetailQuery {
                session_id: "sess-detail".into(),
                events_limit: None,
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();

        let col = |section: &str, name: &str| -> usize {
            json[section]["columns"]
                .as_array()
                .unwrap()
                .iter()
                .position(|c| c == name)
                .unwrap_or_else(|| panic!("{section} has no column {name}"))
        };
        assert_eq!(
            json["summary"]["columns"],
            serde_json::json!(SESSION_SUMMARY_COLUMNS)
        );
        let s = &json["summary"]["rows"][0];
        assert_eq!(s[col("summary", "session_id")], "sess-detail");
        assert_eq!(s[col("summary", "agent_version")], "2.1.0");
        assert_eq!(s[col("summary", "duration_ms")], 5 * 60_000);
        assert_eq!(s[col("summary", "ended")], true);
        assert_eq!(s[col("summary", "events")], 9);
        assert_eq!(s[col("summary", "tool_calls")], 2);
        assert_eq!(s[col("summary", "failures")], 1, "hook/OTel pair deduped");
        assert_eq!(s[col("summary", "subagents")], 1);
        assert_eq!(
            s[col("summary", "input_tokens")],
            1000,
            "plain number, not HUGEINT string"
        );
        assert_eq!(s[col("summary", "estimated_input_tokens")], 3);
        assert_eq!(s[col("summary", "estimated_output_tokens")], 7);
        assert_eq!(s[col("summary", "output_tokens")], 200);
        assert_eq!(s[col("summary", "configured_mcp_servers")], r#"["github"]"#);
        assert_eq!(json["bucket_ms"], 60_000);

        let tools = json["tools"]["rows"].as_array().unwrap();
        assert_eq!(tools.len(), 2, "{tools:?}");
        assert_eq!(tools[0][col("tools", "tool_name")], "Bash");
        assert_eq!(tools[0][col("tools", "failures")], 1);
        assert_eq!(tools[0][col("tools", "p50_duration_ms")], 120.0);
        assert_eq!(tools[0][col("tools", "total_duration_ms")], 120);
        assert_eq!(tools[1][col("tools", "subagent_calls")], 1);

        let subs = json["subagents"]["rows"].as_array().unwrap();
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0][col("subagents", "agent_type")], "Explore");
        assert_eq!(subs[0][col("subagents", "duration_ms")], 5_000);
        assert_eq!(subs[0][col("subagents", "tokens_est")], Value::Null);
        assert_eq!(subs[0][col("subagents", "tools")], "Read");

        let timeline = json["timeline"]["rows"].as_array().unwrap();
        assert_eq!(timeline.len(), 2, "{timeline:?}");
        assert_eq!(timeline[0][col("timeline", "events")], 7);
        assert_eq!(timeline[0][col("timeline", "tokens")], 1200);
        assert_eq!(timeline[0][col("timeline", "subagent_events")], 2);

        let list = json["events"]["rows"].as_array().unwrap();
        assert_eq!(list.len(), 8, "deduped pair listed once");
        assert_eq!(list[0][col("events", "event_type")], "session.start");

        // KKM-34: model / effort surfaced everywhere.
        assert_eq!(s[col("summary", "efforts")], "high,medium");
        assert_eq!(subs[0][col("subagents", "models")], "claude-sonnet");
        assert_eq!(subs[0][col("subagents", "efforts")], "medium");
        assert_eq!(subs[0][col("subagents", "model_source")], "otel_window");
        let api_row = list
            .iter()
            .find(|r| r[col("events", "event_type")] == "api.request")
            .unwrap();
        assert_eq!(api_row[col("events", "effort")], "high");
        assert_eq!(
            json["models"]["columns"],
            serde_json::json!(SESSION_MODELS_COLUMNS)
        );
        let models = json["models"]["rows"].as_array().unwrap();
        assert_eq!(models.len(), 2, "{models:?}");
        assert_eq!(models[0][col("models", "model")], "claude-sonnet");
        assert_eq!(models[0][col("models", "effort")], "high");
        assert_eq!(models[0][col("models", "api_requests")], 1);
        assert_eq!(models[0][col("models", "api_errors")], 0);
        assert_eq!(models[0][col("models", "subagent_api_requests")], 0);
        assert_eq!(models[0][col("models", "input_tokens")], 1000);
        assert_eq!(models[0][col("models", "output_tokens")], 200);
        assert_eq!(models[0][col("models", "complete_usage_requests")], 1);
        assert_eq!(models[0][col("models", "complete_cache_read_tokens")], 30);
        assert_eq!(models[0][col("models", "reasoning_tokens")], Value::Null);
        // The OTel subagent request: its own (model, effort) row, counted as
        // a subagent's by agent_type, no usage -> NULL, sorted last.
        assert_eq!(models[1][col("models", "model")], "claude-sonnet");
        assert_eq!(models[1][col("models", "effort")], "medium");
        assert_eq!(models[1][col("models", "api_requests")], 1);
        assert_eq!(models[1][col("models", "subagent_api_requests")], 1);
        assert_eq!(models[1][col("models", "input_tokens")], Value::Null);
        assert_eq!(models[1][col("models", "complete_usage_requests")], 0);

        // Unknown id -> 404, never an empty 200.
        let resp = session_detail(
            State(state),
            Query(SessionDetailQuery {
                session_id: "nope".into(),
                events_limit: None,
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}

/// Filter choices come from all currently visible records, independent of selection.
pub async fn filter_options(State(state): State<WebAppState>) -> Response {
    let this_host = kikimimi_schema::paths::host_id().unwrap_or_default();
    if !any_parquet_files(&state.data_dir) {
        return Json(serde_json::json!({"this_host_id":this_host,"devices":[],"repositories":[]})).into_response();
    }
    let glob = kikimimi_schema::paths::events_glob_sql_in(&state.data_dir);
    let source = kikimimi_sink::store::resolved_source(&format!("read_parquet('{glob}', union_by_name=true, hive_partitioning=false)"));
    let sql = format!("SELECT DISTINCT host_id, repo FROM {source} ORDER BY host_id, repo");
    match run_duckdb_json(&sql).await {
        Ok(rows) => {
            let mut devices = std::collections::BTreeSet::new();
            let mut repositories = std::collections::BTreeSet::new();
            for row in rows {
                if let Some(host) = row.get("host_id").and_then(Value::as_str) { if !host.is_empty() { devices.insert(host.to_string()); } }
                if let Some(repo) = row.get("repo").and_then(Value::as_str) { if !repo.is_empty() { repositories.insert(repo.to_string()); } }
            }
            Json(serde_json::json!({"this_host_id":this_host,"devices":devices,"repositories":repositories})).into_response()
        }
        Err(error) => respond(&[], Err(error)),
    }
}

#[cfg(test)]
mod improvement_tests {
    use super::*;
    use axum::{body::{to_bytes, Body}, http::Request};
    use tower::ServiceExt;

    async fn call(state: &WebAppState, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
        let response = crate::web::query_router(false).with_state(state.clone()).oneshot(
            Request::builder().method(method).uri(uri).header("content-type", "application/json")
                .body(Body::from(body.to_string())).unwrap()).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }
    fn state(root: &Path) -> WebAppState {
        WebAppState { token: "test".into(), data_dir: root.join("data/events") }
    }
    fn change() -> Value {
        serde_json::json!({"pattern_id":"retry_spiral", "subject":"Read", "marked_dt":"2026-01-01", "note":"Renewed credentials"})
    }
    #[tokio::test]
    async fn change_records_survive_reopen_and_are_scoped_to_display_filters() {
        let root = tempfile::tempdir().unwrap();
        let s = state(root.path());
        let (status, mark) = call(&s, "POST", "/web/marks?repository=owner%2Fone", change()).await;
        assert_eq!(status, StatusCode::OK, "{mark}");
        let reopened = state(root.path());
        let (_, list) = call(&reopened, "GET", "/web/marks?repository=owner%2Fone", Value::Null).await;
        assert_eq!(list["marks"].as_array().unwrap().len(), 1);
        assert_eq!(list["marks"][0]["note"], "Renewed credentials");
        for uri in ["/web/marks", "/web/marks?repository=owner%2Ftwo"] {
            let (_, list) = call(&reopened, "GET", uri, Value::Null).await;
            assert_eq!(list["marks"].as_array().unwrap().len(), 0);
        }
        let id = mark["id"].as_str().unwrap();
        assert_eq!(call(&s, "DELETE", &format!("/web/marks/{id}?repository=owner%2Ftwo"), Value::Null).await.0, StatusCode::NOT_FOUND);
        assert_eq!(call(&s, "DELETE", &format!("/web/marks/{id}?repository=owner%2Fone"), Value::Null).await.0, StatusCode::OK);
    }
    #[tokio::test]
    async fn invalid_changes_and_unreadable_records_never_destroy_history() {
        let root = tempfile::tempdir().unwrap();
        let s = state(root.path());
        let mut blank = change(); blank["note"] = Value::String("  ".into());
        assert_eq!(call(&s, "POST", "/web/marks", blank).await.0, StatusCode::BAD_REQUEST);
        let mut future = change(); future["marked_dt"] = Value::String("2999-01-01".into());
        assert_eq!(call(&s, "POST", "/web/marks", future).await.0, StatusCode::BAD_REQUEST);
        let path = marks_path(&s);
        std::fs::write(&path, b"corrupted record store").unwrap();
        assert_eq!(call(&s, "GET", "/web/marks", Value::Null).await.0, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(call(&s, "POST", "/web/marks", change()).await.0, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(std::fs::read(path).unwrap(), b"corrupted record store");
    }
    #[tokio::test]
    async fn recurrence_excludes_unrelated_work_and_respects_repository_filter() {
        let root = tempfile::tempdir().unwrap();
        let s = state(root.path());
        std::fs::create_dir_all(&s.data_dir).unwrap();
        let store = kikimimi_sink::store::shared(&s.data_dir).unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        for (session, tool, repo, success, count) in [
            ("failing", "Read", "owner/one", false, 3),
            ("healthy", "Read", "owner/one", true, 1),
            ("unrelated", "Write", "owner/one", true, 1),
            ("other-repo", "Read", "owner/two", false, 3),
        ] {
            for index in 0..count {
                let event = kikimimi_schema::Event {
                    event_id: format!("{session}-{index}"), ts: now + index,
                    dt: kikimimi_schema::dt_of(now), host_id: "fixture".into(),
                    repo: Some(repo.into()), session_id: Some(session.into()),
                    event_type: "tool.result".into(), source: "otel".into(), agent: "claude-code".into(),
                    tool_name: Some(tool.into()), success: Some(success),
                    correlation_key: Some(format!("{session}-{index}")), ..Default::default()
                };
                store.lock().unwrap().ingest(&event, None).unwrap();
            }
        }
        let (_, timeline) = call(&s, "GET", "/web/q/pattern-timeline?pattern_id=retry_spiral&subject=Read&repository=owner%2Fone", Value::Null).await;
        assert_eq!(timeline["rows"][0][1], 2, "{timeline}"); // excludes Write and other repo
        assert_eq!(timeline["rows"][0][2], 1, "{timeline}");
        assert_eq!(timeline["rows"][0][3], 50.0, "{timeline}");
        let (status, hits) = call(&s, "GET", "/web/q/pattern-hits?pattern_id=retry_spiral&subject=Read&repository=owner%2Fone", Value::Null).await;
        assert_eq!(status, StatusCode::OK, "{hits}");
        assert_eq!(hits["rows"].as_array().unwrap().len(), 1, "{hits}");
        assert_eq!(hits["rows"][0][1], "failing");
        let (status, candidates) = call(&s, "GET", "/web/q/patterns?repository=owner%2Fone", Value::Null).await;
        assert_eq!(status, StatusCode::OK, "{candidates}");
        assert_eq!(candidates["rows"][0][2], 1, "{candidates}");
    }
}
