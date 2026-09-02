//! `kikimimi query [NAME | --sql <SQL>] [--cloud]` — architecture.md §8。既定はローカル
//! Parquet を `duckdb` CLI (PATH 前提) の `read_parquet` で読む
//! (hive_partitioning=false 必須: パスの dt=YYYY-MM-DD を hive 列として DATE 推論すると
//! ファイル内の VARCHAR dt と union_by_name が衝突し、GROUP BY dt を含むクエリが
//! DuckDB INTERNAL Error になる — 2026-09-01 に reach で実測)
//! (「オフライン時はローカル Parquet に DuckDB でフォールバック」の Stage 0 実装)。
//! `--cloud` を付けると代わりに `GET /v1/query/<name>` (cloud API 契約) を叩き、
//! 返ってきた `{"columns":[...],"rows":[[...]]}` を整列済みテーブルとして表示する
//! (`--sql` は cloud 側では受け付けない — 固定クエリのみ)。
//!
//! `tool.result` 二重計上対策 (architecture.md §4): `kikimimi init` は Claude Code
//! に対して hook (`PostToolUse`/`PostToolUseFailure`) と OTel (`claude_code.tool_result`)
//! を両方有効化するため、同じ `tool_use_id` について `source='hook'` と
//! `source='otel'` の 2 行が別 `event_id` (schema::event_id は `source` を鍵に含む)
//! で両方 events に残る (意図的 — 欠損可視化と相関メトリクスのため ingest 時には
//! 消さない, architecture.md §4)。これを集計時に 1 件として数えるのが
//! `tool_results` CTE (下の各クエリで繰り返し使う共通パターン): 同一
//! `(session_id, correlation_key)` の `tool.result` 行のうち `source='otel'` を
//! 優先し (欠けていれば `source='hook'`)、`correlation_key IS NULL` の行は
//! (相関先が無い = 統合しようがないので) そのまま素通しする。`tool.call` は
//! hook からしか出ないため対象外。詳細は `docs/src/content/docs/queries.md`
//! の honesty note。

use std::io::Write as _;
use std::time::Duration;

use anyhow::Context;
use serde::Deserialize;
use serde_json::Value;

/// `today`: 今日 (dt=today) のイベント数・tool call 数・失敗数・モデル別トークン。
/// `failures` は module doc の `tool_results` dedup を適用 (`tool.result` の
/// hook/OTel 二重行を 1 件に畳んでから数える) — `events`/`tool_calls` は生の
/// 取り込み行数のまま (dedup 対象外、architecture.md §4 の欠損可視化のため)。
const TODAY_SQL: &str = r#"
WITH e AS (SELECT * FROM read_parquet('{glob}', union_by_name=true, hive_partitioning=false) WHERE dt = '{today}'),
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
    (SELECT count(*) FROM e)                                        AS events,
    (SELECT count(*) FROM e WHERE event_type = 'tool.call')         AS tool_calls,
    (SELECT count(*) FROM e WHERE success = false AND event_type <> 'tool.result')
      + (SELECT count(*) FROM tool_results WHERE success = false)   AS failures,
    model,
    sum(input_tokens)  AS input_tokens,
    sum(output_tokens) AS output_tokens,
    sum(cost_usd)      AS cost_usd
FROM e
GROUP BY model
ORDER BY input_tokens DESC NULLS LAST;
"#;

/// `tools`: tool_name 別の呼び出し数・失敗数・p50/p95 所要時間。
/// `failures`/`p50`/`p95` は `tool_results` (module doc) 経由の dedup 済み集計を
/// `results` に切り出し、`calls`/tool_name の集合自体は元の `e` (undeduped) の
/// まま `LEFT JOIN` で被せる — tool.call しか無い tool_name も tool.result しか
/// 無い tool_name も両方 `e` の GROUP BY に出るので既存の行集合は変わらない。
const TOOLS_SQL: &str = r#"
WITH e AS (
    SELECT * FROM read_parquet('{glob}', union_by_name=true, hive_partitioning=false)
    WHERE tool_name IS NOT NULL
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
        approx_quantile(duration_ms, 0.5)        AS p50_duration_ms,
        approx_quantile(duration_ms, 0.95)       AS p95_duration_ms
    FROM tool_results
    GROUP BY tool_name
)
SELECT
    e.tool_name,
    count(*) FILTER (WHERE e.event_type = 'tool.call') AS calls,
    coalesce(max(results.failures), 0)                 AS failures,
    max(results.p50_duration_ms)                       AS p50_duration_ms,
    max(results.p95_duration_ms)                       AS p95_duration_ms
FROM e
LEFT JOIN results ON results.tool_name = e.tool_name
GROUP BY e.tool_name
ORDER BY calls DESC;
"#;

