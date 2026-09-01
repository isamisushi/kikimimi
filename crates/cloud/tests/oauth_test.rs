//! GitHub OAuth (`GET /auth/github` / `GET /auth/github/callback`,
//! architecture.md §6.1) — happy path (brand-new account) and the
//! email-link path (an existing legacy account signs in with GitHub for the
//! first time and gets linked by verified email, not duplicated). Every
//! GitHub call is mocked (`support::MockGithub`) — no real network calls.

mod support;

use support::{web_login, MockGithub, SpawnOpts, TestApp};

/// `GET /auth/github` 503s when GitHub OAuth isn't configured at all —
/// sanity check that the feature flag actually gates the route (every other
/// test in this file spawns with it configured).
#[tokio::test]
async fn auth_github_503s_when_unconfigured() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let resp = client
        .get(format!("{}/auth/github", app.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 503);

    app.teardown().await;
}

#[tokio::test]
async fn oauth_callback_happy_path_creates_a_new_account_and_personal_org() {
    let gh = MockGithub::start(1001, "octonaut", "octonaut@example.com", true).await;
    let app = TestApp::spawn(SpawnOpts {
        github_client_id: Some("test-client-id".to_string()),
        github_client_secret: Some("test-client-secret".to_string()),
        github_api_base: Some(gh.base_url.clone()),
        github_oauth_base: Some(gh.base_url.clone()),
        ..Default::default()
    })
    .await;

    let login = support::oauth_login(&app.base_url).await;

    let client = reqwest::Client::new();
    let me: serde_json::Value = client
        .get(format!("{}/web/me", app.base_url))
        .header(reqwest::header::COOKIE, &login.cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(me["email"], "octonaut@example.com");
    assert_eq!(me["github_login"], "octonaut");
    let orgs = me["orgs"].as_array().unwrap();
    assert_eq!(
        orgs.len(),
        1,
        "brand-new account has exactly its personal org: {me:?}"
    );
    assert_eq!(orgs[0]["kind"], "personal");
    assert_eq!(orgs[0]["role"], "owner");
    assert_eq!(me["active_org"], orgs[0]["slug"]);

    app.teardown().await;
    gh.stop().await;
}

/// Requires a *primary and verified* email — GitHub lets an account hide or
/// leave its email unverified, and neither is acceptable as an account
/// identity here.
#[tokio::test]
async fn oauth_callback_rejects_an_unverified_primary_email() {
    let gh = MockGithub::start(1002, "unverified-user", "unverified@example.com", false).await;
    let app = TestApp::spawn(SpawnOpts {
        github_client_id: Some("test-client-id".to_string()),
        github_client_secret: Some("test-client-secret".to_string()),
        github_api_base: Some(gh.base_url.clone()),
        github_oauth_base: Some(gh.base_url.clone()),
        ..Default::default()
    })
    .await;

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let start = client
        .get(format!("{}/auth/github", app.base_url))
        .send()
        .await
        .unwrap();
    let location = start
        .headers()
        .get(reqwest::header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let state_cookie = start
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let state_value = location
        .split("state=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap();

    let cb = client
        .get(format!(
            "{}/auth/github/callback?code=mock-code&state={state_value}",
            app.base_url
        ))
        .header(reqwest::header::COOKIE, &state_cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(
        cb.status(),
        422,
        "no verified primary email must not create a session"
    );

    app.teardown().await;
    gh.stop().await;
}

/// The state-cookie/query-param CSRF check: replaying an old/foreign state
/// value without the matching cookie must not authenticate.
#[tokio::test]
async fn oauth_callback_rejects_a_state_mismatch() {
    let gh = MockGithub::start(1003, "someone", "someone@example.com", true).await;
    let app = TestApp::spawn(SpawnOpts {
        github_client_id: Some("test-client-id".to_string()),
        github_client_secret: Some("test-client-secret".to_string()),
        github_api_base: Some(gh.base_url.clone()),
        github_oauth_base: Some(gh.base_url.clone()),
        ..Default::default()
    })
    .await;

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let start = client
        .get(format!("{}/auth/github", app.base_url))
        .send()
        .await
        .unwrap();
    let state_cookie = start
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();

    let cb = client
        .get(format!(
            "{}/auth/github/callback?code=mock-code&state=not-the-real-state",
            app.base_url
        ))
        .header(reqwest::header::COOKIE, &state_cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(cb.status(), 401);

    app.teardown().await;
    gh.stop().await;
}

/// Email-link path: an account that already exists (created via the legacy
/// email+invite `POST /web/login`) signs in with GitHub for the first time.
/// It must be *linked* by its verified email (same account, same personal
/// org), not duplicated into a second account. Spawned with
/// `legacy_invite: true` so both login paths work side by side on the same
/// deployment/account, which is exactly the scenario this contract line
/// describes ("upsert account by github_id, else link by verified email").
#[tokio::test]
async fn oauth_callback_links_an_existing_legacy_account_by_verified_email() {
    let gh = MockGithub::start(2001, "linked-login", "link-me@example.com", true).await;
    let app = TestApp::spawn(SpawnOpts {
        github_client_id: Some("test-client-id".to_string()),
        github_client_secret: Some("test-client-secret".to_string()),
        github_api_base: Some(gh.base_url.clone()),
        github_oauth_base: Some(gh.base_url.clone()),
        legacy_invite: true,
        ..Default::default()
    })
    .await;
    let client = reqwest::Client::new();

    // The account pre-exists via the legacy path (no GitHub identity yet).
    let legacy = web_login(&client, &app.base_url, "link-me@example.com").await;
    let legacy_me: serde_json::Value = client
        .get(format!("{}/web/me", app.base_url))
        .header(reqwest::header::COOKIE, &legacy.cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(legacy_me["github_login"], serde_json::Value::Null);
    let legacy_org_slug = legacy_me["active_org"].as_str().unwrap().to_string();

    // Now the same email signs in via GitHub for the first time.
    let oauth = support::oauth_login(&app.base_url).await;
    let oauth_me: serde_json::Value = client
        .get(format!("{}/web/me", app.base_url))
        .header(reqwest::header::COOKIE, &oauth.cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(oauth_me["email"], "link-me@example.com");
    assert_eq!(
        oauth_me["github_login"], "linked-login",
        "the account is now linked to the GitHub identity"
    );
    assert_eq!(
        oauth_me["active_org"], legacy_org_slug,
        "linking must land in the SAME personal org, not create a second account/org"
    );

    app.teardown().await;
    gh.stop().await;
}

/// Security review fix: the legacy `POST /web/login` gate is a single
/// deployment-wide invite code, not proof of *this* email's ownership.
/// Before this fix, anyone who knew the code could keep authenticating as
/// `link-me@example.com` via the legacy path forever, even after the real
/// owner proved control of that mailbox via GitHub's verified email. Two
/// things must now hold once the link happens: (1) the legacy session that
/// predates the link stops working, and (2) the legacy login path refuses
/// to mint a *new* session for that email at all — GitHub is the only way
/// in for a linked account from here on.
#[tokio::test]
async fn oauth_link_revokes_the_pre_existing_legacy_session_and_blocks_further_legacy_logins() {
    let gh = MockGithub::start(2002, "linked-login-2", "takeover-target@example.com", true).await;
    let app = TestApp::spawn(SpawnOpts {
        github_client_id: Some("test-client-id".to_string()),
        github_client_secret: Some("test-client-secret".to_string()),
        github_api_base: Some(gh.base_url.clone()),
        github_oauth_base: Some(gh.base_url.clone()),
        legacy_invite: true,
        ..Default::default()
    })
    .await;
    let client = reqwest::Client::new();

    // An attacker (or anyone else who knows the shared invite code) logs in
    // as this email via the legacy path *before* its real owner ever uses
    // GitHub OAuth -- exactly what the legacy flow allows, since it never
    // asks for proof of mailbox ownership.
    let pre_link = web_login(&client, &app.base_url, "takeover-target@example.com").await;
    let pre_link_me = client
        .get(format!("{}/web/me", app.base_url))
        .header(reqwest::header::COOKIE, &pre_link.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(
        pre_link_me.status(),
        200,
        "the pre-link session must work before linking happens"
    );

    // The real owner now signs in with GitHub, proving control of the same
    // email via GitHub's own verified-email check.
    let oauth = support::oauth_login(&app.base_url).await;
    let oauth_me: serde_json::Value = client
        .get(format!("{}/web/me", app.base_url))
        .header(reqwest::header::COOKIE, &oauth.cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(oauth_me["github_login"], "linked-login-2");

    // (1) The session minted before the link must be dead now.
    let post_link_me = client
        .get(format!("{}/web/me", app.base_url))
        .header(reqwest::header::COOKIE, &pre_link.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(
        post_link_me.status(),
        401,
        "the pre-link legacy session must be revoked the moment GitHub linking happens"
    );

    // (2) The legacy path must refuse to mint a brand new session for this
    // email too -- knowing the invite code is no longer enough once this
    // account is GitHub-linked.
    let blocked = client
        .post(format!("{}/web/login", app.base_url))
        .json(&serde_json::json!({
            "email": "takeover-target@example.com",
            "invite_code": support::TEST_INVITE_CODE,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        blocked.status(),
        403,
        "legacy login must refuse to authenticate as a GitHub-linked account"
    );
    assert!(
        blocked.headers().get(reqwest::header::SET_COOKIE).is_none(),
        "a blocked legacy login must not set a session cookie"
    );

    app.teardown().await;
    gh.stop().await;
}
