//! `kikimimi login` / `kikimimi logout` — device-code auth against kikimimi cloud
//! (architecture.md §6 「デーモン → cloud」, §8, cloud API 契約
//! `POST /v1/device/code` / `POST /v1/device/token`).
//!
//! `kikimimi login` never opens a browser itself (Stage 0: terminal-only UX) — it prints the
//! verification URL and user code and polls until the user approves on the web page (or
//! `KIKIMIMI_DEV_AUTOAPPROVE=1` on the server approves instantly, for tests/CI).

use std::time::Duration;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::config::{CloudConfig, KikimimiConfig};

/// kikimimi cloud's hosted instance -- what `kikimimi login` resolves to when nothing more
/// specific says otherwise. See [`resolve_endpoint`] for the full precedence.
pub const DEFAULT_ENDPOINT: &str = "https://kikimimi.dev";

/// Env var override for [`DEFAULT_ENDPOINT`], for pointing `kikimimi login` at a local
/// `kikimimi cloud` instance during development without typing `--endpoint` on every
/// invocation. Outranked by `--endpoint` itself and by a previously-saved login -- see
/// [`resolve_endpoint`].
pub const ENDPOINT_ENV_VAR: &str = "KIKIMIMI_ENDPOINT";

/// `interval_secs` から来る待ち時間で最大この回数だけポーリングする
/// (`status: "pending"` が続く限り)。
const MAX_POLL_ATTEMPTS: u32 = 30;

/// Resolves the cloud endpoint `kikimimi login` should use, in precedence order:
///
/// 1. `--endpoint <URL>` (`flag`) -- explicit, always wins.
/// 2. [`ENDPOINT_ENV_VAR`] (`env`) -- dev override, e.g. pointing at a local `kikimimi cloud`
///    instance without typing `--endpoint` every time.
/// 3. The endpoint saved by a previous `kikimimi login` (`saved`) -- re-running
///    `kikimimi login` (e.g. to refresh an expired token, or after `kikimimi logout`'s
///    server-side revoke) keeps talking to the same cloud instead of silently switching to
///    the hosted default just because that one detail wasn't repeated.
/// 4. [`DEFAULT_ENDPOINT`] -- kikimimi cloud's hosted instance.
///
/// A pure function of its three inputs (mirrors `config::resolve_otlp_port`'s shape) so the
/// precedence itself is testable without env vars or a real config.json. An empty string at
/// any tier (as opposed to genuinely absent) is treated as "not provided" and falls through,
/// same as `config.rs`'s port-resolution helpers treat an empty `XDG_STATE_HOME`.
fn resolve_endpoint(flag: Option<&str>, env: Option<&str>, saved: Option<&str>) -> String {
    [flag, env, saved]
        .into_iter()
        .find_map(|candidate| candidate.filter(|v| !v.is_empty()))
        .unwrap_or(DEFAULT_ENDPOINT)
        .to_string()
}

#[derive(Debug, Serialize)]
struct DeviceCodeRequest<'a> {
    host_id: &'a str,
    hostname: &'a str,
}

#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_url: String,
    interval_secs: u64,
}

#[derive(Debug, Serialize)]
struct DeviceTokenRequest<'a> {
    device_code: &'a str,
}

#[derive(Debug, Deserialize)]
struct DeviceTokenResponse {
    status: String,
    token: Option<String>,
    org_id: Option<String>,
    #[allow(dead_code)] // part of the contract's response shape; not persisted locally
    user_id: Option<String>,
    email: Option<String>,
}

