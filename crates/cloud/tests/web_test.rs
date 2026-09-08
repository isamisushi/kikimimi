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
fn recent_tool_call_event(
    event_id: &str,
    host_id: &str,
    session_id: &str,
) -> kikimimi_schema::Event {
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
    assert_eq!(
        resp.status(),
        429,
        "11th attempt must be rate-limited even with the right code"
    );

    // A different email is unaffected -- the limit is per-email, not global.
    let resp = client
        .post(format!("{}/web/login", app.base_url))
        .json(&serde_json::json!({ "email": "someone-else@example.com", "invite_code": TEST_INVITE_CODE }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "a different email must not share brute-force@example.com's count"
    );

    app.teardown().await;
}

#[tokio::test]
async fn login_ok_sets_cookie_me_reflects_it_and_logout_clears_it() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();

    // No cookie at all -> 401, before logging in.
    let resp = client
        .get(format!("{}/web/me", app.base_url))
        .send()
        .await
        .unwrap();
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
    assert_eq!(
        body["github_login"],
        serde_json::Value::Null,
        "legacy login has no github_login"
    );
    let orgs = body["orgs"].as_array().unwrap();
    assert_eq!(
        orgs.len(),
        1,
        "a fresh account has exactly its personal org: {body:?}"
    );
    assert_eq!(orgs[0]["kind"], "personal");
    assert_eq!(orgs[0]["role"], "owner");
    assert_eq!(
        body["active_org"], orgs[0]["slug"],
        "active_org points at the personal org"
    );

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
    assert!(
        set_cookie.contains("Max-Age=0"),
        "logout must clear the cookie: {set_cookie}"
    );

    // The logged-out session is revoked server-side too, not just cleared
    // client-side -- replaying the old cookie value must now 401.
    let resp = client
        .get(format!("{}/web/me", app.base_url))
        .header(reqwest::header::COOKIE, &login.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        401,
        "a revoked session must not still authenticate"
    );

    app.teardown().await;
}

#[tokio::test]
async fn web_q_tools_is_rls_scoped_across_two_orgs() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();

    let device_a = login_as(
        &client,
        &app.base_url,
        "host-web-a",
        "web-org-a@example.com",
    )
    .await;
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
        serde_json::json!([
            "tool_name",
            "tool_kind",
            "calls",
            "failures",
            "p50_duration_ms",
            "p95_duration_ms"
        ])
    );
    assert!(
        body["rows"].as_array().unwrap().is_empty(),
        "org B must not see org A's tool calls via /web/q/tools: {body:?}"
    );

    // Org A's own web session (same email as its device login) does see it
    // -- proves the emptiness above is RLS, not a query that's just broken.
    let web_a = web_login(&client, &app.base_url, "web-org-a@example.com").await;
    assert_eq!(
        web_a.org_id, device_a.org_id,
        "sanity: same email -> same personal org"
    );
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
    assert_eq!(
        resp.status(),
        401,
        "an expired session cookie must not authenticate"
    );

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
    assert!(
        content_type.starts_with("text/html"),
        "content-type: {content_type}"
    );
    let body = resp.text().await.unwrap();
    assert!(
        body.to_lowercase().contains("<!doctype html"),
        "expected an HTML document, got: {body}"
    );

    // An unknown client-side route (e.g. the SPA's own router) also falls
    // back to the same shell, not a 404.
    let resp = reqwest::get(format!("{}/sessions", app.base_url))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    app.teardown().await;
}

