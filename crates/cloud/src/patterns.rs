//! Struggle-pattern scanner (architecture.md §7.2 "苦戦検知 (pattern library)",
//! §7.3 改善ループ; KKM-9).
//!
//! The named queries in `query_sql.rs` (`bypass`, `thrash`, ...) re-derive
//! every incident from `events` on each request. That is fine for a CLI, but
//! the §7.2 ranking ("MCP サーバー / スキル単位で組織横断に集約") and the §7.3
//! before/after view need detections that *persist* — with a `pattern_id`, an
//! attributed `subject` (the MCP server / tool the incident points at) and a
//! `wasted_tokens_est` — so they can be aggregated cheaply and compared over
//! time. This module is that batch job:
//!
//! - **What it scans**: one `(org, dt)` partition at a time, through the same
//!   RLS-scoped transaction the API uses (`Pools::org_scoped_tx`), so the
//!   detection SQL itself has no `org_id` filter and can never cross tenants.
//! - **When**: every `Config::pattern_scan_interval_secs`, any partition whose
//!   `max(events.received_at)` moved since its last scan. The §7.2 watermark
//!   applies: once a `dt` is `pattern_watermark_hours` past the end of that
//!   day it is scanned one last time and marked `finalized`; events arriving
//!   after that are only *counted* (`pattern_scan_state.late_events`), never
//!   folded in, so the numbers stop moving under the reader's feet.
//! - **Idempotence**: [`DETECT_SQL`] emits a stable `hit_key` per incident
//!   (the anchoring event ids), the table's primary key includes it, and the
//!   write is an upsert followed by a delete of rows the current scan did not
//!   touch (`scan_seq`). Rescanning is therefore safe, keeps
//!   `first_detected_at`, and a dashboard can show "new since <t>" from
//!   `first_detected_at` alone.
//!
//! Patterns in this v0 batch (the §7.2 table, Stage 0/1 rows):
//!
//! | pattern_id       | anchor                                              | subject       |
//! |------------------|-----------------------------------------------------|---------------|
//! | `mcp_bypass`     | failed MCP `tool.result` → bash/browser call ≤5 rows | `mcp_server`  |
//! | `deny_detour`    | `tool.denied` → bash/browser call ≤5 rows            | denied tool   |
//! | `repeat_failure` | ≥3 failed `tool.result`, no success, same tool       | `tool_name`   |
//!
//! `mcp_bypass` / `deny_detour` reuse `BYPASS_SQL` / `THRASH_SQL`'s windowing
//! verbatim (same row_number-over-session, same ≤5-row distance, same
//! hook/OTel `tool.result` dedup). `retry_spiral`, `permission_denied_loop`,
//! `context_bloat` and `long_tool_tail` are KKM-10; `unused_mcp_server` and
//! `schema_tax` are per-server, not per-incident, and stay query-time
//! (`unused-mcp`, `schema-tax`) until KKM-13 gives them a token cost to persist.
//!
//! **`wasted_tokens_est` (v0 definition, honest)**: the `input_tokens +
//! output_tokens` of the session's OTel `api.request` rows whose `ts` falls in
//! the incident's window (`[first_ts, last_ts]` — anchor to detour, or first
//! to last failure). Cache reads are excluded (they are cheap and would swamp
//! the number). This is "what the model burned while stuck", not a
//! counterfactual; sessions without OTel usage get NULL, never 0, so the
//! §7.1 "usage_source = unknown" gap stays visible. A window that spans a
//! `dt` boundary is truncated to the scanned day (events are scanned per
//! partition), which under-counts long overnight incidents.

use std::time::Duration;

use anyhow::Context;
use chrono::{DateTime, NaiveDate, Utc};
use sqlx::{AssertSqlSafe, Row};

use crate::state::AppState;

