//! `kikimimi self-update [--check]` -- see `Command::SelfUpdate`'s docs (`lib.rs`) for the
//! user-facing overview.
//!
//! Three install shapes, three outcomes, decided by cheap fs/path checks in `update.rs`
//! before anything here ever talks to the network:
//!
//! 1. **cargo-dist install receipt present** (`update::has_install_receipt`) -- the shell
//!    installer (`kikimimi-installer.sh`) set this up, so `axoupdater` can find and replace
//!    the binary itself. This is the only branch that actually updates anything; `--check`
//!    limits it to a report.
//! 2. **No receipt, but the binary's path looks Homebrew/Linuxbrew-managed**
//!    (`update::is_brew_managed`) -- brew owns that binary outright, so this prints the
//!    `brew upgrade` command and exits 0. Not an error: the user just needs a different
//!    command, not `kikimimi self-update` itself.
//! 3. **Neither** -- a manually-downloaded release binary, a distro package, a source build
//!    run in place, `cargo install --git ...`, anything else. Prints the same curl-the-
//!    installer one-liner `README.md`'s Quickstart already documents, and exits 0.
//!    Deliberately does **not** attempt a raw self-replace (overwriting `current_exe()` by
//!    hand) in v1 -- that path has no install receipt to verify against, no atomic-replace
//!    guarantee cargo-dist's own installer provides, and no test coverage here; re-running
//!    the same installer script users already trust is the safer default until a real need
//!    for it shows up.
//!
//! Loading the receipt (`AxoUpdater::load_receipt`) is a plain fs lookup, same as
//! `update::has_install_receipt`, so branch 1 fails fast (no network) if the receipt turns
//! out to be unreadable/corrupt despite existing; only `run_sync`/`is_update_needed_sync`
//! past that point talk to the network and (for a real, non-`--check` run) re-invoke the
//! shell installer.
//!
//! `run_sync`/`is_update_needed_sync` are `axoupdater`'s `blocking`-feature entry points:
//! each spins up a minimal current-thread tokio runtime *inside itself* for the one
//! `block_on` call it needs, then tears it down before returning. Nothing about that puts
//! this command inside a tokio runtime the way `kikimimi agent` is -- this is a one-shot CLI
//! command (`lib.rs::run`'s docs: "only `kikimimi agent` spins up a tokio runtime") that
//! runs once, reports or updates, and exits.

use std::path::PathBuf;

use anyhow::Context;
use axoupdater::{AxoUpdater, AxoupdateError};

use crate::update;

