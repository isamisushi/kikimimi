//! Narrow, versioned bridge used only by the bundled desktop shell.
use clap::Subcommand;

#[derive(Subcommand)]
pub(crate) enum Action {
    Status,
    CollectionInfo,
    Workspaces,
    SwitchCollection,
    Enable,
    Resume,
    ServeDashboard,
    ResetCollectionSettings,
    RestartAfterUpdate,
    OpenCodeRouterStatus,
    OpenCodeRouterEnable,
    OpenCodeRouterDisable,
    OpenCodeRouterConnect,
}

pub(crate) fn run(action: Action) -> anyhow::Result<()> {
    match action {
        Action::Workspaces => crate::collection_cmd::workspaces(),
        Action::CollectionInfo => crate::collection_cmd::info(),
        Action::SwitchCollection => crate::collection_cmd::switch(),
        Action::ServeDashboard => serve_dashboard(),
        Action::ResetCollectionSettings => {
            anyhow::ensure!(!kikimimi_spool::send_control(b'n') && !crate::service::status().installed,
                "Disconnect collection before resetting settings");
            let mut cfg = crate::collection_cmd::load()?;
            cfg.cloud = None;
            cfg.cloud_sync_paused = false;
            cfg.s3 = None;
            cfg.otlp_port = None;
            cfg.otlp_token = None;
            cfg.web_port = None;
            cfg.save()
        }
        Action::OpenCodeRouterStatus => crate::opencode_router::status(),
        Action::OpenCodeRouterEnable => crate::opencode_router::enable(),
        Action::OpenCodeRouterDisable => crate::opencode_router::disable(),
        Action::OpenCodeRouterConnect => crate::opencode_router::connect(),
        Action::Resume => {
            let service = crate::service::status();
            anyhow::ensure!(
                service.installed,
                "Collection is not set up. Complete setup first."
            );
            // Codex is optional: an unreadable config.toml must not block resuming.
            if let Err(e) = crate::codex_otel::ensure() {
                eprintln!("codex [otel]: {e:#}");
            }
            if kikimimi_spool::send_control(b'n') {
                // Let a running collector switch the rollout tailer to reconciling.
                kikimimi_spool::send_control(b'r');
                return Ok(());
            }
            if service.running == Some(true) {
                crate::service::restart()
            } else {
                crate::service::run_install()
            }
        }
        Action::RestartAfterUpdate => {
            let exe = std::env::current_exe()?.canonicalize()?;
            let service = crate::service::status();
            // An installed CLI service may share this data directory. Only restart
            // the service whose executable is this bundle's collector.
            if service.installed {
                if let Some(path) = service.unit_path {
                    let contents = std::fs::read(path)?;
                    if service_uses_executable(&contents, &exe)? {
                        let previous_pid = if kikimimi_spool::send_control(b'n') {
                            Some(crate::init_cmd::stop_manual_daemon()?.ok_or_else(|| {
                                anyhow::anyhow!("Cannot locate the running collector PID; refusing an incomplete restart")
                            })?)
                        } else {
                            None
                        };
                        crate::service::restart()?;
                        return wait_for_dashboard(previous_pid);
                    }
                }
            }
            Ok(())
        }
        Action::Status => {
            let running = kikimimi_spool::send_control(b'n');
            let state = crate::state::load_opt(&kikimimi_schema::paths::state_path());
            let service = crate::service::status();
            let url = state
                .as_ref()
                .filter(|s| running && s.web.port != 0 && s.web_error.is_none())
                .map(|s| format!("http://127.0.0.1:{}/?t={}", s.web.port, s.web.token));
            println!(
                "{}",
                serde_json::json!({
                    "running": running,
                    "health": crate::collection_health::snapshot(),
                    "collection_target": crate::collection_cmd::load_for_display().ok().map(|c| crate::collection_cmd::describe(&c)),
                    "applied_collection_target": state.as_ref().and_then(|s| s.collection_target.clone().or_else(|| {
                        let cfg = crate::collection_cmd::load_for_display().ok()?;
                        (cfg.cloud.is_none() && cfg.s3.is_none() && s.cloud.is_none() && s.s3.is_none()).then(|| crate::collection_cmd::describe(&cfg))
                    })),
                    "service_installed": service.installed,
                    "dashboard_url": url,
                    "duckdb_available": crate::web_query::duckdb_available(),
                    "codex_collection": crate::codex_otel::mode(),
                    "web_error": state.as_ref().and_then(|s| s.web_error.as_ref()),
                    "has_history": state.is_some(),
                    "has_s3_reader": crate::config::KikimimiConfig::load_without_secrets().s3_reader.is_some(),
                    "last_event_ts": state.as_ref().and_then(|s| s.last_event_ts),
                    "cloud_sync": state.as_ref().and_then(|s| s.cloud.clone()),
                })
            );
            Ok(())
        }
        Action::Enable => {
            let executable = std::env::current_exe()?.canonicalize()?;
            crate::init_cmd::init_with_executable(false, true, Some(&executable))?;
            if kikimimi_spool::send_control(b'n') && !crate::service::status().installed {
                anyhow::ensure!(
                    crate::init_cmd::stop_manual_daemon()?.is_some(),
                    "A manually started collector is running but its PID is unavailable. Stop it before enabling desktop collection."
                );
            }
            // Unlike normal init's best-effort service installation, report failure
            // to the desktop UI so it cannot claim setup succeeded on a broken service.
            crate::service::run_install()
        }
    }
}

