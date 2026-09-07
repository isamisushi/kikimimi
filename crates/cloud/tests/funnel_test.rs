mod support;

use support::{activate_device_into_org, login_as, sample_event, web_login, SpawnOpts, TestApp};

async fn funnel(
    client: &reqwest::Client,
    base_url: &str,
    cookie: &str,
    scope: &str,
) -> reqwest::Response {
    client
        .get(format!("{base_url}/web/q/funnel?days=30&scope={scope}"))
        .header(reqwest::header::COOKIE, cookie)
        .send()
        .await
        .unwrap()
}

fn step_counts(body: &serde_json::Value) -> Vec<(String, i64)> {
    body["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["step"].as_str().unwrap().to_string(),
                s["hosts"].as_i64().unwrap(),
            )
        })
        .collect()
}

async fn make_operator(app: &TestApp, email: &str) {
    sqlx::query("UPDATE accounts SET operator = true WHERE email = $1")
        .bind(email)
        .execute(&app.state.pools.superuser)
        .await
        .unwrap();
}

async fn ingest(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    events: &[kikimimi_schema::Event],
) {
    let resp = client
        .post(format!("{base_url}/v1/events"))
        .bearer_auth(token)
        .header("Content-Encoding", "gzip")
        .body(support::gzip(&support::ingest_body_bytes(events)))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

/// KKM-21: three hosts. One does login → init (events) → opens the web
/// Overview; one logs in and never sends anything; one only ran `login`
/// and never approved. The operator (scope=all) sees 3 / 2 / 1 / 1; a
/// personal-org owner (scope=org) sees only their own machine; scope=all
/// does not exist for a non-operator; the table holds ids and timestamps
/// only.
#[tokio::test]
async fn funnel_counts_each_step_once_per_host_and_scopes_by_role() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();

    // Host A: full journey.
    let a = login_as(&client, &app.base_url, "host-a", "a@example.com").await;
    for batch in [
        vec![sample_event("f-a1", "host-a", "sess-a")],
        vec![sample_event("f-a1", "host-a", "sess-a")], // resend: deduped, must not double count
        vec![sample_event("f-a2", "host-a", "sess-a")],
    ] {
        ingest(&client, &app.base_url, &a.token, &batch).await;
    }
    let a_web = web_login(&client, &app.base_url, "a@example.com").await;
    for _ in 0..2 {
        let resp = client
            .get(format!("{}/web/q/overview?days=7", app.base_url))
            .header(reqwest::header::COOKIE, &a_web.cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }

    // Host B: logged in, never sent data, never looked.
    let _b = login_as(&client, &app.base_url, "host-b", "b@example.com").await;

    // Host C: `kikimimi login` started, never approved.
    let resp = client
        .post(format!("{}/v1/device/code", app.base_url))
        .json(&serde_json::json!({ "host_id": "host-c", "hostname": "c" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // A is owner of their personal org: scope=org shows just host-a.
    let resp = funnel(&client, &app.base_url, &a_web.cookie, "org").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["scope"], "org");
    assert_eq!(
        step_counts(&body),
        vec![
            ("login_started".to_string(), 1),
            ("login_done".to_string(), 1),
            ("first_events".to_string(), 1),
            ("first_insight".to_string(), 1),
        ],
        "{body}"
    );
    // ...and the deployment scope does not exist for a non-operator.
    assert_eq!(
        funnel(&client, &app.base_url, &a_web.cookie, "all")
            .await
            .status(),
        404
    );
    assert_eq!(
        funnel(&client, &app.base_url, &a_web.cookie, "bogus")
            .await
            .status(),
        400
    );

    let ops = web_login(&client, &app.base_url, "ops@example.com").await;
    make_operator(&app, "ops@example.com").await;
    let me: serde_json::Value = client
        .get(format!("{}/web/me", app.base_url))
        .header(reqwest::header::COOKIE, &ops.cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(me["operator"], true, "{me}");
    let resp = funnel(&client, &app.base_url, &ops.cookie, "all").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["days"], 30);
    assert_eq!(body["scope"], "all");
    assert_eq!(body["tracking"], true);
    assert_eq!(
        step_counts(&body),
        vec![
            ("login_started".to_string(), 3),
            ("login_done".to_string(), 2),
            ("first_events".to_string(), 1),
            ("first_insight".to_string(), 1),
        ],
        "{body}"
    );
    let median = body["median_minutes_login_to_first_events"]
        .as_f64()
        .unwrap();
    assert!((0.0..1.0).contains(&median), "median minutes {median}");
    assert_eq!(body["retention_30d"]["hosts_eligible"], 0);
    assert_eq!(body["retention_30d"]["hosts_retained"], 0);

    // Nothing personal in the table: opaque ids and timestamps only.
    let cols: Vec<(String,)> = sqlx::query_as(
        "SELECT column_name::text FROM information_schema.columns WHERE table_name = 'funnel_steps' ORDER BY ordinal_position",
    )
    .fetch_all(&app.state.pools.superuser)
    .await
    .unwrap();
    let cols: Vec<&str> = cols.iter().map(|(c,)| c.as_str()).collect();
    assert_eq!(cols, vec!["subject_kind", "subject_id", "step", "at"]);
    let (n_rows,): (i64,) = sqlx::query_as("SELECT count(*) FROM funnel_steps")
        .fetch_one(&app.state.pools.superuser)
        .await
        .unwrap();
    assert_eq!(
        n_rows,
        3 + 2 + 1 + 1,
        "one row per (subject, step), resends and repeat views collapse"
    );

    // A cohort window that excludes everything: counts go to zero.
    sqlx::query("UPDATE funnel_steps SET at = at - interval '40 days'")
        .execute(&app.state.pools.superuser)
        .await
        .unwrap();
    let body: serde_json::Value = funnel(&client, &app.base_url, &ops.cookie, "all")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(body["steps"][0]["hosts"], 0, "{body}");

    app.teardown().await;
}

/// Team org: the admin sees the org's hosts only (not the founder's personal
/// machine, not another org's), a member is refused (403), and the founder
/// -- an owner but not an operator -- still cannot read scope=all.
#[tokio::test]
async fn funnel_org_scope_is_the_active_org_and_needs_admin() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();

    let founder = web_login(&client, &app.base_url, "founder@example.com").await;
    let resp = client
        .post(format!("{}/web/orgs", app.base_url))
        .header(reqwest::header::COOKIE, &founder.cookie)
        .json(&serde_json::json!({ "name": "Acme", "slug": "acme" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    // Founder's own personal machine, outside acme.
    let _personal = login_as(
        &client,
        &app.base_url,
        "founder-laptop",
        "founder@example.com",
    )
    .await;
    // Two machines bound to acme, one of which sends events.
    let acme1 =
        activate_device_into_org(&client, &app.base_url, "acme-1", &founder.cookie, "acme").await;
    let _acme2 =
        activate_device_into_org(&client, &app.base_url, "acme-2", &founder.cookie, "acme").await;
    ingest(
        &client,
        &app.base_url,
        &acme1.token,
        &[sample_event("f-acme", "acme-1", "s")],
    )
    .await;

    // A member of acme.
    let member = web_login(&client, &app.base_url, "member@example.com").await;
    sqlx::query(
        "INSERT INTO memberships (account_id, org_id, role) \
         SELECT a.id, o.id, 'member' FROM accounts a, orgs o WHERE a.email = 'member@example.com' AND o.slug = 'acme'",
    )
    .execute(&app.state.pools.superuser)
    .await
    .unwrap();
    for cookie in [&founder.cookie, &member.cookie] {
        let resp = client
            .post(format!("{}/web/active-org", app.base_url))
            .header(reqwest::header::COOKIE, cookie)
            .json(&serde_json::json!({ "slug": "acme" }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }

    let resp = funnel(&client, &app.base_url, &founder.cookie, "org").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        step_counts(&body),
        vec![
            ("login_started".to_string(), 2),
            ("login_done".to_string(), 2),
            ("first_events".to_string(), 1),
            ("first_insight".to_string(), 0),
        ],
        "{body}"
    );
    assert_eq!(
        funnel(&client, &app.base_url, &member.cookie, "org")
            .await
            .status(),
        403
    );
    assert_eq!(
        funnel(&client, &app.base_url, &founder.cookie, "all")
            .await
            .status(),
        404
    );

    app.teardown().await;
}

/// 30-day retention is read from `devices` (created_at / last_seen_at), so
/// it works for hosts that predate the funnel table. Backdate one device.
#[tokio::test]
async fn funnel_retention_uses_device_first_and_last_seen() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();
    let kept = login_as(&client, &app.base_url, "host-kept", "k@example.com").await;
    let _lost = login_as(&client, &app.base_url, "host-lost", "l@example.com").await;
    sqlx::query("UPDATE devices SET created_at = now() - interval '45 days', last_seen_at = NULL")
        .execute(&app.state.pools.superuser)
        .await
        .unwrap();
    // Any authenticated request stamps last_seen_at = now().
    let resp = client
        .get(format!("{}/v1/query/today", app.base_url))
        .bearer_auth(&kept.token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let ops = web_login(&client, &app.base_url, "ops@example.com").await;
    make_operator(&app, "ops@example.com").await;
    let body: serde_json::Value = funnel(&client, &app.base_url, &ops.cookie, "all")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(body["retention_30d"]["hosts_eligible"], 2, "{body}");
    assert_eq!(body["retention_30d"]["hosts_retained"], 1, "{body}");
    // `kikimimi query --cloud` counted as the kept account's first insight
    // (the funnel rows themselves were not backdated, only `devices`).
    assert_eq!(body["steps"][3]["hosts"], 1, "{body}");
    app.teardown().await;
}
