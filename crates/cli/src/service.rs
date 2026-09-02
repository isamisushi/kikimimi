//! `kikimimi service install|uninstall|status` — user-level service manager registration so
//! `kikimimi agent` survives reboots and crashes (architecture.md §2.2 fail-open: a crash or
//! reboot must not silently stop collection with nobody noticing).
//!
//! Platform abstraction, dispatched at runtime on `std::env::consts::OS`:
//! - **macOS**: a LaunchAgent plist at `~/Library/LaunchAgents/dev.kikimimi.agent.plist`,
//!   loaded with `launchctl bootstrap gui/<uid> <plist>` (falling back to the older
//!   `launchctl load -w` on macOS releases without `bootstrap`/`bootout`).
//! - **Linux**: a systemd user unit at `~/.config/systemd/user/kikimimi-agent.service`,
//!   enabled with `systemctl --user enable --now`. If `systemd --user` isn't available at all
//!   (no `systemctl` on PATH, or no user D-Bus session — common in containers/CI) this is
//!   reported as [`ServiceOutcome::NotSupported`], not an error: the daemon still works fine
//!   started by hand, it just won't survive a reboot on its own.
//! - **anything else**: [`ServiceOutcome::NotSupported`].
//!
//! Both `ExecStart`/`ProgramArguments` point at `[current_exe, "agent", "--foreground"]` —
//! `--foreground` is the right entry point for a service manager (no double-fork fighting the
//! supervisor's own process tracking; see `daemonize.rs`'s docs on why the double-fork exists
//! at all, which is precisely to detach from a *shell*, not from a service manager).
//!
//! Every subprocess call here goes through [`run_cmd`], which can never panic (a spawn
//! failure — e.g. `launchctl`/`systemctl` missing — just becomes `None`, handled like any
//! other failure) and never hangs past however long the short, local command itself takes.
//! Nothing in this module is ever allowed to fail `kikimimi init` — callers (`init_cmd.rs`)
//! only ever report a [`ServiceOutcome`], never propagate it as a hard error.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// launchd job label (macOS) — also the plist's own filename stem and the target passed to
/// `launchctl bootstrap`/`bootout`/`print`.
pub const LAUNCHD_LABEL: &str = "dev.kikimimi.agent";
/// systemd user unit name (Linux).
pub const SYSTEMD_UNIT_NAME: &str = "kikimimi-agent.service";

/// Result of [`install`] or [`uninstall`]. Every variant carries enough to print a one-line
/// human summary ([`ServiceOutcome::summary`]) — none of this is ever turned into a hard
/// `anyhow::Error` by `init_cmd.rs` (fail-open); only the standalone `kikimimi service
/// install`/`uninstall` subcommands (`run_install`/`run_uninstall` below) turn a `Failed`
/// outcome into a non-zero exit, since those are commands the user ran on purpose to find out
/// whether it worked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceOutcome {
    /// Installed (or re-installed, idempotently) and started.
    Installed {
        manager: &'static str,
        unit_path: PathBuf,
    },
    /// Removed a previously-installed service.
    Uninstalled { manager: &'static str },
    /// `uninstall` called with nothing installed — not an error.
    NotInstalled,
    /// This platform, or this host's configuration of it (e.g. no systemd user session),
    /// can't run a user-level service at all. Not an error — `kikimimi agent` still works
    /// run by hand, it just won't survive a reboot unattended.
    NotSupported { reason: String },
    /// Attempted and failed (fs write, or the service manager rejected it).
    Failed {
        manager: &'static str,
        reason: String,
    },
}

impl ServiceOutcome {
    /// One-line, prefix-free human summary. Callers that want the "WARNING "/"NOTE " prefix
    /// convention the rest of `init_cmd.rs`/`status_cmd.rs` use add it themselves based on
    /// the variant (`is_failure`/`is_not_supported`), since which prefix reads best differs
    /// slightly between "install" and "uninstall" phrasing.
    pub fn summary(&self) -> String {
        match self {
            ServiceOutcome::Installed { manager, unit_path } => {
                format!("installed ({manager}) at {}", unit_path.display())
            }
            ServiceOutcome::Uninstalled { manager } => format!("uninstalled ({manager})"),
            ServiceOutcome::NotInstalled => "not installed, nothing to remove".to_string(),
            ServiceOutcome::NotSupported { reason } => format!("not supported: {reason}"),
            ServiceOutcome::Failed { manager, reason } => format!("failed ({manager}): {reason}"),
        }
    }

