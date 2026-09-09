//! Narrow, versioned bridge used only by the bundled desktop shell.
use clap::Subcommand;

#[derive(Subcommand)]
pub(crate) enum Action {
    Status,
    CollectionInfo,
    SwitchCollection,
    Enable,
    Resume,
    ServeDashboard,
    RestartAfterUpdate,
}

pub(crate) fn run(action: Action) -> anyhow::Result<()> {
    match action {
        Action::CollectionInfo => crate::collection_cmd::info(),
        Action::SwitchCollection => crate::collection_cmd::switch(),
        Action::ServeDashboard => serve_dashboard(),
        Action::Resume => {
            let service = crate::service::status();
            anyhow::ensure!(
                service.installed,
                "Collection is not set up. Complete setup first."
            );
            if kikimimi_spool::send_control(b'n') {
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
                        if kikimimi_spool::send_control(b'n') {
                            anyhow::ensure!(crate::init_cmd::stop_manual_daemon()?.is_some(), "Cannot locate the running collector PID; refusing an incomplete restart");
                        }
                        crate::service::restart()?;
                        for _ in 0..100 {
                            if kikimimi_spool::send_control(b'n') {
                                return Ok(());
                            }
                            std::thread::sleep(std::time::Duration::from_millis(100));
                        }
                        anyhow::bail!("Updated collector did not become ready; restart will be retried on the next desktop launch");
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
                    "collection_target": crate::collection_cmd::load().ok().map(|c| crate::collection_cmd::describe(&c)),
                    "applied_collection_target": state.as_ref().and_then(|s| s.collection_target.clone().or_else(|| {
                        let cfg = crate::collection_cmd::load().ok()?;
                        (cfg.cloud.is_none() && cfg.s3.is_none() && s.cloud.is_none() && s.s3.is_none()).then(|| crate::collection_cmd::describe(&cfg))
                    })),
                    "service_installed": service.installed,
                    "dashboard_url": url,
                    "duckdb_available": crate::web_query::duckdb_available(),
                    "web_error": state.as_ref().and_then(|s| s.web_error.as_ref()),
                    "has_history": state.is_some(),
                    "has_s3_reader": crate::config::KikimimiConfig::load().s3_reader.is_some(),
                    "last_event_ts": state.as_ref().and_then(|s| s.last_event_ts),
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

// The desktop owns this process through a pipe. It serves saved data without
// starting hooks, session tailers, OTel, cloud sync, or the launchd collector.
fn serve_dashboard() -> anyhow::Result<()> {
    use std::io::Write;
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
        println!("{}", serde_json::json!({ "url": url }));
        std::io::stdout().flush()?;
        let (closed, shutdown) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink());
            let _ = closed.send(());
        });
        axum::serve(listener, crate::web::router(state))
            .with_graceful_shutdown(async {
                let _ = shutdown.await;
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
