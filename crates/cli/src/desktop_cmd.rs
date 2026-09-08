//! Narrow, versioned bridge used only by the bundled desktop shell.
use clap::Subcommand;

#[derive(Subcommand)]
pub(crate) enum Action {
    Status,
    Enable,
    RestartAfterUpdate,
}

pub(crate) fn run(action: Action) -> anyhow::Result<()> {
    match action {
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
                    "service_installed": service.installed,
                    "dashboard_url": url,
                    "duckdb_available": crate::web_query::duckdb_available(),
                    "web_error": state.as_ref().and_then(|s| s.web_error.as_ref()),
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