    pub fn is_failure(&self) -> bool {
        matches!(self, ServiceOutcome::Failed { .. })
    }

    pub fn is_not_supported(&self) -> bool {
        matches!(self, ServiceOutcome::NotSupported { .. })
    }
}

/// Result of [`status`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceStatus {
    /// `None` only on a platform with no service manager support at all (never `None` on
    /// macOS/Linux, even when nothing is installed there yet).
    pub manager: Option<&'static str>,
    pub installed: bool,
    pub unit_path: Option<PathBuf>,
    /// `None` when `installed` is `false` (nothing to check), or when the service manager
    /// couldn't be asked (e.g. `launchctl`/`systemctl` missing).
    pub running: Option<bool>,
}

pub fn install() -> ServiceOutcome {
    match std::env::consts::OS {
        "macos" => install_launchd(),
        "linux" => install_systemd(),
        other => ServiceOutcome::NotSupported {
            reason: format!(
                "kikimimi service is not supported on {other} -- run `kikimimi agent` yourself"
            ),
        },
    }
}

pub fn uninstall() -> ServiceOutcome {
    match std::env::consts::OS {
        "macos" => uninstall_launchd(),
        "linux" => uninstall_systemd(),
        other => ServiceOutcome::NotSupported {
            reason: format!("kikimimi service is not supported on {other}"),
        },
    }
}

pub fn status() -> ServiceStatus {
    match std::env::consts::OS {
        "macos" => status_launchd(),
        "linux" => status_systemd(),
        _ => ServiceStatus {
            manager: None,
            installed: false,
            unit_path: None,
            running: None,
        },
    }
}

/// `kikimimi service install` — the standalone subcommand (`lib.rs`'s `ServiceAction`).
/// Unlike the fail-open call `init_cmd.rs` makes, this one exits non-zero on failure: the
/// user ran this specifically to find out whether it worked.
pub fn run_install() -> anyhow::Result<()> {
    let outcome = install();
    println!("{}", outcome.summary());
    if outcome.is_failure() {
        anyhow::bail!("service install failed");
    }
    Ok(())
}

pub fn run_uninstall() -> anyhow::Result<()> {
    let outcome = uninstall();
    println!("{}", outcome.summary());
    if outcome.is_failure() {
        anyhow::bail!("service uninstall failed");
    }
    Ok(())
}

pub fn run_status() -> anyhow::Result<()> {
    let s = status();
    match s.manager {
        Some(manager) => {
            println!("manager: {manager}");
            println!("installed: {}", s.installed);
            if let Some(p) = &s.unit_path {
                println!("unit path: {}", p.display());
            }
            println!(
                "running: {}",
                match (s.installed, s.running) {
                    (false, _) => "no",
                    (true, Some(true)) => "yes",
                    (true, Some(false)) => "no",
                    (true, None) => "unknown",
                }
            );
        }
        None => println!(
            "not supported on this OS ({}) -- run `kikimimi agent` yourself",
            std::env::consts::OS
        ),
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// macOS (launchd)
// ---------------------------------------------------------------------------------------

fn launchd_plist_path_in(home: &Path) -> PathBuf {
    home.join("Library")
        .join("LaunchAgents")
        .join(format!("{LAUNCHD_LABEL}.plist"))
}

fn install_launchd() -> ServiceOutcome {
    let Some(home) = home_dir() else {
        return ServiceOutcome::Failed {
            manager: "launchd",
            reason: "HOME is not set".to_string(),
        };
    };
    let exe = match current_exe() {
        Ok(e) => e,
        Err(e) => {
            return ServiceOutcome::Failed {
                manager: "launchd",
                reason: format!("locating the kikimimi executable: {e:#}"),
            }
        }
    };
    let plist_path = launchd_plist_path_in(&home);
    let log_path = agent_log_path();
    let path_env = std::env::var("PATH").unwrap_or_default();
    let contents = render_launchd_plist(&exe, &log_path, &path_env);

    if let Some(parent) = plist_path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return ServiceOutcome::Failed {
                manager: "launchd",
                reason: format!("creating {}: {e:#}", parent.display()),
            };
        }
    }
    if let Err(e) = std::fs::write(&plist_path, contents.as_bytes()) {
        return ServiceOutcome::Failed {
            manager: "launchd",
            reason: format!("writing {}: {e:#}", plist_path.display()),
        };
    }

    let uid = unsafe { libc::getuid() };
    let domain_target = format!("gui/{uid}");
    let service_target = format!("{domain_target}/{LAUNCHD_LABEL}");
    let plist_str = plist_path.to_string_lossy().into_owned();

    // Idempotent: drop any previous registration first (ignored if none was loaded) so a
    // re-run of `kikimimi init` (or `kikimimi service install`) after e.g. a binary move
    // picks up the new plist instead of `bootstrap` erroring "service already loaded".
    let _ = run_cmd("launchctl", &["bootout", &service_target]);

    let bootstrap = run_cmd("launchctl", &["bootstrap", &domain_target, &plist_str]);
    if bootstrap.as_ref().is_some_and(|o| o.status.success()) {
        return ServiceOutcome::Installed {
            manager: "launchd",
            unit_path: plist_path,
        };
    }
    // Fallback for macOS releases predating `bootstrap`/`bootout` (launchctl 1 era).
    let load = run_cmd("launchctl", &["load", "-w", &plist_str]);
    if load.as_ref().is_some_and(|o| o.status.success()) {
        return ServiceOutcome::Installed {
            manager: "launchd",
            unit_path: plist_path,
        };
    }
    ServiceOutcome::Failed {
        manager: "launchd",
        reason: describe_two_attempts("launchctl bootstrap", bootstrap, "launchctl load -w", load),
    }
}

