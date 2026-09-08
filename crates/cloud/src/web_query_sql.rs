//! Postgres ports of the DuckDB SQL in `crates/cli/src/web_query.rs`
//! (`/web/q/*`, contract: `web/src/api/types.ts`, reference impl:
//! `web/mock/server.mjs`) — scoped instead to `events` under RLS (like
//! `query_sql.rs`'s `/v1/query/*` ports: the query text has no `org_id`
//! filter at all, Postgres adds it via the `events` row-security policy).
//! Column names/order match the contract exactly — `web_query.rs` reads
//! them straight off the prepared statement (`query.rs`'s "columns come
//! from `prepare`, not from a row" trick), so a mismatched `AS` alias here
//! would show up directly as a wrong `columns` entry in a test, not a
//! silent bug. [`MEMBERS_SQL`] is cloud-only (no `crates/cli`/DuckDB or
//! `web/mock/server.mjs` counterpart yet) — an explanatory per-member usage
//! view gated admin/owner-only by `web_query.rs`'s `members` handler.
//! [`UNUSED_MCP_SQL`] here is its own design, richer than `query_sql.rs`'s
//! `/v1/query/unused-mcp` (kept 4-column-backward-compatible) — see its own
//! doc comment.
//!
//! Every non-TEXT/BOOL output is cast to INT8 or FLOAT8, same reasoning as
//! `query_sql.rs`: `sum(bigint)` and `percentile_cont` over an integer
//! column both default to NUMERIC in Postgres otherwise, which the generic
//! `pg_value_to_json` decoder (`query.rs`) doesn't have an arm for.
//!
//! `tool.result` double-counting: same `tool_results` dedup CTE and
//! reasoning as `query_sql.rs`'s module doc (`kikimimi init` enables both hook
//! and OTel `tool.result` emission for Claude Code, so every failure/latency
//! aggregate here would otherwise count a hook/OTel duplicate pair as two
//! results). Applied everywhere a query below counts or measures
//! `tool.result` rows: [`OVERVIEW_SQL`]/[`SESSIONS_SQL`]/
//! [`SESSIONS_SQL_SELF`]'s `failures`, [`TOOLS_SQL`]'s `failures`/
//! `p50_duration_ms`/`p95_duration_ms`, [`MCP_SQL`]/[`SKILLS_SQL`]'s
//! `failures`, [`MEMBERS_SQL`]'s `tool_failures`. [`MACHINES_SQL`] never
//! touches `tool.result`, so it's untouched.
//!
//! `$1` is always the `dt >=` lower bound (`days` turned into a `YYYY-MM-DD`
//! string by `web_query.rs`'s `today_minus_days`) except `machines` (no
//! `days` param at all — task spec: "machines has no days param — use 30d
//! window for events_30d and no filter for last_event_ts", so its `$1` is a
//! *fixed* 30-day-ago cutoff used only inside the `events_30d` FILTER, never
//! as a `WHERE`) and `sessions`, whose `$2` is `LIMIT`.

/// `/web/q/overview?days=N` → `[dt, events, tool_calls, failures,
/// input_tokens, output_tokens, cost_usd]`. `failures` applies the
/// `tool_results` dedup (module doc); `events`/`tool_calls` stay raw
/// ingested counts, same choice as `query_sql.rs`'s `TODAY_SQL`.
pub const OVERVIEW_SQL: &str = r#"
WITH e AS (SELECT * FROM events WHERE dt >= $1),
tool_results AS (
    SELECT * FROM (
        SELECT e.*, row_number() OVER (
            PARTITION BY session_id, correlation_key
            ORDER BY CASE source WHEN 'otel' THEN 0 WHEN 'hook' THEN 1 ELSE 2 END, ts
        ) AS src_rank
        FROM e
        WHERE event_type = 'tool.result' AND correlation_key IS NOT NULL
    ) d WHERE src_rank = 1
    UNION ALL
    SELECT e.*, 1 AS src_rank FROM e WHERE event_type = 'tool.result' AND correlation_key IS NULL
),
fails AS (
    SELECT dt, count(*) AS failures
    FROM (
        SELECT dt, success FROM e WHERE event_type <> 'tool.result'
        UNION ALL
        SELECT dt, success FROM tool_results
    ) u
    WHERE success = false
    GROUP BY dt
)
SELECT
    e.dt,
    count(*)::int8                                                  AS events,
    count(*) FILTER (WHERE e.event_type = 'tool.call')::int8        AS tool_calls,
    coalesce(max(fails.failures), 0)::int8                          AS failures,
    sum(e.input_tokens)::int8                                       AS input_tokens,
    sum(e.output_tokens)::int8                                      AS output_tokens,
    sum(e.cost_usd)::float8                                         AS cost_usd
FROM e
LEFT JOIN fails ON fails.dt = e.dt
GROUP BY e.dt
ORDER BY e.dt
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
/// quantile, and dataset sizes here don't need one. `failures`/`p50`/`p95`
/// come from the deduped `tool_results` (module doc), `LEFT JOIN`'d back
/// onto the undeduped `e` — same shape as `query_sql.rs`'s `TOOLS_SQL`.
pub const TOOLS_SQL: &str = r#"
WITH e AS (
    SELECT * FROM events WHERE tool_name IS NOT NULL AND dt >= $1
),
tool_results AS (
    SELECT * FROM (
        SELECT e.*, row_number() OVER (
            PARTITION BY session_id, correlation_key
            ORDER BY CASE source WHEN 'otel' THEN 0 WHEN 'hook' THEN 1 ELSE 2 END, ts
        ) AS src_rank
        FROM e
        WHERE event_type = 'tool.result' AND correlation_key IS NOT NULL
    ) d WHERE src_rank = 1
    UNION ALL
    SELECT e.*, 1 AS src_rank FROM e WHERE event_type = 'tool.result' AND correlation_key IS NULL
),
results AS (
    SELECT
        tool_name,
        count(*) FILTER (WHERE success = false) AS failures,
        percentile_cont(0.5)  WITHIN GROUP (ORDER BY duration_ms::float8) AS p50_duration_ms,
        percentile_cont(0.95) WITHIN GROUP (ORDER BY duration_ms::float8) AS p95_duration_ms
    FROM tool_results
    GROUP BY tool_name
)
SELECT
    e.tool_name,
    max(e.tool_kind)                                          AS tool_kind,
    count(*) FILTER (WHERE e.event_type = 'tool.call')::int8  AS calls,
    coalesce(max(results.failures), 0)::int8                  AS failures,
    max(results.p50_duration_ms)::float8                      AS p50_duration_ms,
    max(results.p95_duration_ms)::float8                      AS p95_duration_ms
FROM e
LEFT JOIN results ON results.tool_name = e.tool_name
GROUP BY e.tool_name
ORDER BY calls DESC
"#;

/// `/web/q/mcp?days=N` → `[mcp_server, calls, failures, distinct_sessions,
/// last_called_dt]`. Only `failures` needs the dedup (module doc; `calls`
/// is hook-only, `distinct_sessions` is `DISTINCT`-immune,
/// `last_called_dt` is `tool.call`-only).
pub const MCP_SQL: &str = r#"
WITH e AS (
    SELECT * FROM events WHERE mcp_server IS NOT NULL AND dt >= $1
),
tool_results AS (
    SELECT * FROM (
        SELECT e.*, row_number() OVER (
            PARTITION BY session_id, correlation_key
            ORDER BY CASE source WHEN 'otel' THEN 0 WHEN 'hook' THEN 1 ELSE 2 END, ts
        ) AS src_rank
        FROM e
        WHERE event_type = 'tool.result' AND correlation_key IS NOT NULL
    ) d WHERE src_rank = 1
    UNION ALL
    SELECT e.*, 1 AS src_rank FROM e WHERE event_type = 'tool.result' AND correlation_key IS NULL
),
results AS (
    SELECT mcp_server, count(*) FILTER (WHERE success = false) AS failures
    FROM tool_results
    GROUP BY mcp_server
)
SELECT
    e.mcp_server,
    count(*) FILTER (WHERE e.event_type = 'tool.call')::int8       AS calls,
    coalesce(max(results.failures), 0)::int8                       AS failures,
    count(DISTINCT e.session_id)::int8                              AS distinct_sessions,
    max(e.dt) FILTER (WHERE e.event_type = 'tool.call')             AS last_called_dt
