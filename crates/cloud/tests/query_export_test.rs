mod support;

use chrono::{Duration, Utc};
use kikimimi_schema::{event_type, Event};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use support::{gzip, ingest_body_bytes, login_as, login_autoapprove, sample_event, SpawnOpts, TestApp};

#[tokio::test]
async fn named_queries_respond_with_columns_and_rows_shape() {
    let app = TestApp::spawn(SpawnOpts {
        dev_autoapprove: true,
        ..Default::default()
    })
    .await;
    let client = reqwest::Client::new();
    let login = login_autoapprove(&client, &app.base_url, "host-query").await;

    for name in ["today", "tools", "mcp", "bypass", "reach", "unused-mcp", "schema-tax"] {
        let resp = client
            .get(format!("{}/v1/query/{name}", app.base_url))
            .bearer_auth(&login.token)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "query {name}");
        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(body["columns"].is_array(), "{name}: {body:?}");
        assert!(body["rows"].is_array(), "{name}: {body:?}");
    }

    let resp = client
        .get(format!("{}/v1/query/not-a-real-query", app.base_url))
        .bearer_auth(&login.token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);

    app.teardown().await;
}

#[tokio::test]
async fn export_returns_parquet_with_expected_row_count_and_column_order() {
    let app = TestApp::spawn(SpawnOpts {
        dev_autoapprove: true,
        ..Default::default()
    })
    .await;
    let client = reqwest::Client::new();
    let login = login_autoapprove(&client, &app.base_url, "host-export").await;

    let events = vec![
        sample_event("exp-1", "host-export", "sess-1"),
        sample_event("exp-2", "host-export", "sess-1"),
        sample_event("exp-3", "host-export", "sess-2"),
    ];
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

    let export_resp = client
        .get(format!("{}/v1/export", app.base_url))
        .bearer_auth(&login.token)
        .send()
        .await
        .unwrap();
    assert_eq!(export_resp.status(), 200);
    assert_eq!(
        export_resp.headers().get("content-type").unwrap(),
        "application/vnd.apache.parquet"
    );
    let bytes = export_resp.bytes().await.unwrap();

    let reader_builder = ParquetRecordBatchReaderBuilder::try_new(bytes).unwrap();
    let field_names: Vec<&str> = reader_builder
        .schema()
        .fields()
        .iter()
        .map(|f| f.name().as_str())
        .collect();
    assert_eq!(
        field_names, kikimimi_schema::COLUMNS,
        "export column order must match kikimimi_schema::COLUMNS exactly"
    );

    let mut reader = reader_builder.build().unwrap();
    let mut total_rows = 0usize;
    while let Some(batch) = reader.next() {
        total_rows += batch.unwrap().num_rows();
    }
    assert_eq!(total_rows, 3, "expected exactly the 3 ingested rows");

    app.teardown().await;
}

