mod support;

use chrono::{Duration, Utc};
use kikimimi_schema::{event_type, Event};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use support::{
    gzip, ingest_body_bytes, login_as, login_autoapprove, sample_event, SpawnOpts, TestApp,
};

#[tokio::test]
async fn named_queries_respond_with_columns_and_rows_shape() {
    let app = TestApp::spawn(SpawnOpts {
        dev_autoapprove: true,
        ..Default::default()
    })
    .await;
    let client = reqwest::Client::new();
    let login = login_autoapprove(&client, &app.base_url, "host-query").await;

    for name in [
        "today",
        "tools",
        "mcp",
        "bypass",
        "reach",
        "unused-mcp",
        "skills",
        "schema-tax",
        "thrash",
        "mcp-tax",
        "patterns",
        "subagents",
    ] {
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
        field_names,
        kikimimi_schema::COLUMNS,
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

    let org_a = login_as(
        &client,
        &app.base_url,
        "host-export-a",
        "export-a@example.com",
    )
    .await;
    let org_b = login_as(
        &client,
        &app.base_url,
        "host-export-b",
        "export-b@example.com",
    )
    .await;
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
    assert_eq!(
        org_a_rows, 1,
        "org A must see its own row in its own export"
    );

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

fn mcp_tool_call_event(
    event_id: &str,
    host_id: &str,
    session_id: &str,
    ts: i64,
    dt: &str,
    mcp_server: &str,
) -> Event {
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

fn row_where_first_col_is<'a>(
    rows: &'a [serde_json::Value],
    want: &str,
) -> Option<&'a Vec<serde_json::Value>> {
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
        api_request_event(
            "tax-a-1",
            "host-schema-tax",
            "sess-a",
            1_700_000_000_000,
            dt,
            10,
            90,
            0,
            5,
        ),
        api_request_event(
            "tax-a-2",
            "host-schema-tax",
            "sess-a",
            1_700_000_001_000,
            dt,
            10,
            190,
            0,
            5,
        ),
        // Session B: single turn = 50 input + 50 cache_read (first_input_tokens
        // = 100 = its whole total) -> fixed_share_pct = 100%.
        api_request_event(
            "tax-b-1",
            "host-schema-tax",
            "sess-b",
            1_700_000_000_500,
            dt,
            50,
            50,
            0,
            5,
        ),
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
    let columns: Vec<&str> = body["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap())
        .collect();
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
    assert_eq!(
        a[6].as_i64(),
        Some(100),
        "sess-a first_input_tokens (turn 1 only)"
    );
    let a_pct = a[7].as_f64().expect("sess-a fixed_share_pct");
    assert!(
        (a_pct - 100.0 / 3.0).abs() < 0.01,
        "sess-a fixed_share_pct = {a_pct}"
    );

    let b = row_where_first_col_is(rows, "sess-b").expect("sess-b row");
    assert_eq!(b[6].as_i64(), Some(100), "sess-b first_input_tokens");
    assert_eq!(
        b[7].as_f64(),
        Some(100.0),
        "sess-b fixed_share_pct: its only turn IS the fixed context"
    );

    let total = row_where_first_col_is(rows, "TOTAL").expect("TOTAL row");
    assert_eq!(total[1].as_i64(), Some(3), "TOTAL api_requests");
    assert_eq!(total[2].as_i64(), Some(70), "TOTAL input_tokens");
    assert_eq!(total[3].as_i64(), Some(330), "TOTAL cache_read_tokens");
    assert_eq!(
        total[6].as_i64(),
        Some(200),
        "TOTAL first_input_tokens = sum of each session's own first_input_tokens"
    );
    assert_eq!(
        total[7].as_f64(),
        Some(50.0),
        "TOTAL fixed_share_pct = 200/400*100"
    );

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
    let row = row_where_first_col_is(rows, "linear")
        .unwrap_or_else(|| panic!("expected linear row: {body:?}"));
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

fn session_start_with_configured(
    event_id: &str,
    host_id: &str,
    session_id: &str,
    ts: i64,
    dt: &str,
    configured: &[&str],
) -> Event {
    Event {
        event_id: event_id.to_string(),
        ts,
        dt: dt.to_string(),
        host_id: host_id.to_string(),
        agent: "claude-code".to_string(),
        source: "hook".to_string(),
        session_id: Some(session_id.to_string()),
        event_type: event_type::SESSION_START.to_string(),
        configured_mcp_servers: Some(serde_json::to_string(configured).unwrap()),
        ..Default::default()
    }
}

/// `mcp-tax` (KKM-13): three sessions with hand-picked configs and token
/// counts so the equal-split allocation, the unused share and the
/// with/without-server contrast can each be checked against a value computed
/// by hand.
///
/// - sess-a: configured [gh, jira], 2 api.requests, first_input_tokens 1000,
///   calls gh only.            -> gh: 1000/2*2 = 1000 used; jira: 1000 unused
/// - sess-b: configured [gh],   1 api.request,  first_input_tokens 400,
///   calls nothing.            -> gh: 400 unused
/// - sess-c: configured [jira], 3 api.requests, first_input_tokens 900,
///   calls jira.               -> jira: 900/1*3 = 2700 used
/// - sess-d: configured [gh], no OTel usage at all -> counted, not priced.
#[tokio::test]
async fn mcp_tax_query_allocates_fixed_context_per_configured_server() {
    let app = TestApp::spawn(SpawnOpts {
        dev_autoapprove: true,
        ..Default::default()
    })
    .await;
    let client = reqwest::Client::new();
    let login = login_autoapprove(&client, &app.base_url, "host-mcp-tax").await;
    let h = "host-mcp-tax";
    let dt = "2023-11-14";
    let t0 = 1_700_000_000_000;
    let events = vec![
        session_start_with_configured("mt-a-s", h, "sess-a", t0, dt, &["gh", "jira"]),
        api_request_event("mt-a-1", h, "sess-a", t0 + 1, dt, 100, 900, 0, 5),
        api_request_event("mt-a-2", h, "sess-a", t0 + 2, dt, 10, 1500, 0, 5),
        mcp_tool_call_event("mt-a-c", h, "sess-a", t0 + 3, dt, "gh"),
        session_start_with_configured("mt-b-s", h, "sess-b", t0 + 10, dt, &["gh"]),
        api_request_event("mt-b-1", h, "sess-b", t0 + 11, dt, 400, 0, 0, 5),
        session_start_with_configured("mt-c-s", h, "sess-c", t0 + 20, dt, &["jira"]),
        api_request_event("mt-c-1", h, "sess-c", t0 + 21, dt, 900, 0, 0, 5),
        api_request_event("mt-c-2", h, "sess-c", t0 + 22, dt, 10, 2000, 0, 5),
        api_request_event("mt-c-3", h, "sess-c", t0 + 23, dt, 10, 2000, 0, 5),
        mcp_tool_call_event("mt-c-c", h, "sess-c", t0 + 24, dt, "jira"),
        session_start_with_configured("mt-d-s", h, "sess-d", t0 + 30, dt, &["gh"]),
    ];
    let ingest_resp = client
        .post(format!("{}/v1/events", app.base_url))
        .bearer_auth(&login.token)
        .header("Content-Encoding", "gzip")
        .header("Content-Type", "application/x-ndjson")
        .body(gzip(&ingest_body_bytes(&events)))
        .send()
        .await
        .unwrap();
    assert_eq!(ingest_resp.status(), 200);

    let body: serde_json::Value = client
        .get(format!(
            "{}/v1/query/mcp-tax?dt_from={dt}&dt_to={dt}",
            app.base_url
        ))
        .bearer_auth(&login.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let columns: Vec<&str> = body["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap())
        .collect();
    assert_eq!(
        columns,
        [
            "mcp_server",
            "sessions_configured",
            "sessions_used",
            "sessions_unused",
            "sessions_with_usage",
            "fixed_tokens_est",
            "unused_tokens_est",
            "marginal_first_tokens_est"
        ]
    );
    let rows = body["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{rows:?}");

    let gh = row_where_first_col_is(rows, "gh").expect("gh row");
    assert_eq!(gh[1], 3, "configured in a, b, d");
    assert_eq!(gh[2], 1, "used in a");
    assert_eq!(gh[3], 2, "unused in b, d");
    assert_eq!(gh[4], 2, "a and b have usage; d does not");
    assert_eq!(gh[5], 1000 + 400, "a: 1000/2*2, b: 400/1*1");
    assert_eq!(gh[6], 400, "only b is unused *and* priced");
    // with gh: first_input_tokens {1000, 400} -> median 700; without gh: {900} -> 900.
    assert_eq!(gh[7], 700 - 900);

    let jira = row_where_first_col_is(rows, "jira").expect("jira row");
    assert_eq!(jira[1], 2);
    assert_eq!(jira[2], 1);
    assert_eq!(jira[3], 1);
    assert_eq!(jira[5], 1000 + 2700);
    assert_eq!(jira[6], 1000, "carried in sess-a without a single call");
    // with jira: {1000, 900} -> 950; without: {400} -> 400.
    assert_eq!(jira[7], 950 - 400);

    app.teardown().await;
}

/// `subagents` (KKM-15): one session with two subagents — `ag1` priced from
/// its transcript `api.request` row, `ag2` unpriced — plus a session with no
/// subagents that must not appear. Durations come from `subagent.stop`
/// (hook and transcript both present for ag1: the larger wins), the
/// session's own span is first-to-last event.
#[tokio::test]
async fn subagents_query_reports_fanout_duration_and_usage_coverage() {
    let app = TestApp::spawn(SpawnOpts {
        dev_autoapprove: true,
        ..Default::default()
    })
    .await;
    let client = reqwest::Client::new();
    let login = login_autoapprove(&client, &app.base_url, "host-sub").await;
    let h = "host-sub";
    let dt = "2023-11-15";
    let t0 = 1_700_100_000_000;
    let t = |off: i64| t0 + off * 1000;
    let ev = |id: &str, ts: i64, source: &str, ty: &str| Event {
        event_id: id.to_string(),
        ts,
        dt: dt.to_string(),
        host_id: h.to_string(),
        agent: "claude-code".to_string(),
        source: source.to_string(),
        session_id: Some("sub-a".to_string()),
        event_type: ty.to_string(),
        ..Default::default()
    };
    let events = vec![
        ev("sa-start", t(0), "hook", event_type::SESSION_START),
        Event {
            tool_name: Some("Read".into()),
            query_source: Some("main".into()),
            ..ev("sa-call0", t(1), "hook", event_type::TOOL_CALL)
        },
        Event {
            input_tokens: Some(1000),
            output_tokens: Some(100),
            query_source: Some("main".into()),
            ..ev("sa-api0", t(2), "otel", event_type::API_REQUEST)
        },
        Event {
            tool_name: Some("Grep".into()),
            agent_id: Some("ag1".into()),
            agent_type: Some("Explore".into()),
            query_source: Some("subagent".into()),
            ..ev("sa-call1", t(3), "hook", event_type::TOOL_CALL)
        },
        Event {
            tool_name: Some("Read".into()),
            agent_id: Some("ag1".into()),
            agent_type: Some("Explore".into()),
            query_source: Some("subagent".into()),
            ..ev("sa-call2", t(4), "hook", event_type::TOOL_CALL)
        },
        Event {
            input_tokens: Some(500),
            output_tokens: Some(50),
            agent_id: Some("ag1".into()),
            query_source: Some("subagent".into()),
            usage_source: Some("log".into()),
            ..ev("sa-api1-log", t(5), "log", event_type::API_REQUEST)
        },
        Event {
            input_tokens: Some(500),
            output_tokens: Some(50),
            query_source: Some("subagent".into()),
            ..ev("sa-api1-otel", t(5) + 1, "otel", event_type::API_REQUEST)
        },
        Event {
            agent_id: Some("ag1".into()),
            agent_type: Some("Explore".into()),
            correlation_key: Some("ag1".into()),
            duration_ms: Some(4000),
            ..ev("sa-stop1-hook", t(7), "hook", event_type::SUBAGENT_STOP)
        },
        Event {
            agent_id: Some("ag1".into()),
            correlation_key: Some("ag1".into()),
            duration_ms: Some(3000),
            ..ev("sa-stop1-log", t(7), "log", event_type::SUBAGENT_STOP)
        },
        Event {
            tool_name: Some("Bash".into()),
            agent_id: Some("ag2".into()),
            agent_type: Some("general-purpose".into()),
            query_source: Some("subagent".into()),
            ..ev("sa-call3", t(6), "hook", event_type::TOOL_CALL)
        },
        Event {
            agent_id: Some("ag2".into()),
            agent_type: Some("general-purpose".into()),
            correlation_key: Some("ag2".into()),
            duration_ms: Some(2000),
            ..ev("sa-stop2-hook", t(8), "hook", event_type::SUBAGENT_STOP)
        },
        // A session without subagents: not a row.
        Event {
            session_id: Some("sub-b".into()),
            ..ev("sb-start", t(0), "hook", event_type::SESSION_START)
        },
        Event {
            session_id: Some("sub-b".into()),
            input_tokens: Some(10),
            output_tokens: Some(1),
            ..ev("sb-api", t(1), "otel", event_type::API_REQUEST)
        },
    ];
    let ingest_resp = client
        .post(format!("{}/v1/events", app.base_url))
        .bearer_auth(&login.token)
        .header("Content-Encoding", "gzip")
        .header("Content-Type", "application/x-ndjson")
        .body(gzip(&ingest_body_bytes(&events)))
        .send()
        .await
        .unwrap();
    assert_eq!(ingest_resp.status(), 200);

    let body: serde_json::Value = client
        .get(format!(
            "{}/v1/query/subagents?dt_from={dt}&dt_to={dt}",
            app.base_url
        ))
        .bearer_auth(&login.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let columns: Vec<&str> = body["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap())
        .collect();
    assert_eq!(
        columns,
        [
            "session_id",
            "started_at",
            "subagents",
            "agent_types",
            "subagent_tool_calls",
            "tool_calls",
            "subagent_duration_ms",
            "session_duration_ms",
            "duration_share",
            "subagent_api_requests",
            "subagent_tokens_est",
            "session_tokens_est",
            "token_share",
            "subagents_with_usage"
        ]
    );
    let rows = body["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "sub-a + TOTAL: {rows:?}");
    let a = row_where_first_col_is(rows, "sub-a").expect("sub-a row");
    assert_eq!(a[2], 2, "ag1 + ag2");
    assert_eq!(a[3], "Explore,general-purpose");
    assert_eq!(a[4], 3, "subagent tool calls");
    assert_eq!(a[5], 4, "session tool calls incl. main");
    assert_eq!(
        a[6],
        4000 + 2000,
        "hook stop duration wins over the transcript's"
    );
    assert_eq!(a[7], 8000);
    assert_eq!(a[8], 0.75);
    assert_eq!(a[9], 1, "only the transcript row carries agent_id");
    assert_eq!(a[10], 550, "ag1 priced from its transcript api.request");
    assert_eq!(a[11], 1100 + 550, "OTel sum incl. the subagent's request");
    assert_eq!(a[12], 0.333);
    assert_eq!(a[13], 1, "ag2 had no usage anywhere");
    let total = row_where_first_col_is(rows, "TOTAL").expect("TOTAL row");
    assert_eq!(total[2], 2);
    assert_eq!(total[13], 1);
    assert!(total[1].is_null());

    app.teardown().await;
}