/// One row per incident for the scanned `dt` (`$1`). Column order matches
/// [`UPSERT_SQL`]'s `INSERT ... SELECT` list.
///
/// The `e` / `tool_results` / `bypass_call` CTEs are the ones `BYPASS_SQL` and
/// `THRASH_SQL` use (see `query_sql.rs` for the dedup rationale) — kept in
/// sync by hand, like every other Postgres/DuckDB pair in this repo.
/// `usage` is the per-session OTel `api.request` stream used to price a
/// window; `LATERAL` sums it per incident.
pub const DETECT_SQL: &str = r#"
WITH e AS (
    SELECT *, row_number() OVER (PARTITION BY session_id ORDER BY ts) AS rn
    FROM events
    WHERE dt = $1 AND session_id IS NOT NULL
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
usage AS (
    SELECT session_id, ts, coalesce(input_tokens, 0) + coalesce(output_tokens, 0) AS tokens
    FROM e
    WHERE event_type = 'api.request' AND source = 'otel'
),
bypass_call AS (
    SELECT session_id, event_id, tool_name, ts AS bypass_ts, rn AS bypass_rn
    FROM e
    WHERE event_type = 'tool.call' AND tool_kind IN ('bash', 'browser')
),
mcp_fail AS (
    SELECT session_id, event_id, mcp_server, tool_name, ts AS fail_ts, rn AS fail_rn
    FROM tool_results
    WHERE success = false AND tool_kind = 'mcp' AND mcp_server IS NOT NULL
),
mcp_bypass AS (
    SELECT
        f.session_id,
        'mcp_bypass'::text AS pattern_id,
        f.mcp_server AS subject,
        f.event_id || '>' || b.event_id AS hit_key,
        f.fail_ts AS first_ts,
        b.bypass_ts AS last_ts,
        1::int8 AS incidents,
        jsonb_build_object('failed_tool', f.tool_name, 'detour_tool', b.tool_name) AS detail
    FROM mcp_fail f
    JOIN bypass_call b
      ON f.session_id = b.session_id
     AND b.bypass_rn > f.fail_rn
     AND b.bypass_rn <= f.fail_rn + 5
),
tool_denied AS (
    SELECT session_id, event_id, tool_name, ts AS fail_ts, rn AS fail_rn
    FROM e
    WHERE event_type = 'tool.denied' AND tool_name IS NOT NULL
),
deny_detour AS (
    SELECT
        f.session_id,
        'deny_detour'::text AS pattern_id,
        f.tool_name AS subject,
        f.event_id || '>' || b.event_id AS hit_key,
        f.fail_ts AS first_ts,
        b.bypass_ts AS last_ts,
        1::int8 AS incidents,
        jsonb_build_object('detour_tool', b.tool_name) AS detail
    FROM tool_denied f
    JOIN bypass_call b
      ON f.session_id = b.session_id
     AND b.bypass_rn > f.fail_rn
     AND b.bypass_rn <= f.fail_rn + 5
),
fail_counts AS (
    SELECT session_id, tool_name, mcp_server,
           count(*)::int8 AS incidents, min(ts) AS first_ts, max(ts) AS last_ts,
           min(event_id) AS anchor_event_id
    FROM tool_results
    WHERE success = false AND tool_name IS NOT NULL
    GROUP BY session_id, tool_name, mcp_server
),
success_pairs AS (
    SELECT DISTINCT session_id, tool_name
    FROM tool_results
    WHERE success = true AND tool_name IS NOT NULL
),
repeat_failure AS (
    SELECT
        f.session_id,
        'repeat_failure'::text AS pattern_id,
        f.tool_name AS subject,
        'repeat'::text AS hit_key,
        f.first_ts,
        f.last_ts,
        f.incidents,
        jsonb_build_object('mcp_server', f.mcp_server) AS detail
    FROM fail_counts f
    LEFT JOIN success_pairs s
      ON f.session_id = s.session_id AND f.tool_name = s.tool_name
    WHERE f.incidents >= 3 AND s.session_id IS NULL
),
hits AS (
    SELECT * FROM mcp_bypass
    UNION ALL SELECT * FROM deny_detour
    UNION ALL SELECT * FROM repeat_failure
)
SELECT
    h.session_id,
    h.pattern_id,
    h.subject,
    h.hit_key,
    h.first_ts,
    h.last_ts,
    h.incidents,
    w.wasted_tokens_est,
    h.detail
FROM hits h
LEFT JOIN LATERAL (
    SELECT sum(u.tokens)::int8 AS wasted_tokens_est
    FROM usage u
    WHERE u.session_id = h.session_id AND u.ts >= h.first_ts AND u.ts <= h.last_ts
) w ON true
"#;

/// Upsert this scan's detections for `dt = $1` under `scan_seq = $2`.
/// `org_id` comes from the RLS setting so the statement never names a tenant.
const UPSERT_SQL: &str = r#"
INSERT INTO pattern_hits
    (org_id, dt, session_id, pattern_id, subject, hit_key,
     first_ts, last_ts, incidents, wasted_tokens_est, detail, scan_seq)
SELECT
    current_setting('app.org_id')::uuid, $1, d.session_id, d.pattern_id, d.subject, d.hit_key,
    d.first_ts, d.last_ts, d.incidents, d.wasted_tokens_est, d.detail, $2
FROM (DETECT) d
ON CONFLICT (org_id, dt, session_id, pattern_id, subject, hit_key) DO UPDATE SET
    first_ts          = EXCLUDED.first_ts,
    last_ts           = EXCLUDED.last_ts,
    incidents         = EXCLUDED.incidents,
    wasted_tokens_est = EXCLUDED.wasted_tokens_est,
    detail            = EXCLUDED.detail,
    scan_seq          = EXCLUDED.scan_seq,
    updated_at        = now()
"#;

/// Partitions that need (re)scanning: never scanned, or not finalized and
/// with events received after the last scan. Superuser pool (spans orgs).
/// Finalized partitions with newer events only get `late_events` bumped.
const DUE_SQL: &str = r#"
WITH latest AS (
    SELECT org_id, dt, max(received_at) AS max_received_at, count(*)::int8 AS n
    FROM events
    GROUP BY org_id, dt
)
SELECT l.org_id, l.dt, l.max_received_at,
       s.finalized, s.max_received_at AS scanned_max, s.scan_seq
FROM latest l
LEFT JOIN pattern_scan_state s ON s.org_id = l.org_id AND s.dt = l.dt
WHERE s.org_id IS NULL OR l.max_received_at > s.max_received_at
ORDER BY l.org_id, l.dt
"#;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ScanSummary {
    pub partitions_scanned: usize,
    pub partitions_finalized: usize,
    pub late_partitions: usize,
    pub hits: i64,
}