/// Security review finding: `rls_test.rs` covers `/v1/query/*` cross-tenant
/// isolation and a direct `kikimimi_app`-role connection, but export (also named
/// in architecture.md §8/§11's "クロステナント漏洩の回帰テストを Stage 0 から
/// 持つ" and §12's Stage 0 success criterion) had no same-shape leak test.
#[tokio::test]
async fn export_scopes_to_the_callers_org_only() {
    let app = TestApp::spawn(SpawnOpts::default()).await; // autoapprove OFF: real two-tenant login
    let client = reqwest::Client::new();

    let org_a = login_as(&client, &app.base_url, "host-export-a", "export-a@example.com").await;
    let org_b = login_as(&client, &app.base_url, "host-export-b", "export-b@example.com").await;
    assert_ne!(org_a.org_id, org_b.org_id, "sanity: two distinct orgs");

    let ev_a = sample_event("export-secret-a", "host-export-a", "sess-a");
    let payload_a = gzip(&ingest_body_bytes(&[ev_a]));
    let resp = client
        .post(format!("{}/v1/events", app.base_url))
        .bearer_auth(&org_a.token)
        .header("Content-Encoding", "gzip")
        .body(payload_a)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Org B's export must contain zero rows — none of org A's data.
    let resp = client
        .get(format!("{}/v1/export", app.base_url))
        .bearer_auth(&org_b.token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let bytes = resp.bytes().await.unwrap();
    let reader_builder = ParquetRecordBatchReaderBuilder::try_new(bytes).unwrap();
    let mut reader = reader_builder.build().unwrap();
    let mut org_b_rows = 0usize;
    while let Some(batch) = reader.next() {
        org_b_rows += batch.unwrap().num_rows();
    }
    assert_eq!(
        org_b_rows, 0,
        "org B's export must not contain any of org A's rows"
    );

    // Org A's own export, same endpoint, does see its own row — proves the
    // emptiness above is tenant isolation, not export being broken/always-empty.
    let resp = client
        .get(format!("{}/v1/export", app.base_url))
        .bearer_auth(&org_a.token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let bytes = resp.bytes().await.unwrap();
    let reader_builder = ParquetRecordBatchReaderBuilder::try_new(bytes).unwrap();
    let mut reader = reader_builder.build().unwrap();
    let mut org_a_rows = 0usize;
    while let Some(batch) = reader.next() {
        org_a_rows += batch.unwrap().num_rows();
    }
    assert_eq!(org_a_rows, 1, "org A must see its own row in its own export");

    app.teardown().await;
}

fn api_request_event(
    event_id: &str,
    host_id: &str,
    session_id: &str,
    ts: i64,
    dt: &str,
    input_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    output_tokens: i64,
) -> Event {
    Event {
        event_id: event_id.to_string(),
        ts,
        dt: dt.to_string(),
        host_id: host_id.to_string(),
        agent: "claude-code".to_string(),
        source: "otel".to_string(),
        session_id: Some(session_id.to_string()),
        event_type: event_type::API_REQUEST.to_string(),
        input_tokens: Some(input_tokens),
        cache_read_tokens: Some(cache_read_tokens),
        cache_write_tokens: Some(cache_write_tokens),
        output_tokens: Some(output_tokens),
        usage_source: Some("otel".to_string()),
        ..Default::default()
    }
}

fn mcp_tool_call_event(event_id: &str, host_id: &str, session_id: &str, ts: i64, dt: &str, mcp_server: &str) -> Event {
    Event {
        event_id: event_id.to_string(),
        ts,
        dt: dt.to_string(),
        host_id: host_id.to_string(),
        agent: "claude-code".to_string(),
        source: "hook".to_string(),
        session_id: Some(session_id.to_string()),
        event_type: event_type::TOOL_CALL.to_string(),
        tool_name: Some(format!("mcp__{mcp_server}__search")),
        tool_kind: Some("mcp".to_string()),
        mcp_server: Some(mcp_server.to_string()),
        ..Default::default()
    }
}

fn row_where_first_col_is<'a>(rows: &'a [serde_json::Value], want: &str) -> Option<&'a Vec<serde_json::Value>> {
    rows.iter()
        .map(|r| r.as_array().unwrap())
        .find(|r| r[0].as_str() == Some(want))
}