/// `mcp`: mcp_server 別の呼び出し数・失敗数・distinct session 数。
/// `failures` のみ dedup 対象 (`calls`/`distinct_sessions` は hook 単独由来か
/// `DISTINCT` で元々重複耐性がある — module doc参照)。
const MCP_SQL: &str = r#"
WITH e AS (
    SELECT * FROM read_parquet('{glob}', union_by_name=true, hive_partitioning=false)
    WHERE mcp_server IS NOT NULL
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
    count(*) FILTER (WHERE e.event_type = 'tool.call') AS calls,
    coalesce(max(results.failures), 0)                 AS failures,
    count(DISTINCT e.session_id)                        AS distinct_sessions
FROM e
LEFT JOIN results ON results.mcp_server = e.mcp_server
GROUP BY e.mcp_server
ORDER BY calls DESC;
"#;

/// `skills`: skill_name 別の呼び出し数・失敗数・distinct session 数・最終使用日。
/// skill_name は Claude Code hook の tool_input.skill 由来 (adapter-claude)。
/// 列追加前の古い parquet は union_by_name=true, hive_partitioning=false が NULL 埋めするので壊れない。
/// `failures` のみ dedup 対象 (MCP_SQL と同じ理由)。
const SKILLS_SQL: &str = r#"
WITH e AS (
    SELECT * FROM read_parquet('{glob}', union_by_name=true, hive_partitioning=false)
    WHERE skill_name IS NOT NULL
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
    count(*) FILTER (WHERE e.event_type = 'tool.call') AS calls,
    coalesce(max(results.failures), 0)                 AS failures,
    count(DISTINCT e.session_id)                        AS distinct_sessions,
    max(e.dt)                                            AS last_used_dt
FROM e
LEFT JOIN results ON results.skill_name = e.skill_name
GROUP BY e.skill_name
ORDER BY calls DESC;
"#;

/// `bypass`: mcp_bypass の簡易版 SQL 検証 (architecture.md §7.2, §12 Stage 0 の主目的)。
/// 同一セッションで MCP の失敗の後、5 イベント以内 (ts 順の行番号差) に
/// bash/browser の tool.call が続くケースを拾う。
/// `mcp_fail` は `tool_results` (module doc) 経由の dedup 済み `tool.result`
/// から拾う — hook/OTel 二重行がそのまま 2 件の bypass インシデントに化けない
/// ようにするため。`e.rn` (全イベント通しの行番号) はそのまま使う: dedup で
/// 勝った方 (OTel 優先) の元の位置を使えば良いので、`rn` 自体の再採番は不要。
const BYPASS_SQL: &str = r#"
WITH e AS (
    SELECT *, row_number() OVER (PARTITION BY session_id ORDER BY ts) AS rn
    FROM read_parquet('{glob}', union_by_name=true, hive_partitioning=false)
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
ORDER BY f.session_id, f.fail_ts, b.bypass_ts;
"#;

/// `reach`: リソースへの到達手段比率の簡易版。dt × session × tool_kind の呼び出し数。
const REACH_SQL: &str = r#"
SELECT
    dt,
    session_id,
    tool_kind,
    count(*) AS calls
FROM read_parquet('{glob}', union_by_name=true, hive_partitioning=false)
WHERE event_type = 'tool.call' AND tool_kind IN ('mcp', 'bash', 'browser')
GROUP BY dt, session_id, tool_kind
ORDER BY dt, session_id, tool_kind;
"#;

/// `unused-mcp`: MCP servers configured in `~/.claude/settings.json` +
/// `~/.claude.json` ("mcpServers" keys, both read defensively — see
/// [`configured_mcp_servers`]) vs. `mcp_server` values actually observed via
/// `tool.call` events (architecture.md §7.2 `unused_mcp_server`). The
/// `{mcp_configured}` placeholder is substituted with a DuckDB `VARCHAR[]`
/// literal by [`render_template`], not by the generic `{glob}`/`{today}`
/// substitution. Servers configured but never called sort first — this is
/// the whole point of the query (context you're paying for on every request
/// and never using).
const UNUSED_MCP_SQL: &str = r#"
WITH e AS (
    SELECT mcp_server, dt
    FROM read_parquet('{glob}', union_by_name=true, hive_partitioning=false)
    WHERE event_type = 'tool.call' AND mcp_server IS NOT NULL
),
calls AS (
    SELECT mcp_server, count(*) AS calls_in_range, max(dt) AS last_called_dt
    FROM e
    GROUP BY mcp_server
),
configured AS (
    SELECT DISTINCT unnest({mcp_configured}) AS mcp_server
)
SELECT
    coalesce(c.mcp_server, calls.mcp_server) AS mcp_server,
    (c.mcp_server IS NOT NULL)               AS configured,
    coalesce(calls.calls_in_range, 0)        AS calls_in_range,
    calls.last_called_dt                     AS last_called_dt
FROM configured c
FULL OUTER JOIN calls ON c.mcp_server = calls.mcp_server
ORDER BY
    (c.mcp_server IS NOT NULL AND coalesce(calls.calls_in_range, 0) = 0) DESC,
    (c.mcp_server IS NOT NULL) DESC,
    coalesce(calls.calls_in_range, 0) ASC,
    mcp_server;
"#;

/// `schema-tax` (v0, architecture.md §7.2 `schema_tax`): per-session token
/// breakdown from OTel `api.request` rows, plus a `TOTAL` rollup row.
///
/// `first_input_tokens` approximates the fixed context (tool schemas +
/// CLAUDE.md + system prompt) paid on *every* request, by taking
/// `input_tokens + cache_read_tokens` of the session's earliest
/// `api.request` — that first turn has nothing else cached yet, so
/// (almost) everything read there is fixed context rather than
/// conversation history. `fixed_share_pct` divides that by the session's
/// total `input_tokens + cache_read_tokens` across all its requests.
///
/// HONESTY NOTE (v0 limitation): this is a coarse proxy, not a true
/// schema-vs-CLAUDE.md-vs-prompt breakdown. OTel gives us token *counts*
/// per request, not what's *inside* those tokens — telling "MCP tool
/// schema" apart from "CLAUDE.md" apart from "actual first user prompt"
/// needs transcript-level (per-message role/content) data, which kikimimi does
/// not collect at Stage 0/1 (architecture.md §5.1's `content` column stays
/// opt-in and is never sent to cloud). A future version could read that
/// from the local transcript JSONL (architecture.md §4 log tailer,
/// "補助のみ") to get the real split; until then, treat `fixed_share_pct`
/// as a same-session-turn-1-vs-rest signal, not an exact accounting.
const SCHEMA_TAX_SQL: &str = r#"
-- schema-tax v0: fixed-context proxy per session, from OTel api.request rows.
-- first_input_tokens = input_tokens + cache_read_tokens of the session's
-- EARLIEST api.request (by ts) -- turn 1 has no prior assistant output to
-- cache, so what's read there is (almost) all fixed context: tool schemas +
-- CLAUDE.md + system prompt, not conversation history.
-- LIMITATION: this is a proxy, not a true schema/CLAUDE.md/prompt
-- breakdown -- that needs transcript-level (per-message) data kikimimi does not
-- collect at Stage 0/1. Treat fixed_share_pct as a signal, not an exact
-- accounting. See architecture.md §7.2 `schema_tax` (Stage 1).
WITH e AS (
    SELECT *
    FROM read_parquet('{glob}', union_by_name=true, hive_partitioning=false)
    WHERE event_type = 'api.request' AND source = 'otel'
),
per_session AS (
    SELECT
        session_id,
        count(*)                                                         AS api_requests,
        sum(input_tokens)                                                AS input_tokens,
        sum(cache_read_tokens)                                           AS cache_read_tokens,
        sum(cache_write_tokens)                                          AS cache_write_tokens,
        sum(output_tokens)                                               AS output_tokens,
        arg_min(coalesce(input_tokens, 0) + coalesce(cache_read_tokens, 0), ts) AS first_input_tokens
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
        100.0 * first_input_tokens / NULLIF(input_tokens + cache_read_tokens, 0) AS fixed_share_pct
    FROM per_session
    UNION ALL
    SELECT
        'TOTAL' AS session_id,
        sum(api_requests),
        sum(input_tokens),
        sum(cache_read_tokens),
        sum(cache_write_tokens),
        sum(output_tokens),
        sum(first_input_tokens),
        100.0 * sum(first_input_tokens) / NULLIF(sum(input_tokens + cache_read_tokens), 0)
    FROM per_session
)
SELECT * FROM combined
ORDER BY (session_id = 'TOTAL'), fixed_share_pct DESC NULLS LAST;
"#;

/// `thrash` (v0, architecture.md §7.2 の再定義): 素朴な MCP 回避検知
/// (`bypass`) ではなく thrash (行き詰まり反復 / 拒否後の迂回) を対象にする
/// (2026-09 X/Reddit リサーチ)。2 つのシグナルを `UNION ALL` で 1 本にまとめる
/// (1 インシデント = 1 行、列は両方とも
/// `session_id, kind, tool_name, incidents, first_ts, last_ts`):
///
/// - **A. `repeat_failure`** (行き詰まり反復): 同一セッション・同一
///   `tool_name` で `tool.result` の失敗が繰り返され、その `tool_name` に
///   一度も成功 (`success = true`) が無いケース。
///
///   HONESTY NOTE (v0 proxy): これは gaps-and-islands (連続する失敗の
///   「島」を厳密に切り出し、島の中に成功が挟まらないことを見る) ではなく、
///   タスクで許容されている簡易 proxy — `(session_id, tool_name)` 単位で
///   失敗回数を数え、その pair に成功 `tool.result` が **一度もなければ**
///   `failures >= 3` の行を出す。よって「3 回失敗 → 1 回成功 → また 3 回
///   失敗」のような、成功を挟んで立て直った反復は拾えない (成功が 1 回でも
///   あればその pair 全体が除外される)。真の連続失敗検知が要る場合は
///   gaps-and-islands 版への差し替えが必要。
///
/// - **B. `deny_detour`** (拒否後の迂回): `BYPASS_SQL` と全く同じ
///   row_number ウィンドウパターン (同一セッション、`ts` 順の行番号差が
///   5 以内) だが、起点を「MCP の失敗 `tool.result`」ではなく
///   「`tool.denied`」に変える。拒否された tool の直後 5 イベント以内に
///   `tool_kind IN ('bash', 'browser')` の `tool.call` が続くケースを拾う。
///   `tool_name` には (A との対称性のため) 迂回先ではなく **拒否された方の
///   tool_name** を出す。`incidents` は常に 1 (1 拒否 + 1 迂回 = 1
///   インシデント; `BYPASS_SQL` 同様、1 回の拒否に複数の迂回 `tool.call` が
///   窓内にあれば複数行になる)。
///
/// `fail_counts`/`success_pairs` は `tool_results` (module doc) の dedup 済み
/// `tool.result` から拾う — hook/OTel 二重行のせいで `incidents >= 3` の閾値に
/// 実際より早く達したり、成功が二重に見えたりしないようにするため。
const THRASH_SQL: &str = r#"
WITH e AS (
    SELECT *, row_number() OVER (PARTITION BY session_id ORDER BY ts) AS rn
    FROM read_parquet('{glob}', union_by_name=true, hive_partitioning=false)
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
    SELECT session_id, tool_name, count(*) AS incidents, min(ts) AS first_ts, max(ts) AS last_ts
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
        1 AS incidents,
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
ORDER BY session_id, first_ts;
"#;

const NAMED_QUERIES: &[(&str, &str)] = &[
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

pub struct QueryArgs {
    pub name: Option<String>,
    pub sql: Option<String>,
    pub show_sql: bool,
    pub cloud: bool,
    /// `--cloud` only. See [`effective_cloud_range`].
    pub dt_from: Option<String>,
    pub dt_to: Option<String>,
}

pub fn run(args: QueryArgs) -> anyhow::Result<()> {
    if args.cloud {
        return run_cloud(&args);
    }

    let sql = match resolve_sql(&args)? {
        Some(sql) => sql,
        None => {
            print_usage();
            return Ok(());
        }
    };

    if args.show_sql {
        eprintln!("-- SQL --\n{}\n---------", sql.trim());
    }

    run_duckdb(&sql)
}

fn resolve_sql(args: &QueryArgs) -> anyhow::Result<Option<String>> {
    match (&args.name, &args.sql) {
        (Some(_), Some(_)) => anyhow::bail!("pass either NAME or --sql, not both"),
        (None, Some(sql)) => Ok(Some(sql.clone())),
        (Some(name), None) => {
            let template = NAMED_QUERIES
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, t)| *t)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "unknown named query {name:?}. Available: {}",
                        available_names()
                    )
                })?;
            Ok(Some(render_template(template)))
        }
        (None, None) => Ok(None),
    }
}