fn uninstall_launchd() -> ServiceOutcome {
    let Some(home) = home_dir() else {
        return ServiceOutcome::Failed {
            manager: "launchd",
            reason: "HOME is not set".to_string(),
        };
    };
    let plist_path = launchd_plist_path_in(&home);
    if !plist_path.exists() {
        return ServiceOutcome::NotInstalled;
    }

    let uid = unsafe { libc::getuid() };
    let service_target = format!("gui/{uid}/{LAUNCHD_LABEL}");
    let bootout = run_cmd("launchctl", &["bootout", &service_target]);
    if !bootout.as_ref().is_some_and(|o| o.status.success()) {
        // Older-launchctl fallback, best-effort -- either way the plist file is removed
        // below, which is what actually prevents it coming back at next login.
        let _ = run_cmd("launchctl", &["unload", &plist_path.to_string_lossy()]);
    }

    if let Err(e) = std::fs::remove_file(&plist_path) {
        return ServiceOutcome::Failed {
            manager: "launchd",
            reason: format!("removing {}: {e:#}", plist_path.display()),
        };
    }
    ServiceOutcome::Uninstalled { manager: "launchd" }
}

fn status_launchd() -> ServiceStatus {
    let plist_path = home_dir().map(|h| launchd_plist_path_in(&h));
    let installed = plist_path.as_ref().is_some_and(|p| p.exists());
    let running = if installed {
        let uid = unsafe { libc::getuid() };
        let service_target = format!("gui/{uid}/{LAUNCHD_LABEL}");
        run_cmd("launchctl", &["print", &service_target]).map(|o| o.status.success())
    } else {
        None
    };
    ServiceStatus {
        manager: Some("launchd"),
        installed,
        unit_path: plist_path,
        running,
    }
}

/// Pure (no I/O) plist renderer, unit-tested directly. `exe`/`log_path`/`path_env` are
/// XML-escaped since `PATH` in particular is attacker/environment-controlled-ish and could in
/// principle contain `&`/`<`/`>`.
pub fn render_launchd_plist(exe: &Path, log_path: &Path, path_env: &str) -> String {
    let label = LAUNCHD_LABEL;
    let exe = xml_escape(&exe.display().to_string());
    let log = xml_escape(&log_path.display().to_string());
    let path = xml_escape(path_env);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>agent</string>
        <string>--foreground</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>ThrottleInterval</key>
    <integer>10</integer>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>PATH</key>
        <string>{path}</string>
    </dict>
</dict>
</plist>
"#
    )
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------------------
// Linux (systemd --user)
// ---------------------------------------------------------------------------------------

fn systemd_unit_path_in(home: &Path) -> PathBuf {
    home.join(".config")
        .join("systemd")
        .join("user")
        .join(SYSTEMD_UNIT_NAME)
}