/// `schema-tax`: two sessions with hand-picked token counts so
/// `first_input_tokens` (the earliest `api.request`'s input+cache_read) and
/// `fixed_share_pct` can be checked against an exact hand-computed value,
/// plus the `TOTAL` rollup row summing both sessions.
#[tokio::test]
async fn schema_tax_query_computes_first_request_fixed_share_and_a_totals_row() {
    let app = TestApp::spawn(SpawnOpts {
        dev_autoapprove: true,
        ..Default::default()
    })
    .await;
    let client = reqwest::Client::new();
    let login = login_autoapprove(&client, &app.base_url, "host-schema-tax").await;

    let dt = "2023-11-14";
    let events = vec![
        // Session A: turn 1 = 10 input + 90 cache_read (first_input_tokens = 100);
        // turn 2 = 10 input + 190 cache_read. Session totals: input=20,
        // cache_read=280 -> fixed_share_pct = 100/300*100 = 33.333...%.
        api_request_event("tax-a-1", "host-schema-tax", "sess-a", 1_700_000_000_000, dt, 10, 90, 0, 5),
        api_request_event("tax-a-2", "host-schema-tax", "sess-a", 1_700_000_001_000, dt, 10, 190, 0, 5),
        // Session B: single turn = 50 input + 50 cache_read (first_input_tokens
        // = 100 = its whole total) -> fixed_share_pct = 100%.
        api_request_event("tax-b-1", "host-schema-tax", "sess-b", 1_700_000_000_500, dt, 50, 50, 0, 5),
    ];
    let ingest_resp = client
        .post(format!("{}/v1/events", app.base_url))
        .bearer_auth(&login.token)
        .header("Content-Encoding", "gzip")
        .body(gzip(&ingest_body_bytes(&events)))
        .send()
        .await
        .unwrap();
    assert_eq!(ingest_resp.status(), 200);

    let resp = client
        .get(format!(
            "{}/v1/query/schema-tax?dt_from={dt}&dt_to={dt}",
            app.base_url
        ))
        .bearer_auth(&login.token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let columns: Vec<&str> = body["columns"].as_array().unwrap().iter().map(|c| c.as_str().unwrap()).collect();
    assert_eq!(
        columns,
        vec![
            "session_id",
            "api_requests",
            "input_tokens",
            "cache_read_tokens",
            "cache_write_tokens",
            "output_tokens",
            "first_input_tokens",
            "fixed_share_pct",
        ]
    );
    let rows = body["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 3, "sess-a + sess-b + TOTAL: {body:?}");

    let a = row_where_first_col_is(rows, "sess-a").expect("sess-a row");
    assert_eq!(a[1].as_i64(), Some(2), "sess-a api_requests");
    assert_eq!(a[2].as_i64(), Some(20), "sess-a input_tokens");
    assert_eq!(a[3].as_i64(), Some(280), "sess-a cache_read_tokens");
    assert_eq!(a[6].as_i64(), Some(100), "sess-a first_input_tokens (turn 1 only)");
    let a_pct = a[7].as_f64().expect("sess-a fixed_share_pct");
    assert!((a_pct - 100.0 / 3.0).abs() < 0.01, "sess-a fixed_share_pct = {a_pct}");

    let b = row_where_first_col_is(rows, "sess-b").expect("sess-b row");
    assert_eq!(b[6].as_i64(), Some(100), "sess-b first_input_tokens");
    assert_eq!(b[7].as_f64(), Some(100.0), "sess-b fixed_share_pct: its only turn IS the fixed context");

    let total = row_where_first_col_is(rows, "TOTAL").expect("TOTAL row");
    assert_eq!(total[1].as_i64(), Some(3), "TOTAL api_requests");
    assert_eq!(total[2].as_i64(), Some(70), "TOTAL input_tokens");
    assert_eq!(total[3].as_i64(), Some(330), "TOTAL cache_read_tokens");
    assert_eq!(
        total[6].as_i64(),
        Some(200),
        "TOTAL first_input_tokens = sum of each session's own first_input_tokens"
    );
    assert_eq!(total[7].as_f64(), Some(50.0), "TOTAL fixed_share_pct = 200/400*100");

    app.teardown().await;
}

/// `unused-mcp` (cloud variant): kikimimi cloud has no local config files, so it
/// treats "observed via `tool.call` in the trailing 30 days" as its proxy
/// for "configured", then reports only the servers with **zero** calls in
/// the caller's queried range. Exercises both halves: a server called
/// 10 days ago (within the 30-day observation window) shows up as unused
/// when queried over a *different* range, but drops out entirely when
/// queried over the range that actually contains its call.
#[tokio::test]
async fn unused_mcp_query_reports_recently_observed_servers_with_zero_calls_in_the_queried_range() {
    let app = TestApp::spawn(SpawnOpts {
        dev_autoapprove: true,
        ..Default::default()
    })
    .await;
    let client = reqwest::Client::new();
    let login = login_autoapprove(&client, &app.base_url, "host-unused-mcp").await;

    let called_at = Utc::now() - Duration::days(10);
    let called_dt = called_at.format("%Y-%m-%d").to_string();
    let today = Utc::now().format("%Y-%m-%d").to_string();

    let events = vec![mcp_tool_call_event(
        "unused-mcp-1",
        "host-unused-mcp",
        "sess-unused",
        called_at.timestamp_millis(),
        &called_dt,
        "linear",
    )];
    let ingest_resp = client
        .post(format!("{}/v1/events", app.base_url))
        .bearer_auth(&login.token)
        .header("Content-Encoding", "gzip")
        .body(gzip(&ingest_body_bytes(&events)))
        .send()
        .await
        .unwrap();
    assert_eq!(ingest_resp.status(), 200);

    // Queried range is "today" only, which does NOT contain the call made
    // 10 days ago -> "linear" was observed in the last 30 days but has zero
    // calls in this range, so it must show up as unused.
    let resp = client
        .get(format!(
            "{}/v1/query/unused-mcp?dt_from={today}&dt_to={today}",
            app.base_url
        ))
        .bearer_auth(&login.token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let rows = body["rows"].as_array().unwrap();
    let row = row_where_first_col_is(rows, "linear").unwrap_or_else(|| panic!("expected linear row: {body:?}"));
    assert_eq!(row[1].as_bool(), Some(true), "configured column");
    assert_eq!(row[2].as_i64(), Some(0), "calls_in_range");
    assert_eq!(row[3].as_str(), Some(called_dt.as_str()), "last_called_dt");

    // Queried range now DOES contain the call -> "linear" has a nonzero
    // calls_in_range, so it must be excluded from the "unused" result.
    let resp = client
        .get(format!(
            "{}/v1/query/unused-mcp?dt_from={called_dt}&dt_to={called_dt}",
            app.base_url
        ))
        .bearer_auth(&login.token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let rows = body["rows"].as_array().unwrap();
    assert!(
        row_where_first_col_is(rows, "linear").is_none(),
        "linear was called in-range, must not appear as unused: {body:?}"
    );

    app.teardown().await;
}
