//! Postgres ports of the named queries in `crates/cli/src/query_cmd.rs`
//! (DuckDB over local Parquet), scoped instead to `events` under RLS
//! (architecture.md §7.2, §8, §12 Stage 0: "「今日のツール呼び出し・トークン・
//! 失敗」と「MCP 失敗の直後に Bash / Playwright が呼ばれた」が SQL 1 本で取り出せる").
//!
//! All outputs are cast to one of TEXT / INT8 / FLOAT8 / BOOL so the generic
//! row → JSON decoder in query.rs never has to guess a type (`SUM(bigint)`
//! and `percentile_cont` over an integer column both default to NUMERIC in
//! Postgres, which we deliberately avoid).

/// `today`: event/tool_call/failure counts + per-model token & cost totals,
/// over `[dt_from, dt_to]`.
pub const TODAY_SQL: &str = r#"
WITH e AS (SELECT * FROM events WHERE dt BETWEEN $1 AND $2)
SELECT
    (SELECT count(*) FROM e)::int8                                         AS events,
    (SELECT count(*) FROM e WHERE event_type = 'tool.call')::int8          AS tool_calls,
    (SELECT count(*) FROM e WHERE success = false)::int8                   AS failures,
    model,
    sum(input_tokens)::int8   AS input_tokens,
    sum(output_tokens)::int8  AS output_tokens,
    sum(cost_usd)::float8     AS cost_usd
FROM e
GROUP BY model
ORDER BY input_tokens DESC NULLS LAST
"#;

/// `tools`: per-`tool_name` call/failure counts and p50/p95 duration.
pub const TOOLS_SQL: &str = r#"
SELECT
    tool_name,
    count(*) FILTER (WHERE event_type = 'tool.call')::int8                       AS calls,
    count(*) FILTER (WHERE event_type = 'tool.result' AND success = false)::int8 AS failures,
    percentile_cont(0.5)  WITHIN GROUP (ORDER BY duration_ms::float8)
        FILTER (WHERE event_type = 'tool.result')::float8                        AS p50_duration_ms,
    percentile_cont(0.95) WITHIN GROUP (ORDER BY duration_ms::float8)
        FILTER (WHERE event_type = 'tool.result')::float8                        AS p95_duration_ms
FROM events
WHERE tool_name IS NOT NULL AND dt BETWEEN $1 AND $2
GROUP BY tool_name
ORDER BY calls DESC
"#;

/// `mcp`: per-`mcp_server` call/failure counts and distinct session count.
pub const MCP_SQL: &str = r#"
SELECT
    mcp_server,
    count(*) FILTER (WHERE event_type = 'tool.call')::int8                       AS calls,
    count(*) FILTER (WHERE event_type = 'tool.result' AND success = false)::int8 AS failures,
    count(DISTINCT session_id)::int8                                             AS distinct_sessions
FROM events
WHERE mcp_server IS NOT NULL AND dt BETWEEN $1 AND $2
GROUP BY mcp_server
ORDER BY calls DESC
"#;

/// `skills`: per-`skill_name` call/failure counts, distinct sessions, last used dt.
/// `skill_name` comes from the Claude Code hook's `tool_input.skill`
/// (adapter-claude, metadata only — never skill args); rows ingested before
/// migration 0008 simply have NULL and drop out of the filter.
pub const SKILLS_SQL: &str = r#"
SELECT
    skill_name,
    count(*) FILTER (WHERE event_type = 'tool.call')::int8                       AS calls,
    count(*) FILTER (WHERE event_type = 'tool.result' AND success = false)::int8 AS failures,
    count(DISTINCT session_id)::int8                                             AS distinct_sessions,
    max(dt)                                                                      AS last_used_dt
FROM events
WHERE skill_name IS NOT NULL AND dt BETWEEN $1 AND $2
GROUP BY skill_name
ORDER BY calls DESC
"#;

