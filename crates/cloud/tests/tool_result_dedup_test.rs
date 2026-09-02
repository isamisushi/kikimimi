//! Task: stop double-counting `tool.result` rows that arrive from both hooks
//! and OTel for the same `tool_use_id` (architecture.md §4). `kikimimi init`
//! enables both for Claude Code, so a hook row (`source='hook'`) and an OTel
//! row (`source='otel'`) land as two distinct `events` rows for the same
//! `(session_id, correlation_key)` -- deliberately (gap-visibility +
//! correlation, never deduped at ingest). `query_sql.rs`/`web_query_sql.rs`'s
//! `tool_results` CTE collapses that pair back to one logical result at
//! query time, preferring the OTel row. This test drives that through the
//! real HTTP surface end-to-end (real Postgres, real ingest, real query
//! SQL) rather than asserting on the SQL text.
//!
//! Needs a local Postgres reachable at `support::base_database_url()`'s DSN
//! (env `DATABASE_URL`, else `postgres://postgres:guru-dev@127.0.0.1:5433/guru`)
//! -- see `crates/cloud/tests/support/mod.rs`. Not necessarily runnable in
//! every environment; skips are not built in on purpose (every other test in
//! this crate makes the same assumption), so a missing Postgres just fails
//! these tests loudly instead of silently passing.

mod support;

use kikimimi_schema::{event_type, Event};
use support::{gzip, ingest_body_bytes, login_autoapprove, SpawnOpts, TestApp};

/// A hook row and an OTel row for the exact same `(session_id,
/// correlation_key)`, both `success = false` -- the task's own suggested
/// fixture shape. Also includes one `tool.call` (hook-only, never
/// duplicated) so `calls` has a value to sanity-check alongside `failures`.
fn dedup_pair_events(
    host_id: &str,
    session_id: &str,
    correlation_key: &str,
    base_ts: i64,
) -> Vec<Event> {
    let dt = kikimimi_schema::dt_of(base_ts);
    let base = Event {
        ts: base_ts,
        dt: dt.clone(),
        host_id: host_id.to_string(),
        agent: "claude-code".to_string(),
        session_id: Some(session_id.to_string()),
        tool_name: Some("mcp__gh__search".to_string()),
        tool_kind: Some("mcp".to_string()),
        mcp_server: Some("gh".to_string()),
        ..Default::default()
    };
    vec![
        Event {
            event_id: format!("dedup-call-{correlation_key}"),
            ts: base_ts,
            source: "hook".to_string(),
            event_type: event_type::TOOL_CALL.to_string(),
            correlation_key: Some(correlation_key.to_string()),
            ..base.clone()
        },
        Event {
            event_id: format!("dedup-hook-{correlation_key}"),
            ts: base_ts + 100,
            source: "hook".to_string(),
            event_type: event_type::TOOL_RESULT.to_string(),
            correlation_key: Some(correlation_key.to_string()),
            success: Some(false),
            ..base.clone()
        },
        Event {
            event_id: format!("dedup-otel-{correlation_key}"),
            ts: base_ts + 150,
            source: "otel".to_string(),
            event_type: event_type::TOOL_RESULT.to_string(),
            correlation_key: Some(correlation_key.to_string()),
            success: Some(false),
            duration_ms: Some(250),
            ..base
        },
    ]
}

fn find_row<'a>(
    body: &'a serde_json::Value,
    col: &str,
    want: &str,
) -> Option<&'a serde_json::Value> {
    let columns = body["columns"].as_array().unwrap();
    let idx = columns.iter().position(|c| c == col)?;
    body["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r[idx] == want)
}

fn col_value(body: &serde_json::Value, col: &str) -> usize {
    body["columns"]
        .as_array()
        .unwrap()
        .iter()
        .position(|c| c == col)
        .unwrap_or_else(|| panic!("no {col} column in {body:?}"))
}