fn install_systemd() -> ServiceOutcome {
    // Check systemd --user is actually usable *before* writing anything: distinguishes "not
    // available here" (container/CI with no session, or systemctl missing entirely) from a
    // real failure, and the task spec calls for reporting the former as NotSupported, not an
    // error.
    match run_cmd("systemctl", &["--user", "daemon-reload"]) {
        None => {
            return ServiceOutcome::NotSupported {
                reason: "systemctl not found on PATH -- run `kikimimi agent` yourself".to_string(),
            }
        }
        Some(o) if !o.status.success() => {
            let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
            return ServiceOutcome::NotSupported {
                reason: format!(
                    "systemd --user is not available on this host{} -- run `kikimimi agent` \
                     yourself",
                    if stderr.is_empty() {
                        String::new()
                    } else {
                        format!(" ({stderr})")
                    }
                ),
            };
        }
        Some(_) => {}
    }

    let Some(home) = home_dir() else {
        return ServiceOutcome::Failed {
            manager: "systemd",
            reason: "HOME is not set".to_string(),
        };
    };
    let exe = match current_exe() {
        Ok(e) => e,
        Err(e) => {
            return ServiceOutcome::Failed {
                manager: "systemd",
                reason: format!("locating the kikimimi executable: {e:#}"),
            }
        }
    };
    let unit_path = systemd_unit_path_in(&home);
    let path_env = std::env::var("PATH").unwrap_or_default();
    let contents = render_systemd_unit(&exe, &path_env);

    if let Some(parent) = unit_path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return ServiceOutcome::Failed {
                manager: "systemd",
                reason: format!("creating {}: {e:#}", parent.display()),
            };
        }
    }
    if let Err(e) = std::fs::write(&unit_path, contents.as_bytes()) {
        return ServiceOutcome::Failed {
            manager: "systemd",
            reason: format!("writing {}: {e:#}", unit_path.display()),
        };
    }

    // Unit file changed (or is new): reload before `enable --now` picks it up. `enable
    // --now` itself is idempotent -- already-enabled-and-running is a no-op success, so a
    // repeat `kikimimi init` doesn't need any special-casing here.
    let _ = run_cmd("systemctl", &["--user", "daemon-reload"]);
    match run_cmd(
        "systemctl",
        &["--user", "enable", "--now", SYSTEMD_UNIT_NAME],
    ) {
        Some(o) if o.status.success() => ServiceOutcome::Installed {
            manager: "systemd",
            unit_path,
        },
        Some(o) => ServiceOutcome::Failed {
            manager: "systemd",
            reason: String::from_utf8_lossy(&o.stderr).trim().to_string(),
        },
        None => ServiceOutcome::Failed {
            manager: "systemd",
            reason: "systemctl not found on PATH".to_string(),
        },
    }
}

fn uninstall_systemd() -> ServiceOutcome {
    let Some(home) = home_dir() else {
        return ServiceOutcome::Failed {
            manager: "systemd",
            reason: "HOME is not set".to_string(),
        };
    };
    let unit_path = systemd_unit_path_in(&home);
    if !unit_path.exists() {
        return ServiceOutcome::NotInstalled;
    }

    // Best-effort: even if systemd --user is unreachable right now (so this does nothing),
    // removing the unit file below still prevents it coming back at next login.
    let _ = run_cmd(
        "systemctl",
        &["--user", "disable", "--now", SYSTEMD_UNIT_NAME],
    );

    if let Err(e) = std::fs::remove_file(&unit_path) {
        return ServiceOutcome::Failed {
            manager: "systemd",
            reason: format!("removing {}: {e:#}", unit_path.display()),
        };
    }
    let _ = run_cmd("systemctl", &["--user", "daemon-reload"]);
    ServiceOutcome::Uninstalled { manager: "systemd" }
}

fn status_systemd() -> ServiceStatus {
    let unit_path = home_dir().map(|h| systemd_unit_path_in(&h));
    let installed = unit_path.as_ref().is_some_and(|p| p.exists());
    let running = if installed {
        run_cmd("systemctl", &["--user", "is-active", SYSTEMD_UNIT_NAME])
            .map(|o| o.status.success())
    } else {
        None
    };
    ServiceStatus {
        manager: Some("systemd"),
        installed,
        unit_path,
        running,
    }
}

/// Pure (no I/O) systemd unit renderer, unit-tested directly.
pub fn render_systemd_unit(exe: &Path, path_env: &str) -> String {
    let exe = exe.display();
    format!(
        r#"[Unit]
Description=kikimimi agent (local-first AI coding agent observability daemon)
After=network.target

[Service]
ExecStart={exe} agent --foreground
Restart=on-failure
RestartSec=5
Environment=PATH={path_env}

[Install]
WantedBy=default.target
"#
    )
}

