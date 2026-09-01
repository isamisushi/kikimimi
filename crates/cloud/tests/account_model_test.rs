//! Org/team management + role enforcement (account-model contract, §"Org/
//! team API" and "Role/purpose limits"): create a team org, invite lifecycle
//! (join / expiry / revoked / max_uses), role enforcement (member cannot
//! create invites; member `/web/q/sessions` scoped to self in a team org;
//! admin drilldown writes `audit_log`), and device listing/revocation.

mod support;

use support::{login_as, recent_tool_call_event, web_login, SpawnOpts, TestApp};
use uuid::Uuid;

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

fn join_path(url: &str) -> String {
    url.trim_start_matches('"').to_string()
}

// ---------------------------------------------------------------------------
// POST /web/orgs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_org_makes_the_caller_owner_and_switches_are_reflected_on_web_me() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();
    let owner = web_login(&client, &app.base_url, "founder@example.com").await;

    create_team_org(&client, &app.base_url, &owner.cookie, "Acme Inc", "acme").await;

    let me: serde_json::Value = client
        .get(format!("{}/web/me", app.base_url))
        .header(reqwest::header::COOKIE, &owner.cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let orgs = me["orgs"].as_array().unwrap();
    assert_eq!(orgs.len(), 2, "personal + the new team org: {me:?}");
    let acme = orgs.iter().find(|o| o["slug"] == "acme").expect("acme in orgs list");
    assert_eq!(acme["role"], "owner");
    assert_eq!(acme["kind"], "team");
    // Creating an org doesn't switch to it automatically.
    assert_ne!(me["active_org"], "acme");

    // POST /web/active-org switches it.
    let switch = client
        .post(format!("{}/web/active-org", app.base_url))
        .header(reqwest::header::COOKIE, &owner.cookie)
        .json(&serde_json::json!({ "slug": "acme" }))
        .send()
        .await
        .unwrap();
    assert_eq!(switch.status(), 200);
    let me2: serde_json::Value = client
        .get(format!("{}/web/me", app.base_url))
        .header(reqwest::header::COOKIE, &owner.cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(me2["active_org"], "acme");

    app.teardown().await;
}

#[tokio::test]
async fn create_org_rejects_a_taken_slug_and_an_invalid_slug() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();
    let a = web_login(&client, &app.base_url, "a@example.com").await;
    let b = web_login(&client, &app.base_url, "b@example.com").await;

    create_team_org(&client, &app.base_url, &a.cookie, "Taken", "dup-slug").await;
    let resp = client
        .post(format!("{}/web/orgs", app.base_url))
        .header(reqwest::header::COOKIE, &b.cookie)
        .json(&serde_json::json!({ "name": "Also Taken", "slug": "dup-slug" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    let bad = client
        .post(format!("{}/web/orgs", app.base_url))
        .header(reqwest::header::COOKIE, &b.cookie)
        .json(&serde_json::json!({ "name": "Bad Slug", "slug": "Not Valid!" }))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);

    app.teardown().await;
}

/// Switching to an org you're not a member of must fail, not silently work.
#[tokio::test]
async fn active_org_switch_requires_membership() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();
    let a = web_login(&client, &app.base_url, "switch-a@example.com").await;
    let b = web_login(&client, &app.base_url, "switch-b@example.com").await;
    create_team_org(&client, &app.base_url, &b.cookie, "B Co", "b-co-switch").await;

    let resp = client
        .post(format!("{}/web/active-org", app.base_url))
        .header(reqwest::header::COOKIE, &a.cookie)
        .json(&serde_json::json!({ "slug": "b-co-switch" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);

    app.teardown().await;
}

// ---------------------------------------------------------------------------
// Invite lifecycle: create -> join, expiry, revoked, max_uses.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn invite_lifecycle_join_then_list_shows_it_then_revoke_blocks_further_joins() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();
    let owner = web_login(&client, &app.base_url, "invite-owner@example.com").await;
    create_team_org(&client, &app.base_url, &owner.cookie, "Invite Co", "invite-co").await;

    let created = create_invite(&client, &app.base_url, &owner.cookie, "invite-co", "member", None).await;
    assert_eq!(created.status(), 200);
    let created_body: serde_json::Value = created.json().await.unwrap();
    let join_url = join_path(created_body["url"].as_str().unwrap());
    assert!(join_url.starts_with("/join/"), "{join_url}");

    // Listing shows the invite, unused so far.
    let list: serde_json::Value = client
        .get(format!("{}/web/orgs/invite-co/invites", app.base_url))
        .header(reqwest::header::COOKIE, &owner.cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let invites = list["invites"].as_array().unwrap();
    assert_eq!(invites.len(), 1);
    assert_eq!(invites[0]["role"], "member");
    assert_eq!(invites[0]["uses"], 0);

    // A different account joins via the link.
    let joiner = web_login(&client, &app.base_url, "joiner@example.com").await;
    let get_page = client
        .get(format!("{}{join_url}", app.base_url))
        .header(reqwest::header::COOKIE, &joiner.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(get_page.status(), 200);
    let post_join = client
        .post(format!("{}{join_url}", app.base_url))
        .header(reqwest::header::COOKIE, &joiner.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(post_join.status(), 200);
    let joined_body: serde_json::Value = post_join.json().await.unwrap();
    assert_eq!(joined_body["org_slug"], "invite-co");
    assert_eq!(joined_body["role"], "member");

    let joiner_me: serde_json::Value = client
        .get(format!("{}/web/me", app.base_url))
        .header(reqwest::header::COOKIE, &joiner.cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let membership = joiner_me["orgs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["slug"] == "invite-co")
        .expect("joiner now a member of invite-co");
    assert_eq!(membership["role"], "member");

    let uses_after: serde_json::Value = client
        .get(format!("{}/web/orgs/invite-co/invites", app.base_url))
        .header(reqwest::header::COOKIE, &owner.cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(uses_after["invites"][0]["uses"], 1);

    // Revoke it; another account can no longer join.
    let invite_id = uses_after["invites"][0]["id"].as_str().unwrap();
    let revoke = client
        .delete(format!("{}/web/orgs/invite-co/invites/{invite_id}", app.base_url))
        .header(reqwest::header::COOKIE, &owner.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(revoke.status(), 200);

    let third = web_login(&client, &app.base_url, "too-late@example.com").await;
    let blocked = client
        .post(format!("{}{join_url}", app.base_url))
        .header(reqwest::header::COOKIE, &third.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(blocked.status(), 400, "a revoked invite must not admit a new member");

    app.teardown().await;
}

#[tokio::test]
async fn invite_with_max_uses_one_admits_exactly_one_member() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();
    let owner = web_login(&client, &app.base_url, "maxuse-owner@example.com").await;
    create_team_org(&client, &app.base_url, &owner.cookie, "Max Use Co", "maxuse-co").await;

    let created = create_invite(&client, &app.base_url, &owner.cookie, "maxuse-co", "viewer", Some(1)).await;
    let body: serde_json::Value = created.json().await.unwrap();
    let join_url = join_path(body["url"].as_str().unwrap());

    let first = web_login(&client, &app.base_url, "first-joiner@example.com").await;
    let resp1 = client
        .post(format!("{}{join_url}", app.base_url))
        .header(reqwest::header::COOKIE, &first.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp1.status(), 200);

    let second = web_login(&client, &app.base_url, "second-joiner@example.com").await;
    let resp2 = client
        .post(format!("{}{join_url}", app.base_url))
        .header(reqwest::header::COOKIE, &second.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp2.status(), 400, "max_uses=1 must block a second joiner");

    app.teardown().await;
}

#[tokio::test]
async fn expired_invite_is_rejected() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();
    let owner = web_login(&client, &app.base_url, "expiry-owner@example.com").await;
    create_team_org(&client, &app.base_url, &owner.cookie, "Expiry Co", "expiry-co").await;

    let created = client
        .post(format!("{}/web/orgs/expiry-co/invites", app.base_url))
        .header(reqwest::header::COOKIE, &owner.cookie)
        .json(&serde_json::json!({ "role": "member", "expires_hours": 1 }))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = created.json().await.unwrap();
    let join_url = join_path(body["url"].as_str().unwrap());

    // Force it into the past directly (only one invite row exists here).
    sqlx::query("UPDATE org_invites SET expires_at = now() - interval '1 hour'")
        .execute(&app.state.pools.superuser)
        .await
        .expect("expire the invite");

    let joiner = web_login(&client, &app.base_url, "too-slow@example.com").await;
    let resp = client
        .post(format!("{}{join_url}", app.base_url))
        .header(reqwest::header::COOKIE, &joiner.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    app.teardown().await;
}

// ---------------------------------------------------------------------------
// Role enforcement.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn member_cannot_create_invites() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();
    let owner = web_login(&client, &app.base_url, "role-owner@example.com").await;
    create_team_org(&client, &app.base_url, &owner.cookie, "Role Co", "role-co").await;

    let created = create_invite(&client, &app.base_url, &owner.cookie, "role-co", "member", None).await;
    let body: serde_json::Value = created.json().await.unwrap();
    let join_url = join_path(body["url"].as_str().unwrap());

    let member = web_login(&client, &app.base_url, "role-member@example.com").await;
    let join = client
        .post(format!("{}{join_url}", app.base_url))
        .header(reqwest::header::COOKIE, &member.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(join.status(), 200);

    // The member tries to mint their own invite -- forbidden.
    let resp = create_invite(&client, &app.base_url, &member.cookie, "role-co", "member", None).await;
    assert_eq!(resp.status(), 403, "a member must not be able to create invites");

    // Listing/revoking are equally admin-gated.
    let list = client
        .get(format!("{}/web/orgs/role-co/invites", app.base_url))
        .header(reqwest::header::COOKIE, &member.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(list.status(), 403);

    app.teardown().await;
}

/// An admin can't mint an invite for a role above their own.
#[tokio::test]
async fn admin_cannot_invite_someone_as_owner() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();
    let owner = web_login(&client, &app.base_url, "cap-owner@example.com").await;
    create_team_org(&client, &app.base_url, &owner.cookie, "Cap Co", "cap-co").await;

    let admin_invite = create_invite(&client, &app.base_url, &owner.cookie, "cap-co", "admin", None).await;
    let body: serde_json::Value = admin_invite.json().await.unwrap();
    let join_url = join_path(body["url"].as_str().unwrap());
    let admin = web_login(&client, &app.base_url, "cap-admin@example.com").await;
    let join = client
        .post(format!("{}{join_url}", app.base_url))
        .header(reqwest::header::COOKIE, &admin.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(join.status(), 200);

    let resp = create_invite(&client, &app.base_url, &admin.cookie, "cap-co", "owner", None).await;
    assert_eq!(resp.status(), 403, "an admin must not be able to mint an owner invite");

    app.teardown().await;
}

// ---------------------------------------------------------------------------
// /web/q/sessions role scoping + audit_log.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn member_sessions_query_is_scoped_to_self_in_a_team_org_admin_sees_all_and_is_audited() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();
    let owner = web_login(&client, &app.base_url, "sessions-owner@example.com").await;
    create_team_org(&client, &app.base_url, &owner.cookie, "Sessions Co", "sessions-co").await;

    let invite = create_invite(&client, &app.base_url, &owner.cookie, "sessions-co", "member", None).await;
    let invite_body: serde_json::Value = invite.json().await.unwrap();
    let join_url = join_path(invite_body["url"].as_str().unwrap());
    let member = web_login(&client, &app.base_url, "sessions-member@example.com").await;
    client
        .post(format!("{}{join_url}", app.base_url))
        .header(reqwest::header::COOKIE, &member.cookie)
        .send()
        .await
        .unwrap();

    // Bind a device for each of owner and member into sessions-co, and
    // ingest one event from each.
    let owner_device = login_as(&client, &app.base_url, "host-sessions-owner", "sessions-owner@example.com").await;
    // login_as always lands in the account's *personal* org -- pull the
    // team org's id/slug explicitly via activation instead for the member
    // and owner so their device events land in sessions-co.
    let owner_dev_team = support::activate_device_into_org(
        &client,
        &app.base_url,
        "host-owner-team",
        &owner.cookie,
        "sessions-co",
    )
    .await;
    let member_dev_team = support::activate_device_into_org(
        &client,
        &app.base_url,
        "host-member-team",
        &member.cookie,
        "sessions-co",
    )
    .await;
    let _ = owner_device;

    let owner_ev = recent_tool_call_event("sessions-owner-ev", "host-owner-team", "sess-owner-team");
    let payload = support::gzip(&support::ingest_body_bytes(&[owner_ev]));
    let resp = client
        .post(format!("{}/v1/events", app.base_url))
        .bearer_auth(&owner_dev_team.token)
        .header("Content-Encoding", "gzip")
        .body(payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let member_ev = recent_tool_call_event("sessions-member-ev", "host-member-team", "sess-member-team");
    let payload = support::gzip(&support::ingest_body_bytes(&[member_ev]));
    let resp = client
        .post(format!("{}/v1/events", app.base_url))
        .bearer_auth(&member_dev_team.token)
        .header("Content-Encoding", "gzip")
        .body(payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Switch both sessions' active org to sessions-co.
    for cookie in [&owner.cookie, &member.cookie] {
        let resp = client
            .post(format!("{}/web/active-org", app.base_url))
            .header(reqwest::header::COOKIE, cookie)
            .json(&serde_json::json!({ "slug": "sessions-co" }))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }

    // The member sees only their own session.
    let member_sessions: serde_json::Value = client
        .get(format!("{}/web/q/sessions?days=14", app.base_url))
        .header(reqwest::header::COOKIE, &member.cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let rows = member_sessions["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "member must see only their own session: {member_sessions:?}");
    assert_eq!(rows[0][0], "sess-member-team");

    // Audit log is empty so far (only the admin/owner drilldown gets logged).
    let audit_before: (i64,) = sqlx::query_as("SELECT count(*) FROM audit_log")
        .fetch_one(&app.state.pools.superuser)
        .await
        .unwrap();
    assert_eq!(audit_before.0, 0);

    // The owner sees both sessions, and this drilldown is audited.
    let owner_sessions: serde_json::Value = client
        .get(format!("{}/web/q/sessions?days=14", app.base_url))
        .header(reqwest::header::COOKIE, &owner.cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let rows = owner_sessions["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "owner must see every session: {owner_sessions:?}");

    let owner_account_id: (Uuid,) = sqlx::query_as("SELECT id FROM accounts WHERE email = $1")
        .bind("sessions-owner@example.com")
        .fetch_one(&app.state.pools.superuser)
        .await
        .unwrap();

    let audit_rows: Vec<(Uuid, String, Option<String>)> =
        sqlx::query_as("SELECT actor, action, target FROM audit_log")
            .fetch_all(&app.state.pools.superuser)
            .await
            .unwrap();
    assert_eq!(audit_rows.len(), 1, "the owner's drilldown must write exactly one audit_log row");
    assert_eq!(audit_rows[0].0, owner_account_id.0, "the audit row's actor is the owner who drilled down");
    assert_eq!(audit_rows[0].1, "sessions_drilldown");
    assert_eq!(audit_rows[0].2, None);

    app.teardown().await;
}

// ---------------------------------------------------------------------------
// GET /web/devices / POST /web/devices/:id/revoke
// ---------------------------------------------------------------------------

#[tokio::test]
async fn devices_admin_sees_the_whole_org_member_sees_only_their_own() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::new();
    let owner = web_login(&client, &app.base_url, "devices-owner@example.com").await;
    create_team_org(&client, &app.base_url, &owner.cookie, "Devices Co", "devices-co").await;

    let invite = create_invite(&client, &app.base_url, &owner.cookie, "devices-co", "member", None).await;
    let invite_body: serde_json::Value = invite.json().await.unwrap();
    let join_url = join_path(invite_body["url"].as_str().unwrap());
    let member = web_login(&client, &app.base_url, "devices-member@example.com").await;
    client
        .post(format!("{}{join_url}", app.base_url))
        .header(reqwest::header::COOKIE, &member.cookie)
        .send()
        .await
        .unwrap();

    let owner_dev = support::activate_device_into_org(&client, &app.base_url, "host-d-owner", &owner.cookie, "devices-co").await;
    let member_dev =
        support::activate_device_into_org(&client, &app.base_url, "host-d-member", &member.cookie, "devices-co").await;

    for cookie in [&owner.cookie, &member.cookie] {
        client
            .post(format!("{}/web/active-org", app.base_url))
            .header(reqwest::header::COOKIE, cookie)
            .json(&serde_json::json!({ "slug": "devices-co" }))
            .send()
            .await
            .unwrap();
    }

    let member_view: serde_json::Value = client
        .get(format!("{}/web/devices", app.base_url))
        .header(reqwest::header::COOKIE, &member.cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let member_devices = member_view["devices"].as_array().unwrap();
    assert_eq!(member_devices.len(), 1, "member sees only their own device: {member_view:?}");
    assert_eq!(member_devices[0]["host_id"], "host-d-member");

    let owner_view: serde_json::Value = client
        .get(format!("{}/web/devices", app.base_url))
        .header(reqwest::header::COOKIE, &owner.cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let owner_devices = owner_view["devices"].as_array().unwrap();
    assert_eq!(owner_devices.len(), 2, "admin/owner sees every device in the org: {owner_view:?}");

    // Member can't revoke the owner's device...
    let owner_device_id = owner_devices
        .iter()
        .find(|d| d["host_id"] == "host-d-owner")
        .unwrap()["id"]
        .as_str()
        .unwrap();
    let denied = client
        .post(format!("{}/web/devices/{owner_device_id}/revoke", app.base_url))
        .header(reqwest::header::COOKIE, &member.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 404, "member cannot revoke someone else's device");

    // ...but the owner can.
    let revoke = client
        .post(format!("{}/web/devices/{owner_device_id}/revoke", app.base_url))
        .header(reqwest::header::COOKIE, &owner.cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(revoke.status(), 200);

    let check = client
        .get(format!("{}/v1/query/today", app.base_url))
        .bearer_auth(&owner_dev.token)
        .send()
        .await
        .unwrap();
    assert_eq!(check.status(), 401, "revoked device token must stop authenticating");
    let _ = member_dev;

    app.teardown().await;
}
