//! `kikimimi orgs` — architecture.md §6.1: "'kikimimi orgs' lists memberships (API)".
//!
//! CONTRACT NOTE: the account-model contract (§6.1/§8) only specifies a *session-cookie*
//! endpoint for this shape (`GET /web/me` -> `{email, github_login, orgs:[...], active_org}`).
//! A device's bearer token has no session cookie, so this CLI instead calls a new, additive
//! endpoint:
//!
//!   `GET /v1/orgs` (Bearer device-token auth, same `AuthContext` extractor as every other
//!   `/v1/*` route — implemented cloud-side as `orgs::list_orgs_v1`, registered in `lib.rs`)
//!   -> `{"orgs": [{"slug", "name", "kind", "role"}, ...]}`
//!
//! Scope: the account's *own* memberships (`AuthContext::account_id`), across every org --
//! not scoped to the org the calling device happens to be bound to. This mirrors `GET
//! /web/me`'s `orgs` list minus the web-session-only `active_org` field (a device's "active
//! org" is fixed at approval time into the token itself, per §6.1 "1 マシン = 1 アクティブ
//! org" -- there's no per-request switching to reflect here, unlike a browser session). This
//! CLI marks the row matching the locally saved `cloud.org_slug` (from `kikimimi login`) as
//! "(this device)" instead of relying on the server to repeat it back.

use anyhow::Context;
use serde::Deserialize;
use std::time::Duration;

#[derive(Debug, Deserialize)]
struct OrgsResponse {
    orgs: Vec<OrgRow>,
}

#[derive(Debug, Deserialize)]
struct OrgRow {
    slug: String,
    name: String,
    /// "personal" | "team".
    kind: String,
    /// "owner" | "admin" | "member" | "viewer".
    role: String,
}

pub fn run() -> anyhow::Result<()> {
    let cfg = crate::config::KikimimiConfig::load();
    let cloud = cfg
        .cloud
        .ok_or_else(|| anyhow::anyhow!("not logged in; run `kikimimi login` first"))?;

    let orgs = fetch_orgs(&cloud.endpoint, &cloud.token)?;
    print_orgs(&orgs.orgs, &cloud.org_slug);
    Ok(())
}

fn fetch_orgs(endpoint: &str, token: &str) -> anyhow::Result<OrgsResponse> {
    let endpoint = endpoint.trim_end_matches('/');
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .context("building HTTP client")?;
    let resp = client
        .get(format!("{endpoint}/v1/orgs"))
        .bearer_auth(token)
        .send()
        .context("GET /v1/orgs")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        anyhow::bail!("GET /v1/orgs returned {status}: {body}");
    }
    resp.json().context("parsing /v1/orgs response")
}

fn print_orgs(orgs: &[OrgRow], active_slug: &str) {
    if orgs.is_empty() {
        println!("no org memberships");
        return;
    }
    println!("orgs:");
    for org in orgs {
        let marker = if org.slug == active_slug {
            " (this device)"
        } else {
            ""
        };
        println!(
            "  {:<20} {:<24} kind={:<8} role={}{}",
            org.slug, org.name, org.kind, org.role, marker
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

    #[test]
    #[serial]
    fn run_errors_when_not_logged_in() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());
        assert!(run().is_err());
        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    fn fetch_orgs_sends_bearer_and_parses_response() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/v1/orgs")
                .header("authorization", "Bearer tok-orgs");
            then.status(200).json_body(json!({
                "orgs": [
                    {"slug": "me-personal", "name": "me (personal)", "kind": "personal", "role": "owner"},
                    {"slug": "acme", "name": "Acme Inc", "kind": "team", "role": "member"}
                ]
            }));
        });

        let resp = fetch_orgs(&server.base_url(), "tok-orgs").unwrap();
        mock.assert_calls(1);
        assert_eq!(resp.orgs.len(), 2);
        assert_eq!(resp.orgs[1].slug, "acme");
        assert_eq!(resp.orgs[1].kind, "team");
        assert_eq!(resp.orgs[1].role, "member");
    }

    #[test]
    fn fetch_orgs_errors_on_non_success_status() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/v1/orgs");
            then.status(401).body("invalid or unknown token");
        });

        let result = fetch_orgs(&server.base_url(), "bad-tok");
        assert!(result.is_err());
    }

    #[test]
    #[serial]
    fn run_end_to_end_against_a_mock_server() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());

        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/v1/orgs");
            then.status(200).json_body(json!({
                "orgs": [
                    {"slug": "acme", "name": "Acme Inc", "kind": "team", "role": "admin"}
                ]
            }));
        });

        let mut cfg = KikimimiConfig::load();
        cfg.cloud = Some(CloudConfig {
            endpoint: server.base_url(),
            token: "tok-orgs".into(),
            email: "dev@example.com".into(),
            org_id: "org-1".into(),
            org_slug: "acme".into(),
            org_kind: "team".into(),
            ..Default::default()
        });
        cfg.save().unwrap();

        assert!(run().is_ok());

        std::env::remove_var("KIKIMIMI_DIR");
    }
}