FROM e
LEFT JOIN results ON results.mcp_server = e.mcp_server
GROUP BY e.mcp_server
ORDER BY calls DESC
"#;

/// `/web/q/skills?days=N` → `[skill_name, calls, failures, distinct_sessions,
/// last_used_dt]`. `skill_name` is hook metadata (tool_input.skill — never
/// args). Only `failures` needs the dedup (same reasoning as `MCP_SQL`).
pub const SKILLS_SQL: &str = r#"
WITH e AS (
    SELECT * FROM events WHERE skill_name IS NOT NULL AND dt >= $1
),
tool_results AS (
    SELECT * FROM (
        SELECT e.*, row_number() OVER (
            PARTITION BY session_id, correlation_key
            ORDER BY CASE source WHEN 'otel' THEN 0 WHEN 'hook' THEN 1 ELSE 2 END, ts
        ) AS src_rank
        FROM e
        WHERE event_type = 'tool.result' AND correlation_key IS NOT NULL
    ) d WHERE src_rank = 1
    UNION ALL
    SELECT e.*, 1 AS src_rank FROM e WHERE event_type = 'tool.result' AND correlation_key IS NULL
),
results AS (
    SELECT skill_name, count(*) FILTER (WHERE success = false) AS failures
    FROM tool_results
    GROUP BY skill_name
)
SELECT
    e.skill_name,
    count(*) FILTER (WHERE e.event_type = 'tool.call')::int8       AS calls,
    coalesce(max(results.failures), 0)::int8                       AS failures,
    count(DISTINCT e.session_id)::int8                              AS distinct_sessions,
    max(e.dt)                                                        AS last_used_dt
FROM e
LEFT JOIN results ON results.skill_name = e.skill_name
GROUP BY e.skill_name
ORDER BY calls DESC
"#;

/// `/web/q/unused-mcp?days=N` (architecture.md §7.1 「導入されているのに呼ばれない
/// サーバー」, §7.2 `unused_mcp_server`) → `[mcp_server, configured, calls,
/// distinct_sessions, last_called_dt, sessions_configured,
/// configured_from_snapshot]`.
///
/// This is the fix for the single most important case `query_sql.rs`'s
/// older `/v1/query/unused-mcp` structurally cannot show: a server
/// *configured but never once called* (that server never appears in
/// `events` at all, so a proxy built only from observed `tool.call` rows
/// can never surface it). `configured` here instead comes from a real
/// snapshot: `configured_mcp_servers` (§5.1) — a JSON array of server
/// names `kikimimi agent` writes onto Claude Code `session.start` rows
/// (`crates/cli/src/mcp_config.rs`) — unnested via
/// `jsonb_array_elements_text` over `session.start` rows in `[$1, $2]`
/// (`$1` is `dt >=`, same as every other `/web/q/*` query; there's no
/// upper bound here). `sessions_configured` counts how many of those
/// `session.start` rows listed each server, so "configured by 1 of 40
/// sessions" is visible instead of a plain boolean.
///
/// `configured_from_snapshot` is `true` when at least one `session.start`
/// row in range actually carries the column (clients running this
/// change); when none do (older clients, or simply no `session.start` in
/// range) this falls back to the pre-existing proxy — "observed via
/// `tool.call` in the trailing 30 days" — and reports `false`, so the UI
/// can say "no config snapshot yet — showing observed servers only"
/// instead of silently passing off a proxy as a real snapshot.
///
/// `calls`/`distinct_sessions`/`last_called_dt` come from `tool.call` rows
/// in `[$1, $2]` only (not the `tool_results` dedup — `tool.call` is
/// hook-only, module doc). Rows are the UNION of configured and observed
/// servers (a server observed but never configured, or configured but
/// never observed, both show up), sorted never-called-but-configured
/// first, then by `calls` ascending — the whole point of the query is
/// surfacing context you're paying for on every request and not using.
pub const UNUSED_MCP_SQL: &str = r#"
WITH snapshot_configured AS (
    SELECT DISTINCT jsonb_array_elements_text(configured_mcp_servers::jsonb) AS mcp_server
    FROM events
    WHERE event_type IN ('session.start', 'session.end')
      AND configured_mcp_servers IS NOT NULL
      AND dt >= $1
),
sessions_configured_count AS (
    SELECT mcp_server, count(*)::int8 AS sessions_configured
    FROM (
        SELECT DISTINCT session_id, jsonb_array_elements_text(configured_mcp_servers::jsonb) AS mcp_server
        FROM events
        WHERE event_type IN ('session.start', 'session.end')
          AND configured_mcp_servers IS NOT NULL
          AND dt >= $1
    ) x
    GROUP BY mcp_server
),
has_snapshot AS (
    SELECT EXISTS (SELECT 1 FROM snapshot_configured) AS v
),
proxy_configured AS (
    SELECT DISTINCT mcp_server
    FROM events
    WHERE event_type = 'tool.call'
      AND mcp_server IS NOT NULL
      AND dt >= to_char(now() - interval '30 days', 'YYYY-MM-DD')
),
configured AS (
    SELECT mcp_server FROM snapshot_configured WHERE (SELECT v FROM has_snapshot)
    UNION
    SELECT mcp_server FROM proxy_configured WHERE NOT (SELECT v FROM has_snapshot)
),
observed AS (
    SELECT
        mcp_server,
        count(*)::int8                    AS calls,
        count(DISTINCT session_id)::int8  AS distinct_sessions,
        max(dt)                           AS last_called_dt
    FROM events
    WHERE event_type = 'tool.call' AND mcp_server IS NOT NULL AND dt >= $1
    GROUP BY mcp_server
),
all_servers AS (
    SELECT mcp_server FROM configured
    UNION
    SELECT mcp_server FROM observed
)
SELECT
    a.mcp_server,
    (c.mcp_server IS NOT NULL)                       AS configured,
    coalesce(o.calls, 0::int8)                       AS calls,
    coalesce(o.distinct_sessions, 0::int8)           AS distinct_sessions,
    o.last_called_dt                                 AS last_called_dt,
    coalesce(sc.sessions_configured, 0::int8)         AS sessions_configured,
    (SELECT v FROM has_snapshot)                     AS configured_from_snapshot
FROM all_servers a
LEFT JOIN configured c ON c.mcp_server = a.mcp_server
LEFT JOIN observed o ON o.mcp_server = a.mcp_server
LEFT JOIN sessions_configured_count sc ON sc.mcp_server = a.mcp_server
ORDER BY
    (c.mcp_server IS NOT NULL AND coalesce(o.calls, 0) = 0) DESC,
    coalesce(o.calls, 0) ASC
"#;