/// KKM-17: `/web/q/coverage` counts what is missing. One session with OTel
/// usage and a hook/OTel tool_result pair, one hooks-only session, and a
/// second host whose only event is a week old.
#[tokio::test]
async fn web_q_coverage_counts_missing_usage_correlation_and_silent_hosts() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();
    let device = login_as(&client, &app.base_url, "host-cov-a", "cov@example.com").await;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let mk = |id: &str, host: &str, sess: &str, ts: i64, source: &str, ty: &str| {
        kikimimi_schema::Event {
            event_id: id.to_string(),
            ts,
            dt: kikimimi_schema::dt_of(ts),
            host_id: host.to_string(),
            agent: "claude-code".to_string(),
            source: source.to_string(),
            session_id: Some(sess.to_string()),
            event_type: ty.to_string(),
            ..Default::default()
        }
    };
    let events = vec![
        mk(
            "cov-a1",
            "host-cov-a",
            "sess-a",
            now_ms - 5000,
            "hook",
            "session.start",
        ),
        kikimimi_schema::Event {
            input_tokens: Some(100),
            output_tokens: Some(10),
            ..mk(
                "cov-a2",
                "host-cov-a",
                "sess-a",
                now_ms - 4000,
                "otel",
                "api.request",
            )
        },
        kikimimi_schema::Event {
            correlation_key: Some("tu1".into()),
            ..mk(
                "cov-a3",
                "host-cov-a",
                "sess-a",
                now_ms - 3000,
                "hook",
                "tool.result",
            )
        },
        kikimimi_schema::Event {
            correlation_key: Some("tu1".into()),
            ..mk(
                "cov-a4",
                "host-cov-a",
                "sess-a",
                now_ms - 2999,
                "otel",
                "tool.result",
            )
        },
        kikimimi_schema::Event {
            correlation_key: Some("tu2".into()),
            ..mk(
                "cov-a5",
                "host-cov-a",
                "sess-a",
                now_ms - 2000,
                "hook",
                "tool.result",
            )
        },
        mk(
            "cov-b1",
            "host-cov-a",
            "sess-b",
            now_ms - 1000,
            "hook",
            "session.start",
        ),
    ];
    let resp = client
        .post(format!("{}/v1/events", app.base_url))
        .bearer_auth(&device.token)
        .header("Content-Encoding", "gzip")
        .body(support::gzip(&support::ingest_body_bytes(&events)))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    // A second device of the same account (ingest pins host_id to the token's), silent for a week.
    let old_device = login_as(&client, &app.base_url, "host-cov-old", "cov@example.com").await;
    let old = vec![mk(
        "cov-c1",
        "host-cov-old",
        "sess-c",
        now_ms - 7 * 86_400_000,
        "hook",
        "session.start",
    )];
    let resp = client
        .post(format!("{}/v1/events", app.base_url))
        .bearer_auth(&old_device.token)
        .header("Content-Encoding", "gzip")
        .body(support::gzip(&support::ingest_body_bytes(&old)))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let web = web_login(&client, &app.base_url, "cov@example.com").await;
    let body: serde_json::Value = client
        .get(format!("{}/web/q/coverage?days=30", app.base_url))
        .header(reqwest::header::COOKIE, &web.cookie)
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
    assert_eq!(cols[0], "events");
    assert_eq!(cols[13], "last_event_ts");
    let r = &body["rows"][0];
    assert_eq!(r[0], 7, "events");
    assert_eq!(
        r[1], 0,
        "ingest attributes every row to the device's account"
    );
    assert_eq!(r[2], 3, "sessions");
    assert_eq!(r[3], 2, "sess-b and sess-c have no usage");
    assert_eq!(r[4], 2, "hook tool results by key");
    assert_eq!(r[5], 1);
    assert_eq!(r[6], 1, "tu1 matched");
    assert_eq!(r[7], 3, "raw tool results");
    assert_eq!(r[8], 2, "deduped");
    assert_eq!(r[9], 0);
    assert_eq!(r[11], 2, "hosts");
    assert_eq!(r[12], 1, "host-cov-old is silent");
    assert!(r[13].as_str().unwrap().ends_with('Z'));

    app.teardown().await;
}