/// `bypass`: `mcp_bypass` simple version — an MCP `tool.result` failure
/// followed, within 5 events of the same session (by `ts` order), by a
/// bash/browser `tool.call`.
pub const BYPASS_SQL: &str = r#"
WITH e AS (
    SELECT *, row_number() OVER (PARTITION BY session_id ORDER BY ts) AS rn
    FROM events
    WHERE dt BETWEEN $1 AND $2
),
mcp_fail AS (
    SELECT session_id, mcp_server, ts AS fail_ts, rn AS fail_rn
    FROM e
    WHERE event_type = 'tool.result' AND success = false AND tool_kind = 'mcp'
),
bypass_call AS (
    SELECT session_id, tool_name, ts AS bypass_ts, rn AS bypass_rn
    FROM e
    WHERE event_type = 'tool.call' AND tool_kind IN ('bash', 'browser')
)
SELECT
    f.session_id,
    f.mcp_server,
    b.tool_name AS following_tool_name,
    f.fail_ts,
    b.bypass_ts
FROM mcp_fail f
JOIN bypass_call b
    ON f.session_id = b.session_id
   AND b.bypass_rn > f.fail_rn
   AND b.bypass_rn <= f.fail_rn + 5
ORDER BY f.session_id, f.fail_ts, b.bypass_ts
"#;

/// `reach`: reach-a-resource mix — dt × session × tool_kind call counts.
pub const REACH_SQL: &str = r#"
SELECT
    dt,
    session_id,
    tool_kind,
    count(*)::int8 AS calls
FROM events
WHERE event_type = 'tool.call' AND tool_kind IN ('mcp', 'bash', 'browser')
  AND dt BETWEEN $1 AND $2
GROUP BY dt, session_id, tool_kind
ORDER BY dt, session_id, tool_kind
"#;

/// `unused-mcp` (cloud variant, architecture.md §7.2 `unused_mcp_server`).
///
/// Unlike the local DuckDB `unused-mcp` (`crates/cli/src/query_cmd.rs`),
/// kikimimi cloud has no access to the client's local `~/.claude/settings.json`
/// / `~/.claude.json`, so it cannot know which MCP servers are
/// *configured*. Instead it treats "observed via `tool.call` in the
/// trailing 30 days" (a fixed window, not `[$1, $2]`) as the proxy for
/// "configured" (`configured` is hardcoded `true` for every row this proxy
/// finds), then reports which of those servers had **zero** `tool.call` in
/// the caller's queried `[$1, $2]` range — that emptiness is the actual
/// signal, so rows failing it are filtered out entirely rather than shown
/// with a nonzero `calls_in_range`.
pub const UNUSED_MCP_SQL: &str = r#"
WITH observed_30d AS (
    SELECT DISTINCT mcp_server
    FROM events
    WHERE event_type = 'tool.call'
      AND mcp_server IS NOT NULL
      AND dt >= to_char(now() - interval '30 days', 'YYYY-MM-DD')
),
last_call AS (
    SELECT mcp_server, max(dt) AS last_called_dt
    FROM events
    WHERE event_type = 'tool.call' AND mcp_server IS NOT NULL
    GROUP BY mcp_server
),
calls_in_range AS (
    SELECT mcp_server, count(*)::int8 AS calls_in_range
    FROM events
    WHERE event_type = 'tool.call' AND mcp_server IS NOT NULL AND dt BETWEEN $1 AND $2
    GROUP BY mcp_server
)
SELECT
    o.mcp_server,
    true                              AS configured,
    coalesce(c.calls_in_range, 0::int8) AS calls_in_range,
    l.last_called_dt
FROM observed_30d o
LEFT JOIN calls_in_range c ON o.mcp_server = c.mcp_server
LEFT JOIN last_call l ON o.mcp_server = l.mcp_server
WHERE coalesce(c.calls_in_range, 0) = 0
ORDER BY l.last_called_dt DESC NULLS LAST, o.mcp_server
"#;