/// `kikimimi login [--endpoint URL] [--no-browser]`。
///
/// `no_browser` は Stage 0 では常に true 相当 (このコマンドはブラウザを一切開かない) —
/// フラグ自体は将来の自動オープン実装に備えて受け取るだけ。
pub fn login(endpoint: Option<String>, _no_browser: bool) -> anyhow::Result<()> {
    let mut cfg = KikimimiConfig::load();
    let env_endpoint = std::env::var(ENDPOINT_ENV_VAR).ok();
    let saved_endpoint = cfg.cloud.as_ref().map(|c| c.endpoint.clone());
    let endpoint = resolve_endpoint(
        endpoint.as_deref(),
        env_endpoint.as_deref(),
        saved_endpoint.as_deref(),
    );
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .context("building HTTP client")?;

    let cloud = device_login(&client, &endpoint)?;

    cfg.cloud = Some(cloud.clone());
    cfg.save().context("saving config.json")?;

    println!("logged in as {} (org {})", cloud.email, cloud.org_id);
    Ok(())
}

/// `kikimimi logout` — `~/.kikimimi/config.json` の `cloud` セクションを消す
/// (`otlp_port` 等それ以外の設定はそのまま) 前に、サーバー側でもトークンを
/// 失効させる (`POST /v1/device/revoke`, ベストエフォート)。
///
/// architecture.md §6 はこのトークンを "`kikimimi logout` / Web から失効可" と
/// 文書化しているが、以前はローカルの config.json を消すだけでサーバー側の
/// トークンは有効なまま残っていた (セキュリティレビュー: 漏洩/侵害されたト
/// ークンをユーザー自身が殺す手段が無いという指摘の一部)。サーバーへの失効
/// リクエストが失敗しても (オフライン、サーバー障害、既に失効済み) ローカル
/// の消去は必ず行う — 「ローカルからは忘れる」というユーザーの意図を、cloud
/// 側の到達性でブロックしてはいけない。
pub fn logout() -> anyhow::Result<()> {
    let mut cfg = KikimimiConfig::load();
    let Some(cloud) = cfg.cloud.clone() else {
        println!("not logged in");
        return Ok(());
    };

    if let Err(e) = revoke_on_server(&cloud) {
        eprintln!("warning: could not revoke token on kikimimi cloud (clearing local token anyway): {e:#}");
    }

    cfg.cloud = None;
    cfg.save().context("saving config.json")?;
    println!("logged out");
    Ok(())
}

fn revoke_on_server(cloud: &CloudConfig) -> anyhow::Result<()> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .context("building HTTP client")?;
    let endpoint = cloud.endpoint.trim_end_matches('/');
    client
        .post(format!("{endpoint}/v1/device/revoke"))
        .bearer_auth(&cloud.token)
        .send()
        .context("POST /v1/device/revoke")?
        .error_for_status()
        .context("POST /v1/device/revoke returned an error status")?;
    Ok(())
}

fn device_login(client: &reqwest::blocking::Client, endpoint: &str) -> anyhow::Result<CloudConfig> {
    let host_id = kikimimi_schema::paths::host_id().context("loading/creating host_id")?;
    let hostname = hostname();

    let code_resp: DeviceCodeResponse = client
        .post(format!("{endpoint}/v1/device/code"))
        .json(&DeviceCodeRequest {
            host_id: &host_id,
            hostname: &hostname,
        })
        .send()
        .context("POST /v1/device/code")?
        .error_for_status()
        .context("POST /v1/device/code returned an error status")?
        .json()
        .context("parsing /v1/device/code response")?;

    println!("To authorize this device, open:");
    println!("  {}", code_resp.verification_url);
    println!("and enter code: {}", code_resp.user_code);
    println!("waiting for approval...");

    let interval = Duration::from_secs(code_resp.interval_secs);

    for attempt in 0..MAX_POLL_ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(interval);
        }

        let resp = client
            .post(format!("{endpoint}/v1/device/token"))
            .json(&DeviceTokenRequest {
                device_code: &code_resp.device_code,
            })
            .send()
            .context("POST /v1/device/token")?;

        if resp.status().as_u16() == 410 {
            anyhow::bail!("device code expired; run `kikimimi login` again");
        }
        let body: DeviceTokenResponse = resp
            .error_for_status()
            .context("POST /v1/device/token returned an error status")?
            .json()
            .context("parsing /v1/device/token response")?;

        match body.status.as_str() {
            "pending" => continue,
            "ok" => {
                let token = body
                    .token
                    .ok_or_else(|| anyhow::anyhow!("device/token \"ok\" response missing token"))?;
                let org_id = body.org_id.ok_or_else(|| {
                    anyhow::anyhow!("device/token \"ok\" response missing org_id")
                })?;
                let email = body
                    .email
                    .ok_or_else(|| anyhow::anyhow!("device/token \"ok\" response missing email"))?;
                return Ok(CloudConfig {
                    endpoint: endpoint.to_string(),
                    token,
                    email,
                    org_id,
                });
            }
            other => anyhow::bail!("unexpected /v1/device/token status {other:?}"),
        }
    }

    anyhow::bail!(
        "timed out waiting for device authorization after {MAX_POLL_ATTEMPTS} attempts; run `kikimimi login` again"
    )
}

