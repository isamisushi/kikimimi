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

/// KKM-10 scenarios in one session, laid out so they don't overlap:
/// - denied Read twice in a row (permission_denied_loop, incidents 2)
/// - a context jump: api.request ctx 10k -> 60k right after a tool.result
///   of `mcp__gh__big_dump` (context_bloat, subject = that tool)
/// - two compaction events (context_bloat, subject = compaction)
/// - three MCP calls at 1s and one at 30s (long_tool_tail on the 30s one)
/// - fail, fail, SUCCESS, fail, fail for one tool: two runs of 2 -> no
///   retry_spiral (the consecutive rule), where the old proxy would also
///   have said nothing because of the success; plus fail x3 consecutive on
///   another tool -> retry_spiral.
fn kkm10_scenario(host_id: &str, session_id: &str) -> Vec<Event> {
    let t = |off: i64| BASE_TS + 100_000 + off * 1000;
    let mut v = Vec::new();
    for i in 0..2 {
        v.push(Event {
            event_id: format!("d-deny{i}"),
            event_type: event_type::TOOL_DENIED.to_string(),
            tool_name: Some("Read".into()),
            tool_kind: Some("file".into()),
            ..base(host_id, session_id, t(i))
        });
    }
    v.push(Event {
        event_id: "d-api0".into(),
        source: "otel".into(),
        event_type: event_type::API_REQUEST.to_string(),
        input_tokens: Some(1000),
        cache_read_tokens: Some(9000),
        ..base(host_id, session_id, t(10))
    });
    v.push(Event {
        event_id: "d-dump".into(),
        event_type: event_type::TOOL_RESULT.to_string(),
        tool_name: Some("mcp__gh__big_dump".into()),
        tool_kind: Some("mcp".into()),
        mcp_server: Some("gh".into()),
        correlation_key: Some("d-dumpk".into()),
        success: Some(true),
        duration_ms: Some(1000),
        ..base(host_id, session_id, t(11))
    });
    v.push(Event {
        event_id: "d-api1".into(),
        source: "otel".into(),
        event_type: event_type::API_REQUEST.to_string(),
        input_tokens: Some(50_000),
        cache_read_tokens: Some(10_000),
        ..base(host_id, session_id, t(12))
    });
    for i in 0..2 {
        v.push(Event {
            event_id: format!("d-compact{i}"),
            event_type: event_type::COMPACTION.to_string(),
            ..base(host_id, session_id, t(20 + i))
        });
    }
    for (i, dur) in [1000i64, 1000, 1000, 30_000].iter().enumerate() {
        v.push(Event {
            event_id: format!("d-slow{i}"),
            event_type: event_type::TOOL_RESULT.to_string(),
            tool_name: Some("mcp__gh__search".into()),
            tool_kind: Some("mcp".into()),
            mcp_server: Some("gh".into()),
            correlation_key: Some(format!("d-slowk{i}")),
            success: Some(true),
            duration_ms: Some(*dur),
            ..base(host_id, session_id, t(30 + i as i64))
        });
    }
    for (i, ok) in [false, false, true, false, false].iter().enumerate() {
        v.push(Event {
            event_id: format!("d-mixed{i}"),
            event_type: event_type::TOOL_RESULT.to_string(),
            tool_name: Some("mcp__jira__update".into()),
            tool_kind: Some("mcp".into()),
            mcp_server: Some("jira".into()),
            correlation_key: Some(format!("d-mixedk{i}")),
            success: Some(*ok),
            ..base(host_id, session_id, t(40 + i as i64))
        });
    }
    v.extend(repeat_failure_scenario(host_id, session_id, "d"));
    v
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
    // sess-a had gh (called), jira (only failed tool.results, never a
    // tool.call) and slack (nothing) configured: two unused_mcp_server hits,
    // each priced by the equal-split allocation.
    ev_a.push(Event {
        event_id: "a-start".into(),
        event_type: event_type::SESSION_START.to_string(),
        configured_mcp_servers: Some(r#"["gh","jira","slack"]"#.into()),
        ..base("host-a", "sess-a", BASE_TS - 1000)
    });
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
            hits: 5
        }
    );

    let rows_a = patterns(&client, &app.base_url, &a.token).await;
    assert_eq!(rows_a.len(), 4, "{rows_a:?}");
    let mut unused_subjects: Vec<&str> = rows_a
        .iter()
        .filter(|r| r[2] == "unused_mcp_server")
        .map(|r| r[3].as_str().unwrap())
        .collect();
    unused_subjects.sort();
    assert_eq!(
        unused_subjects,
        ["jira", "slack"],
        "gh was called, the others never were"
    );
    let unused = rows_a.iter().find(|r| r[3] == "slack").expect("slack row");
    assert_eq!(unused[4], BASE_TS - 1000, "first_ts = session.start");
    // first request = 1000 input + 50000 cache_read = 51000; 3 configured; 2 api.requests.
    assert_eq!(unused[7], 51000 / 3 * 2);
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
        .find(|r| r[2] == "retry_spiral")
        .expect("retry_spiral row");
    assert_eq!(rf[3], "mcp__jira__create");
    assert_eq!(rf[6], 3);
    assert!(
        rf[7].is_null(),
        "no api.request in the failure window -> unknown, not 0"
    );

    let rows_b = patterns(&client, &app.base_url, &b.token).await;
    assert_eq!(rows_b.len(), 1);
    assert_eq!(rows_b[0][2], "retry_spiral");
    assert_eq!(rows_b[0][1], "sess-b");

    // Nothing new arrived: a second pass is a no-op.
    let s2 = scan_once(&app.state, now).await.unwrap();
    assert_eq!(s2, ScanSummary::default());

    app.teardown().await;
}

