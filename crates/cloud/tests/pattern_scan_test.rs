//! KKM-9: the struggle-pattern scanner (`crates/cloud/src/patterns.rs`)
//! persists detections into `pattern_hits`, org-scoped, idempotent across
//! rescans, and stops folding in events once a `dt` is past the §7.2
//! watermark. Driven end-to-end: real ingest over HTTP, real scanner, real
//! `GET /v1/query/patterns`.
//!
//! Needs a local Postgres (see `support/mod.rs`), like every test here.

mod support;

use chrono::{Duration, TimeZone, Utc};
use kikimimi_cloud::patterns::{scan_once, ScanSummary};
use kikimimi_schema::{event_type, Event};
use support::{gzip, ingest_body_bytes, login_as, SpawnOpts, TestApp};

/// 2026-09-01T10:00:00Z — a fixed day so the watermark can be reasoned about.
const BASE_TS: i64 = 1_788_602_400_000;
const DT: &str = "2026-09-01";

fn base(host_id: &str, session_id: &str, ts: i64) -> Event {
    Event {
        ts,
        dt: DT.to_string(),
        host_id: host_id.to_string(),
        agent: "claude-code".to_string(),
        source: "hook".to_string(),
        session_id: Some(session_id.to_string()),
        ..Default::default()
    }
}

/// The canonical §1.1 scenario: an MCP tool fails, the agent reaches for Bash
/// two events later, and OTel says two API requests happened in between.
fn bypass_scenario(host_id: &str, session_id: &str, prefix: &str) -> Vec<Event> {
    let t = |off: i64| BASE_TS + off * 1000;
    vec![
        Event {
            event_id: format!("{prefix}-call"),
            event_type: event_type::TOOL_CALL.to_string(),
            tool_name: Some("mcp__gh__search".into()),
            tool_kind: Some("mcp".into()),
            mcp_server: Some("gh".into()),
            correlation_key: Some(format!("{prefix}-k1")),
            ..base(host_id, session_id, t(0))
        },
        Event {
            event_id: format!("{prefix}-fail"),
            event_type: event_type::TOOL_RESULT.to_string(),
            tool_name: Some("mcp__gh__search".into()),
            tool_kind: Some("mcp".into()),
            mcp_server: Some("gh".into()),
            correlation_key: Some(format!("{prefix}-k1")),
            success: Some(false),
            ..base(host_id, session_id, t(1))
        },
        Event {
            event_id: format!("{prefix}-api1"),
            source: "otel".into(),
            event_type: event_type::API_REQUEST.to_string(),
            input_tokens: Some(1000),
            output_tokens: Some(200),
            cache_read_tokens: Some(50_000),
            ..base(host_id, session_id, t(2))
        },
        Event {
            event_id: format!("{prefix}-api2"),
            source: "otel".into(),
            event_type: event_type::API_REQUEST.to_string(),
            input_tokens: Some(300),
            output_tokens: Some(100),
            ..base(host_id, session_id, t(3))
        },
        Event {
            event_id: format!("{prefix}-bash"),
            event_type: event_type::TOOL_CALL.to_string(),
            tool_name: Some("Bash".into()),
            tool_kind: Some("bash".into()),
            ..base(host_id, session_id, t(4))
        },
    ]
}

/// Three failures of the same tool, never a success.
fn repeat_failure_scenario(host_id: &str, session_id: &str, prefix: &str) -> Vec<Event> {
    (0..3)
        .map(|i| Event {
            event_id: format!("{prefix}-rf{i}"),
            event_type: event_type::TOOL_RESULT.to_string(),
            tool_name: Some("mcp__jira__create".into()),
            tool_kind: Some("mcp".into()),
            mcp_server: Some("jira".into()),
            correlation_key: Some(format!("{prefix}-rk{i}")),
            success: Some(false),
            ..base(host_id, session_id, BASE_TS + 60_000 + i * 1000)
        })
        .collect()
}

async fn ingest(client: &reqwest::Client, base_url: &str, token: &str, events: &[Event]) {
    let resp = client
        .post(format!("{base_url}/v1/events"))
        .bearer_auth(token)
        .header("Content-Encoding", "gzip")
        .header("Content-Type", "application/x-ndjson")
        .body(gzip(&ingest_body_bytes(events)))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "ingest: {}", resp.text().await.unwrap());
}

