//! Additive endpoints built for `web/` (the SPA) that aren't already covered
//! by `account_model_test.rs` / `device_flow_test.rs` / `oauth_test.rs`:
//! `GET /web/config`, `GET /web/orgs/:slug/members`, `GET /web/invites/:token`
//! + the SPA-owned `GET /join/:token` shell, and the Bearer-token
//! `GET /v1/devices` / `POST /v1/devices/:id/revoke` / `GET /v1/orgs` trio
//! the CLI's `kikimimi devices` / `kikimimi orgs` call (crates/cli/src/
//! devices_cmd.rs and orgs_cmd.rs's "CONTRACT NOTE").

mod support;

use support::{web_login, SpawnOpts, TestApp};

async fn create_team_org(client: &reqwest::Client, base_url: &str, cookie: &str, name: &str, slug: &str) {
    let resp = client
        .post(format!("{base_url}/web/orgs"))
        .header(reqwest::header::COOKIE, cookie)
        .json(&serde_json::json!({ "name": name, "slug": slug }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "{}", resp.text().await.unwrap());
}

async fn create_invite(
    client: &reqwest::Client,
    base_url: &str,
    cookie: &str,
    slug: &str,
    role: &str,
    max_uses: Option<i32>,
) -> reqwest::Response {
    client
        .post(format!("{base_url}/web/orgs/{slug}/invites"))
        .header(reqwest::header::COOKIE, cookie)
        .json(&serde_json::json!({ "role": role, "max_uses": max_uses }))
        .send()
        .await
        .unwrap()
}

// ---------------------------------------------------------------------------
// GET /web/config
// ---------------------------------------------------------------------------

#[tokio::test]
async fn config_defaults_to_legacy_login_only() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let body: serde_json::Value = reqwest::Client::new()
        .get(format!("{}/web/config", app.base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["github_oauth"], false);
    assert_eq!(body["legacy_login"], true);
    app.teardown().await;
}

#[tokio::test]
async fn config_reflects_github_configured_without_legacy_flag() {
    let app = TestApp::spawn(SpawnOpts {
        github_client_id: Some("id".to_string()),
        github_client_secret: Some("secret".to_string()),
        legacy_invite: false,
        ..Default::default()
    })
    .await;
    let body: serde_json::Value = reqwest::Client::new()
        .get(format!("{}/web/config", app.base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["github_oauth"], true);
    assert_eq!(body["legacy_login"], false);
    app.teardown().await;
}

#[tokio::test]
async fn config_reflects_github_configured_with_legacy_flag() {
    let app = TestApp::spawn(SpawnOpts {
        github_client_id: Some("id".to_string()),
        github_client_secret: Some("secret".to_string()),
        legacy_invite: true,
        ..Default::default()
    })
    .await;
    let body: serde_json::Value = reqwest::Client::new()
        .get(format!("{}/web/config", app.base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["github_oauth"], true);
    assert_eq!(body["legacy_login"], true);
    app.teardown().await;
}

#[tokio::test]
async fn config_is_unauthenticated() {
    // The login page needs this *before* there's any session -- must not
    // require a cookie.
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let resp = reqwest::Client::new()
        .get(format!("{}/web/config", app.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    app.teardown().await;
}

// ---------------------------------------------------------------------------
// GET /web/orgs/:slug/members
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_members_admin_sees_everyone_member_is_forbidden() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();
    let owner = web_login(&client, &app.base_url, "members-owner@example.com").await;

    let created = client
        .post(format!("{}/web/orgs", app.base_url))
        .header(reqwest::header::COOKIE, &owner.cookie)
        .json(&serde_json::json!({ "name": "Members Co", "slug": "members-co" }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 200);

    let invite: serde_json::Value = client
        .post(format!("{}/web/orgs/members-co/invites", app.base_url))
        .header(reqwest::header::COOKIE, &owner.cookie)
        .json(&serde_json::json!({ "role": "member" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let join_url = invite["url"].as_str().unwrap().to_string();

    let member = web_login(&client, &app.base_url, "members-member@example.com").await;
    let joined = client
        .post(format!("{}{join_url}", app.base_url))
        .header(reqwest::header::COOKIE, &member.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(joined.status(), 200);

    // Owner sees both members, with roles.
    let members_resp: serde_json::Value = client
        .get(format!("{}/web/orgs/members-co/members", app.base_url))
        .header(reqwest::header::COOKIE, &owner.cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let members = members_resp["members"].as_array().unwrap();
    assert_eq!(members.len(), 2, "{members_resp:?}");
    assert!(members.iter().any(|m| m["email"] == "members-owner@example.com" && m["role"] == "owner"));
    assert!(members.iter().any(|m| m["email"] == "members-member@example.com" && m["role"] == "member"));

    // A plain member is forbidden from listing members.
    let member_view = client
        .get(format!("{}/web/orgs/members-co/members", app.base_url))
        .header(reqwest::header::COOKIE, &member.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(member_view.status(), 403);

    app.teardown().await;
}

#[tokio::test]
async fn list_members_404s_for_a_nonexistent_org() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();
    let owner = web_login(&client, &app.base_url, "members-404@example.com").await;
    let resp = client
        .get(format!("{}/web/orgs/does-not-exist/members", app.base_url))
        .header(reqwest::header::COOKIE, &owner.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    app.teardown().await;
}

// ---------------------------------------------------------------------------
// GET /web/invites/:token + GET /join/:token (SPA shell)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn invite_info_reports_usable_then_revoked() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();
    let owner = web_login(&client, &app.base_url, "invite-info-owner@example.com").await;
    client
        .post(format!("{}/web/orgs", app.base_url))
        .header(reqwest::header::COOKIE, &owner.cookie)
        .json(&serde_json::json!({ "name": "Invite Info Co", "slug": "invite-info-co" }))
        .send()
        .await
        .unwrap();
    let invite: serde_json::Value = client
        .post(format!("{}/web/orgs/invite-info-co/invites", app.base_url))
        .header(reqwest::header::COOKIE, &owner.cookie)
        .json(&serde_json::json!({ "role": "viewer" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let url = invite["url"].as_str().unwrap().to_string();
    let token = url.trim_start_matches("/join/").to_string();

    let previewer = web_login(&client, &app.base_url, "invite-info-previewer@example.com").await;
    let info: serde_json::Value = client
        .get(format!("{}/web/invites/{token}", app.base_url))
        .header(reqwest::header::COOKIE, &previewer.cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info["org_name"], "Invite Info Co");
    assert_eq!(info["role"], "viewer");
    assert_eq!(info["usable"], true);
    assert_eq!(info["revoked"], false);

    // Unauthenticated -> 401 (auth required, same as every other
    // WebSessionContext-gated endpoint).
    let anon = client
        .get(format!("{}/web/invites/{token}", app.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status(), 401);

    // Unknown token -> 404.
    let missing = client
        .get(format!("{}/web/invites/not-a-real-token", app.base_url))
        .header(reqwest::header::COOKIE, &previewer.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);

    // Revoke it; usable flips to false with a reason.
    let invites_list: serde_json::Value = client
        .get(format!("{}/web/orgs/invite-info-co/invites", app.base_url))
        .header(reqwest::header::COOKIE, &owner.cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let invite_id = invites_list["invites"][0]["id"].as_str().unwrap();
    client
        .delete(format!("{}/web/orgs/invite-info-co/invites/{invite_id}", app.base_url))
        .header(reqwest::header::COOKIE, &owner.cookie)
        .send()
        .await
        .unwrap();

    let info_after: serde_json::Value = client
        .get(format!("{}/web/invites/{token}", app.base_url))
        .header(reqwest::header::COOKIE, &previewer.cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info_after["usable"], false);
    assert_eq!(info_after["revoked"], true);

    app.teardown().await;
}

#[tokio::test]
async fn join_get_serves_the_spa_shell_not_a_bespoke_html_page() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();

    // No cookie at all -- still 200 (the SPA shell is public; only the data
    // endpoints it calls, like GET /web/invites/:token above, require auth).
    let resp = client
        .get(format!("{}/join/some-token", app.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let content_type = resp.headers().get(reqwest::header::CONTENT_TYPE).unwrap().to_str().unwrap().to_string();
    assert!(content_type.contains("text/html"), "{content_type}");
    let body = resp.text().await.unwrap();
    assert!(body.contains(r#"<div id="root">"#), "expected the SPA shell, got: {body}");

    app.teardown().await;
}

// ---------------------------------------------------------------------------
// GET /v1/devices / POST /v1/devices/:id/revoke (Bearer -- crates/cli's
// `kikimimi devices`)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn v1_devices_lists_only_the_callers_own_devices_across_orgs_and_flags_current() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();
    let owner = web_login(&client, &app.base_url, "v1-devices-owner@example.com").await;
    let other = web_login(&client, &app.base_url, "v1-devices-other@example.com").await;

    let dev_a = support::activate_device_into_org(
        &client,
        &app.base_url,
        "host-a",
        &owner.cookie,
        &support::active_org_slug(&client, &app.base_url, &owner.cookie).await,
    )
    .await;
    let dev_b = support::activate_device_into_org(
        &client,
        &app.base_url,
        "host-b",
        &owner.cookie,
        &support::active_org_slug(&client, &app.base_url, &owner.cookie).await,
    )
    .await;
    // A device belonging to a different account must never show up in
    // owner's listing.
    let _other_dev = support::activate_device_into_org(
        &client,
        &app.base_url,
        "host-other",
        &other.cookie,
        &support::active_org_slug(&client, &app.base_url, &other.cookie).await,
    )
    .await;

    let listed: serde_json::Value = client
        .get(format!("{}/v1/devices", app.base_url))
        .bearer_auth(&dev_a.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let devices = listed["devices"].as_array().unwrap();
    assert_eq!(devices.len(), 2, "{listed:?}");
    let host_ids: Vec<&str> = devices.iter().map(|d| d["host_id"].as_str().unwrap()).collect();
    assert!(host_ids.contains(&"host-a"));
    assert!(host_ids.contains(&"host-b"));
    assert!(!host_ids.contains(&"host-other"));

    // The device whose token authenticated the request is flagged current.
    let current_row = devices.iter().find(|d| d["host_id"] == "host-a").unwrap();
    assert_eq!(current_row["current"], true);
    let other_row = devices.iter().find(|d| d["host_id"] == "host-b").unwrap();
    assert_eq!(other_row["current"], false);
    assert!(current_row["org_slug"].as_str().is_some());
    assert!(current_row["org_kind"].as_str().is_some());

    // Missing/garbage bearer -> 401.
    let unauth = client.get(format!("{}/v1/devices", app.base_url)).send().await.unwrap();
    assert_eq!(unauth.status(), 401);

    let _ = dev_b;
    app.teardown().await;
}

#[tokio::test]
async fn v1_devices_revoke_only_the_owning_accounts_device_then_stops_authenticating() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();
    let owner = web_login(&client, &app.base_url, "v1-revoke-owner@example.com").await;
    let other = web_login(&client, &app.base_url, "v1-revoke-other@example.com").await;

    let dev = support::activate_device_into_org(
        &client,
        &app.base_url,
        "host-revoke-me",
        &owner.cookie,
        &support::active_org_slug(&client, &app.base_url, &owner.cookie).await,
    )
    .await;
    let other_dev = support::activate_device_into_org(
        &client,
        &app.base_url,
        "host-not-yours",
        &other.cookie,
        &support::active_org_slug(&client, &app.base_url, &other.cookie).await,
    )
    .await;

    let listed: serde_json::Value = client
        .get(format!("{}/v1/devices", app.base_url))
        .bearer_auth(&other_dev.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let other_device_id = listed["devices"][0]["id"].as_str().unwrap().to_string();

    // `owner`'s token can't revoke `other`'s device id -- 404, not 403 (never
    // confirm it exists to a non-owner).
    let denied = client
        .post(format!("{}/v1/devices/{other_device_id}/revoke", app.base_url))
        .bearer_auth(&dev.token)
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 404);

    // Revoking a real, owned device id (not necessarily the calling token's
    // own row) succeeds and the token stops authenticating anywhere.
    let self_listed: serde_json::Value = client
        .get(format!("{}/v1/devices", app.base_url))
        .bearer_auth(&dev.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let own_device_id = self_listed["devices"][0]["id"].as_str().unwrap().to_string();
    let revoke = client
        .post(format!("{}/v1/devices/{own_device_id}/revoke", app.base_url))
        .bearer_auth(&dev.token)
        .send()
        .await
        .unwrap();
    assert_eq!(revoke.status(), 200);
    let body: serde_json::Value = revoke.json().await.unwrap();
    assert_eq!(body["status"], "revoked");

    let check = client
        .get(format!("{}/v1/query/today", app.base_url))
        .bearer_auth(&dev.token)
        .send()
        .await
        .unwrap();
    assert_eq!(check.status(), 401, "revoked device token must stop authenticating");

    // Revoking an id that doesn't exist at all -> 404.
    let bogus = client
        .post(format!("{}/v1/devices/{}/revoke", app.base_url, uuid::Uuid::new_v4()))
        .bearer_auth(&other_dev.token)
        .send()
        .await
        .unwrap();
    assert_eq!(bogus.status(), 404);

    app.teardown().await;
}

// ---------------------------------------------------------------------------
// GET /v1/orgs (Bearer -- crates/cli's `kikimimi orgs`)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn v1_orgs_lists_the_bearer_tokens_owning_account_memberships_with_roles() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();
    let owner = web_login(&client, &app.base_url, "v1-orgs-owner@example.com").await;
    create_team_org(&client, &app.base_url, &owner.cookie, "Acme Inc", "v1-orgs-acme").await;

    // A device bound to the owner's *personal* org still sees every one of
    // the account's memberships, not just the org the device happens to be
    // bound to (mirrors `GET /web/me`'s `orgs`, minus `active_org`).
    let personal_slug = support::active_org_slug(&client, &app.base_url, &owner.cookie).await;
    let dev = support::activate_device_into_org(&client, &app.base_url, "host-v1-orgs", &owner.cookie, &personal_slug).await;

    let listed: serde_json::Value = client
        .get(format!("{}/v1/orgs", app.base_url))
        .bearer_auth(&dev.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let orgs = listed["orgs"].as_array().unwrap();
    assert_eq!(orgs.len(), 2, "{listed:?}");
    let personal = orgs.iter().find(|o| o["slug"] == personal_slug).expect("personal org listed");
    assert_eq!(personal["kind"], "personal");
    assert_eq!(personal["role"], "owner");
    let acme = orgs.iter().find(|o| o["slug"] == "v1-orgs-acme").expect("team org listed");
    assert_eq!(acme["kind"], "team");
    assert_eq!(acme["role"], "owner");
    assert!(acme["name"].as_str().is_some());
    // No `active_org` field -- a device's active org is fixed at approval
    // time, unlike a browser session's `/web/me`.
    assert!(listed.get("active_org").is_none());

    // A different account, joined to the same team org as `member`, sees its
    // own personal org plus that membership's role -- not the owner's.
    let created = create_invite(&client, &app.base_url, &owner.cookie, "v1-orgs-acme", "member", None).await;
    assert_eq!(created.status(), 200);
    let created_body: serde_json::Value = created.json().await.unwrap();
    let join_url = created_body["url"].as_str().unwrap().to_string();
    let joiner = web_login(&client, &app.base_url, "v1-orgs-joiner@example.com").await;
    let post_join = client
        .post(format!("{}{join_url}", app.base_url))
        .header(reqwest::header::COOKIE, &joiner.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(post_join.status(), 200);
    let joiner_dev =
        support::activate_device_into_org(&client, &app.base_url, "host-v1-orgs-joiner", &joiner.cookie, "v1-orgs-acme").await;

    let joiner_listed: serde_json::Value = client
        .get(format!("{}/v1/orgs", app.base_url))
        .bearer_auth(&joiner_dev.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let joiner_orgs = joiner_listed["orgs"].as_array().unwrap();
    assert_eq!(joiner_orgs.len(), 2, "{joiner_listed:?}");
    let joiner_acme = joiner_orgs.iter().find(|o| o["slug"] == "v1-orgs-acme").expect("joiner sees acme");
    assert_eq!(joiner_acme["role"], "member");

    // Missing/garbage bearer -> 401.
    let unauth = client.get(format!("{}/v1/orgs", app.base_url)).send().await.unwrap();
    assert_eq!(unauth.status(), 401);
    let bad_token = client
        .get(format!("{}/v1/orgs", app.base_url))
        .bearer_auth("not-a-real-token")
        .send()
        .await
        .unwrap();
    assert_eq!(bad_token.status(), 401);

    app.teardown().await;
}
