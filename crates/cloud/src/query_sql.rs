//! Postgres ports of the named queries in `crates/cli/src/query_cmd.rs`
//! (DuckDB over local Parquet), scoped instead to `events` under RLS
//! (architecture.md §7.2, §8, §12 Stage 0: "「今日のツール呼び出し・トークン・
//! 失敗」と「MCP 失敗の直後に Bash / Playwright が呼ばれた」が SQL 1 本で取り出せる").
//!
//! All outputs are cast to one of TEXT / INT8 / FLOAT8 / BOOL so the generic
//! row → JSON decoder in query.rs never has to guess a type (`SUM(bigint)`
//! and `percentile_cont` over an integer column both default to NUMERIC in
//! Postgres, which we deliberately avoid).
//!
//! `tool.result` double-counting (architecture.md §4): `kikimimi init` enables
//! BOTH hooks (`PostToolUse`/`PostToolUseFailure`) and OTel
//! (`claude_code.tool_result`) for Claude Code, so the same `tool_use_id`
//! lands as two `events` rows — `source='hook'` and `source='otel'` — with
//! different `event_id`s (`event_id` hashes in `source`). This is
//! deliberate (kept for gap-visibility and correlation metrics, never
//! deduped at ingest), so every query that counts or measures `tool.result`
//! rows uses the `tool_results` CTE below to collapse the pair back to one
//! logical result before counting: same `(session_id, correlation_key)`,
//! prefer `source='otel'` (reliable success + duration), else
//! `source='hook'`; rows with `correlation_key IS NULL` are never merged
//! (own group each — nothing to correlate against). `tool.call` never
//! duplicates (hook-only), so it stays untouched. See
//! `docs/src/content/docs/queries.md`'s honesty note.

/// `today`: event/tool_call/failure counts + per-model token & cost totals,
/// over `[dt_from, dt_to]`. `failures` applies the `tool_results` dedup
/// (module doc); `events`/`tool_calls` stay raw ingested counts (dedup
/// out of scope for them — see module doc).
pub const TODAY_SQL: &str = r#"
WITH e AS (SELECT * FROM events WHERE dt BETWEEN $1 AND $2),
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
)
SELECT
    (SELECT count(*) FROM e)::int8                                        AS events,
    (SELECT count(*) FROM e WHERE event_type = 'tool.call')::int8         AS tool_calls,
    ((SELECT count(*) FROM e WHERE success = false AND event_type <> 'tool.result')
      + (SELECT count(*) FROM tool_results WHERE success = false))::int8  AS failures,
    model,
    sum(input_tokens)::int8   AS input_tokens,
    sum(output_tokens)::int8  AS output_tokens,
    sum(cost_usd)::float8     AS cost_usd
FROM e
GROUP BY model
ORDER BY input_tokens DESC NULLS LAST
"#;