#[tokio::test]
async fn scanner_detects_the_kkm10_patterns() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();
    let a = login_as(&client, &app.base_url, "host-a", "a@example.com").await;
    ingest(
        &client,
        &app.base_url,
        &a.token,
        &kkm10_scenario("host-a", "sess-k"),
    )
    .await;
    let now = Utc.with_ymd_and_hms(2026, 9, 2, 12, 0, 0).unwrap();
    scan_once(&app.state, now).await.unwrap();

    let rows = patterns(&client, &app.base_url, &a.token).await;
    let mut kinds: Vec<(String, String)> = rows
        .iter()
        .map(|r| {
            (
                r[2].as_str().unwrap().to_string(),
                r[3].as_str().unwrap().to_string(),
            )
        })
        .collect();
    kinds.sort();
    assert_eq!(
        kinds,
        [
            ("context_bloat".to_string(), "compaction".to_string()),
            ("context_bloat".to_string(), "mcp__gh__big_dump".to_string()),
            ("long_tool_tail".to_string(), "gh".to_string()),
            ("permission_denied_loop".to_string(), "Read".to_string()),
            ("retry_spiral".to_string(), "mcp__jira__create".to_string()),
        ],
        "{rows:?}"
    );
    let find = |p: &str, sub: &str| rows.iter().find(|r| r[2] == p && r[3] == sub).unwrap();
    assert_eq!(find("permission_denied_loop", "Read")[6], 2);
    assert_eq!(find("context_bloat", "compaction")[6], 2);
    let jump: serde_json::Value = serde_json::from_str(
        find("context_bloat", "mcp__gh__big_dump")[8]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(jump["delta_tokens"], 50_000);
    let tail: serde_json::Value =
        serde_json::from_str(find("long_tool_tail", "gh")[8].as_str().unwrap()).unwrap();
    assert_eq!(tail["duration_ms"], 30_000);
    assert_eq!(find("retry_spiral", "mcp__jira__create")[6], 3);
    assert!(
        !rows.iter().any(|r| r[3] == "mcp__jira__update"),
        "fail,fail,ok,fail,fail is two runs of 2, not a spiral: {rows:?}"
    );

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
    let rf = rows.iter().find(|r| r[2] == "retry_spiral").unwrap();
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

/// `/web/q/patterns` (KKM-11): the ranking aggregates `pattern_hits` per
/// (pattern_id, subject), priority = wasted × sessions with NULL (not 0) for
/// unpriced groups, and `/web/q/pattern-hits` returns the incidents behind
/// one row -- both via the session cookie the web UI uses, and both RLS
/// scoped (org B sees nothing of org A).
#[tokio::test]
async fn web_ranking_aggregates_hits_and_drills_down() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();
    let a = login_as(&client, &app.base_url, "host-a", "a@example.com").await;
    let web_a = support::web_login(&client, &app.base_url, "a@example.com").await;
    let web_b = support::web_login(&client, &app.base_url, "b@example.com").await;

    let mut ev = bypass_scenario("host-a", "sess-1", "p");
    ev.extend(bypass_scenario("host-a", "sess-2", "q"));
    ev.extend(repeat_failure_scenario("host-a", "sess-1", "r"));
    ingest(&client, &app.base_url, &a.token, &ev).await;
    scan_once(&app.state, Utc::now()).await.unwrap();

    // The scenario's fixed dt (2026-09-01) may be older than the default
    // 30-day window depending on when this runs; ask for the max.
    let body: serde_json::Value = client
        .get(format!("{}/web/q/patterns?days=365", app.base_url))
        .header(reqwest::header::COOKIE, &web_a.cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let cols: Vec<&str> = body["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap())
        .collect();
    assert_eq!(
        cols,
        [
            "pattern_id",
            "subject",
            "sessions",
            "incidents",
            "wasted_tokens_est",
            "priced_hits",
            "hits",
            "priority",
            "first_seen_dt",
            "last_seen_dt"
        ]
    );
    let rows = body["rows"].as_array().unwrap();
    let bypass = rows
        .iter()
        .find(|r| r[0] == "mcp_bypass" && r[1] == "gh")
        .expect("mcp_bypass gh row");
    assert_eq!(bypass[2], 2, "two sessions");
    assert_eq!(bypass[3], 2);
    assert_eq!(bypass[4], 1600 * 2);
    assert_eq!(bypass[7], 1600 * 2 * 2, "priority = wasted x sessions");
    let spiral = rows
        .iter()
        .find(|r| r[0] == "retry_spiral")
        .expect("retry_spiral row");
    assert!(
        spiral[4].is_null() && spiral[7].is_null(),
        "unpriced -> null cost and null priority: {spiral:?}"
    );
    assert_eq!(
        rows[0][0], "mcp_bypass",
        "priced rows sort before unpriced: {rows:?}"
    );

    let hits: serde_json::Value = client
        .get(format!(
            "{}/web/q/pattern-hits?pattern_id=mcp_bypass&subject=gh&days=365&limit=50",
            app.base_url
        ))
        .header(reqwest::header::COOKIE, &web_a.cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let hit_rows = hits["rows"].as_array().unwrap();
    assert_eq!(hit_rows.len(), 2, "{hits:?}");
    let mut sessions: Vec<&str> = hit_rows.iter().map(|r| r[1].as_str().unwrap()).collect();
    sessions.sort();
    assert_eq!(sessions, ["sess-1", "sess-2"]);

    let missing = client
        .get(format!("{}/web/q/pattern-hits?days=365", app.base_url))
        .header(reqwest::header::COOKIE, &web_a.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 400);

    let other: serde_json::Value = client
        .get(format!("{}/web/q/patterns?days=365", app.base_url))
        .header(reqwest::header::COOKIE, &web_b.cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        other["rows"].as_array().unwrap().is_empty(),
        "org B must not see org A's ranking"
    );

    app.teardown().await;
}

/// KKM-12: the per-day timeline splits cleanly around an improvement mark,
/// marks are org-scoped, and a bad date is a 400.
#[tokio::test]
async fn web_timeline_and_marks_give_the_before_after_view() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();
    let a = login_as(&client, &app.base_url, "host-a", "a@example.com").await;
    let web_a = support::web_login(&client, &app.base_url, "a@example.com").await;
    let web_b = support::web_login(&client, &app.base_url, "b@example.com").await;

    // Day 1 (2026-09-01): two sessions, both bypass. Day 2: two sessions, none.
    let mut ev = bypass_scenario("host-a", "sess-1", "p");
    ev.extend(bypass_scenario("host-a", "sess-2", "q"));
    let day2 = |id: &str, sid: &str| Event {
        event_id: id.to_string(),
        dt: "2026-09-02".to_string(),
        event_type: event_type::TOOL_CALL.to_string(),
        tool_name: Some("Read".into()),
        tool_kind: Some("file".into()),
        ..base("host-a", sid, BASE_TS + 86_400_000)
    };
    ev.push(day2("d2-a", "sess-3"));
    ev.push(day2("d2-b", "sess-4"));
    ingest(&client, &app.base_url, &a.token, &ev).await;
    scan_once(&app.state, Utc::now()).await.unwrap();

    let q = "pattern_id=mcp_bypass&subject=gh";
    let body: serde_json::Value = client
        .get(format!(
            "{}/web/q/pattern-timeline?{q}&days=365",
            app.base_url
        ))
        .header(reqwest::header::COOKIE, &web_a.cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let cols: Vec<&str> = body["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap())
        .collect();
    assert_eq!(
        cols,
        [
            "dt",
            "sessions_total",
            "sessions_hit",
            "rate_pct",
            "incidents",
            "wasted_tokens_est"
        ]
    );
    let rows = body["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0][0], "2026-09-01");
    assert_eq!(rows[0][1], 2);
    assert_eq!(rows[0][2], 2);
    assert_eq!(rows[0][3], 100.0);
    assert_eq!(rows[0][5], 3200);
    assert_eq!(rows[1][0], "2026-09-02");
    assert_eq!(
        rows[1][2], 0,
        "a day with sessions but no hit is present as zeros"
    );
    assert_eq!(rows[1][3], 0.0);
    assert!(rows[1][5].is_null());

    let created = client
        .post(format!("{}/web/marks", app.base_url))
        .header(reqwest::header::COOKIE, &web_a.cookie)
        .json(&serde_json::json!({
            "pattern_id": "mcp_bypass", "subject": "gh", "marked_dt": "2026-09-02", "note": "fixed gh search"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 200, "{}", created.text().await.unwrap());
    let mark: serde_json::Value = created.json().await.unwrap();
    assert_eq!(mark["marked_dt"], "2026-09-02");
    let bad = client
        .post(format!("{}/web/marks", app.base_url))
        .header(reqwest::header::COOKIE, &web_a.cookie)
        .json(&serde_json::json!({ "pattern_id": "mcp_bypass", "subject": "gh", "marked_dt": "yesterday" }))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);

    let mine: serde_json::Value = client
        .get(format!("{}/web/marks?{q}", app.base_url))
        .header(reqwest::header::COOKIE, &web_a.cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(mine["marks"].as_array().unwrap().len(), 1);
    let theirs: serde_json::Value = client
        .get(format!("{}/web/marks?{q}", app.base_url))
        .header(reqwest::header::COOKIE, &web_b.cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        theirs["marks"].as_array().unwrap().is_empty(),
        "marks are org-scoped"
    );

    let foreign_delete = client
        .delete(format!(
            "{}/web/marks/{}",
            app.base_url,
            mark["id"].as_str().unwrap()
        ))
        .header(reqwest::header::COOKIE, &web_b.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(
        foreign_delete.status(),
        404,
        "RLS turns a foreign delete into a no-op"
    );
    let own_delete = client
        .delete(format!(
            "{}/web/marks/{}",
            app.base_url,
            mark["id"].as_str().unwrap()
        ))
        .header(reqwest::header::COOKIE, &web_a.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(own_delete.status(), 200);

    app.teardown().await;
}
