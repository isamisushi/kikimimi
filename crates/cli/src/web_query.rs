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

pub async fn overview(State(state): State<WebAppState>, Query(q): Query<DaysQuery>) -> Response {
    let days = match validate_range(q.days, 14, 1, 365, "days") {
        Ok(d) => d,
        Err(r) => return r,
    };
    if !any_parquet_files(&state.data_dir) {
        return query_result_response(OVERVIEW_COLUMNS, vec![]);
    }
    let glob = kikimimi_schema::paths::events_glob_sql_in(&state.data_dir);
    let from_dt = today_minus_days(days.saturating_sub(1));
    let sql = format!(
        "WITH e AS ( \
           SELECT * FROM read_parquet('{glob}', union_by_name=true, hive_partitioning=false) \
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
    // events_30d is a fixed trailing 30-day window, independent of any caller
    // param -- machines has none (contract: `GET /web/q/machines` takes no
    // query params).
    let from_30d = today_minus_days(29);
    let sql = format!(
        "SELECT host_id, \
           max(env_kind) AS env_kind, \
           max(os) AS os, \
           strftime(to_timestamp(max(ts) / 1000.0), '%Y-%m-%dT%H:%M:%SZ') AS last_event_ts, \
           count(*) FILTER (WHERE dt >= '{from_30d}') AS events_30d \
         FROM read_parquet('{glob}', union_by_name=true, hive_partitioning=false) \
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
    let from_dt = today_minus_days(days.saturating_sub(1));
    let sql = format!(
        "WITH e AS ( \
           SELECT * FROM read_parquet('{glob}', union_by_name=true, hive_partitioning=false) \
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
    let from_dt = today_minus_days(days.saturating_sub(1));
    let sql = format!(
        "WITH e AS ( \
           SELECT * FROM read_parquet('{glob}', union_by_name=true, hive_partitioning=false) \
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
    let from_dt = today_minus_days(days.saturating_sub(1));
    let sql = format!(
        "WITH e AS ( \
           SELECT * FROM read_parquet('{glob}', union_by_name=true, hive_partitioning=false) \
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
    let from_dt = today_minus_days(days.saturating_sub(1));
    let sql = format!(
        "WITH observed AS ( \
           SELECT mcp_server, \
             count(*) AS calls, \
             count(DISTINCT session_id) AS distinct_sessions, \
             max(dt) AS last_called_dt \
           FROM read_parquet('{glob}', union_by_name=true, hive_partitioning=false) \
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
    let from_dt = today_minus_days(days.saturating_sub(1));
    let sql = format!(
        "WITH e AS (\
           SELECT * FROM read_parquet('{glob}', union_by_name=true, hive_partitioning=false) \
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
           strftime(to_timestamp(min(e.ts) / 1000.0), '%Y-%m-%dT%H:%M:%SZ') AS started_at, \
           count(*) AS events, \
           count(*) FILTER (WHERE e.event_type = 'tool.call') AS tool_calls, \
           coalesce(max(fails.failures), 0) AS failures, \
           coalesce(string_agg(DISTINCT e.model, ','), '') AS models, \
           CAST(sum(e.input_tokens) AS BIGINT) AS input_tokens, \
           CAST(sum(e.output_tokens) AS BIGINT) AS output_tokens, \
           sum(e.cost_usd) AS cost_usd \
         FROM e \
         LEFT JOIN fails ON fails.session_id = e.session_id \
         GROUP BY e.session_id \
         ORDER BY min(e.ts) DESC \
         LIMIT {limit};"
    );
    respond(SESSIONS_COLUMNS, run_duckdb_json(&sql).await)
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
    let Ok(entries) = std::fs::read_dir(data_dir) else {
        return false;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if !path.is_dir() {
            continue;
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
async fn run_duckdb_json(sql: &str) -> Result<Vec<Map<String, Value>>, DuckDbError> {
    let mut cmd = tokio::process::Command::new("duckdb");
    cmd.args(["-json", "-c", sql])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

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
}
