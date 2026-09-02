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
