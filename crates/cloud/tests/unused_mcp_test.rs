mod support;

use kikimimi_schema::{event_type, Event};
use support::{gzip, ingest_body_bytes, login_autoapprove, web_login, SpawnOpts, TestApp};

/// A `session.start` row carrying the `configured_mcp_servers` snapshot
/// (architecture.md §5.1, `crates/cli/src/mcp_config.rs`'s enrichment).
fn session_start_event(
    event_id: &str,
    host_id: &str,
    session_id: &str,
    configured: &[&str],
) -> Event {
    let now_ms = chrono::Utc::now().timestamp_millis();
    Event {
        event_id: event_id.to_string(),
        ts: now_ms,
        dt: kikimimi_schema::dt_of(now_ms),
        host_id: host_id.to_string(),
        agent: "claude-code".to_string(),
        source: "hook".to_string(),
        session_id: Some(session_id.to_string()),
        event_type: event_type::SESSION_START.to_string(),
        configured_mcp_servers: Some(serde_json::to_string(configured).unwrap()),
        ..Default::default()
    }
}

fn mcp_tool_call_event(event_id: &str, host_id: &str, session_id: &str, mcp_server: &str) -> Event {
    let now_ms = chrono::Utc::now().timestamp_millis();
    Event {
        event_id: event_id.to_string(),
        ts: now_ms,
        dt: kikimimi_schema::dt_of(now_ms),
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
    value: &str,
) -> Option<&'a Vec<serde_json::Value>> {
    rows.iter()
        .map(|r| r.as_array().unwrap())
        .find(|r| r[0] == value)
}