/// `/web/q/sessions?days=N&limit=M` → `[session_id, agent, host_id,
/// started_at, events, tool_calls, failures, models, input_tokens,
/// output_tokens, cost_usd]`. `$2` is `LIMIT`. `failures` applies the
/// `tool_results` dedup (module doc) via the `fails` CTE, same
/// non-tool.result-plus-deduped-tool.result union as `OVERVIEW_SQL`;
/// `events`/`tool_calls` stay raw ingested counts.
pub const SESSIONS_SQL: &str = r#"
WITH e AS (
    SELECT * FROM events WHERE session_id IS NOT NULL AND dt >= $1
),
tool_results AS (
    SELECT * FROM (
        SELECT e.*, row_number() OVER (
            PARTITION BY session_id, correlation_key
            ORDER BY CASE source WHEN 'otel' THEN 0 WHEN 'hook' THEN 1 ELSE 2 END, ts
        ) AS src_rank
        FROM e
        WHERE event_type = 'tool.result' AND correlation_key IS NOT NULL
    ) d WHERE src_rank = 1
    UNION ALL
    SELECT e.*, 1 AS src_rank FROM e WHERE event_type = 'tool.result' AND correlation_key IS NULL
),
fails AS (
    SELECT session_id, count(*) AS failures
    FROM (
        SELECT session_id, success FROM e WHERE event_type <> 'tool.result'
        UNION ALL
        SELECT session_id, success FROM tool_results
    ) u
    WHERE success = false
    GROUP BY session_id
)
SELECT
    e.session_id,
    max(e.agent)                                                    AS agent,
    max(e.host_id)                                                  AS host_id,
    to_char(to_timestamp(min(e.ts) / 1000.0) AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS started_at,
    count(*)::int8                                                  AS events,
    count(*) FILTER (WHERE e.event_type = 'tool.call')::int8        AS tool_calls,
    coalesce(max(fails.failures), 0)::int8                          AS failures,
    coalesce(string_agg(DISTINCT e.model, ','), '')                 AS models,
    sum(e.input_tokens)::int8                                       AS input_tokens,
    sum(e.output_tokens)::int8                                      AS output_tokens,
    sum(e.cost_usd)::float8                                         AS cost_usd
FROM e
LEFT JOIN fails ON fails.session_id = e.session_id
GROUP BY e.session_id
ORDER BY min(e.ts) DESC
LIMIT $2
"#;

/// Role-scoped sibling of [`SESSIONS_SQL`] (account-model contract: "member's
/// /web/q/sessions returns ONLY their own sessions in a team org") -- exact
/// same shape/columns (dedup included), with an extra `AND user_id = $2`
/// (events.user_id is the kikimimi account id, ingest.rs) and `LIMIT` bumped
/// to `$3`. `web_query.rs`'s `sessions` handler picks between the two based
/// on the caller's role + the active org's kind.
pub const SESSIONS_SQL_SELF: &str = r#"
WITH e AS (
    SELECT * FROM events WHERE session_id IS NOT NULL AND dt >= $1 AND user_id = $2
),
tool_results AS (
    SELECT * FROM (
        SELECT e.*, row_number() OVER (
            PARTITION BY session_id, correlation_key
            ORDER BY CASE source WHEN 'otel' THEN 0 WHEN 'hook' THEN 1 ELSE 2 END, ts
        ) AS src_rank
        FROM e
        WHERE event_type = 'tool.result' AND correlation_key IS NOT NULL
    ) d WHERE src_rank = 1
    UNION ALL
    SELECT e.*, 1 AS src_rank FROM e WHERE event_type = 'tool.result' AND correlation_key IS NULL
),
fails AS (
    SELECT session_id, count(*) AS failures
    FROM (
        SELECT session_id, success FROM e WHERE event_type <> 'tool.result'
        UNION ALL
        SELECT session_id, success FROM tool_results
    ) u
    WHERE success = false
    GROUP BY session_id
)
SELECT
    e.session_id,
    max(e.agent)                                                    AS agent,
    max(e.host_id)                                                  AS host_id,
    to_char(to_timestamp(min(e.ts) / 1000.0) AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS started_at,
    count(*)::int8                                                  AS events,
    count(*) FILTER (WHERE e.event_type = 'tool.call')::int8        AS tool_calls,
    coalesce(max(fails.failures), 0)::int8                          AS failures,
    coalesce(string_agg(DISTINCT e.model, ','), '')                 AS models,
    sum(e.input_tokens)::int8                                       AS input_tokens,
    sum(e.output_tokens)::int8                                      AS output_tokens,
    sum(e.cost_usd)::float8                                         AS cost_usd
FROM e
LEFT JOIN fails ON fails.session_id = e.session_id
GROUP BY e.session_id
ORDER BY min(e.ts) DESC
LIMIT $3
"#;

/// `/web/q/subagents?days=N&limit=M` → the per-session subagent fan-out view
/// (`query_sql::SUBAGENTS_SQL` minus its `TOTAL` row, most recent first,
/// `$2` = `LIMIT`). Same columns as the named query; see it for the
/// honesty notes on `subagent_tokens_est` / `subagents_with_usage`.
pub const SUBAGENTS_SQL: &str = r#"
WITH e AS (
    SELECT * FROM events WHERE session_id IS NOT NULL AND dt >= $1
),
agents AS (
    SELECT session_id, agent_id,
           max(agent_type) AS agent_type,
           min(ts) AS first_ts, max(ts) AS last_ts,
           count(*) FILTER (WHERE event_type = 'tool.call')::int8 AS tool_calls,
           count(*) FILTER (WHERE event_type = 'api.request')::int8 AS api_requests,
           sum(coalesce(input_tokens, 0) + coalesce(output_tokens, 0))
               FILTER (WHERE event_type = 'api.request'
                         AND (input_tokens IS NOT NULL OR output_tokens IS NOT NULL)) AS api_tokens,
           max(coalesce(input_tokens, 0) + coalesce(output_tokens, 0))
               FILTER (WHERE event_type = 'subagent.stop'
                         AND (input_tokens IS NOT NULL OR output_tokens IS NOT NULL)) AS stop_tokens,
           max(duration_ms) FILTER (WHERE event_type = 'subagent.stop') AS stop_duration_ms
    FROM e
    WHERE agent_id IS NOT NULL
    GROUP BY session_id, agent_id
),
per_session AS (
    SELECT session_id,
           count(*)::int8 AS subagents,
           string_agg(DISTINCT agent_type, ',' ORDER BY agent_type) AS agent_types,
           sum(tool_calls)::int8 AS subagent_tool_calls,
           sum(api_requests)::int8 AS subagent_api_requests,
           sum(coalesce(stop_duration_ms, last_ts - first_ts))::int8 AS subagent_duration_ms,
           sum(coalesce(api_tokens, stop_tokens))::int8 AS subagent_tokens_est,
           count(*) FILTER (WHERE coalesce(api_tokens, stop_tokens) IS NOT NULL)::int8 AS subagents_with_usage
    FROM agents
    GROUP BY session_id
),
sess AS (
    SELECT session_id, min(ts) AS first_ts, (max(ts) - min(ts))::int8 AS session_duration_ms,
           count(*) FILTER (WHERE event_type = 'tool.call')::int8 AS tool_calls,
           coalesce(
               sum(coalesce(input_tokens, 0) + coalesce(output_tokens, 0))
                   FILTER (WHERE event_type = 'api.request' AND source = 'otel'),
               sum(coalesce(input_tokens, 0) + coalesce(output_tokens, 0))
                   FILTER (WHERE event_type = 'api.request' AND source = 'log'))::int8 AS session_tokens_est
    FROM e
    GROUP BY session_id
)
SELECT s.session_id,
       to_char(to_timestamp(s.first_ts / 1000.0) AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS started_at,
       p.subagents, p.agent_types, p.subagent_tool_calls, s.tool_calls,
       p.subagent_duration_ms, s.session_duration_ms,
       round((p.subagent_duration_ms::float8 / nullif(s.session_duration_ms, 0))::numeric, 3)::float8 AS duration_share,
       p.subagent_api_requests, p.subagent_tokens_est, s.session_tokens_est,
       round((p.subagent_tokens_est::float8 / nullif(s.session_tokens_est, 0))::numeric, 3)::float8 AS token_share,
       p.subagents_with_usage
FROM per_session p
JOIN sess s ON s.session_id = p.session_id
ORDER BY s.first_ts DESC
LIMIT $2
"#;

/// Role-scoped sibling of [`SUBAGENTS_SQL`] (member of a team org: own
/// sessions only, `user_id = $2`, `LIMIT $3`), the [`SESSIONS_SQL_SELF`]
/// pattern.
pub const SUBAGENTS_SQL_SELF: &str = r#"
WITH e AS (
    SELECT * FROM events WHERE session_id IS NOT NULL AND dt >= $1 AND user_id = $2
),
agents AS (
    SELECT session_id, agent_id,
           max(agent_type) AS agent_type,
           min(ts) AS first_ts, max(ts) AS last_ts,
           count(*) FILTER (WHERE event_type = 'tool.call')::int8 AS tool_calls,
           count(*) FILTER (WHERE event_type = 'api.request')::int8 AS api_requests,
           sum(coalesce(input_tokens, 0) + coalesce(output_tokens, 0))
               FILTER (WHERE event_type = 'api.request'
                         AND (input_tokens IS NOT NULL OR output_tokens IS NOT NULL)) AS api_tokens,
           max(coalesce(input_tokens, 0) + coalesce(output_tokens, 0))
               FILTER (WHERE event_type = 'subagent.stop'
                         AND (input_tokens IS NOT NULL OR output_tokens IS NOT NULL)) AS stop_tokens,
           max(duration_ms) FILTER (WHERE event_type = 'subagent.stop') AS stop_duration_ms
    FROM e
    WHERE agent_id IS NOT NULL
    GROUP BY session_id, agent_id
),
per_session AS (
    SELECT session_id,
           count(*)::int8 AS subagents,
           string_agg(DISTINCT agent_type, ',' ORDER BY agent_type) AS agent_types,
           sum(tool_calls)::int8 AS subagent_tool_calls,
           sum(api_requests)::int8 AS subagent_api_requests,
           sum(coalesce(stop_duration_ms, last_ts - first_ts))::int8 AS subagent_duration_ms,
           sum(coalesce(api_tokens, stop_tokens))::int8 AS subagent_tokens_est,
           count(*) FILTER (WHERE coalesce(api_tokens, stop_tokens) IS NOT NULL)::int8 AS subagents_with_usage
    FROM agents
    GROUP BY session_id
),
sess AS (
    SELECT session_id, min(ts) AS first_ts, (max(ts) - min(ts))::int8 AS session_duration_ms,
           count(*) FILTER (WHERE event_type = 'tool.call')::int8 AS tool_calls,
           coalesce(
               sum(coalesce(input_tokens, 0) + coalesce(output_tokens, 0))
                   FILTER (WHERE event_type = 'api.request' AND source = 'otel'),
               sum(coalesce(input_tokens, 0) + coalesce(output_tokens, 0))
                   FILTER (WHERE event_type = 'api.request' AND source = 'log'))::int8 AS session_tokens_est
    FROM e
    GROUP BY session_id
)
SELECT s.session_id,
       to_char(to_timestamp(s.first_ts / 1000.0) AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS started_at,
       p.subagents, p.agent_types, p.subagent_tool_calls, s.tool_calls,
       p.subagent_duration_ms, s.session_duration_ms,
       round((p.subagent_duration_ms::float8 / nullif(s.session_duration_ms, 0))::numeric, 3)::float8 AS duration_share,
       p.subagent_api_requests, p.subagent_tokens_est, s.session_tokens_est,
       round((p.subagent_tokens_est::float8 / nullif(s.session_tokens_est, 0))::numeric, 3)::float8 AS token_share,
       p.subagents_with_usage
FROM per_session p
JOIN sess s ON s.session_id = p.session_id
ORDER BY s.first_ts DESC
LIMIT $3
"#;

/// `/web/q/coverage?days=N` (KKM-17, architecture.md §7.1 "数字の信頼度を
///隠さない"): one row of *how much of the picture is missing*, computed over
/// `[$1, today]`. Every rate the UI shows is a ratio of two of these counts,
/// so the numbers stay auditable:
///
/// - `sessions_without_usage` / `sessions` — sessions with no `api.request`
///   that carried tokens (`usage_source = unknown` in §7.1 terms): hooks-only
///   machines, OTel not restarted after `init`, Codex without usage.
/// - `events_user_id_null` / `events` — rows the cloud could not attribute to
///   an account (locally always 100%: there is no account).
/// - `tool_results_matched` / `tool_results_hook` — hook `tool.result`s whose
///   `tool_use_id` also arrived via OTel (§5.1 correlation), and
///   `tool_results_otel` for the other direction.
/// - `tool_results_raw` vs `tool_results_deduped` — what the hook/OTel dedup
///   folds away (the module-doc `tool_results` rule).
/// - `subagents_with_usage` / `subagents` — [`SUBAGENTS_SQL`]'s coverage.
/// - `hosts_silent_24h` / `hosts` — devices whose last event is older than
///   `$2` (ms; the caller passes now − 24h), over *all* time like `MACHINES_SQL`.
pub const COVERAGE_SQL: &str = r#"
WITH e AS (SELECT * FROM events WHERE dt >= $1),
sess AS (
    SELECT session_id,
           bool_or(event_type = 'api.request' AND (input_tokens IS NOT NULL OR output_tokens IS NOT NULL)) AS has_usage
    FROM e WHERE session_id IS NOT NULL
    GROUP BY session_id
),
keys AS (
    SELECT session_id, correlation_key,
           bool_or(source = 'hook') AS in_hook,
           bool_or(source = 'otel') AS in_otel
    FROM e WHERE event_type = 'tool.result' AND correlation_key IS NOT NULL
    GROUP BY session_id, correlation_key
),
agents AS (
    SELECT session_id, agent_id,
           bool_or(input_tokens IS NOT NULL OR output_tokens IS NOT NULL) AS has_usage
    FROM e WHERE agent_id IS NOT NULL AND event_type IN ('api.request', 'subagent.stop')
    GROUP BY session_id, agent_id
),
hosts AS (SELECT host_id, max(ts) AS last_ts FROM events GROUP BY host_id)
SELECT
    (SELECT count(*) FROM e)::int8                                          AS events,
    (SELECT count(*) FROM e WHERE user_id IS NULL)::int8                    AS events_user_id_null,
    (SELECT count(*) FROM sess)::int8                                       AS sessions,
    (SELECT count(*) FROM sess WHERE NOT has_usage)::int8                   AS sessions_without_usage,
    (SELECT count(*) FROM keys WHERE in_hook)::int8                         AS tool_results_hook,
    (SELECT count(*) FROM keys WHERE in_otel)::int8                         AS tool_results_otel,
    (SELECT count(*) FROM keys WHERE in_hook AND in_otel)::int8             AS tool_results_matched,
    (SELECT count(*) FROM e WHERE event_type = 'tool.result')::int8         AS tool_results_raw,
    ((SELECT count(*) FROM e WHERE event_type = 'tool.result' AND correlation_key IS NULL)
     + (SELECT count(*) FROM keys))::int8                                   AS tool_results_deduped,
    (SELECT count(*) FROM agents)::int8                                     AS subagents,
    (SELECT count(*) FROM agents WHERE has_usage)::int8                     AS subagents_with_usage,
    (SELECT count(*) FROM hosts)::int8                                      AS hosts,
    (SELECT count(*) FROM hosts WHERE last_ts < $2)::int8                   AS hosts_silent_24h,
    (SELECT to_char(to_timestamp(max(ts) / 1000.0) AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') FROM e) AS last_event_ts
"#;

/// `/web/q/unused-skills?days=N` → `query_sql::UNUSED_SKILLS_SQL` over
/// `dt >= $1` (KKM-18). Same columns.
pub const UNUSED_SKILLS_SQL: &str = r#"
WITH e AS (
    SELECT * FROM events WHERE dt >= $1
),
snap AS (
    SELECT DISTINCT session_id, jsonb_array_elements_text(configured_skills::jsonb) AS skill_name
    FROM e
    WHERE event_type IN ('session.start', 'session.end') AND configured_skills IS NOT NULL
      AND session_id IS NOT NULL
),
configured AS (
    SELECT skill_name, count(*)::int8 AS sessions_configured FROM snap GROUP BY skill_name
),
used AS (
    SELECT skill_name, count(*)::int8 AS calls, count(DISTINCT session_id)::int8 AS distinct_sessions,
           max(dt) AS last_used_dt
    FROM e WHERE event_type = 'tool.call' AND skill_name IS NOT NULL
    GROUP BY skill_name
)
SELECT coalesce(c.skill_name, u.skill_name)          AS skill_name,
       (c.skill_name IS NOT NULL)                     AS configured,
       coalesce(c.sessions_configured, 0)::int8       AS sessions_configured,
       coalesce(u.calls, 0)::int8                     AS calls,
       coalesce(u.distinct_sessions, 0)::int8         AS distinct_sessions,
       u.last_used_dt                                 AS last_used_dt
FROM configured c
FULL OUTER JOIN used u ON u.skill_name = c.skill_name
ORDER BY (c.skill_name IS NOT NULL AND coalesce(u.calls, 0) = 0) DESC,
         sessions_configured DESC, calls ASC, skill_name
"#;

/// `/web/q/members?days=N` → `[user_id, sessions, api_requests, tool_calls,
/// tool_failures, input_tokens, output_tokens, cache_read_tokens, cost_usd,
/// loop_suspect_sessions]`. An **explanatory** per-member usage view, not a
/// spending leaderboard -- `ORDER BY user_id` is alphabetical on purpose,
/// never by cost/usage, so this never reads as a ranking (2026-09 リサーチ:
/// leaderboard 演出は IC の反発を招き churn につながる, guru-direction memo).
/// `web_query.rs`'s `members` handler gates this admin/owner-only in a team
/// org (unlike [`SESSIONS_SQL`]/[`SESSIONS_SQL_SELF`], there is no
/// self-scoped variant -- a member below admin gets a 403, not their own
/// row).
///
/// HONESTY NOTE (v0 の雑な閾値): `loop_suspect_sessions` は「セッションあたり
/// `api.request` が 50 件以上」を機械的にループ疑いとみなした件数。50 という
/// しきい値に統計的根拠はなく、v0 で決め打ちした目安に過ぎない -- 正当に長い
/// セッションを誤検知することもあれば、本当に暴走しているループを見逃す
/// こともある。「気になったら見る」ための補助シグナルであって、確定的な異常
/// 判定ではない。
/// `tool_failures` applies the `tool_results` dedup (module doc) via the
/// `fails_by_user` CTE below -- unlike `SESSIONS_SQL`'s generic `failures`,
/// this column was already `tool.result`-scoped, so `fails_by_user` reads
/// straight off `tool_results` with no non-tool.result union needed.
pub const MEMBERS_SQL: &str = r#"
WITH e AS (
    SELECT * FROM events WHERE user_id IS NOT NULL AND dt >= $1
),
per_session AS (
    SELECT
        user_id,
        session_id,
        count(*) FILTER (WHERE event_type = 'api.request') AS api_requests
    FROM e
    WHERE session_id IS NOT NULL
    GROUP BY user_id, session_id
),
loop_suspects AS (
    SELECT user_id, count(*) AS loop_suspect_sessions
    FROM per_session
    WHERE api_requests >= 50
    GROUP BY user_id
),
tool_results AS (
    SELECT * FROM (
        SELECT e.*, row_number() OVER (
            PARTITION BY session_id, correlation_key
            ORDER BY CASE source WHEN 'otel' THEN 0 WHEN 'hook' THEN 1 ELSE 2 END, ts
        ) AS src_rank
        FROM e
        WHERE event_type = 'tool.result' AND correlation_key IS NOT NULL
    ) d WHERE src_rank = 1
    UNION ALL
    SELECT e.*, 1 AS src_rank FROM e WHERE event_type = 'tool.result' AND correlation_key IS NULL
),
fails_by_user AS (
    SELECT user_id, count(*) AS tool_failures
    FROM tool_results
    WHERE success = false
    GROUP BY user_id
)
SELECT
    e.user_id                                                       AS user_id,
    count(DISTINCT e.session_id)::int8                              AS sessions,
    count(*) FILTER (WHERE e.event_type = 'api.request')::int8      AS api_requests,
    count(*) FILTER (WHERE e.event_type = 'tool.call')::int8        AS tool_calls,
    coalesce(max(fu.tool_failures), 0)::int8                        AS tool_failures,
    sum(e.input_tokens)::int8                                       AS input_tokens,
    sum(e.output_tokens)::int8                                      AS output_tokens,
    sum(e.cache_read_tokens)::int8                                  AS cache_read_tokens,
    sum(e.cost_usd)::float8                                         AS cost_usd,
    coalesce(max(ls.loop_suspect_sessions), 0)::int8                AS loop_suspect_sessions
FROM e
LEFT JOIN loop_suspects ls ON ls.user_id = e.user_id
LEFT JOIN fails_by_user fu ON fu.user_id = e.user_id
GROUP BY e.user_id
ORDER BY e.user_id
"#;

/// `/web/q/patterns?days=N` (WEB API CONTRACT; architecture.md §7.2 "MCP
/// サーバー / スキル / ツール単位で組織横断に集約したランキング", KKM-11).
/// Aggregates the scanner's `pattern_hits` (RLS-scoped) over `dt >= $1`
/// per `(pattern_id, subject)`. `priority` = `wasted_tokens_est × sessions`
/// is the improvement-backlog sort key; it is NULL — not 0 — when no hit
/// in the group could be priced, so unpriced patterns (`long_tool_tail`,
/// `permission_denied_loop`) never sink below priced ones silently: the
/// UI shows them with "unknown" cost. Never per-person (§11): the only
/// dimensions are the pattern and what it points at.
pub const PATTERNS_SQL: &str = r#"
SELECT
    pattern_id,
    subject,
    count(DISTINCT session_id)::int8                      AS sessions,
    sum(incidents)::int8                                  AS incidents,
    sum(wasted_tokens_est)::int8                          AS wasted_tokens_est,
    count(*) FILTER (WHERE wasted_tokens_est IS NOT NULL)::int8 AS priced_hits,
    count(*)::int8                                        AS hits,
    (sum(wasted_tokens_est) * count(DISTINCT session_id))::int8 AS priority,
    min(dt)                                               AS first_seen_dt,
    max(dt)                                               AS last_seen_dt
FROM pattern_hits
WHERE dt >= $1
GROUP BY pattern_id, subject
ORDER BY priority DESC NULLS LAST, sessions DESC, incidents DESC, pattern_id, subject
"#;

/// `/web/q/pattern-hits?pattern_id=&subject=&days=&limit=`: the incidents
/// behind one ranking row, newest first — the "sample sessions" §7.3 hands
/// to the platform team. Metadata only (there is nothing else in
/// `pattern_hits`). `$1` from_dt, `$2` pattern_id, `$3` subject, `$4` limit.
pub const PATTERN_HITS_SQL: &str = r#"
SELECT
    h.dt,
    h.session_id,
    h.first_ts,
    h.last_ts,
    h.incidents,
    h.wasted_tokens_est,
    h.detail::text AS detail
FROM pattern_hits h
WHERE h.dt >= $1 AND h.pattern_id = $2 AND h.subject = $3
ORDER BY h.first_ts DESC
LIMIT $4
"#;

/// [`PATTERN_HITS_SQL`] for a `team`-org member below `admin`: only the
/// caller's own sessions (`events.user_id = $4`, same rule as
/// [`SESSIONS_SQL_SELF`]). `$5` limit.
pub const PATTERN_HITS_SQL_SELF: &str = r#"
WITH mine AS (
    SELECT DISTINCT session_id FROM events
    WHERE dt >= $1 AND user_id = $4 AND session_id IS NOT NULL
)
SELECT
    h.dt,
    h.session_id,
    h.first_ts,
    h.last_ts,
    h.incidents,
    h.wasted_tokens_est,
    h.detail::text AS detail
FROM pattern_hits h
JOIN mine m ON m.session_id = h.session_id
WHERE h.dt >= $1 AND h.pattern_id = $2 AND h.subject = $3
ORDER BY h.first_ts DESC
LIMIT $5
"#;

/// `/web/q/pattern-timeline?pattern_id=&subject=&days=` (KKM-12, §7.3
/// before/after): one row per day that had any session, with how many of
/// those sessions hit this (pattern, subject) and what it cost. `rate_pct`
/// is the §7.3 KPI "発生セッション率". Days with sessions but no hit are
/// present with zeros so a drop to nothing after a fix is visible as a
/// line of zeros, not a gap. `$1` from_dt, `$2` pattern_id, `$3` subject.
pub const PATTERN_TIMELINE_SQL: &str = r#"
WITH days AS (
    SELECT dt, count(DISTINCT session_id)::int8 AS sessions_total
    FROM events
    WHERE dt >= $1 AND session_id IS NOT NULL
    GROUP BY dt
),
hits AS (
    SELECT dt,
           count(DISTINCT session_id)::int8 AS sessions_hit,
           sum(incidents)::int8 AS incidents,
           sum(wasted_tokens_est)::int8 AS wasted_tokens_est
    FROM pattern_hits
    WHERE dt >= $1 AND pattern_id = $2 AND subject = $3
    GROUP BY dt
)
SELECT
    d.dt,
    d.sessions_total,
    coalesce(h.sessions_hit, 0)::int8 AS sessions_hit,
    (100.0 * coalesce(h.sessions_hit, 0) / NULLIF(d.sessions_total, 0))::float8 AS rate_pct,
    coalesce(h.incidents, 0)::int8 AS incidents,
    h.wasted_tokens_est
FROM days d
LEFT JOIN hits h ON h.dt = d.dt
ORDER BY d.dt
"#;

// ---------------------------------------------------------------------------
// /web/q/session?session_id=... — one session, drilled down (KKM session detail)
// ---------------------------------------------------------------------------
//
// Five queries, one response: `web_query.rs`'s `session_detail` runs them in
// one RLS transaction and returns `{summary, tools, subagents, timeline,
// events}`, each in the usual `{columns, rows}` shape. Every query takes the
// same two leading binds: `$1` = `session_id`, `$2` = the caller's account
// id when the role model scopes them to their own sessions (member of a
// team org, same rule as [`SESSIONS_SQL_SELF`]) and NULL otherwise — so one
// SQL text serves both scopes instead of a `_SELF` twin per query.
// `tool.result` rows are deduped (module doc) everywhere they are counted,
// measured or listed.

/// One row (or none → 404): the session's header numbers. `failures` is
/// the [`SESSIONS_SQL`] definition (every `success = false` row, tool
/// results deduped) so the number matches the Sessions list the reader
/// came from. `subagents` counts distinct `agent_id`s. `ended` says whether
/// a `session.end` was seen — without it `ended_at`/`duration_ms` are just
/// "last event so far". `configured_mcp_servers`/`configured_skills` are the
/// `session.start`/`session.end` snapshots (JSON array strings), NULL when
/// no row carried one.
pub const SESSION_SUMMARY_SQL: &str = r#"
WITH e AS (
    SELECT * FROM events WHERE session_id = $1 AND ($2::text IS NULL OR user_id = $2::text)
),
tool_results AS (
    SELECT * FROM (
        SELECT e.*, row_number() OVER (
            PARTITION BY session_id, correlation_key
            ORDER BY CASE source WHEN 'otel' THEN 0 WHEN 'hook' THEN 1 ELSE 2 END, ts
        ) AS src_rank
        FROM e
        WHERE event_type = 'tool.result' AND correlation_key IS NOT NULL
    ) d WHERE src_rank = 1
    UNION ALL
    SELECT e.*, 1 AS src_rank FROM e WHERE event_type = 'tool.result' AND correlation_key IS NULL
),
fails AS (
    SELECT count(*) AS failures
    FROM (
        SELECT success FROM e WHERE event_type <> 'tool.result'
        UNION ALL
        SELECT success FROM tool_results
    ) u
    WHERE success = false
)
SELECT
    max(e.session_id)                                               AS session_id,
    max(e.agent)                                                    AS agent,
    max(e.agent_version)                                            AS agent_version,
    max(e.host_id)                                                  AS host_id,
    max(e.repo)                                                     AS repo,
    to_char(to_timestamp(min(e.ts) / 1000.0) AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS started_at,
    to_char(to_timestamp(max(e.ts) / 1000.0) AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS ended_at,
    (max(e.ts) - min(e.ts))::int8                                   AS duration_ms,
    bool_or(e.event_type = 'session.end')                           AS ended,
    count(*)::int8                                                  AS events,
    count(*) FILTER (WHERE e.event_type = 'turn')::int8             AS turns,
    count(*) FILTER (WHERE e.event_type = 'tool.call')::int8        AS tool_calls,
    (SELECT failures FROM fails)::int8                              AS failures,
    count(*) FILTER (WHERE e.event_type = 'tool.denied')::int8      AS tool_denied,
    count(*) FILTER (WHERE e.event_type = 'api.request')::int8      AS api_requests,
    count(*) FILTER (WHERE e.event_type = 'api.error')::int8        AS api_errors,
    count(*) FILTER (WHERE e.event_type = 'compaction')::int8       AS compactions,
    count(DISTINCT e.agent_id)::int8                                AS subagents,
    coalesce(string_agg(DISTINCT e.model, ','), '')                 AS models,
    coalesce(string_agg(DISTINCT e.source, ','), '')                AS sources,
    sum(e.input_tokens)::int8                                       AS input_tokens,
    sum(e.output_tokens)::int8                                      AS output_tokens,
    sum(e.cache_read_tokens)::int8                                  AS cache_read_tokens,
    sum(e.cache_write_tokens)::int8                                 AS cache_write_tokens,
    sum(e.cost_usd)::float8                                         AS cost_usd,
    max(e.configured_mcp_servers)                                   AS configured_mcp_servers,
    max(e.configured_skills)                                        AS configured_skills,
    coalesce(string_agg(DISTINCT e.effort, ','), '')                AS efforts
FROM e
HAVING count(*) > 0
"#;

/// Per tool in this session: `[tool_name, tool_kind, mcp_server, calls,
/// subagent_calls, failures, denied, p50_duration_ms, p95_duration_ms,
/// total_duration_ms]`. Same dedup/percentile treatment as [`TOOLS_SQL`];
/// `subagent_calls` is how many of `calls` came from a subagent
/// (`agent_id IS NOT NULL`), `total_duration_ms` is the summed deduped
/// result duration — the "where did the wall-clock go" column.
pub const SESSION_TOOLS_SQL: &str = r#"
WITH e AS (
    SELECT * FROM events
    WHERE session_id = $1 AND ($2::text IS NULL OR user_id = $2::text) AND tool_name IS NOT NULL
),
tool_results AS (
    SELECT * FROM (
        SELECT e.*, row_number() OVER (
            PARTITION BY session_id, correlation_key
            ORDER BY CASE source WHEN 'otel' THEN 0 WHEN 'hook' THEN 1 ELSE 2 END, ts
        ) AS src_rank
        FROM e
        WHERE event_type = 'tool.result' AND correlation_key IS NOT NULL
    ) d WHERE src_rank = 1
    UNION ALL
    SELECT e.*, 1 AS src_rank FROM e WHERE event_type = 'tool.result' AND correlation_key IS NULL
),
results AS (
    SELECT
        tool_name,
        count(*) FILTER (WHERE success = false) AS failures,
        percentile_cont(0.5)  WITHIN GROUP (ORDER BY duration_ms::float8) AS p50_duration_ms,
        percentile_cont(0.95) WITHIN GROUP (ORDER BY duration_ms::float8) AS p95_duration_ms,
        sum(duration_ms) AS total_duration_ms
    FROM tool_results
    GROUP BY tool_name
)
SELECT
    e.tool_name,
    max(e.tool_kind)                                                              AS tool_kind,
    max(e.mcp_server)                                                             AS mcp_server,
    count(*) FILTER (WHERE e.event_type = 'tool.call')::int8                      AS calls,
    count(*) FILTER (WHERE e.event_type = 'tool.call' AND e.agent_id IS NOT NULL)::int8 AS subagent_calls,
    coalesce(max(results.failures), 0)::int8                                      AS failures,
    count(*) FILTER (WHERE e.event_type = 'tool.denied')::int8                    AS denied,
    max(results.p50_duration_ms)::float8                                          AS p50_duration_ms,
    max(results.p95_duration_ms)::float8                                          AS p95_duration_ms,
    max(results.total_duration_ms)::int8                                          AS total_duration_ms
FROM e
LEFT JOIN results ON results.tool_name = e.tool_name
GROUP BY e.tool_name
ORDER BY calls DESC, e.tool_name
"#;

/// Per subagent in this session: `[agent_id, agent_type, turn_id,
/// started_at, duration_ms, events, tool_calls, failures, api_requests,
/// tokens_est, tools]`, in start order. Same estimates as [`SUBAGENTS_SQL`]:
/// `duration_ms` is the `subagent.stop` duration, else last − first event;
/// `tokens_est` is the agent's `api.request` usage, else the
/// `SubagentStop` hook's usage block, else NULL (never 0). Subagent rows
/// are hook/transcript only (OTel carries no `agent_id`), so `failures`
/// needs no dedup here. `tools` is the distinct tool names it used.
pub const SESSION_SUBAGENTS_SQL: &str = r#"
WITH e AS (
    SELECT * FROM events
    WHERE session_id = $1 AND ($2::text IS NULL OR user_id = $2::text) AND agent_id IS NOT NULL
)
SELECT
    agent_id,
    max(agent_type)                                                 AS agent_type,
    max(turn_id)                                                    AS turn_id,
    to_char(to_timestamp(min(ts) / 1000.0) AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS started_at,
    coalesce(max(duration_ms) FILTER (WHERE event_type = 'subagent.stop'), max(ts) - min(ts))::int8 AS duration_ms,
    count(*)::int8                                                  AS events,
    count(*) FILTER (WHERE event_type = 'tool.call')::int8          AS tool_calls,
    count(*) FILTER (WHERE success = false)::int8                   AS failures,
    count(*) FILTER (WHERE event_type = 'api.request')::int8        AS api_requests,
    coalesce(
        sum(coalesce(input_tokens, 0) + coalesce(output_tokens, 0))
            FILTER (WHERE event_type = 'api.request'
                      AND (input_tokens IS NOT NULL OR output_tokens IS NOT NULL)),
        max(coalesce(input_tokens, 0) + coalesce(output_tokens, 0))
            FILTER (WHERE event_type = 'subagent.stop'
                      AND (input_tokens IS NOT NULL OR output_tokens IS NOT NULL)))::int8 AS tokens_est,
    string_agg(DISTINCT tool_name, ',' ORDER BY tool_name)         AS tools,
    string_agg(DISTINCT model, ',' ORDER BY model)                 AS models,
    string_agg(DISTINCT effort, ',' ORDER BY effort)               AS efforts
FROM e
GROUP BY agent_id
ORDER BY min(ts), agent_id
"#;

/// Activity over time: `[bucket_ts, events, tool_calls, failures,
/// api_requests, tokens, subagent_events]` per `$3`-millisecond bucket
/// (`bucket_ts` = bucket start, epoch ms; empty buckets are simply absent).
/// The handler picks `$3` from the session's span so a 3-minute session
/// and a 3-day session both fit in a few hundred bars.
pub const SESSION_TIMELINE_SQL: &str = r#"
WITH e AS (
    SELECT * FROM events WHERE session_id = $1 AND ($2::text IS NULL OR user_id = $2::text)
),
tool_results AS (
    SELECT * FROM (
        SELECT e.*, row_number() OVER (
            PARTITION BY session_id, correlation_key
            ORDER BY CASE source WHEN 'otel' THEN 0 WHEN 'hook' THEN 1 ELSE 2 END, ts
        ) AS src_rank
        FROM e
        WHERE event_type = 'tool.result' AND correlation_key IS NOT NULL
    ) d WHERE src_rank = 1
    UNION ALL
    SELECT e.*, 1 AS src_rank FROM e WHERE event_type = 'tool.result' AND correlation_key IS NULL
),
u AS (
    SELECT ts, event_type, success, input_tokens, output_tokens, agent_id FROM e WHERE event_type <> 'tool.result'
    UNION ALL
    SELECT ts, event_type, success, input_tokens, output_tokens, agent_id FROM tool_results
)
SELECT
    ((ts / $3::int8) * $3::int8)::int8                                          AS bucket_ts,
    count(*)::int8                                                              AS events,
    count(*) FILTER (WHERE event_type = 'tool.call')::int8                      AS tool_calls,
    count(*) FILTER (WHERE success = false)::int8                               AS failures,
    count(*) FILTER (WHERE event_type = 'api.request')::int8                    AS api_requests,
    sum(coalesce(input_tokens, 0) + coalesce(output_tokens, 0))::int8           AS tokens,
    count(*) FILTER (WHERE agent_id IS NOT NULL)::int8                          AS subagent_events
FROM u
GROUP BY 1
ORDER BY 1
"#;

/// The event list, chronological, first `$3` rows, metadata columns only
/// (no `tool_input_json`/`prompt_text`/excerpt — the cloud never stores
/// them anyway, architecture.md §5.2): `[ts, event_type, source, tool_name,
/// tool_kind, mcp_server, skill_name, agent_id, agent_type, duration_ms,
/// success, error_type, decision, model, input_tokens, output_tokens,
/// cost_usd, turn_id]`. `tool.result` rows are deduped so a hook/OTel pair
/// shows once. `summary.events` is the uncapped count, so the UI can say
/// "first N of M".
pub const SESSION_EVENTS_SQL: &str = r#"
WITH e AS (
    SELECT * FROM events WHERE session_id = $1 AND ($2::text IS NULL OR user_id = $2::text)
),
tool_results AS (
    SELECT * FROM (
        SELECT e.*, row_number() OVER (
            PARTITION BY session_id, correlation_key
            ORDER BY CASE source WHEN 'otel' THEN 0 WHEN 'hook' THEN 1 ELSE 2 END, ts
        ) AS src_rank
        FROM e
        WHERE event_type = 'tool.result' AND correlation_key IS NOT NULL
    ) d WHERE src_rank = 1
    UNION ALL
    SELECT e.*, 1 AS src_rank FROM e WHERE event_type = 'tool.result' AND correlation_key IS NULL
),
u AS (
    SELECT e.*, 1 AS src_rank FROM e WHERE event_type <> 'tool.result'
    UNION ALL
    SELECT * FROM tool_results
)
SELECT
    ts::int8            AS ts,
    event_type,
    source,
    tool_name,
    tool_kind,
    mcp_server,
    skill_name,
    agent_id,
    agent_type,
    duration_ms::int8   AS duration_ms,
    success,
    error_type,
    decision,
    model,
    input_tokens::int8  AS input_tokens,
    output_tokens::int8 AS output_tokens,
    cost_usd::float8    AS cost_usd,
    turn_id,
    effort
FROM u
ORDER BY ts, event_id
LIMIT $3
"#;

/// Session-level model × effort breakdown for `/web/q/session` (KKM-34):
/// `[model, effort, api_requests, api_errors, subagent_api_requests,
/// input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
/// reasoning_tokens, cost_usd]`. Usage comes from `api.request` rows only,
/// and — like [`SUBAGENTS_SQL`]'s `session_tokens_est` — from **one** source
/// per session: OTel when the session has any OTel `api.request`, else the
/// transcript (`log`), else whatever is left, so a request seen by both
/// OTel and the transcript backfill is never counted twice. `model` is
/// `'unknown'` when the row carried none; `effort` stays NULL when Claude
/// Code reported none (its Haiku helper calls). `api_errors` counts
/// `api.error` rows of the same (model, effort) from every source. Sums are
/// NULL (unknown) when nothing carried usage, never 0.
pub const SESSION_MODELS_SQL: &str = r#"
WITH e AS (
    SELECT * FROM events
    WHERE session_id = $1 AND ($2::text IS NULL OR user_id = $2::text)
      AND event_type IN ('api.request', 'api.error')
),
pref AS (
    SELECT session_id,
           min(CASE source WHEN 'otel' THEN 0 WHEN 'log' THEN 1 ELSE 2 END) AS src_rank
    FROM e WHERE event_type = 'api.request'
    GROUP BY session_id
),
u AS (
    SELECT e.* FROM e
    JOIN pref ON pref.session_id IS NOT DISTINCT FROM e.session_id
    WHERE e.event_type = 'api.request'
      AND (CASE e.source WHEN 'otel' THEN 0 WHEN 'log' THEN 1 ELSE 2 END) = pref.src_rank
),
errs AS (
    SELECT coalesce(model, 'unknown') AS model, effort, count(*)::int8 AS api_errors
    FROM e WHERE event_type = 'api.error'
    GROUP BY 1, 2
),
usage AS (
    SELECT coalesce(model, 'unknown') AS model, effort,
           count(*)::int8 AS api_requests,
           count(*) FILTER (WHERE agent_id IS NOT NULL OR agent_type IS NOT NULL OR query_source LIKE 'agent:%')::int8 AS subagent_api_requests,
           sum(input_tokens)::int8 AS input_tokens,
           sum(output_tokens)::int8 AS output_tokens,
           sum(cache_read_tokens)::int8 AS cache_read_tokens,
           sum(cache_write_tokens)::int8 AS cache_write_tokens,
           sum(reasoning_tokens)::int8 AS reasoning_tokens,
           sum(cost_usd)::float8 AS cost_usd
    FROM u
    GROUP BY 1, 2
)
SELECT coalesce(usage.model, errs.model) AS model,
       coalesce(usage.effort, errs.effort) AS effort,
       coalesce(usage.api_requests, 0)::int8 AS api_requests,
       coalesce(errs.api_errors, 0)::int8 AS api_errors,
       coalesce(usage.subagent_api_requests, 0)::int8 AS subagent_api_requests,
       usage.input_tokens, usage.output_tokens, usage.cache_read_tokens,
       usage.cache_write_tokens, usage.reasoning_tokens, usage.cost_usd
FROM usage
FULL OUTER JOIN errs ON errs.model = usage.model AND errs.effort IS NOT DISTINCT FROM usage.effort
ORDER BY coalesce(usage.input_tokens, 0) + coalesce(usage.output_tokens, 0) DESC, 1, 2
"#;

/// `/web/q/models?days=N` → `[model, effort, api_requests, api_errors,
/// sessions, subagent_api_requests, subagent_tokens, input_tokens,
/// output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens,
/// cost_usd]` (KKM-34), one row per (model, effort) over the window.
/// Org-wide aggregate only — no per-person column, so every role may read
/// it. Same rules as [`SESSION_MODELS_SQL`]: usage from `api.request` rows
/// of one preferred source per session (OTel > transcript > other, so a
/// request seen by both is counted once), `model` `'unknown'` when absent,
/// `effort` NULL when not reported, `api_errors` from every source, sums
/// NULL — never 0 — when nothing carried usage. `subagent_*` are the rows
/// carrying an `agent_id` (transcript rows; OTel has none). `$1` is the
/// `dt >=` lower bound.
/// `subagent_api_requests` / `subagent_tokens`: rows a subagent made. Transcript rows carry
/// `agent_id`; OTel rows carry only `agent.name` (→ `agent_type`) and a `query_source` of the
/// form `agent:builtin:<type>`, so all three are accepted (KKM-15 §honesty note).
pub const MODELS_SQL: &str = r#"
WITH e AS (
    SELECT * FROM events WHERE dt >= $1 AND event_type IN ('api.request', 'api.error')
),
pref AS (
    SELECT session_id,
           min(CASE source WHEN 'otel' THEN 0 WHEN 'log' THEN 1 ELSE 2 END) AS src_rank
    FROM e WHERE event_type = 'api.request'
    GROUP BY session_id
),
u AS (
    SELECT e.* FROM e
    JOIN pref ON pref.session_id IS NOT DISTINCT FROM e.session_id
    WHERE e.event_type = 'api.request'
      AND (CASE e.source WHEN 'otel' THEN 0 WHEN 'log' THEN 1 ELSE 2 END) = pref.src_rank
),
errs AS (
    SELECT coalesce(model, 'unknown') AS model, effort, count(*)::int8 AS api_errors
    FROM e WHERE event_type = 'api.error'
    GROUP BY 1, 2
),
usage AS (
    SELECT coalesce(model, 'unknown') AS model, effort,
           count(*)::int8 AS api_requests,
           count(DISTINCT session_id)::int8 AS sessions,
           count(*) FILTER (WHERE agent_id IS NOT NULL OR agent_type IS NOT NULL OR query_source LIKE 'agent:%')::int8 AS subagent_api_requests,
           sum(coalesce(input_tokens, 0) + coalesce(output_tokens, 0))
               FILTER (WHERE agent_id IS NOT NULL OR agent_type IS NOT NULL OR query_source LIKE 'agent:%')::int8 AS subagent_tokens,
           sum(input_tokens)::int8 AS input_tokens,
           sum(output_tokens)::int8 AS output_tokens,
           sum(cache_read_tokens)::int8 AS cache_read_tokens,
           sum(cache_write_tokens)::int8 AS cache_write_tokens,
           sum(reasoning_tokens)::int8 AS reasoning_tokens,
           sum(cost_usd)::float8 AS cost_usd
    FROM u
    GROUP BY 1, 2
)
SELECT coalesce(usage.model, errs.model) AS model,
       coalesce(usage.effort, errs.effort) AS effort,
       coalesce(usage.api_requests, 0)::int8 AS api_requests,
       coalesce(errs.api_errors, 0)::int8 AS api_errors,
       coalesce(usage.sessions, 0)::int8 AS sessions,
       coalesce(usage.subagent_api_requests, 0)::int8 AS subagent_api_requests,
       usage.subagent_tokens,
       usage.input_tokens, usage.output_tokens, usage.cache_read_tokens,
       usage.cache_write_tokens, usage.reasoning_tokens, usage.cost_usd
FROM usage
FULL OUTER JOIN errs ON errs.model = usage.model AND errs.effort IS NOT DISTINCT FROM usage.effort
ORDER BY coalesce(usage.input_tokens, 0) + coalesce(usage.output_tokens, 0) DESC, 1, 2
"#;

/// `/web/q/models?days=N`'s second section → `[dt, model, input_tokens,
/// output_tokens, cost_usd]` per day and model, from exactly the rows
/// [`MODELS_SQL`] counts (same per-session source preference). `$1` is the
/// `dt >=` lower bound.
pub const MODELS_DAILY_SQL: &str = r#"
WITH e AS (
    SELECT * FROM events WHERE dt >= $1 AND event_type = 'api.request'
),
pref AS (
    SELECT session_id,
           min(CASE source WHEN 'otel' THEN 0 WHEN 'log' THEN 1 ELSE 2 END) AS src_rank
    FROM e
    GROUP BY session_id
),
u AS (
    SELECT e.* FROM e
    JOIN pref ON pref.session_id IS NOT DISTINCT FROM e.session_id
    WHERE (CASE e.source WHEN 'otel' THEN 0 WHEN 'log' THEN 1 ELSE 2 END) = pref.src_rank
)
SELECT dt,
       coalesce(model, 'unknown') AS model,
       sum(input_tokens)::int8 AS input_tokens,
       sum(output_tokens)::int8 AS output_tokens,
       sum(cost_usd)::float8 AS cost_usd
FROM u
GROUP BY 1, 2
ORDER BY 1, 2
"#;
