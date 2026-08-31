//! `/web/*` (hosted web UI: WEB API CONTRACT) — login/logout/me, the
//! `/web/q/*` RLS scoping, session-cookie expiry, days/limit validation, and
//! static SPA serving. Live-PG integration tests, same harness as the rest
//! of this crate's `tests/` (`support::TestApp`).

mod support;

use support::{login_as, web_login, SpawnOpts, TestApp, TEST_INVITE_CODE};

/// Builds a minimal `tool.call` event dated "now" (unlike
/// `support::sample_event`, which is pinned to a fixed 2023 date and so
/// falls outside any `/web/q/*` `days<=365` window against a present-day
/// clock).
fn recent_tool_call_event(event_id: &str, host_id: &str, session_id: &str) -> kikimimi_schema::Event {
    let now_ms = chrono::Utc::now().timestamp_millis();
    kikimimi_schema::Event {
        event_id: event_id.to_string(),
        ts: now_ms,
        dt: kikimimi_schema::dt_of(now_ms),
        host_id: host_id.to_string(),
        agent: "claude-code".to_string(),
        source: "hook".to_string(),
        session_id: Some(session_id.to_string()),
        event_type: kikimimi_schema::event_type::TOOL_CALL.to_string(),
        tool_name: Some("Bash".to_string()),
        tool_kind: Some("bash".to_string()),
        duration_ms: Some(50),
        success: Some(true),
        input_tokens: Some(10),
        output_tokens: Some(5),
        cost_usd: Some(0.001),
        model: Some("claude-sonnet".to_string()),
        usage_source: Some("hook".to_string()),
        ..Default::default()
    }
}

