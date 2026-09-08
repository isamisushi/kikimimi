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

    // Snapped to a minute boundary so the events 0..9 s after t0 never
    // straddle two timeline buckets (the assertion below counts buckets).
    let t0 = (chrono::Utc::now().timestamp_millis() - 10 * 60_000) / 60_000 * 60_000;
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
            effort: Some("high".into()),
            input_tokens: Some(1000),
            output_tokens: Some(200),
            cost_usd: Some(0.05),
            usage_source: Some("otel".into()),
            ..ev("sd-api", t0 + 3_000, "api.request")
        },
        // The transcript backfill's copy of the same request: `models` must
        // count the OTel row only, never both.
        kikimimi_schema::Event {
            source: "log".into(),
            model: Some("claude-sonnet".into()),
            effort: Some("high".into()),
            input_tokens: Some(1000),
            output_tokens: Some(200),
            reasoning_tokens: Some(40),
            usage_source: Some("log".into()),
            ..ev("sd-api-log", t0 + 3_001, "api.request")
        },
        kikimimi_schema::Event {
            agent_id: Some("ag1".into()),
            agent_type: Some("Explore".into()),
            tool_name: Some("Read".into()),
            tool_kind: Some("builtin".into()),
            correlation_key: Some("tu2".into()),
            ..ev("sd-sub-call", t0 + 4_000, "tool.call")
        },
        // What OTel emits for a subagent's request: agent.name but no
        // agent_id, inside ag1's window -> attributed to ag1 by type + time.
        kikimimi_schema::Event {
            source: "otel".into(),
            agent_type: Some("Explore".into()),
            model: Some("claude-sonnet".into()),
            effort: Some("medium".into()),
            ..ev("sd-sub-api", t0 + 6_000, "api.request")
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
    assert_eq!(s[col("summary", "events")], 10, "raw ingested count");
    assert_eq!(s[col("summary", "tool_calls")], 2);
    assert_eq!(s[col("summary", "failures")], 1, "hook/OTel pair deduped");
    assert_eq!(
        s[col("summary", "api_requests")],
        3,
        "raw count, both sources"
    );
    assert_eq!(s[col("summary", "subagents")], 1);
    assert_eq!(
        s[col("summary", "input_tokens")],
        2000,
        "raw sum, both sources"
    );
    assert_eq!(s[col("summary", "efforts")], "high,medium");
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
    assert_eq!(subs[0][col("subagents", "models")], "claude-sonnet");
    assert_eq!(subs[0][col("subagents", "efforts")], "medium");
    assert_eq!(subs[0][col("subagents", "model_source")], "otel_window");

    // KKM-34: per-session model × effort, one source per session (OTel wins).
    let models = body["models"]["rows"].as_array().unwrap();
    assert_eq!(models.len(), 2, "{models:?}");
    let m = &models[0];
    assert_eq!(m[col("models", "model")], "claude-sonnet");
    assert_eq!(m[col("models", "effort")], "high");
    assert_eq!(m[col("models", "api_requests")], 1, "OTel row only");
    assert_eq!(m[col("models", "api_errors")], 0);
    assert_eq!(m[col("models", "subagent_api_requests")], 0);
    assert_eq!(m[col("models", "input_tokens")], 1000);
    assert_eq!(m[col("models", "output_tokens")], 200);
    assert_eq!(
        m[col("models", "cache_read_tokens")],
        serde_json::Value::Null
    );
    assert_eq!(
        m[col("models", "reasoning_tokens")],
        serde_json::Value::Null,
        "transcript-only column, transcript row not counted"
    );
    assert_eq!(m[col("models", "cost_usd")], 0.05);
    // The OTel subagent request: its own (model, effort) row, counted as a
    // subagent's by agent_type, no usage -> NULL, sorted last.
    let m2 = &models[1];
    assert_eq!(m2[col("models", "effort")], "medium");
    assert_eq!(m2[col("models", "api_requests")], 1);
    assert_eq!(m2[col("models", "subagent_api_requests")], 1);
    assert_eq!(m2[col("models", "input_tokens")], serde_json::Value::Null);

    let timeline = body["timeline"]["rows"].as_array().unwrap();
    assert_eq!(timeline.len(), 2, "minute 0 and minute 5: {timeline:?}");
    assert_eq!(
        timeline[0][col("timeline", "events")],
        8,
        "9 raw in minute 0, pair deduped"
    );
    assert_eq!(timeline[0][col("timeline", "failures")], 1);
    assert_eq!(timeline[0][col("timeline", "subagent_events")], 2);
    assert_eq!(timeline[0][col("timeline", "tokens")], 2400);

    let list = body["events"]["rows"].as_array().unwrap();
    assert_eq!(list.len(), 9, "10 raw rows, deduped pair listed once");
    assert_eq!(list[0][col("events", "event_type")], "session.start");
    let res = list
        .iter()
        .find(|r| r[col("events", "event_type")] == "tool.result")
        .unwrap();
    assert_eq!(res[col("events", "source")], "otel");
    assert_eq!(res[col("events", "duration_ms")], 120);
    let api = list
        .iter()
        .find(|r| r[col("events", "event_type")] == "api.request")
        .unwrap();
    assert_eq!(api[col("events", "effort")], "high");

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

/// KKM-34: `/web/q/models` groups `api.request` usage by (model, effort),
/// counting one source per session (OTel over transcript), so a request
/// seen by both is not doubled; `api.error`s join by (model, effort);
/// effort NULL stays NULL; subagent rows are split out; `daily` is per
/// (dt, model). Visible to every role.
#[tokio::test]
async fn web_q_models_groups_by_model_and_effort_preferring_otel_per_session() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();
    let device = login_as(&client, &app.base_url, "host-mo", "mo@example.com").await;

    // Snapped to a minute boundary so the events 0..9 s after t0 never
    // straddle two timeline buckets (the assertion below counts buckets).
    let t0 = (chrono::Utc::now().timestamp_millis() - 10 * 60_000) / 60_000 * 60_000;
    let base = recent_tool_call_event("mo-0", "host-mo", "sess-a");
    let api =
        |id: &str, ts: i64, session: &str, source: &str, model: &str| kikimimi_schema::Event {
            event_id: id.to_string(),
            ts,
            dt: kikimimi_schema::dt_of(ts),
            event_type: "api.request".to_string(),
            session_id: Some(session.to_string()),
            source: source.to_string(),
            model: Some(model.to_string()),
            effort: Some("high".to_string()),
            tool_name: None,
            tool_kind: None,
            duration_ms: None,
            success: None,
            input_tokens: Some(1000),
            output_tokens: Some(100),
            cache_read_tokens: Some(5000),
            cost_usd: Some(0.10),
            usage_source: Some(source.to_string()),
            ..base.clone()
        };
    let events = vec![
        // sess-a: OTel and transcript saw the same request -> OTel counted once.
        api("mo-a-otel", t0, "sess-a", "otel", "claude-sonnet"),
        kikimimi_schema::Event {
            cost_usd: None,
            reasoning_tokens: Some(30),
            ..api("mo-a-log", t0 + 1, "sess-a", "log", "claude-sonnet")
        },
        // sess-a: a Haiku helper call with no effort.
        kikimimi_schema::Event {
            effort: None,
            input_tokens: Some(10),
            output_tokens: Some(5),
            cache_read_tokens: None,
            cost_usd: Some(0.001),
            ..api("mo-a-haiku", t0 + 2, "sess-a", "otel", "claude-haiku")
        },
        // sess-a: an api.error for the same model/effort.
        kikimimi_schema::Event {
            event_type: "api.error".to_string(),
            input_tokens: None,
            output_tokens: None,
            cache_read_tokens: None,
            cost_usd: None,
            ..api("mo-a-err", t0 + 3, "sess-a", "otel", "claude-sonnet")
        },
        // sess-b: transcript only -> the log rows count, one of them a subagent's.
        kikimimi_schema::Event {
            cost_usd: None,
            reasoning_tokens: Some(20),
            ..api("mo-b-log", t0 + 10, "sess-b", "log", "claude-sonnet")
        },
        kikimimi_schema::Event {
            cost_usd: None,
            agent_id: Some("ag1".into()),
            agent_type: Some("Explore".into()),
            input_tokens: Some(300),
            output_tokens: Some(50),
            ..api("mo-b-sub", t0 + 11, "sess-b", "log", "claude-sonnet")
        },
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

    let web = web_login(&client, &app.base_url, "mo@example.com").await;
    let resp = client
        .get(format!("{}/web/q/models?days=14", app.base_url))
        .header(reqwest::header::COOKIE, &web.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["days"], 14);
    assert_eq!(
        body["models"]["columns"],
        serde_json::json!([
            "model",
            "effort",
            "api_requests",
            "api_errors",
            "sessions",
            "subagent_api_requests",
            "subagent_tokens",
            "input_tokens",
            "output_tokens",
            "cache_read_tokens",
            "cache_write_tokens",
            "reasoning_tokens",
            "cost_usd"
        ])
    );
    assert_eq!(
        body["daily"]["columns"],
        serde_json::json!(["dt", "model", "input_tokens", "output_tokens", "cost_usd"])
    );
    let col = |section: &str, name: &str| -> usize {
        body[section]["columns"]
            .as_array()
            .unwrap()
            .iter()
            .position(|c| c == name)
            .unwrap_or_else(|| panic!("{section} has no column {name}: {body:?}"))
    };
    let rows = body["models"]["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{rows:?}");
    let sonnet = &rows[0];
    assert_eq!(sonnet[col("models", "model")], "claude-sonnet");
    assert_eq!(sonnet[col("models", "effort")], "high");
    assert_eq!(
        sonnet[col("models", "api_requests")],
        3,
        "sess-a OTel once + sess-b's two log rows"
    );
    assert_eq!(sonnet[col("models", "api_errors")], 1);
    assert_eq!(sonnet[col("models", "sessions")], 2);
    assert_eq!(sonnet[col("models", "subagent_api_requests")], 1);
    assert_eq!(sonnet[col("models", "subagent_tokens")], 350);
    assert_eq!(sonnet[col("models", "input_tokens")], 2300);
    assert_eq!(sonnet[col("models", "output_tokens")], 250);
    assert_eq!(sonnet[col("models", "cache_read_tokens")], 15000);
    assert_eq!(
        sonnet[col("models", "cache_write_tokens")],
        serde_json::Value::Null,
        "nothing carried it -> null, never 0"
    );
    assert_eq!(
        sonnet[col("models", "reasoning_tokens")],
        20,
        "sess-b's log row only; sess-a's log copy was dropped"
    );
    assert_eq!(sonnet[col("models", "cost_usd")], 0.10, "OTel row only");
    let haiku = &rows[1];
    assert_eq!(haiku[col("models", "model")], "claude-haiku");
    assert_eq!(haiku[col("models", "effort")], serde_json::Value::Null);
    assert_eq!(haiku[col("models", "api_requests")], 1);
    assert_eq!(haiku[col("models", "api_errors")], 0);
    assert_eq!(haiku[col("models", "input_tokens")], 10);

    let daily = body["daily"]["rows"].as_array().unwrap();
    assert_eq!(daily.len(), 2, "{daily:?}");
    assert_eq!(daily[0][col("daily", "dt")], kikimimi_schema::dt_of(t0));
    assert_eq!(daily[0][col("daily", "model")], "claude-haiku");
    assert_eq!(daily[1][col("daily", "model")], "claude-sonnet");
    assert_eq!(daily[1][col("daily", "input_tokens")], 2300);
    assert_eq!(daily[1][col("daily", "cost_usd")], 0.10);

    // Another org sees nothing (RLS), not an error.
    let other = web_login(&client, &app.base_url, "mo-other@example.com").await;
    let resp = client
        .get(format!("{}/web/q/models", app.base_url))
        .header(reqwest::header::COOKIE, &other.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["models"]["rows"].as_array().unwrap().len(), 0);

    let resp = client
        .get(format!("{}/web/q/models?days=0", app.base_url))
        .header(reqwest::header::COOKIE, &web.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    app.teardown().await;
}