fn print_usage() {
    println!("usage: kikimimi query <NAME> | --sql <SQL> [--show-sql]");
    println!("available named queries: {}", available_names());
}

fn available_names() -> String {
    NAMED_QUERIES
        .iter()
        .map(|(n, _)| *n)
        .collect::<Vec<_>>()
        .join(", ")
}

/// `{glob}` → `<data_dir>/dt=*/*.parquet` (single-quote をエスケープ)、
/// `{today}` → 今日の日付 (`kikimimi_schema::dt_of` と同じ書式)、
/// `{mcp_configured}` → [`configured_mcp_servers`] を DuckDB の `VARCHAR[]`
/// リテラルにしたもの (`unused-mcp` 専用。他のクエリには現れないので、
/// プレースホルダが無ければ設定ファイルの読み取り自体を省く)。
fn render_template(template: &str) -> String {
    let glob_escaped = kikimimi_schema::paths::events_glob_sql();
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let mut rendered = template
        .replace("{glob}", &glob_escaped)
        .replace("{today}", &today);
    if rendered.contains("{mcp_configured}") {
        let list = mcp_configured_sql_list(&configured_mcp_servers());
        rendered = rendered.replace("{mcp_configured}", &list);
    }
    rendered
}

/// `~/.claude.json` の場所。テスト用に `KIKIMIMI_CLAUDE_JSON_PATH` で上書きできる
/// (本番の既定は常に `$HOME/.claude.json`)。`claude_settings::settings_path()`
/// が `~/.claude/settings.json` 側の同じパターンを持つ。
fn claude_json_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("KIKIMIMI_CLAUDE_JSON_PATH") {
        return std::path::PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(home).join(".claude.json")
}

