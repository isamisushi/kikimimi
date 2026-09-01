//! Legacy email+invite login gating matrix (account-model contract):
//! "the legacy email+invite login KEEPS working until OAuth is configured,
//! and is disabled (404) when GITHUB_CLIENT_ID is set unless
//! KIKIMIMI_LEGACY_INVITE=1". Also checks `GET /auth/github`'s own
//! independent gate (needs *both* GITHUB_CLIENT_ID and _SECRET, not just the
//! one `POST /web/login`'s gate keys on).

mod support;

use support::{SpawnOpts, TestApp};

async fn web_login_status(base_url: &str) -> u16 {
    reqwest::Client::new()
        .post(format!("{base_url}/web/login"))
        .json(&serde_json::json!({ "email": "matrix@example.com", "invite_code": support::TEST_INVITE_CODE }))
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

async fn auth_github_status(base_url: &str) -> u16 {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .get(format!("{base_url}/auth/github"))
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

/// Neither `GITHUB_CLIENT_ID` nor `_SECRET` set: legacy login works, GitHub
/// OAuth 503s.
#[tokio::test]
async fn oauth_unconfigured_legacy_login_works_github_503s() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    assert_eq!(web_login_status(&app.base_url).await, 200);
    assert_eq!(auth_github_status(&app.base_url).await, 503);
    app.teardown().await;
}

/// `GITHUB_CLIENT_ID` + `_SECRET` both set, `KIKIMIMI_LEGACY_INVITE` unset:
/// legacy login 404s (looks unrouted), GitHub OAuth works (redirects).
#[tokio::test]
async fn oauth_configured_without_legacy_flag_disables_legacy_login() {
    let app = TestApp::spawn(SpawnOpts {
        github_client_id: Some("id".to_string()),
        github_client_secret: Some("secret".to_string()),
        legacy_invite: false,
        ..Default::default()
    })
    .await;
    assert_eq!(web_login_status(&app.base_url).await, 404);
    assert_eq!(auth_github_status(&app.base_url).await, 302);
    app.teardown().await;
}

/// `GITHUB_CLIENT_ID` + `_SECRET` set AND `KIKIMIMI_LEGACY_INVITE=1`: both
/// paths work side by side.
#[tokio::test]
async fn oauth_configured_with_legacy_flag_keeps_both_paths_open() {
    let app = TestApp::spawn(SpawnOpts {
        github_client_id: Some("id".to_string()),
        github_client_secret: Some("secret".to_string()),
        legacy_invite: true,
        ..Default::default()
    })
    .await;
    assert_eq!(web_login_status(&app.base_url).await, 200);
    assert_eq!(auth_github_status(&app.base_url).await, 302);
    app.teardown().await;
}

/// The legacy-login gate keys only on `GITHUB_CLIENT_ID` being set (account-
/// model contract's literal wording), independent of whether `_SECRET` is
/// also set -- even a half-configured `GITHUB_CLIENT_ID` signals "this
/// deployment intends to use GitHub auth" and disables the legacy path,
/// while `GET /auth/github` itself still needs both to do anything but 503.
#[tokio::test]
async fn client_id_alone_disables_legacy_login_even_without_a_secret() {
    let app = TestApp::spawn(SpawnOpts {
        github_client_id: Some("id-without-a-secret".to_string()),
        github_client_secret: None,
        legacy_invite: false,
        ..Default::default()
    })
    .await;
    assert_eq!(web_login_status(&app.base_url).await, 404);
    assert_eq!(
        auth_github_status(&app.base_url).await,
        503,
        "GET /auth/github still needs both id and secret"
    );
    app.teardown().await;
}

/// Same half-configured case, but with `KIKIMIMI_LEGACY_INVITE=1`: legacy
/// login is back on (the flag alone reopens it), GitHub OAuth still 503s.
#[tokio::test]
async fn client_id_alone_with_legacy_flag_reopens_legacy_login() {
    let app = TestApp::spawn(SpawnOpts {
        github_client_id: Some("id-without-a-secret".to_string()),
        github_client_secret: None,
        legacy_invite: true,
        ..Default::default()
    })
    .await;
    assert_eq!(web_login_status(&app.base_url).await, 200);
    assert_eq!(auth_github_status(&app.base_url).await, 503);
    app.teardown().await;
}
