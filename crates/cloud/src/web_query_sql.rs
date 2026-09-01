//! Postgres ports of the DuckDB SQL in `crates/cli/src/web_query.rs`
//! (`/web/q/*`, contract: `web/src/api/types.ts`, reference impl:
//! `web/mock/server.mjs`) — same five queries, scoped instead to `events`
//! under RLS (like `query_sql.rs`'s `/v1/query/*` ports: the query text has
//! no `org_id` filter at all, Postgres adds it via the `events` row-security
//! policy). Column names/order match the contract exactly — `web_query.rs`
//! reads them straight off the prepared statement (`query.rs`'s "columns
//! come from `prepare`, not from a row" trick), so a mismatched `AS` alias
//! here would show up directly as a wrong `columns` entry in a test, not a
//! silent bug.
//!
//! Every non-TEXT/BOOL output is cast to INT8 or FLOAT8, same reasoning as
//! `query_sql.rs`: `sum(bigint)` and `percentile_cont` over an integer
//! column both default to NUMERIC in Postgres otherwise, which the generic
//! `pg_value_to_json` decoder (`query.rs`) doesn't have an arm for.
//!
//! `$1` is always the `dt >=` lower bound (`days` turned into a `YYYY-MM-DD`
//! string by `web_query.rs`'s `today_minus_days`) except `machines` (no
//! `days` param at all — task spec: "machines has no days param — use 30d
//! window for events_30d and no filter for last_event_ts", so its `$1` is a
//! *fixed* 30-day-ago cutoff used only inside the `events_30d` FILTER, never
//! as a `WHERE`) and `sessions`, whose `$2` is `LIMIT`.

/// `/web/q/overview?days=N` → `[dt, events, tool_calls, failures,
/// input_tokens, output_tokens, cost_usd]`.
pub const OVERVIEW_SQL: &str = r#"
SELECT
    dt,
    count(*)::int8                                                  AS events,
    count(*) FILTER (WHERE event_type = 'tool.call')::int8          AS tool_calls,
    count(*) FILTER (WHERE success = false)::int8                   AS failures,
    sum(input_tokens)::int8                                         AS input_tokens,
    sum(output_tokens)::int8                                        AS output_tokens,
    sum(cost_usd)::float8                                           AS cost_usd
FROM events
WHERE dt >= $1
GROUP BY dt
ORDER BY dt
"#;