// ---------------------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------------------

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Same log path `kikimimi agent`'s default (non-`--foreground`) double-fork daemonize
/// (`daemonize.rs`) already redirects stdout/stderr to -- reusing it means `kikimimi agent
/// --foreground` under the service and a manual `kikimimi agent &` both end up logging to the
/// exact same file.
fn agent_log_path() -> PathBuf {
    kikimimi_schema::paths::kikimimi_dir().join("agent.log")
}

fn current_exe() -> std::io::Result<PathBuf> {
    std::env::current_exe().and_then(|p| p.canonicalize())
}

/// Runs a short local command and returns its output, or `None` if it couldn't even be
/// spawned (binary missing from PATH, permissions, etc.) -- never panics, no explicit timeout
/// needed since `launchctl`/`systemctl` calls here are all local, near-instant operations.
fn run_cmd(bin: &str, args: &[&str]) -> Option<Output> {
    Command::new(bin).args(args).output().ok()
}

fn describe_two_attempts(
    label_a: &str,
    a: Option<Output>,
    label_b: &str,
    b: Option<Output>,
) -> String {
    format!(
        "{label_a}: {}; {label_b}: {}",
        describe_attempt(a),
        describe_attempt(b)
    )
}

fn describe_attempt(o: Option<Output>) -> String {
    match o {
        Some(o) if o.status.success() => "ok".to_string(),
        Some(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
            if stderr.is_empty() {
                format!("exited {}", o.status)
            } else {
                stderr
            }
        }
        None => "command not found".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- render_launchd_plist ------------------------------------------------------------

    #[test]
    fn render_launchd_plist_is_well_formed_and_has_expected_program_arguments() {
        let plist = render_launchd_plist(
            Path::new("/usr/local/bin/kikimimi"),
            Path::new("/home/me/.kikimimi/agent.log"),
            "/usr/bin:/bin",
        );

        assert!(plist.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n"));
        assert!(plist.trim_end().ends_with("</plist>"));
        assert_balanced_xml_tags(&plist);

        assert!(plist.contains(&format!(
            "<key>Label</key>\n    <string>{LAUNCHD_LABEL}</string>"
        )));

        // Exact ProgramArguments block, in order: exe, "agent", "--foreground" -- this is
        // the whole point of running under a service manager (daemonize.rs's double-fork is
        // for detaching from a *shell*, not for this).
        let expected_args = "<key>ProgramArguments</key>\n    <array>\n        \
             <string>/usr/local/bin/kikimimi</string>\n        <string>agent</string>\n        \
             <string>--foreground</string>\n    </array>";
        assert!(plist.contains(expected_args), "got:\n{plist}");

        assert!(plist.contains("<key>RunAtLoad</key>\n    <true/>"));
        // KeepAlive: restart on crash, but never loop on a clean (0) exit.
        assert!(plist.contains(
            "<key>KeepAlive</key>\n    <dict>\n        <key>SuccessfulExit</key>\n        <false/>\n    </dict>"
        ));
        assert!(plist.contains("<key>ThrottleInterval</key>\n    <integer>10</integer>"));
        assert!(plist.contains(
            "<key>StandardOutPath</key>\n    <string>/home/me/.kikimimi/agent.log</string>"
        ));
        assert!(plist.contains(
            "<key>StandardErrorPath</key>\n    <string>/home/me/.kikimimi/agent.log</string>"
        ));
        assert!(plist.contains(
            "<key>EnvironmentVariables</key>\n    <dict>\n        <key>PATH</key>\n        <string>/usr/bin:/bin</string>\n    </dict>"
        ));
    }

    #[test]
    fn render_launchd_plist_xml_escapes_special_characters_in_path_env() {
        let plist = render_launchd_plist(
            Path::new("/usr/local/bin/kikimimi"),
            Path::new("/home/me/.kikimimi/agent.log"),
            "/opt/a&b:/usr/bin",
        );
        assert!(plist.contains("/opt/a&amp;b:/usr/bin"));
        assert!(
            !plist.contains("a&b"),
            "raw '&' must never appear unescaped in the rendered plist:\n{plist}"
        );
        assert_balanced_xml_tags(&plist);
    }

    /// Not a full XML parser -- just enough of a sanity check that every opening tag this
    /// template uses has a matching closer, and no bare/unescaped `&` slipped through.
    fn assert_balanced_xml_tags(xml: &str) {
        for tag in ["dict", "array"] {
            let opens = xml.matches(&format!("<{tag}>")).count();
            let closes = xml.matches(&format!("</{tag}>")).count();
            assert_eq!(opens, closes, "unbalanced <{tag}> in:\n{xml}");
        }
        // `<plist ...>` always carries a `version` attribute, so it can't match the bare
        // `<tag>` shape the other tags use above.
        assert_eq!(
            xml.matches("<plist ").count(),
            xml.matches("</plist>").count(),
            "unbalanced <plist> in:\n{xml}"
        );
        for (i, _) in xml.match_indices('&') {
            let rest = &xml[i..];
            assert!(
                rest.starts_with("&amp;")
                    || rest.starts_with("&lt;")
                    || rest.starts_with("&gt;")
                    || rest.starts_with("&quot;")
                    || rest.starts_with("&apos;"),
                "unescaped '&' at byte {i} in:\n{xml}"
            );
        }
    }

    // -- render_systemd_unit --------------------------------------------------------------

    #[test]
    fn render_systemd_unit_is_well_formed_ini_with_expected_exec_start() {
        let unit = render_systemd_unit(Path::new("/usr/bin/kikimimi"), "/usr/bin:/bin");

        assert!(unit.contains("[Unit]\n"));
        assert!(unit.contains("[Service]\n"));
        assert!(unit.contains("[Install]\n"));
        // Sections appear in order and each has at least one key under it.
        let unit_i = unit.find("[Unit]").unwrap();
        let service_i = unit.find("[Service]").unwrap();
        let install_i = unit.find("[Install]").unwrap();
        assert!(unit_i < service_i && service_i < install_i);

        assert!(
            unit.contains("ExecStart=/usr/bin/kikimimi agent --foreground\n"),
            "got:\n{unit}"
        );
        assert!(unit.contains("Restart=on-failure\n"));
        assert!(unit.contains("RestartSec=5\n"));
        assert!(unit.contains("Environment=PATH=/usr/bin:/bin\n"));
        assert!(unit.contains("WantedBy=default.target\n"));

        // Every non-blank, non-section line is a `key=value` pair -- the shape systemd's
        // unit-file parser (and any other basic INI reader) expects.
        for line in unit.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('[') {
                continue;
            }
            assert!(line.contains('='), "not a key=value line: {line:?}");
        }
    }

    // -- path builders (pure) --------------------------------------------------------------

    #[test]
    fn launchd_plist_path_in_is_under_library_launchagents() {
        let p = launchd_plist_path_in(Path::new("/home/me"));
        assert_eq!(
            p,
            PathBuf::from("/home/me/Library/LaunchAgents/dev.kikimimi.agent.plist")
        );
    }

    #[test]
    fn systemd_unit_path_in_is_under_config_systemd_user() {
        let p = systemd_unit_path_in(Path::new("/home/me"));
        assert_eq!(
            p,
            PathBuf::from("/home/me/.config/systemd/user/kikimimi-agent.service")
        );
    }

    // -- ServiceOutcome::summary -----------------------------------------------------------

    #[test]
    fn service_outcome_summary_covers_every_variant() {
        assert_eq!(
            ServiceOutcome::Installed {
                manager: "launchd",
                unit_path: PathBuf::from("/x/y.plist"),
            }
            .summary(),
            "installed (launchd) at /x/y.plist"
        );
        assert_eq!(
            ServiceOutcome::Uninstalled { manager: "systemd" }.summary(),
            "uninstalled (systemd)"
        );
        assert_eq!(
            ServiceOutcome::NotInstalled.summary(),
            "not installed, nothing to remove"
        );
        assert_eq!(
            ServiceOutcome::NotSupported {
                reason: "no dbus session".to_string()
            }
            .summary(),
            "not supported: no dbus session"
        );
        assert_eq!(
            ServiceOutcome::Failed {
                manager: "systemd",
                reason: "boom".to_string()
            }
            .summary(),
            "failed (systemd): boom"
        );
    }

    #[test]
    fn service_outcome_is_failure_and_is_not_supported() {
        let failed = ServiceOutcome::Failed {
            manager: "systemd",
            reason: "boom".to_string(),
        };
        assert!(failed.is_failure());
        assert!(!failed.is_not_supported());

        let not_supported = ServiceOutcome::NotSupported {
            reason: "nope".to_string(),
        };
        assert!(!not_supported.is_failure());
        assert!(not_supported.is_not_supported());

        assert!(!ServiceOutcome::NotInstalled.is_failure());
    }

    // -- run_cmd never panics on a missing binary -------------------------------------------

    #[test]
    fn run_cmd_returns_none_for_a_nonexistent_binary() {
        assert!(run_cmd(
            "kikimimi-service-test-definitely-not-a-real-binary-xyz",
            &[]
        )
        .is_none());
    }
}