/// `/web/q/session` (single-session drilldown): summary/tools/subagents/
/// timeline/events over one ingested session, hook/OTel `tool.result` pair
/// deduped once everywhere, subagent rows attributed by `agent_id`; another
/// org's web session gets a 404 (RLS), a missing id a 400.
#[tokio::test]
async fn web_q_session_drills_into_one_session() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();
    let device = login_as(&client, &app.base_url, "host-sd", "sd@example.com").await;

    let t0 = chrono::Utc::now().timestamp_millis() - 10 * 60_000;
    let base = recent_tool_call_event("sd-0", "host-sd", "sess-detail");
    let ev = |id: &str, ts: i64, event_type: &str| kikimimi_schema::Event {
        event_id: id.to_string(),
        ts,
        dt: kikimimi_schema::dt_of(ts),
        event_type: event_type.to_string(),
        tool_name: None,
        tool_kind: None,
        duration_ms: None,
        success: None,
        input_tokens: None,
        output_tokens: None,
        cost_usd: None,
        model: None,
        usage_source: None,
        ..base.clone()
    };
    let events = vec![
        kikimimi_schema::Event {
            agent_version: Some("2.1.0".into()),
            configured_mcp_servers: Some(r#"["github"]"#.into()),
            ..ev("sd-start", t0, "session.start")
        },
        kikimimi_schema::Event {
            tool_name: Some("Bash".into()),
            tool_kind: Some("bash".into()),
            correlation_key: Some("tu1".into()),
            ..ev("sd-call", t0 + 1_000, "tool.call")
        },
        // hook + OTel result for the same tool_use_id: one failure, OTel's duration wins.
        kikimimi_schema::Event {
            tool_name: Some("Bash".into()),
            tool_kind: Some("bash".into()),
            correlation_key: Some("tu1".into()),
            success: Some(false),
            duration_ms: Some(100),
            ..ev("sd-res-hook", t0 + 2_000, "tool.result")
        },
        kikimimi_schema::Event {
            source: "otel".into(),
            tool_name: Some("Bash".into()),
            tool_kind: Some("bash".into()),
            correlation_key: Some("tu1".into()),
            success: Some(false),
            duration_ms: Some(120),
            ..ev("sd-res-otel", t0 + 2_100, "tool.result")
        },
        kikimimi_schema::Event {
            source: "otel".into(),
            model: Some("claude-sonnet".into()),
            input_tokens: Some(1000),
            output_tokens: Some(200),
            cost_usd: Some(0.05),
            usage_source: Some("otel".into()),
            ..ev("sd-api", t0 + 3_000, "api.request")
        },
        kikimimi_schema::Event {
            agent_id: Some("ag1".into()),
            agent_type: Some("Explore".into()),
            tool_name: Some("Read".into()),
            tool_kind: Some("builtin".into()),
            correlation_key: Some("tu2".into()),
            ..ev("sd-sub-call", t0 + 4_000, "tool.call")
        },
        kikimimi_schema::Event {
            agent_id: Some("ag1".into()),
            agent_type: Some("Explore".into()),
            duration_ms: Some(5_000),
            ..ev("sd-sub-stop", t0 + 9_000, "subagent.stop")
        },
        ev("sd-end", t0 + 5 * 60_000, "session.end"),
    ];
    let resp = client
        .post(format!("{}/v1/events", app.base_url))
        .bearer_auth(&device.token)
        .header("Content-Encoding", "gzip")
        .body(support::gzip(&support::ingest_body_bytes(&events)))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let web = web_login(&client, &app.base_url, "sd@example.com").await;
    let resp = client
        .get(format!(
            "{}/web/q/session?session_id=sess-detail",
            app.base_url
        ))
        .header(reqwest::header::COOKIE, &web.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();

    let col = |section: &str, name: &str| -> usize {
        body[section]["columns"]
            .as_array()
            .unwrap()
            .iter()
            .position(|c| c == name)
            .unwrap_or_else(|| panic!("{section} has no column {name}: {body:?}"))
    };
    let s = &body["summary"]["rows"][0];
    assert_eq!(s[col("summary", "session_id")], "sess-detail");
    assert_eq!(s[col("summary", "agent_version")], "2.1.0");
    assert_eq!(s[col("summary", "duration_ms")], 5 * 60_000);
    assert_eq!(s[col("summary", "ended")], true);
    assert_eq!(s[col("summary", "events")], 8, "raw ingested count");
    assert_eq!(s[col("summary", "tool_calls")], 2);
    assert_eq!(s[col("summary", "failures")], 1, "hook/OTel pair deduped");
    assert_eq!(s[col("summary", "api_requests")], 1);
    assert_eq!(s[col("summary", "subagents")], 1);
    assert_eq!(s[col("summary", "input_tokens")], 1000);
    assert_eq!(s[col("summary", "configured_mcp_servers")], r#"["github"]"#);
    assert_eq!(
        body["bucket_ms"], 60_000,
        "5-minute span -> 1-minute buckets"
    );
    assert_eq!(body["events_limit"], 500);

    let tools = body["tools"]["rows"].as_array().unwrap();
    assert_eq!(tools.len(), 2, "{tools:?}");
    let bash = &tools[0];
    assert_eq!(bash[col("tools", "tool_name")], "Bash");
    assert_eq!(bash[col("tools", "calls")], 1);
    assert_eq!(bash[col("tools", "subagent_calls")], 0);
    assert_eq!(bash[col("tools", "failures")], 1);
    assert_eq!(
        bash[col("tools", "p50_duration_ms")],
        120.0,
        "OTel row kept"
    );
    assert_eq!(bash[col("tools", "total_duration_ms")], 120);
    let read = &tools[1];
    assert_eq!(read[col("tools", "tool_name")], "Read");
    assert_eq!(read[col("tools", "subagent_calls")], 1);

    let subs = body["subagents"]["rows"].as_array().unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0][col("subagents", "agent_id")], "ag1");
    assert_eq!(subs[0][col("subagents", "agent_type")], "Explore");
    assert_eq!(
        subs[0][col("subagents", "duration_ms")],
        5_000,
        "SubagentStop duration"
    );
    assert_eq!(subs[0][col("subagents", "tool_calls")], 1);
    assert_eq!(
        subs[0][col("subagents", "tokens_est")],
        serde_json::Value::Null,
        "no usage -> null, never 0"
    );
    assert_eq!(subs[0][col("subagents", "tools")], "Read");

    let timeline = body["timeline"]["rows"].as_array().unwrap();
    assert_eq!(timeline.len(), 2, "minute 0 and minute 5: {timeline:?}");
    assert_eq!(
        timeline[0][col("timeline", "events")],
        6,
        "7 raw in minute 0, pair deduped"
    );
    assert_eq!(timeline[0][col("timeline", "failures")], 1);
    assert_eq!(timeline[0][col("timeline", "subagent_events")], 2);
    assert_eq!(timeline[0][col("timeline", "tokens")], 1200);

    let list = body["events"]["rows"].as_array().unwrap();
    assert_eq!(list.len(), 7, "8 raw rows, deduped pair listed once");
    assert_eq!(list[0][col("events", "event_type")], "session.start");
    let res = list
        .iter()
        .find(|r| r[col("events", "event_type")] == "tool.result")
        .unwrap();
    assert_eq!(res[col("events", "source")], "otel");
    assert_eq!(res[col("events", "duration_ms")], 120);

    // Another org's web session: 404, not the data.
    let other = web_login(&client, &app.base_url, "sd-other@example.com").await;
    let resp = client
        .get(format!(
            "{}/web/q/session?session_id=sess-detail",
            app.base_url
        ))
        .header(reqwest::header::COOKIE, &other.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);

    let resp = client
        .get(format!("{}/web/q/session", app.base_url))
        .header(reqwest::header::COOKIE, &web.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "session_id is required");
    let resp = client
        .get(format!(
            "{}/web/q/session?session_id=sess-detail&events_limit=0",
            app.base_url
        ))
        .header(reqwest::header::COOKIE, &web.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    app.teardown().await;
}
