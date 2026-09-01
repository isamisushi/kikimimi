mod support;

use support::{active_org_slug, login_autoapprove, web_login, SpawnOpts, TestApp};

#[tokio::test]
async fn autoapprove_device_flow_issues_a_43_char_token() {
    let app = TestApp::spawn(SpawnOpts {
        dev_autoapprove: true,
        dev_email: "auto@example.com".to_string(),
        ..Default::default()
    })
    .await;
    let client = reqwest::Client::new();

    let login = login_autoapprove(&client, &app.base_url, "host-auto-1").await;
    assert_eq!(login.token.len(), 43, "token must be 43 chars: {:?}", login.token);
    assert_eq!(login.email, "auto@example.com");
    uuid::Uuid::parse_str(&login.org_id).expect("org_id must be a uuid");
    uuid::Uuid::parse_str(&login.user_id).expect("user_id must be a uuid");

    // The bearer token must actually authenticate against a protected route.
    let resp = client
        .get(format!("{}/v1/query/today", app.base_url))
        .bearer_auth(&login.token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    app.teardown().await;
}

#[tokio::test]
async fn device_code_is_single_use_second_poll_is_410() {
    let app = TestApp::spawn(SpawnOpts {
        dev_autoapprove: true,
        ..Default::default()
    })
    .await;
    let client = reqwest::Client::new();

    let code_resp: serde_json::Value = client
        .post(format!("{}/v1/device/code", app.base_url))
        .json(&serde_json::json!({ "host_id": "host-reuse", "hostname": "h" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let device_code = code_resp["device_code"].as_str().unwrap();

    let first = client
        .post(format!("{}/v1/device/token", app.base_url))
        .json(&serde_json::json!({ "device_code": device_code }))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), 200);
    let first_body: serde_json::Value = first.json().await.unwrap();
    assert_eq!(first_body["status"], "ok");

    let second = client
        .post(format!("{}/v1/device/token", app.base_url))
        .json(&serde_json::json!({ "device_code": device_code }))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), 410, "replaying a consumed device_code must be 410");

    app.teardown().await;
}