async fn patterns(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
) -> Vec<Vec<serde_json::Value>> {
    let body: serde_json::Value = client
        .get(format!("{base_url}/v1/query/patterns"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let cols: Vec<String> = body["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        cols,
        [
            "dt",
            "session_id",
            "pattern_id",
            "subject",
            "first_ts",
            "last_ts",
            "incidents",
            "wasted_tokens_est",
            "detail",
            "first_detected_at"
        ]
    );
    body["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_array().unwrap().clone())
        .collect()
}

#[tokio::test]
async fn scanner_persists_hits_per_org_and_prices_the_window() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();
    let a = login_as(&client, &app.base_url, "host-a", "a@example.com").await;
    let b = login_as(&client, &app.base_url, "host-b", "b@example.com").await;
    assert_ne!(a.org_id, b.org_id);

    let mut ev_a = bypass_scenario("host-a", "sess-a", "a");
    ev_a.extend(repeat_failure_scenario("host-a", "sess-a", "a"));
    ingest(&client, &app.base_url, &a.token, &ev_a).await;
    // org B only has the repeat-failure half; it must never see A's bypass.
    ingest(
        &client,
        &app.base_url,
        &b.token,
        &repeat_failure_scenario("host-b", "sess-b", "b"),
    )
    .await;

    // Before any scan the query is empty (it reads pattern_hits, not events).
    assert!(patterns(&client, &app.base_url, &a.token).await.is_empty());

    let now = Utc.with_ymd_and_hms(2026, 9, 2, 12, 0, 0).unwrap(); // inside the 72h grace
    let s = scan_once(&app.state, now).await.unwrap();
    assert_eq!(
        s,
        ScanSummary {
            partitions_scanned: 2,
            partitions_finalized: 0,
            late_partitions: 0,
            hits: 3
        }
    );

    let rows_a = patterns(&client, &app.base_url, &a.token).await;
    assert_eq!(rows_a.len(), 2, "{rows_a:?}");
    let bypass = rows_a
        .iter()
        .find(|r| r[2] == "mcp_bypass")
        .expect("mcp_bypass row");
    assert_eq!(bypass[0], DT);
    assert_eq!(bypass[1], "sess-a");
    assert_eq!(bypass[3], "gh", "subject is the MCP server that failed");
    assert_eq!(bypass[4], BASE_TS + 1000);
    assert_eq!(bypass[5], BASE_TS + 4000);
    assert_eq!(bypass[6], 1);
    // input+output of the two api.requests inside the window; cache reads excluded.
    assert_eq!(bypass[7], 1000 + 200 + 300 + 100);
    let detail: serde_json::Value = serde_json::from_str(bypass[8].as_str().unwrap()).unwrap();
    assert_eq!(detail["failed_tool"], "mcp__gh__search");
    assert_eq!(detail["detour_tool"], "Bash");

    let rf = rows_a
        .iter()
        .find(|r| r[2] == "repeat_failure")
        .expect("repeat_failure row");
    assert_eq!(rf[3], "mcp__jira__create");
    assert_eq!(rf[6], 3);
    assert!(
        rf[7].is_null(),
        "no api.request in the failure window -> unknown, not 0"
    );

    let rows_b = patterns(&client, &app.base_url, &b.token).await;
    assert_eq!(rows_b.len(), 1);
    assert_eq!(rows_b[0][2], "repeat_failure");
    assert_eq!(rows_b[0][1], "sess-b");

    // Nothing new arrived: a second pass is a no-op.
    let s2 = scan_once(&app.state, now).await.unwrap();
    assert_eq!(s2, ScanSummary::default());

    app.teardown().await;
}

#[tokio::test]
async fn rescan_folds_in_late_events_until_the_watermark_then_only_counts_them() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();
    let a = login_as(&client, &app.base_url, "host-a", "a@example.com").await;

    // Two failures only: below the repeat_failure threshold.
    let mut two = repeat_failure_scenario("host-a", "sess-a", "a");
    let third = two.pop().unwrap();
    ingest(&client, &app.base_url, &a.token, &two).await;
    let inside = Utc.with_ymd_and_hms(2026, 9, 3, 0, 0, 0).unwrap();
    let s = scan_once(&app.state, inside).await.unwrap();
    assert_eq!(
        (s.partitions_scanned, s.hits, s.partitions_finalized),
        (1, 0, 0)
    );
    assert!(patterns(&client, &app.base_url, &a.token).await.is_empty());

    // The third failure arrives late (offline resend) while still inside the
    // 72h grace: the rescan now sees a repeat_failure.
    ingest(&client, &app.base_url, &a.token, &[third]).await;
    let s = scan_once(&app.state, inside).await.unwrap();
    assert_eq!((s.partitions_scanned, s.hits), (1, 1));
    let rows = patterns(&client, &app.base_url, &a.token).await;
    assert_eq!(rows.len(), 1);
    let first_detected = rows[0][9].clone();

    // Past the watermark (2026-09-02T00:00Z + 72h = 2026-09-05T00:00Z): this
    // scan is the final one for the partition.
    ingest(
        &client,
        &app.base_url,
        &a.token,
        &bypass_scenario("host-a", "sess-late", "late"),
    )
    .await;
    let past = Utc.with_ymd_and_hms(2026, 9, 5, 0, 0, 1).unwrap();
    let s = scan_once(&app.state, past).await.unwrap();
    assert_eq!(
        (s.partitions_scanned, s.partitions_finalized, s.hits),
        (1, 1, 2)
    );
    let rows = patterns(&client, &app.base_url, &a.token).await;
    assert_eq!(rows.len(), 2);
    let rf = rows.iter().find(|r| r[2] == "repeat_failure").unwrap();
    assert_eq!(
        rf[9], first_detected,
        "an upsert keeps first_detected_at across rescans"
    );

    // Anything after finalization is counted as late, never folded in.
    ingest(
        &client,
        &app.base_url,
        &a.token,
        &bypass_scenario("host-a", "sess-too-late", "toolate"),
    )
    .await;
    let s = scan_once(&app.state, past + Duration::hours(1))
        .await
        .unwrap();
    assert_eq!(
        s,
        ScanSummary {
            partitions_scanned: 0,
            partitions_finalized: 0,
            late_partitions: 1,
            hits: 0
        }
    );
    assert_eq!(patterns(&client, &app.base_url, &a.token).await.len(), 2);
    let (late, finalized): (i64, bool) =
        sqlx::query_as("SELECT late_events, finalized FROM pattern_scan_state WHERE dt = $1")
            .bind(DT)
            .fetch_one(&app.state.pools.superuser)
            .await
            .unwrap();
    assert!(finalized);
    assert_eq!(late, 1);

    app.teardown().await;
}
