//! `kikimimi devices` / `kikimimi devices revoke <id>` — architecture.md §6.1:
//! "'kikimimi devices' lists own devices + 'kikimimi devices revoke <id>'".
//!
//! CONTRACT NOTE: the account-model contract only specifies *session-cookie* endpoints for
//! this (`GET /web/devices`, `POST /web/devices/:id/revoke` — admin of the active org
//! additionally sees/revokes the whole org's devices). A CLI device authenticates with its
//! own bearer device-token, not a session cookie, so per the task's explicit instruction this
//! CLI instead calls new, additive `/v1/*` endpoints (Bearer, same `AuthContext` extractor as
//! every other `/v1/*` route — implemented cloud-side in `device.rs`'s `list_devices_v1` /
//! `revoke_device_v1`, registered in `lib.rs`):
//!
//!   `GET /v1/devices` -> `{"devices": [{"id", "host_id", "hostname", "org_slug", "org_kind",
//!   "created_at", "last_seen_at", "revoked", "current"}, ...]}` — every device belonging to
//!   `AuthContext::account_id` (across all the account's orgs, not just the calling device's
//!   own org). `current` is `true` for exactly the row `AuthContext::device_id` resolved to
//!   (lets the CLI mark "(this device)" without needing to know its own device id locally).
//!
//!   `POST /v1/devices/:id/revoke` -> `{"status": "revoked"}` (200) if `:id` belongs to
//!   `AuthContext::account_id`, else 404 (never reveal whether a *different* account's device
//!   id exists). Same effect as the existing `POST /v1/device/revoke` (self-revoke of the
//!   calling token) but targets an arbitrary owned device by id instead of only "whichever
//!   token authenticated this request" -- needed so `kikimimi devices revoke <id>` can kill a
//!   *different*, e.g. lost/stolen, machine's token from this one.
//!
//! Admin-wide "all org devices" listing/revocation (the web-only half of the contract's
//! `GET /web/devices`) is intentionally left out of this Bearer-token surface: a device token
//! carries no role, and mixing "my own devices" with "every device in my org, if I happen to
//! be admin" into one flat list would need a role lookup this endpoint has no other reason to
//! do. That stays a web (`/web/devices`) capability, same split as `/web/orgs/:slug/invites`
//! (admin) vs. `/v1/orgs` (own memberships only) in `orgs_cmd.rs`.

use anyhow::Context;
use serde::Deserialize;
use std::time::Duration;

#[derive(Debug, Deserialize)]
struct DevicesResponse {
    devices: Vec<DeviceRow>,
}

#[derive(Debug, Deserialize)]
struct DeviceRow {
    id: String,
    host_id: String,
    #[serde(default)]
    hostname: Option<String>,
    org_slug: String,
    org_kind: String,
    #[serde(default)]
    last_seen_at: Option<String>,
    revoked: bool,
    #[serde(default)]
    current: bool,
}

#[derive(Debug, Deserialize)]
struct RevokeResponse {
    #[allow(dead_code)]
    status: String,
}

/// `kikimimi devices`.
pub fn list() -> anyhow::Result<()> {
    let cfg = crate::config::KikimimiConfig::load();
    let cloud = cfg
        .cloud
        .ok_or_else(|| anyhow::anyhow!("not logged in; run `kikimimi login` first"))?;

    let resp = fetch_devices(&cloud.endpoint, &cloud.token)?;
    print_devices(&resp.devices);
    Ok(())
}

/// `kikimimi devices revoke <id>`.
pub fn revoke(id: &str) -> anyhow::Result<()> {
    let cfg = crate::config::KikimimiConfig::load();
    let cloud = cfg
        .cloud
        .ok_or_else(|| anyhow::anyhow!("not logged in; run `kikimimi login` first"))?;

    revoke_device(&cloud.endpoint, &cloud.token, id)?;
    println!("device {id} revoked");
    Ok(())
}