/// `~/.claude/settings.json` + `~/.claude.json` から設定済み MCP サーバー名を
/// 集める (`unused-mcp` の "configured" 側)。両ファイルとも **defensive** に
/// 読む: 存在しない・読めない・JSON として壊れている場合はそのファイルから
/// 単に何も得ない (エラーにしない) — デーモンが同時に書き込み中でも
/// `kikimimi query unused-mcp` を壊さないため。
fn configured_mcp_servers() -> Vec<String> {
    let mut servers = std::collections::BTreeSet::new();
    collect_mcp_servers_from_file(&crate::claude_settings::settings_path(), &mut servers);
    collect_mcp_servers_from_file(&claude_json_path(), &mut servers);
    servers.into_iter().collect()
}

fn collect_mcp_servers_from_file(
    path: &std::path::Path,
    out: &mut std::collections::BTreeSet<String>,
) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(json) = serde_json::from_str::<Value>(&content) else {
        return;
    };
    collect_mcp_server_keys(&json, out);
}

/// トップレベルの `mcpServers` オブジェクトのキーに加えて、`~/.claude.json`
/// の実際のレイアウトである `projects.<path>.mcpServers` のキーも集める
/// (プロジェクトごとの MCP 設定はここにしか出てこない — 2026-08-31 時点の
/// 実データで確認済み)。
fn collect_mcp_server_keys(json: &Value, out: &mut std::collections::BTreeSet<String>) {
    if let Some(obj) = json.get("mcpServers").and_then(Value::as_object) {
        out.extend(obj.keys().cloned());
    }
    if let Some(projects) = json.get("projects").and_then(Value::as_object) {
        for project in projects.values() {
            if let Some(obj) = project.get("mcpServers").and_then(Value::as_object) {
                out.extend(obj.keys().cloned());
            }
        }
    }
}

/// `servers` を DuckDB の `VARCHAR[]` リテラルにレンダリングする
/// (例: `['a','b']::VARCHAR[]`、空なら `[]::VARCHAR[]`)。単一引用符は
/// 2 個に倍化してエスケープする (`{glob}` と同じ方式)。
fn mcp_configured_sql_list(servers: &[String]) -> String {
    let items: Vec<String> = servers
        .iter()
        .map(|s| format!("'{}'", s.replace('\'', "''")))
        .collect();
    format!("[{}]::VARCHAR[]", items.join(", "))
}

fn run_duckdb(sql: &str) -> anyhow::Result<()> {
    let result = std::process::Command::new("duckdb")
        .arg("-c")
        .arg(sql)
        .output();

    let output = match result {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!(
                "the `duckdb` CLI was not found in PATH. Install it from https://duckdb.org \
                 (e.g. `brew install duckdb` or download a release binary) to use `kikimimi query`."
            );
        }
        Err(e) => return Err(anyhow::Error::new(e).context("running duckdb")),
    };

    std::io::stdout().write_all(&output.stdout).ok();
    std::io::stderr().write_all(&output.stderr).ok();

    if !output.status.success() {
        anyhow::bail!("duckdb exited with {}", output.status);
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct CloudQueryResponse {
    columns: Vec<String>,
    rows: Vec<Vec<Value>>,
}

fn run_cloud(args: &QueryArgs) -> anyhow::Result<()> {
    if args.sql.is_some() {
        anyhow::bail!("--cloud only runs named queries; --sql is a local (DuckDB) option");
    }
    let name = args
        .name
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("usage: kikimimi query --cloud <NAME>"))?;

    let cfg = crate::config::KikimimiConfig::load();
    let cloud = cfg
        .cloud
        .ok_or_else(|| anyhow::anyhow!("not logged in; run `kikimimi login` first"))?;

    let (dt_from, dt_to) =
        effective_cloud_range(name, args.dt_from.as_deref(), args.dt_to.as_deref());
    let resp = fetch_cloud_query(
        &cloud.endpoint,
        &cloud.token,
        name,
        dt_from.as_deref(),
        dt_to.as_deref(),
    )?;
    print!("{}", render_table(&resp.columns, &resp.rows));
    Ok(())
}

/// Spec review: `kikimimi query today --cloud` sent no `dt_from`/`dt_to` at all,
/// so the cloud port (which defaults to an unbounded `[0001-01-01,
/// 9999-12-31]` range when the caller omits them — `crates/cloud/src/
/// query.rs`) returned *all-time* totals under the "today" label, unlike the
/// local DuckDB `today` query's hardcoded `dt = '{today}'`. Explicit
/// `--from`/`--to` always win; otherwise `today` defaults to today's date
/// (matching the local query's semantics — architecture.md §12 Stage 0's
/// "ported SQL equivalence" requirement). Every other named query is
/// unbounded on both paths by design (they don't filter by date locally
/// either), so they keep defaulting to no range.
fn effective_cloud_range(
    name: &str,
    dt_from: Option<&str>,
    dt_to: Option<&str>,
) -> (Option<String>, Option<String>) {
    if dt_from.is_some() || dt_to.is_some() {
        return (dt_from.map(String::from), dt_to.map(String::from));
    }
    if name == "today" {
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        return (Some(today.clone()), Some(today));
    }
    (None, None)
}

fn cloud_query_url(
    endpoint: &str,
    name: &str,
    dt_from: Option<&str>,
    dt_to: Option<&str>,
) -> String {
    let endpoint = endpoint.trim_end_matches('/');
    let mut params = Vec::new();
    if let Some(from) = dt_from {
        params.push(format!("dt_from={from}"));
    }
    if let Some(to) = dt_to {
        params.push(format!("dt_to={to}"));
    }
    if params.is_empty() {
        format!("{endpoint}/v1/query/{name}")
    } else {
        format!("{endpoint}/v1/query/{name}?{}", params.join("&"))
    }
}

