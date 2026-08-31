mod support;

use support::{login_autoapprove, SpawnOpts, TestApp, TEST_INVITE_CODE};

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

    // GET /activate renders a form containing the user_code, host_id, and
    // (since SpawnOpts::default() configures an invite code) a required
    // invite code input.
    let page = client
        .get(format!("{}/activate?code={user_code}", app.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), 200);
    let html = page.text().await.unwrap();
    assert!(html.contains(&user_code));
    assert!(html.contains("host-manual"));
    assert!(html.contains("<form"));
    assert!(html.contains(r#"name="invite_code""#));
    assert!(html.contains("required"));

    // POST /activate approves (form-urlencoded, like a plain HTML <form>)
    // with the correct invite code.
    let approve = client
        .post(format!("{}/activate", app.base_url))
        .form(&[
            ("code", user_code.as_str()),
            ("email", "person@example.com"),
            ("invite_code", TEST_INVITE_CODE),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(approve.status(), 200);

    // Now the poll materializes the account/org/device and returns a token.
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
// Invite-code gate (KIKIMIMI_INVITE_CODE) — public deployment activation gating.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn activate_with_wrong_invite_code_is_403_and_not_approved() {
    let app = TestApp::spawn(SpawnOpts::default()).await; // invite_code = Some(TEST_INVITE_CODE)
    let client = reqwest::Client::new();

    let code_resp: serde_json::Value = client
        .post(format!("{}/v1/device/code", app.base_url))
        .json(&serde_json::json!({ "host_id": "host-wrong-invite", "hostname": "h" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let device_code = code_resp["device_code"].as_str().unwrap().to_string();
    let user_code = code_resp["user_code"].as_str().unwrap().to_string();

    let approve = client
        .post(format!("{}/activate", app.base_url))
        .form(&[
            ("code", user_code.as_str()),
            ("email", "person@example.com"),
            ("invite_code", "definitely-not-the-right-code"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(approve.status(), 403, "wrong invite code must be rejected with 403");

    // Not approved: the device_code must still be pending, not materialized.
    let pending: serde_json::Value = client
        .post(format!("{}/v1/device/token", app.base_url))
        .json(&serde_json::json!({ "device_code": device_code }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(pending["status"], "pending", "a wrong invite code must not approve the device");

    // Missing invite_code field entirely must also be rejected with 403.
    let missing = client
        .post(format!("{}/activate", app.base_url))
        .form(&[("code", user_code.as_str()), ("email", "person@example.com")])
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 403, "missing invite code must also be rejected with 403");

    app.teardown().await;
}

#[tokio::test]
async fn five_wrong_invite_code_attempts_expires_the_device_code() {
    let app = TestApp::spawn(SpawnOpts::default()).await; // invite_code = Some(TEST_INVITE_CODE)
    let client = reqwest::Client::new();

    let code_resp: serde_json::Value = client
        .post(format!("{}/v1/device/code", app.base_url))
        .json(&serde_json::json!({ "host_id": "host-brute-force", "hostname": "h" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let device_code = code_resp["device_code"].as_str().unwrap().to_string();
    let user_code = code_resp["user_code"].as_str().unwrap().to_string();

    for attempt in 1..=5 {
        let approve = client
            .post(format!("{}/activate", app.base_url))
            .form(&[
                ("code", user_code.as_str()),
                ("email", "person@example.com"),
                ("invite_code", "still-wrong"),
            ])
            .send()
            .await
            .unwrap();
        assert_eq!(approve.status(), 403, "attempt {attempt} should be a 403");
    }

    // 5 wrong attempts must have expired the device_code: the CLI's next
    // poll gets 410, same as any other expired/unknown code.
    let resp = client
        .post(format!("{}/v1/device/token", app.base_url))
        .json(&serde_json::json!({ "device_code": device_code }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        410,
        "device_code must be expired after 5 wrong invite code attempts"
    );

    // A subsequent activate attempt (even with the correct code) must now
    // see the code as invalid/expired, not silently succeed.
    let approve_after_expiry = client
        .post(format!("{}/activate", app.base_url))
        .form(&[
            ("code", user_code.as_str()),
            ("email", "person@example.com"),
            ("invite_code", TEST_INVITE_CODE),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(approve_after_expiry.status(), 200);
    let body = approve_after_expiry.text().await.unwrap();
    assert!(body.contains("invalid or has expired"));

    app.teardown().await;
}

#[tokio::test]
async fn activate_with_neither_invite_code_nor_autoapprove_configured_is_503() {
    let app = TestApp::spawn(SpawnOpts {
        dev_autoapprove: false,
        invite_code: None,
        ..Default::default()
    })
    .await;
    let client = reqwest::Client::new();

    let code_resp: serde_json::Value = client
        .post(format!("{}/v1/device/code", app.base_url))
        .json(&serde_json::json!({ "host_id": "host-unconfigured", "hostname": "h" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let user_code = code_resp["user_code"].as_str().unwrap().to_string();

    // GET /activate must not render an invite code field when unconfigured.
    let page = client
        .get(format!("{}/activate?code={user_code}", app.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), 200);
    let html = page.text().await.unwrap();
    assert!(!html.contains(r#"name="invite_code""#));

    let approve = client
        .post(format!("{}/activate", app.base_url))
        .form(&[("code", user_code.as_str()), ("email", "person@example.com")])
        .send()
        .await
        .unwrap();
    assert_eq!(
        approve.status(),
        503,
        "with neither KIKIMIMI_INVITE_CODE nor KIKIMIMI_DEV_AUTOAPPROVE set, activation must fail closed"
    );

    app.teardown().await;
}

#[tokio::test]
async fn autoapprove_still_works_even_with_no_invite_code_configured() {
    // KIKIMIMI_DEV_AUTOAPPROVE=1 must keep working for tests/CI regardless of
    // whether an invite code is configured — it never touches POST
    // /activate at all (approval happens immediately in POST
    // /v1/device/code), so the invite-code gate is simply not in its path.
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
