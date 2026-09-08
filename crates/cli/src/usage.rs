//! Subscription snapshots are local metadata, separate from billable token events.
//! An explicit account label is mandatory: neither a host nor a session identifies
//! a subscription. Never infer historical ownership from today's credentials.
use anyhow::{bail, Context, Result};
use axum::{
    extract::State,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Debug, clap::Subcommand)]
pub enum Action {
    /// Record Claude status-line JSON from stdin. Emits a compact status line.
    Claude {
        #[arg(long)]
        account: String,
    },
    /// Fetch current limits using an already logged-in Codex profile.
    Codex {
        #[arg(long)]
        account: String,
        /// Explicit profile directory; do not reuse a label for another subscription.
        #[arg(long)]
        profile: PathBuf,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Window {
    name: String,
    used_percent: f64,
    window_minutes: Option<i64>,
    resets_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    agent: String,
    account: String,
    observed_at: i64,
    windows: Vec<Window>,
}

fn directory(data_dir: &Path) -> PathBuf {
    data_dir.join("subscription-usage")
}

fn validate_account(account: &str) -> Result<()> {
    if account.trim().is_empty() || account.len() > 120 || account.chars().any(char::is_control) {
        bail!("account must be a non-empty label of at most 120 bytes without control characters");
    }
    Ok(())
}

fn window(name: String, raw: &Value, claude: bool, minutes: Option<i64>) -> Option<Window> {
    let used = raw
        .get(if claude {
            "used_percentage"
        } else {
            "usedPercent"
        })?
        .as_f64()?;
    if !used.is_finite() || !(0.0..=100.0).contains(&used) {
        return None;
    }
    Some(Window {
        name,
        used_percent: used,
        window_minutes: minutes.or_else(|| {
            raw.get("windowDurationMins")
                .and_then(Value::as_i64)
                .filter(|n| *n > 0)
        }),
        resets_at: raw
            .get(if claude { "resets_at" } else { "resetsAt" })
            .and_then(Value::as_i64)
            .filter(|n| *n > 0 && *n < 8_640_000_000_000),
    })
}

fn parse(agent: &str, account: &str, raw: &Value, observed_at: i64) -> Result<Snapshot> {
    validate_account(account)?;
    let mut windows = vec![];
    if agent == "claude" {
        for (key, minutes) in [("five_hour", 300), ("seven_day", 10080)] {
            if let Some(w) = window(key.into(), &raw["rate_limits"][key], true, Some(minutes)) {
                windows.push(w);
            }
        }
    } else {
        let buckets: Vec<(&str, &Value)> = match raw
            .get("rateLimitsByLimitId")
            .and_then(Value::as_object)
            .filter(|m| !m.is_empty())
        {
            Some(map) => map.iter().map(|(k, v)| (k.as_str(), v)).collect(),
            None => vec![("codex", &raw["rateLimits"])],
        };
        for (id, bucket) in buckets {
            for key in ["primary", "secondary"] {
                if let Some(w) = window(format!("{id} / {key}"), &bucket[key], false, None) {
                    windows.push(w);
                }
            }
        }
    }
    Ok(Snapshot {
        agent: agent.into(),
        account: account.into(),
        observed_at,
        windows,
    })
}

fn save(data_dir: &Path, snapshot: &Snapshot) -> Result<()> {
    let dir = directory(data_dir);
    std::fs::create_dir_all(&dir)?;
    // Serialize concurrent status lines for this account, then atomically replace
    // its snapshot. Keep storage bounded even for long-running sessions.
    use std::os::fd::AsRawFd;
    let key = kikimimi_schema::event_id("usage", &snapshot.agent, "account", &snapshot.account);
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(format!("{key}.lock")))?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let path = dir.join(format!("{key}.json"));
    if let Ok(bytes) = std::fs::read(&path) {
        let old: Snapshot = serde_json::from_slice(&bytes)?;
        if old.observed_at > snapshot.observed_at {
            return Ok(());
        }
    }
    let id = uuid::Uuid::new_v4();
    let tmp = dir.join(format!("{id}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec(snapshot)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn latest(data_dir: &Path) -> Result<Vec<Snapshot>> {
    let dir = directory(data_dir);
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e.into()),
    };
    let mut snapshots: BTreeMap<(String, String), Snapshot> = BTreeMap::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let snapshot: Snapshot = serde_json::from_slice(&std::fs::read(path)?)?;
        let key = (snapshot.agent.clone(), snapshot.account.clone());
        if snapshots
            .get(&key)
            .is_none_or(|old| snapshot.observed_at > old.observed_at)
        {
            snapshots.insert(key, snapshot);
        }
    }
    Ok(snapshots.into_values().collect())
}