/// `tools`: per-`tool_name` call/failure counts and p50/p95 duration.
/// `failures`/`p50`/`p95` come from the deduped `tool_results` (module doc,
/// `results` CTE below), `LEFT JOIN`'d back onto the undeduped `e` so a
/// `tool_name` that only ever appears via `tool.result` still shows up
/// (`calls` naturally 0 for it, matching the original single-table query).
pub const TOOLS_SQL: &str = r#"
WITH e AS (
    SELECT * FROM events WHERE tool_name IS NOT NULL AND dt BETWEEN $1 AND $2
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
    count(*) FILTER (WHERE e.event_type = 'tool.call')::int8 AS calls,
    coalesce(max(results.failures), 0)::int8                 AS failures,
    max(results.p50_duration_ms)::float8                     AS p50_duration_ms,
    max(results.p95_duration_ms)::float8                     AS p95_duration_ms
FROM e
LEFT JOIN results ON results.tool_name = e.tool_name
GROUP BY e.tool_name
ORDER BY calls DESC
"#;

/// `mcp`: per-`mcp_server` call/failure counts and distinct session count.
/// Only `failures` needs the dedup (`calls` is hook-only, `distinct_sessions`
/// is already dedup-immune via `DISTINCT` — module doc).
pub const MCP_SQL: &str = r#"
WITH e AS (
    SELECT * FROM events WHERE mcp_server IS NOT NULL AND dt BETWEEN $1 AND $2
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
    count(*) FILTER (WHERE e.event_type = 'tool.call')::int8 AS calls,
    coalesce(max(results.failures), 0)::int8                 AS failures,
    count(DISTINCT e.session_id)::int8                        AS distinct_sessions
FROM e
LEFT JOIN results ON results.mcp_server = e.mcp_server
GROUP BY e.mcp_server
ORDER BY calls DESC
"#;

/// `skills`: per-`skill_name` call/failure counts, distinct sessions, last used dt.
/// `skill_name` comes from the Claude Code hook's `tool_input.skill`
/// (adapter-claude, metadata only — never skill args); rows ingested before
/// migration 0008 simply have NULL and drop out of the filter. Only
/// `failures` needs the dedup (same reasoning as `MCP_SQL`).
pub const SKILLS_SQL: &str = r#"
WITH e AS (
    SELECT * FROM events WHERE skill_name IS NOT NULL AND dt BETWEEN $1 AND $2
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
    count(*) FILTER (WHERE e.event_type = 'tool.call')::int8 AS calls,
    coalesce(max(results.failures), 0)::int8                 AS failures,
    count(DISTINCT e.session_id)::int8                        AS distinct_sessions,
    max(e.dt)                                                  AS last_used_dt
FROM e
LEFT JOIN results ON results.skill_name = e.skill_name
GROUP BY e.skill_name
ORDER BY calls DESC
"#;

/// `bypass`: `mcp_bypass` simple version — an MCP `tool.result` failure
/// followed, within 5 events of the same session (by `ts` order), by a
/// bash/browser `tool.call`. `mcp_fail` sources from the deduped
/// `tool_results` (module doc) so a hook/OTel duplicate pair never turns
/// into two bypass incidents; `e.rn` (whole-session row order) is left as
/// computed over the raw stream — the winning dedup row just keeps using
/// its own original position.
pub const BYPASS_SQL: &str = r#"
WITH e AS (
    SELECT *, row_number() OVER (PARTITION BY session_id ORDER BY ts) AS rn
    FROM events
    WHERE dt BETWEEN $1 AND $2
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
mcp_fail AS (
    SELECT session_id, mcp_server, ts AS fail_ts, rn AS fail_rn
    FROM tool_results
    WHERE success = false AND tool_kind = 'mcp'
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
/// "Configured" prefers a real snapshot: `configured_mcp_servers` (§5.1,
/// populated by `kikimimi agent` on Claude Code `session.start` rows only,
/// `crates/cli/src/mcp_config.rs`) unnested via `jsonb_array_elements_text`
/// over `session.start` rows in the caller's queried `[$1, $2]` range. When
/// NO such row exists in range (older clients that predate this column, or
/// simply no `session.start` in range), this falls back to the previous
/// proxy — "observed via `tool.call` in the trailing 30 days" (a fixed
/// window, not `[$1, $2]`) — exactly as before, so this stays backward
/// compatible with callers pinning the old 4-column shape. Either way,
/// `configured` servers with **zero** `tool.call` in `[$1, $2]` are what
/// gets reported — that emptiness is the actual signal, so rows failing it
/// are filtered out entirely rather than shown with a nonzero
/// `calls_in_range`.
pub const UNUSED_MCP_SQL: &str = r#"
WITH snapshot_configured AS (
    SELECT DISTINCT jsonb_array_elements_text(configured_mcp_servers::jsonb) AS mcp_server
    FROM events
    WHERE event_type = 'session.start'
      AND configured_mcp_servers IS NOT NULL
      AND dt BETWEEN $1 AND $2
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
FROM configured o
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

/// `thrash` (v0, cloud variant — Postgres port of the local DuckDB query in
/// `crates/cli/src/query_cmd.rs`; keep both in sync). Reframing per
/// architecture.md §7.2: not a naive MCP-bypass detector (`bypass`) but
/// thrash (行き詰まり反復 / 拒否後の迂回) — 2026-09 X/Reddit リサーチ.
/// Two signals `UNION ALL`'d into one row per incident, both branches
/// sharing `session_id, kind, tool_name, incidents, first_ts, last_ts`:
///
/// - **A. `repeat_failure`**: same session + same `tool_name`, its
///   `tool.result` rows failing repeatedly with no success ever seen.
///
///   HONESTY NOTE (v0 proxy, same as the local query): not gaps-and-islands
///   (isolating consecutive-failure "islands" with no success inside).
///   Instead: count failures per `(session_id, tool_name)`, keep the pair if
///   it has `>= 3` failures AND no `success = true` `tool.result` ever for
///   that pair. A recover-then-fail-again-3x pattern (success in between)
///   is *not* caught by this proxy — one success anywhere for the pair
///   drops it entirely. A true consecutive-run detector needs a
///   gaps-and-islands rewrite.
///
/// - **B. `deny_detour`**: identical row_number windowing to `BYPASS_SQL`
///   (same session, `ts`-ordered row-number difference <= 5), anchored on
///   `tool.denied` instead of a failed MCP `tool.result` — a denied tool
///   followed within 5 events by a `tool_kind IN ('bash', 'browser')`
///   `tool.call`. `tool_name` reports the *denied* tool (symmetric with A),
///   not the detour tool. `incidents` is always 1 (as with `BYPASS_SQL`, one
///   denial with several in-window detour calls yields several rows).
///
/// `fail_counts`/`success_pairs` source from the deduped `tool_results`
/// (module doc) — otherwise a hook/OTel duplicate pair could push
/// `incidents` past the `>= 3` threshold early, or a duplicated success
/// could mask a real repeat-failure streak.
pub const THRASH_SQL: &str = r#"
WITH e AS (
    SELECT *, row_number() OVER (PARTITION BY session_id ORDER BY ts) AS rn
    FROM events
    WHERE dt BETWEEN $1 AND $2
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
fail_counts AS (
    SELECT session_id, tool_name, count(*)::int8 AS incidents, min(ts) AS first_ts, max(ts) AS last_ts
    FROM tool_results
    WHERE success = false AND tool_name IS NOT NULL
    GROUP BY session_id, tool_name
),
success_pairs AS (
    SELECT DISTINCT session_id, tool_name
    FROM tool_results
    WHERE success = true AND tool_name IS NOT NULL
),
repeat_failure AS (
    SELECT
        f.session_id,
        'repeat_failure' AS kind,
        f.tool_name,
        f.incidents,
        f.first_ts,
        f.last_ts
    FROM fail_counts f
    LEFT JOIN success_pairs s
        ON f.session_id = s.session_id AND f.tool_name = s.tool_name
    WHERE f.incidents >= 3 AND s.session_id IS NULL
),
tool_denied AS (
    SELECT session_id, tool_name, ts AS fail_ts, rn AS fail_rn
    FROM e
    WHERE event_type = 'tool.denied'
),
bypass_call AS (
    SELECT session_id, ts AS bypass_ts, rn AS bypass_rn
    FROM e
    WHERE event_type = 'tool.call' AND tool_kind IN ('bash', 'browser')
),
deny_detour AS (
    SELECT
        f.session_id,
        'deny_detour' AS kind,
        f.tool_name,
        1::int8 AS incidents,
        f.fail_ts AS first_ts,
        b.bypass_ts AS last_ts
    FROM tool_denied f
    JOIN bypass_call b
        ON f.session_id = b.session_id
       AND b.bypass_rn > f.fail_rn
       AND b.bypass_rn <= f.fail_rn + 5
)
SELECT * FROM repeat_failure
UNION ALL
SELECT * FROM deny_detour
ORDER BY session_id, first_ts
"#;

pub const NAMED_QUERIES: &[(&str, &str)] = &[
    ("today", TODAY_SQL),
    ("tools", TOOLS_SQL),
    ("mcp", MCP_SQL),
    ("skills", SKILLS_SQL),
    ("thrash", THRASH_SQL),
    ("bypass", BYPASS_SQL),
    ("reach", REACH_SQL),
    ("unused-mcp", UNUSED_MCP_SQL),
    ("schema-tax", SCHEMA_TAX_SQL),
];