/// `schema-tax` (v0, cloud variant — Postgres port of the local DuckDB query
/// in `crates/cli/src/query_cmd.rs`; keep both in sync). Per-session token
/// breakdown from OTel `api.request` rows over `[$1, $2]`, plus a `TOTAL`
/// rollup row.
///
/// `first_input_tokens` approximates the fixed context (tool schemas +
/// CLAUDE.md + system prompt) paid on *every* request, by taking
/// `input_tokens + cache_read_tokens` of the session's earliest
/// `api.request` in range — that first turn has nothing else cached yet, so
/// (almost) everything read there is fixed context rather than
/// conversation history. `fixed_share_pct` divides that by the session's
/// total `input_tokens + cache_read_tokens` across all its requests in
/// range. Postgres has no `arg_min`, so `first_input_tokens` uses the
/// standard `array_agg(... ORDER BY ts)[1]` idiom instead (DuckDB's
/// `arg_min` on the local side is equivalent).
///
/// HONESTY NOTE (v0 limitation): this is a coarse proxy, not a true
/// schema-vs-CLAUDE.md-vs-prompt breakdown. OTel gives us token *counts*
/// per request, not what's *inside* those tokens — telling "MCP tool
/// schema" apart from "CLAUDE.md" apart from "actual first user prompt"
/// needs transcript-level (per-message role/content) data, which kikimimi does
/// not collect at Stage 0/1 (architecture.md §5.1's `content` column stays
/// opt-in and is never sent to cloud). Treat `fixed_share_pct` as a
/// same-session-turn-1-vs-rest signal, not an exact accounting. See
/// architecture.md §7.2 `schema_tax` (Stage 1).
pub const SCHEMA_TAX_SQL: &str = r#"
WITH e AS (
    SELECT *
    FROM events
    WHERE event_type = 'api.request' AND source = 'otel' AND dt BETWEEN $1 AND $2
),
per_session AS (
    SELECT
        session_id,
        count(*)::int8                                                                        AS api_requests,
        sum(input_tokens)::int8                                                                AS input_tokens,
        sum(cache_read_tokens)::int8                                                            AS cache_read_tokens,
        sum(cache_write_tokens)::int8                                                           AS cache_write_tokens,
        sum(output_tokens)::int8                                                                AS output_tokens,
        (array_agg(coalesce(input_tokens, 0) + coalesce(cache_read_tokens, 0) ORDER BY ts))[1]::int8 AS first_input_tokens
    FROM e
    WHERE session_id IS NOT NULL
    GROUP BY session_id
),
-- UNION'd, then ordered from a plain SELECT below -- ORDER BY can't
-- reference an expression over the pre-UNION columns directly.
combined AS (
    SELECT
        session_id,
        api_requests,
        input_tokens,
        cache_read_tokens,
        cache_write_tokens,
        output_tokens,
        first_input_tokens,
        (100.0 * first_input_tokens / NULLIF(input_tokens + cache_read_tokens, 0))::float8 AS fixed_share_pct
    FROM per_session
    UNION ALL
    SELECT
        'TOTAL' AS session_id,
        sum(api_requests)::int8,
        sum(input_tokens)::int8,
        sum(cache_read_tokens)::int8,
        sum(cache_write_tokens)::int8,
        sum(output_tokens)::int8,
        sum(first_input_tokens)::int8,
        (100.0 * sum(first_input_tokens) / NULLIF(sum(input_tokens + cache_read_tokens), 0))::float8
    FROM per_session
)
SELECT * FROM combined
ORDER BY (session_id = 'TOTAL'), fixed_share_pct DESC NULLS LAST
"#;

pub const NAMED_QUERIES: &[(&str, &str)] = &[
    ("today", TODAY_SQL),
    ("tools", TOOLS_SQL),
    ("mcp", MCP_SQL),
    ("skills", SKILLS_SQL),
    ("bypass", BYPASS_SQL),
    ("reach", REACH_SQL),
    ("unused-mcp", UNUSED_MCP_SQL),
    ("schema-tax", SCHEMA_TAX_SQL),
];