fn fetch_cloud_query(
    endpoint: &str,
    token: &str,
    name: &str,
    dt_from: Option<&str>,
    dt_to: Option<&str>,
) -> anyhow::Result<CloudQueryResponse> {
    let url = cloud_query_url(endpoint, name, dt_from, dt_to);
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("building HTTP client")?;
    let resp = client
        .get(&url)
        .bearer_auth(token)
        .send()
        .with_context(|| format!("GET /v1/query/{name}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        anyhow::bail!("GET /v1/query/{name} returned {status}: {body}");
    }
    resp.json()
        .with_context(|| format!("parsing /v1/query/{name} response"))
}

/// `columns`/`rows` を空白 2 個区切りで揃えたテーブルにレンダリングする
/// (末尾に改行つき)。`null` は空文字として表示する。
fn render_table(columns: &[String], rows: &[Vec<Value>]) -> String {
    let cell_str = |v: &Value| -> String {
        match v {
            Value::Null => String::new(),
            Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    };
    let str_rows: Vec<Vec<String>> = rows
        .iter()
        .map(|r| r.iter().map(cell_str).collect())
        .collect();

    let mut widths: Vec<usize> = columns.iter().map(|c| c.chars().count()).collect();
    for row in &str_rows {
        for (i, cell) in row.iter().enumerate() {
            if let Some(w) = widths.get_mut(i) {
                *w = (*w).max(cell.chars().count());
            }
        }
    }

    let width_of = |widths: &[usize], i: usize| widths.get(i).copied().unwrap_or(0);
    let render_row = |widths: &[usize], cells: &[String]| -> String {
        cells
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{c:<width$}", width = width_of(widths, i)))
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_string()
    };

    let mut out = String::new();
    out.push_str(&render_row(&widths, columns));
    out.push('\n');
    let sep: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
    out.push_str(sep.join("  ").trim_end());
    out.push('\n');
    for row in &str_rows {
        out.push_str(&render_row(&widths, row));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_sql_rejects_both_name_and_sql() {
        let args = QueryArgs {
            name: Some("tools".into()),
            sql: Some("select 1".into()),
            show_sql: false,
            cloud: false,
            dt_from: None,
            dt_to: None,
        };
        assert!(resolve_sql(&args).is_err());
    }

    #[test]
    fn resolve_sql_passes_through_raw_sql_unmodified() {
        let args = QueryArgs {
            name: None,
            sql: Some("select 1".into()),
            show_sql: false,
            cloud: false,
            dt_from: None,
            dt_to: None,
        };
        assert_eq!(resolve_sql(&args).unwrap(), Some("select 1".to_string()));
    }

    #[test]
    fn resolve_sql_rejects_unknown_name() {
        let args = QueryArgs {
            name: Some("nope".into()),
            sql: None,
            show_sql: false,
            cloud: false,
            dt_from: None,
            dt_to: None,
        };
        assert!(resolve_sql(&args).is_err());
    }

    #[test]
    fn resolve_sql_none_none_yields_none() {
        let args = QueryArgs {
            name: None,
            sql: None,
            show_sql: false,
            cloud: false,
            dt_from: None,
            dt_to: None,
        };
        assert_eq!(resolve_sql(&args).unwrap(), None);
    }

    #[test]
    fn all_named_queries_substitute_placeholders() {
        for (name, template) in NAMED_QUERIES {
            let rendered = render_template(template);
            assert!(
                !rendered.contains("{glob}"),
                "{name} left {{glob}} unsubstituted"
            );
            assert!(
                !rendered.contains("{today}"),
                "{name} left {{today}} unsubstituted"
            );
            assert!(
                !rendered.contains("{mcp_configured}"),
                "{name} left {{mcp_configured}} unsubstituted"
            );
            assert!(
                rendered.contains("read_parquet"),
                "{name} must read parquet"
            );
        }
    }

    #[test]
    fn bypass_query_uses_row_number_window_within_five_events() {
        assert!(BYPASS_SQL.contains("bypass_rn <= f.fail_rn + 5"));
        assert!(BYPASS_SQL.contains("tool_kind = 'mcp'"));
        assert!(BYPASS_SQL.contains("tool_kind IN ('bash', 'browser')"));
    }

    #[test]
    fn thrash_query_covers_repeat_failure_and_deny_detour_signals() {
        assert!(THRASH_SQL.contains("'repeat_failure' AS kind"));
        assert!(THRASH_SQL.contains("'deny_detour' AS kind"));
        // A: repeat_failure v0 proxy -- failures >= 3 with no success ever seen.
        assert!(THRASH_SQL.contains("WHERE f.incidents >= 3 AND s.session_id IS NULL"));
        // B: deny_detour mirrors BYPASS_SQL's row_number window, anchored on tool.denied.
        assert!(THRASH_SQL.contains("event_type = 'tool.denied'"));
        assert!(THRASH_SQL.contains("bypass_rn <= f.fail_rn + 5"));
        assert!(THRASH_SQL.contains("tool_kind IN ('bash', 'browser')"));
    }

    #[test]
    fn all_dedup_touched_queries_reference_the_tool_results_cte() {
        // Every query the task lists as needing the hook/OTel `tool.result`
        // dedup must actually embed the shared `tool_results AS (...)` CTE --
        // a cheap regression pin against someone editing one back out later.
        for (name, sql) in [
            ("today", TODAY_SQL),
            ("tools", TOOLS_SQL),
            ("mcp", MCP_SQL),
            ("skills", SKILLS_SQL),
            ("bypass", BYPASS_SQL),
            ("thrash", THRASH_SQL),
        ] {
            assert!(
                sql.contains("tool_results AS (")
                    && sql.contains("PARTITION BY session_id, correlation_key")
                    && sql.contains("CASE source WHEN 'otel' THEN 0 WHEN 'hook' THEN 1 ELSE 2 END"),
                "{name} must use the tool_results dedup CTE"
            );
        }
        // reach/unused-mcp/schema-tax never touch tool.result -- must stay untouched.
        for (name, sql) in [
            ("reach", REACH_SQL),
            ("unused-mcp", UNUSED_MCP_SQL),
            ("schema-tax", SCHEMA_TAX_SQL),
        ] {
            assert!(
                !sql.contains("tool_results AS ("),
                "{name} doesn't touch tool.result and shouldn't need the dedup CTE"
            );
        }
    }

    /// Runs `sql` through the real `duckdb` CLI (`-json`), returning the
    /// parsed rows, or `None` (after printing a skip note) if `duckdb` isn't
    /// on PATH -- same "skip gracefully" convention as `web_query.rs`'s
    /// `duckdb_available()`-gated tests.
    fn run_duckdb_json_for_test(sql: &str) -> Option<Vec<Value>> {
        let output = match std::process::Command::new("duckdb")
            .args(["-json", "-c", sql])
            .output()
        {
            Ok(o) => o,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("skipping: duckdb CLI not installed");
                return None;
            }
            Err(e) => panic!("running duckdb: {e}"),
        };
        assert!(
            output.status.success(),
            "duckdb query failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Some(serde_json::from_slice(&output.stdout).expect("parsing duckdb -json output"))
    }

    /// Shells out to `duckdb` itself to build a tiny Parquet fixture at
    /// `<data_dir>/dt=<dt>/events.parquet`: runs `setup_sql` (a `CREATE TABLE`
    /// plus `INSERT`s) and then `COPY`s that table out as Parquet. Naming only
    /// the columns the test needs (not the full `kikimimi_schema::COLUMNS` list)
    /// is fine: `union_by_name=true` just means "this file's own columns", and
    /// the queries under test never touch anything outside `setup_sql`'s column
    /// list.
    fn write_duckdb_fixture(data_dir: &std::path::Path, dt: &str, setup_sql: &str) {
        let part_dir = data_dir.join(format!("dt={dt}"));
        std::fs::create_dir_all(&part_dir).expect("mkdir dt= partition");
        let file = part_dir.join("events.parquet");
        let script = format!(
            "{setup_sql}\nCOPY t TO '{}' (FORMAT PARQUET);",
            file.display().to_string().replace('\'', "''")
        );
        let output = std::process::Command::new("duckdb")
            .args(["-c", &script])
            .output()
            .expect("run duckdb to build the fixture (checked available via run_duckdb_json_for_test first)");
        assert!(
            output.status.success(),
            "building the duckdb fixture failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// `tools`: a hook row and an OTel row for the exact same
    /// `(session_id, correlation_key)`, both `success = false`, must collapse
    /// to ONE failure -- not two (the double-counting bug this task fixes).
    /// The OTel row alone carries `duration_ms`, so `p50_duration_ms`
    /// reflecting it (not NULL) additionally confirms the OTel row -- not
    /// just "some" row -- won the dedup.
    #[test]
    #[serial_test::serial]
    fn tools_query_dedups_a_hook_otel_tool_result_pair_into_one_failure() {
        // Probe first, before touching KIKIMIMI_DIR / the filesystem at all.
        if run_duckdb_json_for_test("SELECT 1;").is_none() {
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());
        let data_dir = dir.path().join("data").join("events");
        write_duckdb_fixture(
            &data_dir,
            "2026-09-02",
            r#"
CREATE TABLE t (
    ts BIGINT, dt TEXT, session_id TEXT, correlation_key TEXT, source TEXT,
    event_type TEXT, tool_name TEXT, tool_kind TEXT, mcp_server TEXT,
    success BOOLEAN, duration_ms BIGINT
);
INSERT INTO t (ts, dt, session_id, correlation_key, source, event_type, tool_name, tool_kind, mcp_server, success)
    VALUES (1000, '2026-09-02', 'sess-1', 'tu-1', 'hook', 'tool.result', 'mcp__gh__search', 'mcp', 'gh', false);
INSERT INTO t (ts, dt, session_id, correlation_key, source, event_type, tool_name, tool_kind, mcp_server, success, duration_ms)
    VALUES (1100, '2026-09-02', 'sess-1', 'tu-1', 'otel', 'tool.result', 'mcp__gh__search', 'mcp', 'gh', false, 250);
"#,
        );

        let sql = render_template(TOOLS_SQL);
        let rows = run_duckdb_json_for_test(&sql).expect("duckdb available (probed above)");
        std::env::remove_var("KIKIMIMI_DIR");

        let row = rows
            .iter()
            .find(|r| r["tool_name"] == "mcp__gh__search")
            .unwrap_or_else(|| panic!("no mcp__gh__search row in {rows:?}"));
        assert_eq!(row["failures"], 1, "deduped row: {rows:?}");
        assert_eq!(
            row["p50_duration_ms"], 250,
            "duration must come from the winning (OTel) row: {rows:?}"
        );
    }

    /// `thrash`'s `repeat_failure` signal: 3 distinct `tool_use_id`s, each
    /// duplicated hook+OTel (6 raw `tool.result` rows), never a success --
    /// deduped that's exactly 3 incidents, not 6.
    #[test]
    #[serial_test::serial]
    fn thrash_query_dedups_repeat_failure_incidents_across_hook_otel_pairs() {
        if run_duckdb_json_for_test("SELECT 1;").is_none() {
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());
        let data_dir = dir.path().join("data").join("events");
        let mut setup = String::from(
            "CREATE TABLE t (\
                ts BIGINT, dt TEXT, session_id TEXT, correlation_key TEXT, source TEXT, \
                event_type TEXT, tool_name TEXT, tool_kind TEXT, success BOOLEAN\
            );\n",
        );
        for (i, tu) in ["tu-1", "tu-2", "tu-3"].iter().enumerate() {
            let base_ts = 1000 + (i as i64) * 100;
            setup.push_str(&format!(
                "INSERT INTO t (ts, dt, session_id, correlation_key, source, event_type, tool_name, tool_kind, success) \
                 VALUES ({}, '2026-09-02', 'sess-thrash', '{tu}', 'hook', 'tool.result', 'mcp__gh__search', 'mcp', false);\n",
                base_ts
            ));
            setup.push_str(&format!(
                "INSERT INTO t (ts, dt, session_id, correlation_key, source, event_type, tool_name, tool_kind, success) \
                 VALUES ({}, '2026-09-02', 'sess-thrash', '{tu}', 'otel', 'tool.result', 'mcp__gh__search', 'mcp', false);\n",
                base_ts + 10
            ));
        }
        write_duckdb_fixture(&data_dir, "2026-09-02", &setup);

        let sql = render_template(THRASH_SQL);
        let rows = run_duckdb_json_for_test(&sql).expect("duckdb available (probed above)");
        std::env::remove_var("KIKIMIMI_DIR");

        let row = rows
            .iter()
            .find(|r| r["kind"] == "repeat_failure" && r["tool_name"] == "mcp__gh__search")
            .unwrap_or_else(|| panic!("no repeat_failure row for mcp__gh__search in {rows:?}"));
        assert_eq!(
            row["incidents"], 3,
            "3 distinct tool_use_ids, each hook+otel-duplicated, must dedup to 3 incidents, not 6: {rows:?}"
        );
    }

    #[test]
    fn unused_mcp_query_joins_configured_against_observed_calls_and_sorts_unused_first() {
        assert!(UNUSED_MCP_SQL.contains("{mcp_configured}"));
        assert!(UNUSED_MCP_SQL.contains("unnest({mcp_configured})"));
        assert!(UNUSED_MCP_SQL.contains("FULL OUTER JOIN calls"));
        // The "unused" flag (configured AND zero calls) is the first ORDER BY key.
        let order_by = UNUSED_MCP_SQL.split("ORDER BY").nth(1).unwrap();
        let normalized = order_by.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(normalized.starts_with(
            "(c.mcp_server IS NOT NULL AND coalesce(calls.calls_in_range, 0) = 0) DESC,"
        ));
    }

    #[test]
    fn unused_mcp_query_renders_configured_servers_as_a_duckdb_array_literal() {
        let rendered_none = render_template_with_configured(UNUSED_MCP_SQL, &[]);
        assert!(rendered_none.contains("unnest([]::VARCHAR[])"));

        let rendered_some = render_template_with_configured(
            UNUSED_MCP_SQL,
            &["notion".to_string(), "it's-weird".to_string()],
        );
        assert!(rendered_some.contains("unnest(['notion', 'it''s-weird']::VARCHAR[])"));
    }

    #[test]
    fn mcp_configured_sql_list_renders_empty_and_escapes_quotes() {
        assert_eq!(mcp_configured_sql_list(&[]), "[]::VARCHAR[]");
        assert_eq!(
            mcp_configured_sql_list(&["github".to_string()]),
            "['github']::VARCHAR[]"
        );
        assert_eq!(
            mcp_configured_sql_list(&["it's".to_string(), "fine".to_string()]),
            "['it''s', 'fine']::VARCHAR[]"
        );
    }

    #[test]
    fn collect_mcp_server_keys_finds_top_level_and_per_project_entries() {
        let json = serde_json::json!({
            "mcpServers": { "github": {}, "linear": {} },
            "projects": {
                "/home/me/proj-a": { "mcpServers": { "notion": {} } },
                "/home/me/proj-b": { "otherField": true },
                "/home/me/proj-c": { "mcpServers": { "github": {} } }
            }
        });
        let mut out = std::collections::BTreeSet::new();
        collect_mcp_server_keys(&json, &mut out);
        assert_eq!(
            out,
            ["github", "linear", "notion"]
                .into_iter()
                .map(String::from)
                .collect()
        );
    }

    #[test]
    fn collect_mcp_server_keys_tolerates_missing_or_malformed_shapes() {
        let mut out = std::collections::BTreeSet::new();
        collect_mcp_server_keys(&serde_json::json!({}), &mut out);
        assert!(out.is_empty());

        collect_mcp_server_keys(
            &serde_json::json!({"mcpServers": "not-an-object"}),
            &mut out,
        );
        assert!(out.is_empty());

        collect_mcp_server_keys(
            &serde_json::json!({"projects": {"p": {"mcpServers": null}}}),
            &mut out,
        );
        assert!(out.is_empty());
    }

    #[test]
    #[serial_test::serial]
    fn configured_mcp_servers_reads_both_files_defensively() {
        let dir = tempfile::tempdir().unwrap();
        let settings_path = dir.path().join("settings.json");
        let claude_json_path = dir.path().join("claude.json");

        // Neither file exists yet: must yield an empty list, not an error.
        std::env::set_var("KIKIMIMI_CLAUDE_SETTINGS_PATH", &settings_path);
        std::env::set_var("KIKIMIMI_CLAUDE_JSON_PATH", &claude_json_path);
        assert!(configured_mcp_servers().is_empty());

        // settings.json has a top-level mcpServers; claude.json is malformed JSON
        // (must be skipped, not fail the whole call).
        std::fs::write(
            &settings_path,
            serde_json::json!({"mcpServers": {"github": {}}}).to_string(),
        )
        .unwrap();
        std::fs::write(&claude_json_path, "{ not valid json").unwrap();
        assert_eq!(configured_mcp_servers(), vec!["github".to_string()]);

        // Now give claude.json a real, per-project mcpServers block too.
        std::fs::write(
            &claude_json_path,
            serde_json::json!({
                "projects": { "/x": { "mcpServers": {"notion": {}} } }
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            configured_mcp_servers(),
            vec!["github".to_string(), "notion".to_string()]
        );

        std::env::remove_var("KIKIMIMI_CLAUDE_SETTINGS_PATH");
        std::env::remove_var("KIKIMIMI_CLAUDE_JSON_PATH");
    }

    #[test]
    fn schema_tax_query_derives_first_input_tokens_from_earliest_api_request_with_a_totals_row() {
        assert!(SCHEMA_TAX_SQL.contains("event_type = 'api.request' AND source = 'otel'"));
        assert!(SCHEMA_TAX_SQL
            .contains("arg_min(coalesce(input_tokens, 0) + coalesce(cache_read_tokens, 0), ts)"));
        assert!(SCHEMA_TAX_SQL.contains("fixed_share_pct"));
        assert!(SCHEMA_TAX_SQL.contains("UNION ALL"));
        assert!(SCHEMA_TAX_SQL.contains("'TOTAL' AS session_id"));
    }

    /// Test-only helper: render a template with an explicit configured-server
    /// list instead of going through the real `~/.claude*` files, so the
    /// array-literal rendering can be asserted without touching the FS/env.
    fn render_template_with_configured(template: &str, servers: &[String]) -> String {
        template.replace("{mcp_configured}", &mcp_configured_sql_list(servers))
    }

    #[test]
    fn render_table_aligns_columns_and_renders_null_as_blank() {
        let columns = vec!["tool_name".to_string(), "calls".to_string()];
        let rows = vec![
            vec![Value::String("Bash".into()), Value::from(12)],
            vec![Value::String("mcp__github__get_issue".into()), Value::Null],
        ];
        let table = render_table(&columns, &rows);
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines.len(), 4, "header + separator + 2 rows");

        // Column 0 is left-aligned and wide enough for the longest cell (the mcp name).
        assert!(lines[0].starts_with("tool_name"));
        assert!(lines[2].starts_with("Bash"));
        assert!(lines[3].starts_with("mcp__github__get_issue"));

        // "calls" and its values line up at the same column across every row.
        let calls_col = lines[0].find("calls").unwrap();
        assert_eq!(lines[2][calls_col..].trim(), "12");
        assert!(lines[1].chars().all(|c| c == '-' || c == ' '));
        // A null cell renders as blank, and trailing blanks are trimmed off the row.
        assert_eq!(lines[3].trim_end(), "mcp__github__get_issue");
    }

    #[test]
    fn render_table_handles_no_rows() {
        let columns = vec!["a".to_string()];
        let table = render_table(&columns, &[]);
        assert_eq!(table, "a\n-\n");
    }

    mod cloud {
        use super::super::*;
        use crate::config::{CloudConfig, KikimimiConfig};
        use httpmock::prelude::*;
        use serde_json::json;
        use serial_test::serial;

        fn login_with(server: &MockServer) {
            let mut cfg = KikimimiConfig::load();
            cfg.cloud = Some(CloudConfig {
                endpoint: server.base_url(),
                token: "tok-query".into(),
                email: "dev@example.com".into(),
                org_id: "org-1".into(),
                ..Default::default()
            });
            cfg.save().unwrap();
        }

        #[test]
        #[serial]
        fn fetch_cloud_query_sends_bearer_and_parses_columns_rows() {
            let server = MockServer::start();
            let mock = server.mock(|when, then| {
                when.method(GET)
                    .path("/v1/query/today")
                    .header("authorization", "Bearer tok-query");
                then.status(200).json_body(json!({
                    "columns": ["events", "tool_calls"],
                    "rows": [[3, 1]]
                }));
            });

            let resp =
                fetch_cloud_query(&server.base_url(), "tok-query", "today", None, None).unwrap();
            mock.assert_calls(1);
            assert_eq!(resp.columns, vec!["events", "tool_calls"]);
            assert_eq!(resp.rows, vec![vec![Value::from(3), Value::from(1)]]);
        }

        #[test]
        #[serial]
        fn fetch_cloud_query_errors_on_non_success_status() {
            let server = MockServer::start();
            server.mock(|when, then| {
                when.method(GET).path("/v1/query/bypass");
                then.status(500).body("boom");
            });
            assert!(fetch_cloud_query(&server.base_url(), "tok", "bypass", None, None).is_err());
        }

        #[test]
        fn cloud_query_url_builds_query_string_only_when_range_given() {
            assert_eq!(
                cloud_query_url("http://x", "today", None, None),
                "http://x/v1/query/today"
            );
            assert_eq!(
                cloud_query_url("http://x/", "today", Some("2026-08-01"), None),
                "http://x/v1/query/today?dt_from=2026-08-01"
            );
            assert_eq!(
                cloud_query_url("http://x", "today", Some("2026-08-01"), Some("2026-08-31")),
                "http://x/v1/query/today?dt_from=2026-08-01&dt_to=2026-08-31"
            );
        }

        #[test]
        fn effective_cloud_range_defaults_today_to_todays_date_when_no_explicit_range_given() {
            let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
            assert_eq!(
                effective_cloud_range("today", None, None),
                (Some(today.clone()), Some(today))
            );
        }

        #[test]
        fn effective_cloud_range_leaves_other_named_queries_unbounded_by_default() {
            for name in ["tools", "mcp", "bypass", "reach", "thrash"] {
                assert_eq!(
                    effective_cloud_range(name, None, None),
                    (None, None),
                    "{name} must stay unbounded by default, matching its local (unfiltered) query"
                );
            }
        }

        #[test]
        fn effective_cloud_range_explicit_flags_always_win_even_for_today() {
            assert_eq!(
                effective_cloud_range("today", Some("2026-01-01"), None),
                (Some("2026-01-01".to_string()), None)
            );
            assert_eq!(
                effective_cloud_range("today", None, Some("2026-01-31")),
                (None, Some("2026-01-31".to_string()))
            );
        }

        #[test]
        #[serial]
        fn fetch_cloud_query_today_sends_todays_date_as_the_range() {
            let server = MockServer::start();
            let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
            let mock = server.mock(|when, then| {
                when.method(GET)
                    .path("/v1/query/today")
                    .query_param("dt_from", today.as_str())
                    .query_param("dt_to", today.as_str());
                then.status(200)
                    .json_body(json!({"columns": [], "rows": []}));
            });

            let (dt_from, dt_to) = effective_cloud_range("today", None, None);
            fetch_cloud_query(
                &server.base_url(),
                "tok-query",
                "today",
                dt_from.as_deref(),
                dt_to.as_deref(),
            )
            .unwrap();
            mock.assert_calls(1);
        }

        #[test]
        #[serial]
        fn run_cloud_errors_when_not_logged_in() {
            let dir = tempfile::tempdir().unwrap();
            std::env::set_var("KIKIMIMI_DIR", dir.path());

            let args = QueryArgs {
                name: Some("today".into()),
                sql: None,
                show_sql: false,
                cloud: true,
                dt_from: None,
                dt_to: None,
            };
            assert!(run(args).is_err());

            std::env::remove_var("KIKIMIMI_DIR");
        }

        #[test]
        #[serial]
        fn run_cloud_rejects_sql_flag() {
            let dir = tempfile::tempdir().unwrap();
            std::env::set_var("KIKIMIMI_DIR", dir.path());
            let server = MockServer::start();
            login_with(&server);

            let args = QueryArgs {
                name: Some("today".into()),
                sql: Some("select 1".into()),
                show_sql: false,
                cloud: true,
                dt_from: None,
                dt_to: None,
            };
            assert!(run(args).is_err());

            std::env::remove_var("KIKIMIMI_DIR");
        }

        #[test]
        #[serial]
        fn run_cloud_requires_a_name() {
            let dir = tempfile::tempdir().unwrap();
            std::env::set_var("KIKIMIMI_DIR", dir.path());
            let server = MockServer::start();
            login_with(&server);

            let args = QueryArgs {
                name: None,
                sql: None,
                show_sql: false,
                cloud: true,
                dt_from: None,
                dt_to: None,
            };
            assert!(run(args).is_err());

            std::env::remove_var("KIKIMIMI_DIR");
        }
    }
}