/// One pass over every due `(org, dt)`; returns what it did. `now` is
/// injectable so tests can move the watermark without sleeping.
pub async fn scan_once(state: &AppState, now: DateTime<Utc>) -> anyhow::Result<ScanSummary> {
    let watermark = chrono::Duration::hours(state.config.pattern_watermark_hours);
    let due = sqlx::query(DUE_SQL)
        .fetch_all(&state.pools.superuser)
        .await
        .context("listing partitions due for a pattern scan")?;

    let mut summary = ScanSummary::default();
    for row in due {
        let org_id: uuid::Uuid = row.try_get("org_id")?;
        let dt: String = row.try_get("dt")?;
        let max_received_at: DateTime<Utc> = row.try_get("max_received_at")?;
        let finalized: Option<bool> = row.try_get("finalized")?;
        let prev_seq: Option<i64> = row.try_get("scan_seq")?;

        if finalized == Some(true) {
            // §7.2 watermark passed: keep the numbers stable, but make the
            // gap visible (§7.1) by counting what arrived too late.
            sqlx::query(
                "UPDATE pattern_scan_state SET late_events = late_events + 1, max_received_at = $3 \
                 WHERE org_id = $1 AND dt = $2",
            )
            .bind(org_id)
            .bind(&dt)
            .bind(max_received_at)
            .execute(&state.pools.superuser)
            .await?;
            summary.late_partitions += 1;
            continue;
        }

        let scan_seq = prev_seq.unwrap_or(0) + 1;
        let hits = scan_partition(state, org_id, &dt, scan_seq).await?;
        let finalize = partition_is_past_watermark(&dt, now, watermark);

        sqlx::query(
            "INSERT INTO pattern_scan_state (org_id, dt, scan_seq, max_received_at, finalized, hits, scanned_at) \
             VALUES ($1, $2, $3, $4, $5, $6, now()) \
             ON CONFLICT (org_id, dt) DO UPDATE SET \
               scan_seq = EXCLUDED.scan_seq, max_received_at = EXCLUDED.max_received_at, \
               finalized = EXCLUDED.finalized, hits = EXCLUDED.hits, scanned_at = now()",
        )
        .bind(org_id)
        .bind(&dt)
        .bind(scan_seq)
        .bind(max_received_at)
        .bind(finalize)
        .bind(hits)
        .execute(&state.pools.superuser)
        .await
        .context("recording pattern scan state")?;

        summary.partitions_scanned += 1;
        summary.hits += hits;
        if finalize {
            summary.partitions_finalized += 1;
        }
    }
    Ok(summary)
}