#[tokio::test]
async fn tools_query_reports_one_failure_for_a_hook_otel_tool_result_pair() {
    let app = TestApp::spawn(SpawnOpts {
        dev_autoapprove: true,
        ..Default::default()
    })
    .await;
    let client = reqwest::Client::new();
    let login = login_autoapprove(&client, &app.base_url, "host-dedup").await;

    let now_ms = chrono::Utc::now().timestamp_millis();
    let events = dedup_pair_events("host-dedup", "sess-dedup", "tu-1", now_ms);
    let payload = gzip(&ingest_body_bytes(&events));
    let ingest_resp = client
        .post(format!("{}/v1/events", app.base_url))
        .bearer_auth(&login.token)
        .header("Content-Encoding", "gzip")
        .body(payload)
        .send()
        .await
        .unwrap();
    assert_eq!(ingest_resp.status(), 200, "{:?}", ingest_resp.text().await);

    let resp = client
        .get(format!("{}/v1/query/tools", app.base_url))
        .bearer_auth(&login.token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();

    let row = find_row(&body, "tool_name", "mcp__gh__search")
        .unwrap_or_else(|| panic!("no mcp__gh__search row in {body:?}"));
    let failures_idx = col_value(&body, "failures");
    let calls_idx = col_value(&body, "calls");
    let p50_idx = col_value(&body, "p50_duration_ms");
    assert_eq!(
        row[failures_idx], 1,
        "hook+otel duplicate pair must dedup to 1 failure, not 2: {body:?}"
    );
    assert_eq!(
        row[calls_idx], 1,
        "the lone tool.call is untouched: {body:?}"
    );
    assert_eq!(
        row[p50_idx], 250.0,
        "duration must come from the winning (OTel) row: {body:?}"
    );

    app.teardown().await;
}

/// Same fixture, checked against `/v1/query/mcp` (`failures` there) and
/// `/v1/query/today?dt_from&dt_to` (its `failures` is the generic
/// `success = false` count across all event types -- confirms the dedup
/// also holds for that broader aggregation, not only the `tool.result`-typed
/// `FILTER` in `tools`/`mcp`/`skills`).
#[tokio::test]
async fn mcp_and_today_queries_also_dedup_the_same_pair() {
    let app = TestApp::spawn(SpawnOpts {
        dev_autoapprove: true,
        ..Default::default()
    })
    .await;
    let client = reqwest::Client::new();
    let login = login_autoapprove(&client, &app.base_url, "host-dedup-2").await;

    let now_ms = chrono::Utc::now().timestamp_millis();
    let dt = kikimimi_schema::dt_of(now_ms);
    let events = dedup_pair_events("host-dedup-2", "sess-dedup-2", "tu-2", now_ms);
    let payload = gzip(&ingest_body_bytes(&events));
    let ingest_resp = client
        .post(format!("{}/v1/events", app.base_url))
        .bearer_auth(&login.token)
        .header("Content-Encoding", "gzip")
        .body(payload)
        .send()
        .await
        .unwrap();
    assert_eq!(ingest_resp.status(), 200);

    let resp = client
        .get(format!("{}/v1/query/mcp", app.base_url))
        .bearer_auth(&login.token)
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let row =
        find_row(&body, "mcp_server", "gh").unwrap_or_else(|| panic!("no gh row in {body:?}"));
    let failures_idx = col_value(&body, "failures");
    assert_eq!(row[failures_idx], 1, "mcp: {body:?}");

    let resp = client
        .get(format!(
            "{}/v1/query/today?dt_from={dt}&dt_to={dt}",
            app.base_url
        ))
        .bearer_auth(&login.token)
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let failures_idx = col_value(&body, "failures");
    // today is grouped by model (all NULL here) -- a single row either way.
    let rows = body["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "today: {body:?}");
    assert_eq!(rows[0][failures_idx], 1, "today: {body:?}");

    app.teardown().await;
}