fn fetch_devices(endpoint: &str, token: &str) -> anyhow::Result<DevicesResponse> {
    let endpoint = endpoint.trim_end_matches('/');
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .context("building HTTP client")?;
    let resp = client
        .get(format!("{endpoint}/v1/devices"))
        .bearer_auth(token)
        .send()
        .context("GET /v1/devices")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        anyhow::bail!("GET /v1/devices returned {status}: {body}");
    }
    resp.json().context("parsing /v1/devices response")
}

fn revoke_device(endpoint: &str, token: &str, id: &str) -> anyhow::Result<RevokeResponse> {
    let endpoint = endpoint.trim_end_matches('/');
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .context("building HTTP client")?;
    let resp = client
        .post(format!("{endpoint}/v1/devices/{id}/revoke"))
        .bearer_auth(token)
        .send()
        .with_context(|| format!("POST /v1/devices/{id}/revoke"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        anyhow::bail!("POST /v1/devices/{id}/revoke returned {status}: {body}");
    }
    resp.json()
        .with_context(|| format!("parsing /v1/devices/{id}/revoke response"))
}

fn print_devices(devices: &[DeviceRow]) {
    if devices.is_empty() {
        println!("no devices");
        return;
    }
    println!("devices:");
    for d in devices {
        let marker = if d.current { " (this device)" } else { "" };
        let status = if d.revoked { "revoked" } else { "active" };
        println!(
            "  {:<36} host={:<20} org={}[{}] last_seen={} {}{}",
            d.id,
            d.hostname.as_deref().unwrap_or(&d.host_id),
            d.org_slug,
            d.org_kind,
            d.last_seen_at.as_deref().unwrap_or("-"),
            status,
            marker
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CloudConfig, KikimimiConfig};
    use httpmock::prelude::*;
    use serde_json::json;
    use serial_test::serial;

    fn login_with(server: &MockServer) {
        let mut cfg = KikimimiConfig::load();
        cfg.cloud = Some(CloudConfig {
            endpoint: server.base_url(),
            token: "tok-devices".into(),
            email: "dev@example.com".into(),
            org_id: "org-1".into(),
            org_slug: "acme".into(),
            org_kind: "team".into(),
            ..Default::default()
        });
        cfg.save().unwrap();
    }

    #[test]
    #[serial]
    fn list_errors_when_not_logged_in() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());
        assert!(list().is_err());
        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    #[serial]
    fn revoke_errors_when_not_logged_in() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());
        assert!(revoke("dev-1").is_err());
        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    fn fetch_devices_sends_bearer_and_parses_response() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/v1/devices")
                .header("authorization", "Bearer tok-devices");
            then.status(200).json_body(json!({
                "devices": [
                    {
                        "id": "dev-1",
                        "host_id": "host-abc",
                        "hostname": "laptop",
                        "org_slug": "acme",
                        "org_kind": "team",
                        "last_seen_at": "2026-08-30T00:00:00Z",
                        "revoked": false,
                        "current": true
                    }
                ]
            }));
        });

        let resp = fetch_devices(&server.base_url(), "tok-devices").unwrap();
        mock.assert_calls(1);
        assert_eq!(resp.devices.len(), 1);
        assert_eq!(resp.devices[0].id, "dev-1");
        assert!(resp.devices[0].current);
        assert!(!resp.devices[0].revoked);
    }

    #[test]
    #[serial]
    fn list_end_to_end_against_a_mock_server() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());

        let server = MockServer::start();
        login_with(&server);
        server.mock(|when, then| {
            when.method(GET).path("/v1/devices");
            then.status(200).json_body(json!({"devices": []}));
        });

        assert!(list().is_ok());

        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    #[serial]
    fn revoke_sends_bearer_and_posts_to_the_right_id() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());

        let server = MockServer::start();
        login_with(&server);
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v1/devices/dev-42/revoke")
                .header("authorization", "Bearer tok-devices");
            then.status(200).json_body(json!({"status": "revoked"}));
        });

        assert!(revoke("dev-42").is_ok());
        mock.assert_calls(1);

        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    fn revoke_device_errors_on_non_success_status() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v1/devices/dev-9/revoke");
            then.status(404).body("not found");
        });

        let result = revoke_device(&server.base_url(), "tok", "dev-9");
        assert!(result.is_err());
    }
}