async fn ingest(client: &reqwest::Client, base_url: &str, token: &str, events: &[Event]) {
    let resp = client
        .post(format!("{base_url}/v1/events"))
        .bearer_auth(token)
        .header("Content-Encoding", "gzip")
        .body(gzip(&ingest_body_bytes(events)))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

/// The case `query_sql.rs`'s old cloud `unused-mcp` proxy structurally could
/// never show: a server *configured but never once called*. With a real
/// `session.start` snapshot (`configured_mcp_servers = ["github","notion"]`)
/// and a single `tool.call` for `github` only, `/web/q/unused-mcp` must
/// report `notion` (calls 0, configured true) and `github` (calls 1).
#[tokio::test]
async fn web_unused_mcp_reports_configured_but_never_called_server_from_a_real_snapshot() {
    let app = TestApp::spawn(SpawnOpts {
        dev_autoapprove: true,
        ..Default::default()
    })
    .await;
    let client = reqwest::Client::new();
    let login = login_autoapprove(&client, &app.base_url, "host-unused-mcp-web").await;
    let web = web_login(&client, &app.base_url, &login.email).await;
    assert_eq!(
        web.org_id, login.org_id,
        "device login and web login for the same email must land in the same (personal) org"
    );

    ingest(
        &client,
        &app.base_url,
        &login.token,
        &[
            session_start_event(
                "sess-start-1",
                "host-unused-mcp-web",
                "sess-1",
                &["github", "notion"],
            ),
            mcp_tool_call_event("call-github-1", "host-unused-mcp-web", "sess-1", "github"),
        ],
    )
    .await;

    let resp = client
        .get(format!("{}/web/q/unused-mcp?days=30", app.base_url))
        .header(reqwest::header::COOKIE, &web.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["columns"],
        serde_json::json!([
            "mcp_server",
            "configured",
            "calls",
            "distinct_sessions",
            "last_called_dt",
            "sessions_configured",
            "configured_from_snapshot"
        ])
    );
    let rows = body["rows"].as_array().unwrap();

    let notion = row_where_first_col_is(rows, "notion")
        .unwrap_or_else(|| panic!("expected a notion row: {body:?}"));
    assert_eq!(
        notion[1].as_bool(),
        Some(true),
        "notion configured: {notion:?}"
    );
    assert_eq!(notion[2].as_i64(), Some(0), "notion calls: {notion:?}");
    assert_eq!(
        notion[3].as_i64(),
        Some(0),
        "notion distinct_sessions: {notion:?}"
    );
    assert_eq!(
        notion[4],
        serde_json::Value::Null,
        "notion last_called_dt: {notion:?}"
    );
    assert_eq!(
        notion[5].as_i64(),
        Some(1),
        "notion sessions_configured: {notion:?}"
    );
    assert_eq!(
        notion[6].as_bool(),
        Some(true),
        "notion configured_from_snapshot: {notion:?}"
    );

    let github = row_where_first_col_is(rows, "github")
        .unwrap_or_else(|| panic!("expected a github row: {body:?}"));
    assert_eq!(
        github[1].as_bool(),
        Some(true),
        "github configured: {github:?}"
    );
    assert_eq!(github[2].as_i64(), Some(1), "github calls: {github:?}");
    assert_eq!(
        github[6].as_bool(),
        Some(true),
        "github configured_from_snapshot: {github:?}"
    );

    // "notion" (never called) must sort before "github" (called once) —
    // never-called-but-configured first, then by calls ascending.
    let notion_idx = rows.iter().position(|r| r[0] == "notion").unwrap();
    let github_idx = rows.iter().position(|r| r[0] == "github").unwrap();
    assert!(
        notion_idx < github_idx,
        "unused-but-configured must sort first: {rows:?}"
    );

    app.teardown().await;
}

/// No `session.start` row in range carries `configured_mcp_servers` (older
/// clients that predate this column) -> `/web/q/unused-mcp` falls back to
/// the observed-in-the-last-30-days proxy and reports
/// `configured_from_snapshot: false` so the UI can say so.
#[tokio::test]
async fn web_unused_mcp_falls_back_to_the_observed_proxy_when_no_snapshot_exists() {
    let app = TestApp::spawn(SpawnOpts {
        dev_autoapprove: true,
        ..Default::default()
    })
    .await;
    let client = reqwest::Client::new();
    let login = login_autoapprove(&client, &app.base_url, "host-unused-mcp-proxy").await;
    let web = web_login(&client, &app.base_url, &login.email).await;

    // Only an observed tool.call, no session.start snapshot at all.
    ingest(
        &client,
        &app.base_url,
        &login.token,
        &[mcp_tool_call_event(
            "call-linear-1",
            "host-unused-mcp-proxy",
            "sess-1",
            "linear",
        )],
    )
    .await;

    let resp = client
        .get(format!("{}/web/q/unused-mcp?days=30", app.base_url))
        .header(reqwest::header::COOKIE, &web.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let rows = body["rows"].as_array().unwrap();

    let linear = row_where_first_col_is(rows, "linear")
        .unwrap_or_else(|| panic!("expected a linear row via the observed proxy: {body:?}"));
    assert_eq!(
        linear[1].as_bool(),
        Some(true),
        "linear configured (proxy): {linear:?}"
    );
    assert_eq!(linear[2].as_i64(), Some(1), "linear calls: {linear:?}");
    assert_eq!(
        linear[6].as_bool(),
        Some(false),
        "configured_from_snapshot must be false under the proxy fallback: {linear:?}"
    );

    app.teardown().await;
}

/// `/v1/query/unused-mcp` (`query_sql.rs`, bearer-token contract) stays
/// backward compatible on its 4-column shape while gaining the same
/// snapshot-over-proxy fix: a configured-but-never-called server now shows
/// up even though it has zero `tool.call` rows at all.
#[tokio::test]
async fn v1_query_unused_mcp_reports_configured_but_never_called_server_from_a_real_snapshot() {
    let app = TestApp::spawn(SpawnOpts {
        dev_autoapprove: true,
        ..Default::default()
    })
    .await;
    let client = reqwest::Client::new();
    let login = login_autoapprove(&client, &app.base_url, "host-v1-unused-mcp").await;

    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    ingest(
        &client,
        &app.base_url,
        &login.token,
        &[
            session_start_event(
                "sess-start-2",
                "host-v1-unused-mcp",
                "sess-2",
                &["github", "notion"],
            ),
            mcp_tool_call_event("call-github-2", "host-v1-unused-mcp", "sess-2", "github"),
        ],
    )
    .await;

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
    assert_eq!(
        body["columns"],
        serde_json::json!([
            "mcp_server",
            "configured",
            "calls_in_range",
            "last_called_dt"
        ]),
        "the /v1/query/unused-mcp contract shape must stay exactly as before: {body:?}"
    );
    let rows = body["rows"].as_array().unwrap();

    let notion = row_where_first_col_is(rows, "notion").unwrap_or_else(|| {
        panic!(
            "notion was configured (session.start snapshot) but never called at all -- the old \
             observed-only proxy could never show this row, the snapshot must: {body:?}"
        )
    });
    assert_eq!(
        notion[1].as_bool(),
        Some(true),
        "configured column: {notion:?}"
    );
    assert_eq!(notion[2].as_i64(), Some(0), "calls_in_range: {notion:?}");

    assert!(
        row_where_first_col_is(rows, "github").is_none(),
        "github was called in-range, must not appear as unused: {body:?}"
    );

    app.teardown().await;
}