pub fn run(check: bool) -> anyhow::Result<()> {
    let exe = std::env::current_exe().and_then(|p| p.canonicalize()).ok();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let xdg_config_home = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
    let has_receipt = update::has_install_receipt(
        update::APP_NAME,
        xdg_config_home.as_deref(),
        home.as_deref(),
    );

    if !has_receipt {
        if exe.as_deref().is_some_and(update::is_brew_managed) {
            println!("{}", update::BREW_UPGRADE_COMMAND);
            std::process::exit(0);
        }
        println!("{}", update::CURL_INSTALLER_ONE_LINER);
        std::process::exit(0);
    }

    let mut updater = AxoUpdater::new_for(update::APP_NAME);
    if let Err(err) = updater.load_receipt() {
        let why = if matches!(err, AxoupdateError::NoReceipt { .. }) {
            "this install of kikimimi has a receipt directory but no readable install \
             receipt in it -- `kikimimi self-update` has nothing to work from."
                .to_owned()
        } else {
            format!("kikimimi self-update couldn't read the install receipt: {err}")
        };
        eprintln!("{why}");
        eprintln!("upgrade with: {}", update::CURL_INSTALLER_ONE_LINER);
        std::process::exit(1);
    }

    // axoupdater's own default client has no request timeout at all (see
    // update::SELF_UPDATE_NETWORK_TIMEOUT's doc comment) -- set one before either
    // is_update_needed_sync or run_sync makes the only network calls this command performs,
    // so a stalled connection reports and exits instead of hanging forever with no way out
    // but Ctrl+C.
    let client = reqwest::Client::builder()
        .timeout(update::SELF_UPDATE_NETWORK_TIMEOUT)
        .build()
        .context("failed to build the HTTP client for kikimimi self-update")?;
    updater.set_client(client);

    if check {
        return match updater.is_update_needed_sync() {
            Ok(true) => {
                println!("update available (run: kikimimi self-update)");
                Ok(())
            }
            Ok(false) => {
                println!(
                    "kikimimi is already up to date (v{})",
                    env!("CARGO_PKG_VERSION")
                );
                Ok(())
            }
            Err(err) => anyhow::bail!("checking for updates failed: {err}"),
        };
    }

    match updater.run_sync() {
        Ok(Some(result)) => {
            match result.old_version {
                Some(old) => println!("kikimimi updated: v{old} -> v{}", result.new_version),
                None => println!("kikimimi updated to v{}", result.new_version),
            }
            restart_daemon_if_running()?;
            Ok(())
        }
        Ok(None) => {
            println!(
                "kikimimi is already up to date (v{})",
                env!("CARGO_PKG_VERSION")
            );
            Ok(())
        }
        Err(err) => anyhow::bail!("self-update failed: {err}"),
    }
}

/// After a successful binary replacement, restarts any daemon that was running the old
/// binary -- otherwise `kikimimi self-update` would silently leave a stale `kikimimi agent`
/// process (mapped to the now-replaced-on-disk executable, still running fine from the
/// kernel's page cache) as the *only* thing actually observing events until someone thinks
/// to restart it by hand. A no-op (`Ok(())`, no output) when `state.json` is missing/
/// unreadable or its `pid` is already dead -- both mean "no daemon was running", not a
/// failure of the update itself (which already succeeded by the time this runs).
fn restart_daemon_if_running() -> anyhow::Result<()> {
    let Some(state) = crate::state::load_opt(&kikimimi_schema::paths::state_path()) else {
        return Ok(());
    };
    if !update::pid_alive(state.pid) {
        return Ok(());
    }

    update::kill_and_wait(state.pid, update::DAEMON_STOP_TIMEOUT)
        .with_context(|| format!("stopping the running daemon (pid {})", state.pid))?;

    // Task B: if the daemon is registered as a user-level service (macOS LaunchAgent / Linux
    // systemd --user), that service's own restart policy (KeepAlive / Restart=on-failure)
    // notices the SIGTERM'd process exit and restarts it from the now-updated binary on its
    // own -- a manual respawn below would just race it, and the losing instance's `kikimimi
    // agent` exits right back out on the control-socket liveness check (agent.rs) anyway.
    if crate::service::status().installed {
        println!("daemon stopped; the installed service will restart it");
        return Ok(());
    }

    // `kikimimi agent` (invoked with no `--foreground`, same as `README.md`'s own
    // `kikimimi agent &`) daemonizes itself via `daemonize::daemonize` -- reuse that exact
    // startup path by re-invoking the (now-updated) binary as a subprocess and letting it
    // detach itself, rather than re-deriving the double-fork/setsid dance here.
    let exe = std::env::current_exe()
        .context("locating the updated kikimimi executable to respawn the daemon")?;
    let mut child = std::process::Command::new(&exe)
        .arg("agent")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("spawning `kikimimi agent` to restart the daemon")?;
    // `daemonize`'s first fork exits its immediate child (this one) within milliseconds,
    // once its own grandchild -- the actual daemon -- is detached (setsid + second fork);
    // waiting for it here just reaps that short-lived intermediate process promptly instead
    // of leaving it a zombie for however long this command takes to exit on its own.
    let _ = child.wait();

    println!("daemon restarted");
    Ok(())
}