// The control socket can accept connections while state.json still advertises
// the previous process's token. Wait for the authenticated endpoint used by the UI.
fn wait_for_dashboard(previous_pid: Option<u32>) -> anyhow::Result<()> {
    use std::time::{Duration, Instant};

    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(1))
        .build()?;
    let path = kikimimi_schema::paths::state_path();
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if let Some(state) = crate::state::load_opt(&path) {
            if Some(state.pid) != previous_pid
                && state.web.port != 0
                && !state.web.token.is_empty()
                && state.web_error.is_none()
            {
                let session = client
                    .get(format!("http://127.0.0.1:{}/web/me", state.web.port))
                    .header("Cookie", format!("kikimimi_local={}", state.web.token))
                    .send()
                    .and_then(|response| response.error_for_status())
                    .and_then(|response| response.json::<serde_json::Value>());
                if session.is_ok_and(|body| body["local"] == true) {
                    return Ok(());
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    anyhow::bail!("Updated collector's local dashboard did not become ready; restart will be retried on the next desktop launch")
}

// The desktop owns this process through a pipe. It serves saved data without
// starting hooks, session tailers, OTel, cloud sync, or the launchd collector.
fn serve_dashboard() -> anyhow::Result<()> {
    use std::io::Write;
    // With collection stopped, the viewer may own the ledger for offline reads.
    // While it runs, local requests go to the collector; S3 stays independent.
    let local_history = !kikimimi_spool::send_control(b'n');
    if local_history {
        kikimimi_sink::store::shared(&kikimimi_schema::paths::data_dir())?;
    }
    tokio::runtime::Runtime::new()?.block_on(async {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let token = crate::web::generate_local_token();
        let url = format!(
            "http://127.0.0.1:{}/?t={token}",
            listener.local_addr()?.port()
        );
        let state = crate::web::WebAppState {
            token,
            data_dir: kikimimi_schema::paths::data_dir(),
        };
        // This line goes only to the desktop's private stdout pipe, never logs.
        println!("{}", serde_json::json!({ "url": url, "local_history": local_history }));
        std::io::stdout().flush()?;
        let (closed, shutdown) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink());
            let _ = closed.send(());
        });
        // The collector binds its control socket before opening DuckDB. Release the
        // offline connection even when the desktop is idle and its pipe stays open.
        let collector_starting = async move {
            if !local_history {
                std::future::pending::<()>().await;
            }
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                if kikimimi_spool::send_control(b'n') {
                    break;
                }
            }
        };
        axum::serve(listener, crate::web::router(state))
            .with_graceful_shutdown(async {
                tokio::select! {
                    _ = shutdown => {},
                    _ = collector_starting => {},
                }
            })
            .await?;
        Ok(())
    })
}

fn service_uses_executable(contents: &[u8], exe: &std::path::Path) -> anyhow::Result<bool> {
    let value = plist::Value::from_reader(std::io::Cursor::new(contents))?;
    Ok(value
        .as_dictionary()
        .and_then(|value| value.get("ProgramArguments"))
        .and_then(plist::Value::as_array)
        .and_then(|args| args.first())
        .and_then(plist::Value::as_string)
        .is_some_and(|program| std::path::Path::new(program) == exe))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[serial_test::serial]
    fn display_commands_and_reader_never_request_keychain_secrets() {
        use crate::config::{CloudConfig, KikimimiConfig};
        let dir = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("KIKIMIMI_DIR");
        std::env::set_var("KIKIMIMI_DIR", dir.path());
        let cfg = KikimimiConfig {
            cloud: Some(CloudConfig {
                token: "fixture-cloud-token".into(),
                ..Default::default()
            }),
            opencode_router_cloud: Some(CloudConfig {
                token: "fixture-opencode-token".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        cfg.save().unwrap();
        let reads = crate::secret_store::read_count();
        for _ in 0..3 {
            run(Action::Status).unwrap();
            run(Action::CollectionInfo).unwrap();
            let _reader = crate::s3_reader::Reader::new();
        }
        assert_eq!(crate::secret_store::read_count(), reads);
        assert_eq!(
            crate::collection_cmd::describe(&crate::collection_cmd::load_for_display().unwrap()),
            crate::collection_cmd::describe(&cfg)
        );
        assert_eq!(
            KikimimiConfig::load().cloud.unwrap().token,
            "fixture-cloud-token"
        );
        assert_eq!(crate::secret_store::read_count(), reads + 2);
        if let Some(previous) = previous {
            std::env::set_var("KIKIMIMI_DIR", previous);
        } else {
            std::env::remove_var("KIKIMIMI_DIR");
        }
    }
    #[test]
    fn service_ownership_uses_program_arguments_not_other_plist_strings() {
        let exe = std::path::Path::new("/Applications/kikimimi.app/Contents/MacOS/kikimimi");
        let xml = crate::service::render_launchd_plist(
            std::path::Path::new("/opt/homebrew/bin/kikimimi"),
            exe,
            "/bin",
        );
        assert!(!service_uses_executable(xml.as_bytes(), exe).unwrap());
        let exe = std::path::Path::new("/tmp/A&B/kikimimi");
        let xml =
            crate::service::render_launchd_plist(exe, std::path::Path::new("/tmp/log"), "/bin");
        assert!(service_uses_executable(xml.as_bytes(), exe).unwrap());
        assert!(service_uses_executable(b"broken", exe).is_err());
    }
}

#[cfg(test)]
mod router_command_tests {
    use super::*;
    use clap::Parser;
    #[derive(Parser)]
    struct Bridge {
        #[command(subcommand)]
        action: Action,
    }
    #[test]
    fn opencode_native_commands_match_the_cli() {
        for action in ["status", "enable", "disable", "connect"] {
            assert!(
                Bridge::try_parse_from(["desktop", &format!("open-code-router-{action}")]).is_ok()
            );
        }
    }
}