/// `/web/q/machines` → `[host_id, env_kind, os, last_event_ts, events_30d]`.
/// `$1` is a trailing-30-day cutoff (`YYYY-MM-DD`), used only inside the
/// `events_30d` FILTER — every other column is computed over *all* of a
/// host's events, unfiltered by any `days`/date param (task spec, module
/// docs above).
pub const MACHINES_SQL: &str = r#"
SELECT
    host_id,
    max(env_kind)                                                   AS env_kind,
    max(os)                                                         AS os,
    to_char(to_timestamp(max(ts) / 1000.0) AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS last_event_ts,
    count(*) FILTER (WHERE dt >= $1)::int8                          AS events_30d
FROM events
GROUP BY host_id
ORDER BY max(ts) DESC NULLS LAST
"#;

/// `/web/q/tools?days=N` → `[tool_name, tool_kind, calls, failures,
/// p50_duration_ms, p95_duration_ms]`. `percentile_cont` (exact), not
/// DuckDB's `approx_quantile` — Postgres has no built-in approximate
/// quantile, and dataset sizes here don't need one.
pub const TOOLS_SQL: &str = r#"
SELECT
    tool_name,
    max(tool_kind)                                                  AS tool_kind,
    count(*) FILTER (WHERE event_type = 'tool.call')::int8                       AS calls,
    count(*) FILTER (WHERE event_type = 'tool.result' AND success = false)::int8 AS failures,
    percentile_cont(0.5)  WITHIN GROUP (ORDER BY duration_ms::float8)
        FILTER (WHERE event_type = 'tool.result')::float8                        AS p50_duration_ms,
    percentile_cont(0.95) WITHIN GROUP (ORDER BY duration_ms::float8)
        FILTER (WHERE event_type = 'tool.result')::float8                        AS p95_duration_ms
FROM events
WHERE tool_name IS NOT NULL AND dt >= $1
GROUP BY tool_name
ORDER BY calls DESC
"#;

/// `/web/q/mcp?days=N` → `[mcp_server, calls, failures, distinct_sessions,
/// last_called_dt]`.
pub const MCP_SQL: &str = r#"
SELECT
    mcp_server,
    count(*) FILTER (WHERE event_type = 'tool.call')::int8                       AS calls,
    count(*) FILTER (WHERE event_type = 'tool.result' AND success = false)::int8 AS failures,
    count(DISTINCT session_id)::int8                                             AS distinct_sessions,
    max(dt) FILTER (WHERE event_type = 'tool.call')                              AS last_called_dt
FROM events
WHERE mcp_server IS NOT NULL AND dt >= $1
GROUP BY mcp_server
ORDER BY calls DESC
"#;

/// `/web/q/skills?days=N` → `[skill_name, calls, failures, distinct_sessions,
/// last_used_dt]`. `skill_name` is hook metadata (tool_input.skill — never args).
pub const SKILLS_SQL: &str = r#"
SELECT
    skill_name,
    count(*) FILTER (WHERE event_type = 'tool.call')::int8                       AS calls,
    count(*) FILTER (WHERE event_type = 'tool.result' AND success = false)::int8 AS failures,
    count(DISTINCT session_id)::int8                                             AS distinct_sessions,
    max(dt)                                                                      AS last_used_dt
FROM events
WHERE skill_name IS NOT NULL AND dt >= $1
GROUP BY skill_name
ORDER BY calls DESC
"#;

/// `/web/q/sessions?days=N&limit=M` → `[session_id, agent, host_id,
/// started_at, events, tool_calls, failures, models, input_tokens,
/// output_tokens, cost_usd]`. `$2` is `LIMIT`.
pub const SESSIONS_SQL: &str = r#"
WITH e AS (
    SELECT * FROM events WHERE session_id IS NOT NULL AND dt >= $1
)
SELECT
    session_id,
    max(agent)                                                      AS agent,
    max(host_id)                                                    AS host_id,
    to_char(to_timestamp(min(ts) / 1000.0) AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS started_at,
    count(*)::int8                                                  AS events,
    count(*) FILTER (WHERE event_type = 'tool.call')::int8          AS tool_calls,
    count(*) FILTER (WHERE success = false)::int8                   AS failures,
    coalesce(string_agg(DISTINCT model, ','), '')                   AS models,
    sum(input_tokens)::int8                                         AS input_tokens,
    sum(output_tokens)::int8                                        AS output_tokens,
    sum(cost_usd)::float8                                           AS cost_usd
FROM e
GROUP BY session_id
ORDER BY min(ts) DESC
LIMIT $2
"#;

/// Role-scoped sibling of [`SESSIONS_SQL`] (account-model contract: "member's
/// /web/q/sessions returns ONLY their own sessions in a team org") -- exact
/// same shape/columns, with an extra `AND user_id = $2` (events.user_id is
/// the kikimimi account id, ingest.rs) and `LIMIT` bumped to `$3`.
/// `web_query.rs`'s `sessions` handler picks between the two based on the
/// caller's role + the active org's kind.
pub const SESSIONS_SQL_SELF: &str = r#"
WITH e AS (
    SELECT * FROM events WHERE session_id IS NOT NULL AND dt >= $1 AND user_id = $2
)
SELECT
    session_id,
    max(agent)                                                      AS agent,
    max(host_id)                                                    AS host_id,
    to_char(to_timestamp(min(ts) / 1000.0) AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS started_at,
    count(*)::int8                                                  AS events,
    count(*) FILTER (WHERE event_type = 'tool.call')::int8          AS tool_calls,
    count(*) FILTER (WHERE success = false)::int8                   AS failures,
    coalesce(string_agg(DISTINCT model, ','), '')                   AS models,
    sum(input_tokens)::int8                                         AS input_tokens,
    sum(output_tokens)::int8                                        AS output_tokens,
    sum(cost_usd)::float8                                           AS cost_usd
FROM e
GROUP BY session_id
ORDER BY min(ts) DESC
LIMIT $3
"#;