#[tokio::test]
async fn unknown_device_code_is_410() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/v1/device/token", app.base_url))
        .json(&serde_json::json!({ "device_code": "does-not-exist" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 410);

    app.teardown().await;
}

/// account-model contract: `/activate` now requires an authenticated web
/// session (obtained here via the legacy email+invite `POST /web/login`,
/// still active because `SpawnOpts::default()` leaves `GITHUB_CLIENT_ID`
/// unset) instead of trusting a bare `email` field typed into the form.
#[tokio::test]
async fn manual_activate_flow_pending_then_ok() {
    let app = TestApp::spawn(SpawnOpts::default()).await; // autoapprove OFF
    let client = reqwest::Client::new();

    let code_resp: serde_json::Value = client
        .post(format!("{}/v1/device/code", app.base_url))
        .json(&serde_json::json!({ "host_id": "host-manual", "hostname": "laptop" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let device_code = code_resp["device_code"].as_str().unwrap().to_string();
    let user_code = code_resp["user_code"].as_str().unwrap().to_string();
    assert_eq!(code_resp["interval_secs"], 2);
    assert!(code_resp["verification_url"]
        .as_str()
        .unwrap()
        .contains(&user_code));

    // Not approved yet: pending.
    let pending: serde_json::Value = client
        .post(format!("{}/v1/device/token", app.base_url))
        .json(&serde_json::json!({ "device_code": device_code }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(pending["status"], "pending");

    // GET /activate without a session at all: 401, not a form.
    let anon = client
        .get(format!("{}/activate?code={user_code}", app.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status(), 401, "GET /activate must require a web session");

    // Log in (legacy email+invite -- GitHub OAuth isn't configured here).
    let web = web_login(&client, &app.base_url, "person@example.com").await;

    // GET /activate, now with the session cookie, renders a form containing
    // the user_code, host_id, and an org dropdown (the account's personal
    // org, since it has no others yet).
    let page = client
        .get(format!("{}/activate?code={user_code}", app.base_url))
        .header(reqwest::header::COOKIE, &web.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), 200);
    let html = page.text().await.unwrap();
    assert!(html.contains(&user_code));
    assert!(html.contains("host-manual"));
    assert!(html.contains("<form"));
    assert!(html.contains(r#"name="org_slug""#));
    assert!(html.contains("person@example.com"), "shows who's signed in: {html}");

    let org_slug = active_org_slug(&client, &app.base_url, &web.cookie).await;

    // POST /activate approves (form-urlencoded, like a plain HTML <form>),
    // authenticated by the session cookie -- no email/invite_code field.
    let approve = client
        .post(format!("{}/activate", app.base_url))
        .header(reqwest::header::COOKIE, &web.cookie)
        .form(&[("code", user_code.as_str()), ("org_slug", org_slug.as_str())])
        .send()
        .await
        .unwrap();
    assert_eq!(approve.status(), 200);

    // Now the poll materializes the device and returns a token, bound to the
    // session's account + chosen org -- and (account-model contract) the
    // response also carries org_slug/org_kind.
    let ok: serde_json::Value = client
        .post(format!("{}/v1/device/token", app.base_url))
        .json(&serde_json::json!({ "device_code": device_code }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ok["status"], "ok");
    assert_eq!(ok["email"], "person@example.com");
    assert_eq!(ok["token"].as_str().unwrap().len(), 43);
    assert_eq!(ok["org_slug"], org_slug);
    assert_eq!(ok["org_kind"], "personal");

    app.teardown().await;
}

/// POST /activate without a session at all: 401 (same requirement as GET).
#[tokio::test]
async fn activate_post_requires_a_web_session() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/activate", app.base_url))
        .form(&[("code", "AAAA-BBBB"), ("org_slug", "whatever")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    app.teardown().await;
}

/// A session can only approve into an org it's actually a member of --
/// typing another org's slug into the form must not work.
#[tokio::test]
async fn activate_rejects_an_org_the_session_is_not_a_member_of() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();

    let code_resp: serde_json::Value = client
        .post(format!("{}/v1/device/code", app.base_url))
        .json(&serde_json::json!({ "host_id": "host-foreign-org" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let user_code = code_resp["user_code"].as_str().unwrap().to_string();

    let alice = web_login(&client, &app.base_url, "alice-activate@example.com").await;
    let bob = web_login(&client, &app.base_url, "bob-activate@example.com").await;
    let bobs_org_slug = active_org_slug(&client, &app.base_url, &bob.cookie).await;

    // Alice tries to approve the device into Bob's (personal) org.
    let resp = client
        .post(format!("{}/activate", app.base_url))
        .header(reqwest::header::COOKIE, &alice.cookie)
        .form(&[("code", user_code.as_str()), ("org_slug", bobs_org_slug.as_str())])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403, "must not approve into an org alice isn't a member of");

    app.teardown().await;
}

/// Security review compounding factor on the account-takeover finding: prior
/// to `POST /v1/device/revoke`, `devices.revoked` was only ever *read*
/// (auth.rs), never set by any handler, so there was no way — not even for
/// the legitimate account holder — to kill a token server-side; `kikimimi
/// logout` only ever deleted the local config file. This proves the new
/// endpoint actually revokes the calling token, and that a revoked token is
/// rejected on its very next use (matches architecture.md §6's "`kikimimi
/// logout` / Web から失効可").
#[tokio::test]
async fn device_revoke_invalidates_the_calling_token() {
    let app = TestApp::spawn(SpawnOpts {
        dev_autoapprove: true,
        ..Default::default()
    })
    .await;
    let client = reqwest::Client::new();
    let login = login_autoapprove(&client, &app.base_url, "host-revoke").await;

    // Works before revoke.
    let resp = client
        .get(format!("{}/v1/query/today", app.base_url))
        .bearer_auth(&login.token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let revoke_resp = client
        .post(format!("{}/v1/device/revoke", app.base_url))
        .bearer_auth(&login.token)
        .send()
        .await
        .unwrap();
    assert_eq!(revoke_resp.status(), 200);

    // The exact same token must now be rejected everywhere.
    let resp = client
        .get(format!("{}/v1/query/today", app.base_url))
        .bearer_auth(&login.token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401, "a revoked token must no longer authenticate");

    let resp = client
        .post(format!("{}/v1/device/revoke", app.base_url))
        .bearer_auth(&login.token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401, "revoking an already-revoked token must also 401, not 500");

    app.teardown().await;
}

#[tokio::test]
async fn device_revoke_requires_a_bearer_token() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/v1/device/revoke", app.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    app.teardown().await;
}

// ---------------------------------------------------------------------------
// Org selection at activation time (account-model contract: "kikimimi login
// --org <slug> passes desired org hint to /v1/device/code (server pre-
// selects in dropdown)"; "approval binds the device token to (account,
// chosen org, host_id)"). The legacy KIKIMIMI_INVITE_CODE gate on `/activate`
// itself is gone — see `manual_activate_flow_pending_then_ok` /
// `activate_post_requires_a_web_session` / `activate_rejects_an_org_the_
// session_is_not_a_member_of` above for what replaced it (session-gating).
// `KIKIMIMI_INVITE_CODE` still gates the legacy `POST /web/login` (web_test.rs).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn activate_binds_the_device_to_the_chosen_team_org_not_the_personal_one() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();

    let web = web_login(&client, &app.base_url, "team-activate@example.com").await;
    let create = client
        .post(format!("{}/web/orgs", app.base_url))
        .header(reqwest::header::COOKIE, &web.cookie)
        .json(&serde_json::json!({ "name": "Acme", "slug": "acme-activate" }))
        .send()
        .await
        .unwrap();
    assert_eq!(create.status(), 200, "{}", create.text().await.unwrap());

    let code_resp: serde_json::Value = client
        .post(format!("{}/v1/device/code", app.base_url))
        .json(&serde_json::json!({ "host_id": "host-team-activate", "org_hint": "acme-activate" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let device_code = code_resp["device_code"].as_str().unwrap().to_string();
    let user_code = code_resp["user_code"].as_str().unwrap().to_string();

    // The org_hint pre-selects "acme-activate" in the dropdown.
    let page = client
        .get(format!("{}/activate?code={user_code}", app.base_url))
        .header(reqwest::header::COOKIE, &web.cookie)
        .send()
        .await
        .unwrap();
    let html = page.text().await.unwrap();
    assert!(
        html.contains(r#"value="acme-activate" selected"#),
        "org_hint should pre-select the team org: {html}"
    );

    let approve = client
        .post(format!("{}/activate", app.base_url))
        .header(reqwest::header::COOKIE, &web.cookie)
        .form(&[("code", user_code.as_str()), ("org_slug", "acme-activate")])
        .send()
        .await
        .unwrap();
    assert_eq!(approve.status(), 200);

    let ok: serde_json::Value = client
        .post(format!("{}/v1/device/token", app.base_url))
        .json(&serde_json::json!({ "device_code": device_code }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ok["status"], "ok");
    assert_eq!(ok["org_slug"], "acme-activate");
    assert_eq!(ok["org_kind"], "team");

    app.teardown().await;
}

#[tokio::test]
async fn autoapprove_still_works_even_with_no_invite_code_configured() {
    // KIKIMIMI_DEV_AUTOAPPROVE=1 must keep working for tests/CI regardless of
    // whether the legacy web-login invite code is configured — it never
    // touches POST /activate or POST /web/login at all (approval happens
    // immediately in POST /v1/device/code), so that gate is simply not in
    // its path.
    let app = TestApp::spawn(SpawnOpts {
        dev_autoapprove: true,
        invite_code: None,
        ..Default::default()
    })
    .await;
    let client = reqwest::Client::new();

    let login = support::login_autoapprove(&client, &app.base_url, "host-auto-no-invite").await;
    assert_eq!(login.token.len(), 43);

    app.teardown().await;
}