pub async fn get_usage(State(state): State<crate::web::WebAppState>) -> Response {
    match tokio::task::spawn_blocking(move || latest(&state.data_dir)).await {
        Ok(Ok(snapshots)) => Json(snapshots).into_response(),
        _ => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":"Unable to read subscription usage"})),
        )
            .into_response(),
    }
}

pub fn run(action: Option<Action>) -> Result<()> {
    let data_dir = kikimimi_schema::paths::data_dir();
    let snapshot = match action {
        None => {
            println!("{}", serde_json::to_string_pretty(&latest(&data_dir)?)?);
            return Ok(());
        }
        Some(Action::Claude { account }) => {
            validate_account(&account)?;
            let mut input = String::new();
            std::io::stdin()
                .take(1_048_577)
                .read_to_string(&mut input)?;
            if input.len() > 1_048_576 {
                bail!("status-line input too large");
            }
            parse(
                "claude",
                &account,
                &serde_json::from_str::<Value>(&input)
                    .context("expected Claude status-line JSON")?,
                chrono::Utc::now().timestamp_millis(),
            )?
        }
        Some(Action::Codex { account, profile }) => {
            validate_account(&account)?;
            if !profile.is_absolute() || !profile.is_dir() {
                bail!("profile must be an existing absolute directory");
            }
            let runtime = tokio::runtime::Runtime::new()?;
            let raw = runtime.block_on(fetch_codex(&profile))?;
            parse(
                "codex",
                &account,
                &raw,
                chrono::Utc::now().timestamp_millis(),
            )?
        }
    };
    save(&data_dir, &snapshot)?;
    if snapshot.windows.is_empty() {
        println!("{}: usage unavailable", snapshot.account);
    } else {
        println!(
            "{}: {}",
            snapshot.account,
            snapshot
                .windows
                .iter()
                .map(|w| format!("{} {:.0}%", w.name, w.used_percent))
                .collect::<Vec<_>>()
                .join(" · ")
        );
    }
    Ok(())
}

async fn fetch_codex(profile: &Path) -> Result<Value> {
    fetch_codex_with(profile, Path::new("codex")).await
}