fn hostname() -> String {
    let mut buf = vec![0u8; 256];
    let ret = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if ret != 0 {
        return "unknown-host".to_string();
    }
    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..len]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;
    use serde_json::json;
    use serial_test::serial;

    // --- resolve_endpoint precedence (pure, no network/env/fs) -----------------

    #[test]
    fn resolve_endpoint_falls_back_to_the_hosted_default_when_nothing_else_is_set() {
        assert_eq!(resolve_endpoint(None, None, None), "https://kikimimi.dev");
    }

    #[test]
    fn resolve_endpoint_prefers_the_saved_endpoint_over_the_default() {
        assert_eq!(
            resolve_endpoint(None, None, Some("https://saved.example")),
            "https://saved.example"
        );
    }

    #[test]
    fn resolve_endpoint_prefers_env_over_the_saved_endpoint() {
        assert_eq!(
            resolve_endpoint(
                None,
                Some("https://env.example"),
                Some("https://saved.example")
            ),
            "https://env.example"
        );
    }

    #[test]
    fn resolve_endpoint_prefers_the_flag_over_everything_else() {
        assert_eq!(
            resolve_endpoint(
                Some("https://flag.example"),
                Some("https://env.example"),
                Some("https://saved.example")
            ),
            "https://flag.example"
        );
    }

    #[test]
    fn resolve_endpoint_treats_an_empty_flag_or_env_as_absent() {
        // A flag/env present-but-empty (as opposed to genuinely unset) must fall through to
        // the next tier rather than resolving to "" -- mirrors config.rs's port-resolution
        // helpers treating an empty XDG_STATE_HOME the same way.
        assert_eq!(
            resolve_endpoint(Some(""), Some(""), Some("https://saved.example")),
            "https://saved.example"
        );
        assert_eq!(
            resolve_endpoint(Some(""), Some(""), Some("")),
            DEFAULT_ENDPOINT
        );
    }

    // --- resolve_endpoint wired through the real `login` call site --------------

    #[test]
    #[serial]
    fn login_uses_the_endpoint_env_var_when_no_flag_and_no_saved_login() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());
        std::env::remove_var(ENDPOINT_ENV_VAR);

        let server = MockServer::start();
        std::env::set_var(ENDPOINT_ENV_VAR, server.base_url());

        server.mock(|when, then| {
            when.method(POST).path("/v1/device/code");
            then.status(200).json_body(json!({
                "device_code": "dc-1",
                "user_code": "ABCD-1234",
                "verification_url": "https://example.com/activate",
                "interval_secs": 0
            }));
        });
        server.mock(|when, then| {
            when.method(POST).path("/v1/device/token");
            then.status(200).json_body(json!({
                "status": "ok",
                "token": "a".repeat(43),
                "org_id": "org-env",
                "user_id": "user-env",
                "email": "env@example.com"
            }));
        });

        // endpoint=None (no --endpoint flag): must resolve via KIKIMIMI_ENDPOINT, not the
        // hosted default, since there's no saved login yet either.
        login(None, true).unwrap();
        assert_eq!(
            KikimimiConfig::load().cloud.unwrap().endpoint,
            server.base_url()
        );

        std::env::remove_var(ENDPOINT_ENV_VAR);
        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    #[serial]
    fn login_reuses_the_previously_saved_endpoint_when_no_flag_or_env_is_given() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());
        std::env::remove_var(ENDPOINT_ENV_VAR);

        let server = MockServer::start();

        // Simulate a prior `kikimimi login` against this same server.
        KikimimiConfig {
            cloud: Some(CloudConfig {
                endpoint: server.base_url(),
                token: "stale-token".into(),
                email: "old@example.com".into(),
                org_id: "org-old".into(),
            }),
            ..Default::default()
        }
        .save()
        .unwrap();

        server.mock(|when, then| {
            when.method(POST).path("/v1/device/code");
            then.status(200).json_body(json!({
                "device_code": "dc-2",
                "user_code": "EFGH-5678",
                "verification_url": "https://example.com/activate",
                "interval_secs": 0
            }));
        });
        server.mock(|when, then| {
            when.method(POST).path("/v1/device/token");
            then.status(200).json_body(json!({
                "status": "ok",
                "token": "b".repeat(43),
                "org_id": "org-refreshed",
                "user_id": "user-refreshed",
                "email": "refreshed@example.com"
            }));
        });

        // Re-running `kikimimi login` with neither --endpoint nor KIKIMIMI_ENDPOINT must
        // keep talking to the same cloud the last login used, not fall back to
        // https://kikimimi.dev.
        login(None, true).unwrap();
        assert_eq!(
            KikimimiConfig::load().cloud.unwrap().endpoint,
            server.base_url()
        );

        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    #[serial]
    fn login_saves_cloud_config_on_immediate_ok() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());

        let server = MockServer::start();
        let code_mock = server.mock(|when, then| {
            when.method(POST).path("/v1/device/code");
            then.status(200).json_body(json!({
                "device_code": "dc-1",
                "user_code": "ABCD-1234",
                "verification_url": server_url(&server, "/activate?code=ABCD-1234"),
                "interval_secs": 1
            }));
        });
        let token_mock = server.mock(|when, then| {
            when.method(POST).path("/v1/device/token");
            then.status(200).json_body(json!({
                "status": "ok",
                "token": "a".repeat(43),
                "org_id": "org-1",
                "user_id": "user-1",
                "email": "dev@example.com"
            }));
        });

        login(Some(server.base_url()), true).unwrap();

        code_mock.assert_calls(1);
        token_mock.assert_calls(1);

        let cfg = KikimimiConfig::load();
        let cloud = cfg.cloud.expect("cloud config must be saved");
        assert_eq!(cloud.endpoint, server.base_url());
        assert_eq!(cloud.token, "a".repeat(43));
        assert_eq!(cloud.org_id, "org-1");
        assert_eq!(cloud.email, "dev@example.com");

        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    #[serial]
    fn login_polls_through_pending_before_ok() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());

        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v1/device/code");
            then.status(200).json_body(json!({
                "device_code": "dc-1",
                "user_code": "ABCD-1234",
                "verification_url": "https://example.com/activate?code=ABCD-1234",
                "interval_secs": 0
            }));
        });

        let call_count = std::sync::Arc::new(std::sync::Mutex::new(0u32));
        let call_count2 = call_count.clone();
        let token_mock = server.mock(move |when, then| {
            when.method(POST).path("/v1/device/token");
            then.respond_with(move |_req: &HttpMockRequest| {
                let mut n = call_count2.lock().unwrap();
                *n += 1;
                if *n < 3 {
                    HttpMockResponse::builder()
                        .status(200)
                        .header("content-type", "application/json")
                        .body(serde_json::to_vec(&json!({"status": "pending"})).unwrap())
                        .build()
                } else {
                    HttpMockResponse::builder()
                        .status(200)
                        .header("content-type", "application/json")
                        .body(
                            serde_json::to_vec(&json!({
                                "status": "ok",
                                "token": "b".repeat(43),
                                "org_id": "org-2",
                                "user_id": "user-2",
                                "email": "dev2@example.com"
                            }))
                            .unwrap(),
                        )
                        .build()
                }
            });
        });

        login(Some(server.base_url()), true).unwrap();
        token_mock.assert_calls(3);

        let cfg = KikimimiConfig::load();
        assert_eq!(cfg.cloud.unwrap().email, "dev2@example.com");

        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    #[serial]
    fn login_errors_on_expired_device_code() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());

        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v1/device/code");
            then.status(200).json_body(json!({
                "device_code": "dc-1",
                "user_code": "ABCD-1234",
                "verification_url": "https://example.com/activate?code=ABCD-1234",
                "interval_secs": 0
            }));
        });
        server.mock(|when, then| {
            when.method(POST).path("/v1/device/token");
            then.status(410);
        });

        let result = login(Some(server.base_url()), true);
        assert!(result.is_err());
        assert!(
            KikimimiConfig::load().cloud.is_none(),
            "must not save on expiry"
        );

        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    #[serial]
    fn login_times_out_after_max_poll_attempts() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());

        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v1/device/code");
            then.status(200).json_body(json!({
                "device_code": "dc-1",
                "user_code": "ABCD-1234",
                "verification_url": "https://example.com/activate?code=ABCD-1234",
                "interval_secs": 0
            }));
        });
        let token_mock = server.mock(|when, then| {
            when.method(POST).path("/v1/device/token");
            then.status(200).json_body(json!({"status": "pending"}));
        });

        let result = login(Some(server.base_url()), true);
        assert!(result.is_err(), "must give up eventually, not poll forever");
        token_mock.assert_calls(MAX_POLL_ATTEMPTS as usize);

        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    #[serial]
    fn logout_clears_cloud_but_preserves_otlp_port() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());

        let mut cfg = KikimimiConfig::load();
        cfg.otlp_port = Some(4318);
        cfg.cloud = Some(CloudConfig {
            endpoint: "http://127.0.0.1:8787".into(),
            token: "tok".into(),
            email: "dev@example.com".into(),
            org_id: "org-1".into(),
        });
        cfg.save().unwrap();

        logout().unwrap();

        let after = KikimimiConfig::load();
        assert_eq!(after.cloud, None);
        assert_eq!(after.otlp_port, Some(4318));

        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    #[serial]
    fn logout_revokes_token_on_server_before_clearing_local_config() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());

        let server = MockServer::start();
        let revoke_mock = server.mock(|when, then| {
            when.method(POST)
                .path("/v1/device/revoke")
                .header("authorization", "Bearer tok-logout");
            then.status(200).json_body(json!({"status": "revoked"}));
        });

        let mut cfg = KikimimiConfig::load();
        cfg.cloud = Some(CloudConfig {
            endpoint: server.base_url(),
            token: "tok-logout".into(),
            email: "dev@example.com".into(),
            org_id: "org-1".into(),
        });
        cfg.save().unwrap();

        logout().unwrap();

        revoke_mock.assert_calls(1);
        assert!(KikimimiConfig::load().cloud.is_none());

        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    #[serial]
    fn logout_clears_local_config_even_if_server_revoke_fails() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());

        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/v1/device/revoke");
            then.status(500);
        });

        let mut cfg = KikimimiConfig::load();
        cfg.cloud = Some(CloudConfig {
            endpoint: server.base_url(),
            token: "tok-logout-2".into(),
            email: "dev@example.com".into(),
            org_id: "org-1".into(),
        });
        cfg.save().unwrap();

        // Must still succeed and clear the local token — server-side revoke
        // is best-effort, never a reason to keep a token around locally that
        // the user asked to forget.
        logout().unwrap();
        assert!(KikimimiConfig::load().cloud.is_none());

        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    #[serial]
    fn logout_when_not_logged_in_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());
        assert!(logout().is_ok());
        std::env::remove_var("KIKIMIMI_DIR");
    }

    fn server_url(server: &MockServer, path: &str) -> String {
        server.url(path)
    }
}