#[tokio::test]
async fn login_wrong_invite_is_403_then_429_after_ten_failures() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();
    let email = "brute-force@example.com";

    for i in 0..10 {
        let resp = client
            .post(format!("{}/web/login", app.base_url))
            .json(&serde_json::json!({ "email": email, "invite_code": "definitely-wrong" }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 403, "failure #{i}");
    }

    // 11th attempt: the rate limiter must reject before even checking
    // credentials -- verified by trying the *correct* invite code here and
    // still getting 429, not 200.
    let resp = client
        .post(format!("{}/web/login", app.base_url))
        .json(&serde_json::json!({ "email": email, "invite_code": TEST_INVITE_CODE }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 429, "11th attempt must be rate-limited even with the right code");

    // A different email is unaffected -- the limit is per-email, not global.
    let resp = client
        .post(format!("{}/web/login", app.base_url))
        .json(&serde_json::json!({ "email": "someone-else@example.com", "invite_code": TEST_INVITE_CODE }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "a different email must not share brute-force@example.com's count");

    app.teardown().await;
}

#[tokio::test]
async fn login_ok_sets_cookie_me_reflects_it_and_logout_clears_it() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();

    // No cookie at all -> 401, before logging in.
    let resp = client.get(format!("{}/web/me", app.base_url)).send().await.unwrap();
    assert_eq!(resp.status(), 401);

    let login = web_login(&client, &app.base_url, "alice@example.com").await;

    let resp = client
        .get(format!("{}/web/me", app.base_url))
        .header(reqwest::header::COOKIE, &login.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["email"], "alice@example.com");
    assert_eq!(body["org_id"], login.org_id);

    // Logging in again with the same email must land in the same org
    // (personal org is reused, exactly like device activation).
    let login2 = web_login(&client, &app.base_url, "alice@example.com").await;
    assert_eq!(login2.org_id, login.org_id);

    let logout_resp = client
        .post(format!("{}/web/logout", app.base_url))
        .header(reqwest::header::COOKIE, &login.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(logout_resp.status(), 200);
    let set_cookie = logout_resp
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(set_cookie.contains("Max-Age=0"), "logout must clear the cookie: {set_cookie}");

    // The logged-out session is revoked server-side too, not just cleared
    // client-side -- replaying the old cookie value must now 401.
    let resp = client
        .get(format!("{}/web/me", app.base_url))
        .header(reqwest::header::COOKIE, &login.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401, "a revoked session must not still authenticate");

    app.teardown().await;
}

#[tokio::test]
async fn web_q_tools_is_rls_scoped_across_two_orgs() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();

    let device_a = login_as(&client, &app.base_url, "host-web-a", "web-org-a@example.com").await;
    let web_b = web_login(&client, &app.base_url, "web-org-b@example.com").await;
    assert_ne!(device_a.org_id, web_b.org_id, "sanity: two distinct orgs");

    let ev = recent_tool_call_event("web-tools-secret-a", "host-web-a", "sess-web-a");
    let payload = support::gzip(&support::ingest_body_bytes(&[ev]));
    let resp = client
        .post(format!("{}/v1/events", app.base_url))
        .bearer_auth(&device_a.token)
        .header("Content-Encoding", "gzip")
        .body(payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Org B's web session must see zero tool rows -- org A's Bash call must
    // not leak across the RLS boundary via /web/q/tools.
    let resp = client
        .get(format!("{}/web/q/tools?days=14", app.base_url))
        .header(reqwest::header::COOKIE, &web_b.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["columns"],
        serde_json::json!(["tool_name", "tool_kind", "calls", "failures", "p50_duration_ms", "p95_duration_ms"])
    );
    assert!(
        body["rows"].as_array().unwrap().is_empty(),
        "org B must not see org A's tool calls via /web/q/tools: {body:?}"
    );

    // Org A's own web session (same email as its device login) does see it
    // -- proves the emptiness above is RLS, not a query that's just broken.
    let web_a = web_login(&client, &app.base_url, "web-org-a@example.com").await;
    assert_eq!(web_a.org_id, device_a.org_id, "sanity: same email -> same personal org");
    let resp = client
        .get(format!("{}/web/q/tools?days=14", app.base_url))
        .header(reqwest::header::COOKIE, &web_a.cookie)
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let rows = body["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "org A must see its own Bash call: {body:?}");
    assert_eq!(rows[0][0], "Bash");
    assert_eq!(rows[0][2], 1, "calls");

    app.teardown().await;
}

#[tokio::test]
async fn session_cookie_expiry_is_respected() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();
    let login = web_login(&client, &app.base_url, "expiring@example.com").await;

    // Sanity: authenticates right after login.
    let resp = client
        .get(format!("{}/web/me", app.base_url))
        .header(reqwest::header::COOKIE, &login.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // The real TTL is 30 days -- force it into the past directly rather than
    // waiting. Only one session row exists in this fresh test database, so
    // an unfiltered UPDATE is unambiguous.
    sqlx::query("UPDATE web_sessions SET expires_at = now() - interval '1 hour'")
        .execute(&app.state.pools.superuser)
        .await
        .expect("expire the session");

    let resp = client
        .get(format!("{}/web/me", app.base_url))
        .header(reqwest::header::COOKIE, &login.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401, "an expired session cookie must not authenticate");

    app.teardown().await;
}

#[tokio::test]
async fn web_q_days_and_limit_params_are_validated() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();
    let login = web_login(&client, &app.base_url, "validate@example.com").await;

    let cases: &[(&str, u16)] = &[
        ("/web/q/overview?days=0", 400),
        ("/web/q/overview?days=366", 400),
        ("/web/q/overview?days=1", 200),
        ("/web/q/overview?days=365", 200),
        ("/web/q/overview", 200), // absent -> default (14), not an error
        ("/web/q/tools?days=0", 400),
        ("/web/q/mcp?days=9999", 400),
        ("/web/q/sessions?days=14&limit=0", 400),
        ("/web/q/sessions?days=14&limit=501", 400),
        ("/web/q/sessions?days=14&limit=500", 200),
        ("/web/q/sessions", 200), // both absent -> defaults
        ("/web/q/machines", 200), // no params at all, ever
    ];
    for (path, expected) in cases {
        let resp = client
            .get(format!("{}{path}", app.base_url))
            .header(reqwest::header::COOKIE, &login.cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), *expected, "{path}");
    }

    app.teardown().await;
}

#[tokio::test]
async fn static_root_serves_the_spa_html() {
    let app = TestApp::spawn(SpawnOpts::default()).await;

    let resp = reqwest::get(format!("{}/", app.base_url)).await.unwrap();
    assert_eq!(resp.status(), 200);
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(content_type.starts_with("text/html"), "content-type: {content_type}");
    let body = resp.text().await.unwrap();
    assert!(
        body.to_lowercase().contains("<!doctype html"),
        "expected an HTML document, got: {body}"
    );

    // An unknown client-side route (e.g. the SPA's own router) also falls
    // back to the same shell, not a 404.
    let resp = reqwest::get(format!("{}/sessions", app.base_url)).await.unwrap();
    assert_eq!(resp.status(), 200);

    app.teardown().await;
}