async fn fetch_codex_with(profile: &Path, executable: &Path) -> Result<Value> {
    use std::process::Stdio;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let mut child = tokio::process::Command::new(executable)
        .args(["app-server", "--listen", "stdio://"])
        .env("CODEX_HOME", profile)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("could not start codex app-server")?;
    let result = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let mut input = child.stdin.take().context("missing app-server stdin")?;
        let mut output = BufReader::new(child.stdout.take().context("missing app-server stdout")?).lines();
        input.write_all(b"{\"id\":1,\"method\":\"initialize\",\"params\":{\"clientInfo\":{\"name\":\"kikimimi\",\"version\":\"0.1.0\"}}}\n").await?;
        while let Some(line) = output.next_line().await? {
            let reply: Value = serde_json::from_str(&line)?;
            if reply["id"] == 1 {
                if reply.get("error").is_some() { bail!("Codex initialization failed"); }
                input.write_all(b"{\"method\":\"initialized\"}\n{\"id\":2,\"method\":\"account/rateLimits/read\"}\n").await?;
            } else if reply["id"] == 2 {
                if reply.get("error").is_some() { bail!("Codex limits unavailable; check this profile's login with Codex"); }
                return reply.get("result").cloned().context("missing rate-limit result");
            }
        }
        bail!("Codex app-server closed before returning limits")
    }).await.context("Codex usage request timed out")?;
    let _ = child.kill().await;
    let _ = child.wait().await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn usage_endpoint_requires_auth_and_returns_separate_accounts() {
        use axum::{
            body::{to_bytes, Body},
            http::Request,
        };
        use tower::ServiceExt;
        let dir = tempfile::tempdir().unwrap();
        for account in ["personal", "work"] {
            save(
                dir.path(),
                &parse("claude", account, &json!({}), 1).unwrap(),
            )
            .unwrap();
        }
        let app = crate::web::router(crate::web::WebAppState {
            token: "test-token".into(),
            data_dir: dir.path().into(),
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/web/usage")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 401);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/web/usage")
                    .header("cookie", "kikimimi_local=test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let rows: Vec<Snapshot> =
            serde_json::from_slice(&to_bytes(response.into_body(), 100_000).await.unwrap())
                .unwrap();
        assert_eq!(rows.len(), 2);
        assert_ne!(rows[0].account, rows[1].account);
    }

    #[tokio::test]
    async fn codex_handshake_reads_limits_from_explicit_profile() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("codex-fixture");
        std::fs::write(&executable, r##"#!/bin/sh
read -r init
case "$init" in *'"method":"initialize"'*) ;; *) exit 1 ;; esac
printf '%s\n' '{"id":1,"result":{}}'
read -r ready
read -r request
case "$request" in *'account/rateLimits/read'*) ;; *) exit 1 ;; esac
test -d "$CODEX_HOME" || exit 1
printf '%s\n' '{"id":2,"result":{"rateLimits":{"primary":{"usedPercent":42,"windowDurationMins":300}}}}'
"##).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let raw = fetch_codex_with(dir.path(), &executable).await.unwrap();
        let snapshot = parse("codex", "work", &raw, 1).unwrap();
        assert_eq!(snapshot.windows[0].used_percent, 42.0);
    }

    #[test]
    fn concurrent_updates_are_bounded_and_do_not_regress() {
        let dir = tempfile::tempdir().unwrap();
        std::thread::scope(|scope| {
            for ts in 1..=20 {
                let path = dir.path();
                scope.spawn(move || {
                    save(path, &parse("claude", "work", &json!({}), ts).unwrap()).unwrap()
                });
            }
        });
        let rows = latest(dir.path()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].observed_at, 20);
        assert_eq!(std::fs::read_dir(directory(dir.path())).unwrap().count(), 2);
    }

    #[test]
    fn isolates_accounts_and_agents_and_keeps_latest_not_sum() {
        let dir = tempfile::tempdir().unwrap();
        for (agent, account, ts) in [
            ("claude", "work", 2),
            ("claude", "work", 1),
            ("claude", "personal", 3),
            ("codex", "work", 4),
        ] {
            save(dir.path(), &parse(agent, account, &json!({}), ts).unwrap()).unwrap();
        }
        let rows = latest(dir.path()).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows.iter()
                .find(|s| s.agent == "claude" && s.account == "work")
                .unwrap()
                .observed_at,
            2
        );
    }
    #[test]
    fn parses_native_shapes_without_inventing_missing_values() {
        let claude = parse(
            "claude",
            "work",
            &json!({"rate_limits":{"five_hour":{"used_percentage":42,"resets_at":1800000000}}}),
            1,
        )
        .unwrap();
        assert_eq!(claude.windows.len(), 1);
        assert_eq!(claude.windows[0].used_percent, 42.0);
        assert_eq!(claude.windows[0].window_minutes, Some(300));
        let codex = parse("codex", "work", &json!({"rateLimitsByLimitId":{"codex":{"primary":{"usedPercent":25,"windowDurationMins":10080}},"other":{"secondary":{"usedPercent":70}}},"rateLimits":{"primary":{"usedPercent":25}}}), 1).unwrap();
        assert_eq!(codex.windows.len(), 2);
        assert_eq!(codex.windows[0].resets_at, None);
        assert!(parse(
            "claude",
            "work",
            &json!({"rate_limits":{"five_hour":{"used_percentage":101}}}),
            1
        )
        .unwrap()
        .windows
        .is_empty());
        assert!(parse("claude", "\n", &json!({}), 1).is_err());
        assert!(latest(tempfile::tempdir().unwrap().path())
            .unwrap()
            .is_empty());
    }
}