/// Detect + upsert + prune for one partition inside one RLS-scoped
/// transaction. Returns the number of hits now stored for `(org, dt)`.
async fn scan_partition(
    state: &AppState,
    org_id: uuid::Uuid,
    dt: &str,
    scan_seq: i64,
) -> anyhow::Result<i64> {
    let mut tx = state.pools.org_scoped_tx(org_id).await?;
    // Two compile-time constants spliced together; no caller input reaches
    // the text (the only parameters are `$1`/`$2` binds), so this is not the
    // injection risk `SqlSafeStr` guards against.
    let upsert = UPSERT_SQL.replace("(DETECT)", &format!("({DETECT_SQL})"));
    sqlx::query(AssertSqlSafe(upsert))
        .bind(dt)
        .bind(scan_seq)
        .execute(&mut *tx)
        .await
        .with_context(|| format!("upserting pattern hits for dt={dt}"))?;
    sqlx::query("DELETE FROM pattern_hits WHERE dt = $1 AND scan_seq < $2")
        .bind(dt)
        .bind(scan_seq)
        .execute(&mut *tx)
        .await
        .with_context(|| format!("pruning stale pattern hits for dt={dt}"))?;
    let (hits,): (i64,) = sqlx::query_as("SELECT count(*)::int8 FROM pattern_hits WHERE dt = $1")
        .bind(dt)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await.context("committing pattern scan")?;
    Ok(hits)
}

/// `dt` ("YYYY-MM-DD", UTC) is final once `now` is `watermark` past the end
/// of that day. An unparseable `dt` never finalizes (it keeps being rescanned
/// while events arrive, which is the safe direction).
pub fn partition_is_past_watermark(
    dt: &str,
    now: DateTime<Utc>,
    watermark: chrono::Duration,
) -> bool {
    let Ok(day) = NaiveDate::parse_from_str(dt, "%Y-%m-%d") else {
        return false;
    };
    let Some(next_day) = day.succ_opt() else {
        return false;
    };
    let end_of_day = next_day.and_hms_opt(0, 0, 0).unwrap().and_utc();
    now >= end_of_day + watermark
}

/// Runs [`scan_once`] forever on the configured interval. A no-op when the
/// interval is 0 (tests). Errors are logged and the loop continues — a
/// transient DB error must not kill the scanner for the life of the process.
pub fn spawn_scanner(state: AppState) {
    let secs = state.config.pattern_scan_interval_secs;
    if secs == 0 {
        tracing::info!("pattern scanner disabled (PATTERN_SCAN_INTERVAL_SECS=0)");
        return;
    }
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(secs));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            match scan_once(&state, Utc::now()).await {
                Ok(s) if s.partitions_scanned > 0 || s.late_partitions > 0 => {
                    tracing::info!(
                        scanned = s.partitions_scanned,
                        finalized = s.partitions_finalized,
                        late = s.late_partitions,
                        hits = s.hits,
                        "pattern scan"
                    );
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %format!("{e:#}"), "pattern scan failed"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watermark_is_end_of_day_plus_grace() {
        let wm = chrono::Duration::hours(72);
        let t = |s: &str| DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc);
        // 2026-09-01 ends at 2026-09-02T00:00Z; +72h = 2026-09-05T00:00Z.
        assert!(!partition_is_past_watermark(
            "2026-09-01",
            t("2026-09-04T23:59:59Z"),
            wm
        ));
        assert!(partition_is_past_watermark(
            "2026-09-01",
            t("2026-09-05T00:00:00Z"),
            wm
        ));
        assert!(!partition_is_past_watermark(
            "not-a-date",
            t("2030-01-01T00:00:00Z"),
            wm
        ));
    }

    #[test]
    fn upsert_sql_embeds_the_detection_query() {
        let upsert = UPSERT_SQL.replace("(DETECT)", &format!("({DETECT_SQL})"));
        assert!(upsert.contains("WITH e AS"));
        assert!(!upsert.contains("(DETECT)"));
    }
}
