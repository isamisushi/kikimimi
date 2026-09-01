//! Shared plumbing for `kikimimi self-update` (`self_update_cmd.rs`) and the daemon's
//! background update notifier (`spawn_notifier`, started once from `agent::run`).
//!
//! Design mirrors a sibling project's (katamari's) own `update.rs`/`self-update` command --
//! see that crate's module docs for the full rationale -- adapted to kikimimi's shape in a
//! few deliberate ways:
//!
//! - katamari is a TUI that shows an update notice once per interactive session and never
//!   itself listens for anything in the background; kikimimi is a headless daemon that's
//!   already running the whole time, so *it* (not `kikimimi status`) owns the network side
//!   of the update check ([`spawn_notifier`]), and `kikimimi status` only ever reads the
//!   cache [`spawn_notifier`] last wrote ([`available_update`]) -- never a live request.
//! - the on-disk cache lives at `~/.kikimimi/update-check.json` (via
//!   [`kikimimi_schema::paths::kikimimi_dir`]), not a separate `$XDG_STATE_HOME` tree --
//!   kikimimi already keeps every other piece of daemon state (`state.json`, `config.json`)
//!   directly under `kikimimi_dir()`, so this follows that existing convention rather than
//!   introducing a second state root the rest of the crate doesn't have.
//! - cache writes reuse this crate's own [`crate::state::write_atomic`] (tmp-file-then-
//!   rename, owner-only from creation -- see that function's docs) instead of katamari's
//!   hand-rolled equivalent, since kikimimi already has exactly that helper and every other
//!   piece of daemon state uses it.
//! - the network call is async (`reqwest::Client`, already a dependency here with its
//!   default -- non-blocking -- feature set, unlike katamari which added a `reqwest`
//!   dependency solely for `axoupdater`'s `set_client`) and driven by a detached
//!   `tokio::spawn` task with `tokio::time::sleep`, since kikimimi's daemon already runs
//!   inside a tokio runtime end to end (`agent.rs`) -- katamari's equivalent spawns a plain
//!   `std::thread` because its TUI has no runtime to spawn a task onto.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The app name cargo-dist's shell installer writes into the install receipt
/// (`source.app_name`/`name` in `kikimimi-installer.sh`'s `RECEIPT` template) and the name a
/// receipt file itself is stamped with (`<app_name>-receipt.json`) -- the package name
/// (`crates/cli/Cargo.toml`'s `[package] name`), not either `[[bin]]` name (`kikimimi`
/// happens to match both here, but `kkmm` never would). Shared between
/// [`has_install_receipt`] here and `self_update_cmd.rs`'s `AxoUpdater::new_for`, so the two
/// can never name-mismatch and silently look at different receipts.
pub(crate) const APP_NAME: &str = "kikimimi";

/// The exact one-liner `README.md`'s Quickstart already tells users to run, printed by
/// `kikimimi self-update` for an install it has no other way to upgrade (no cargo-dist
/// receipt, not Homebrew-managed) -- see `self_update_cmd.rs`.
pub(crate) const CURL_INSTALLER_ONE_LINER: &str =
    "curl -fsSL https://github.com/isamisushi/kikimimi/releases/latest/download/kikimimi-installer.sh | sh";

/// The `brew upgrade` invocation for a Homebrew/Linuxbrew-managed install (`dist-workspace.toml`'s
/// `tap = "isamisushi/homebrew-tap"`, installed via `brew install isamisushi/tap/kikimimi` per
/// `README.md`).
pub(crate) const BREW_UPGRADE_COMMAND: &str = "brew upgrade isamisushi/tap/kikimimi";

/// The request timeout `self_update_cmd.rs` sets on the `reqwest::Client` it hands
/// `axoupdater` via `set_client`, before `run_sync`/`is_update_needed_sync`. `axoupdater`
/// 0.10.2 builds its own default client as a bare `reqwest::Client::new()` with no timeout
/// set anywhere in the crate, so left alone, every request that client makes -- the GitHub
/// releases-API lookup and the fetch of the installer script itself (the actual binary
/// download happens later, as a subprocess running that fetched shell script, outside
/// `axoupdater`'s `reqwest` client entirely) -- can hang forever against a connection that
/// accepts a TCP handshake but never completes a response (a firewall/VPN blackhole, a
/// stalled TLS handshake, some corporate proxies), with no feedback and no way out but
/// Ctrl+C. That's exactly the failure mode [`NOTIFIER_NETWORK_TIMEOUT`] is designed to avoid
/// for the daemon's own network call -- `self_update_cmd.rs` is a one-shot foreground
/// command, so it can't lean on "a background task nobody's waiting on" the way
/// [`spawn_notifier`] does. Sized well above [`NOTIFIER_NETWORK_TIMEOUT`]'s 10s (a single
/// small JSON GET) since a shell-installer script is a bigger, if still modest, payload --
/// but nowhere near what a multi-megabyte binary would need, because this client never
/// actually carries one.
pub(crate) const SELF_UPDATE_NETWORK_TIMEOUT: Duration = Duration::from_secs(30);

/// How long `kikimimi self-update`'s bounded wait for the daemon to exit, after sending
/// SIGTERM, keeps polling before giving up and escalating to SIGKILL -- see
/// `self_update_cmd.rs::restart_daemon_if_running`.
pub(crate) const DAEMON_STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// How long after daemon startup [`spawn_notifier`] waits before its first check --
/// deliberately well past a typical `kikimimi agent` session's startup burst of activity (the
/// initial spool drain, OTLP/web-UI bind, `state.json` write), so a fresh daemon boot never
/// competes with those for anything (not that this check would meaningfully compete --
/// see the module docs -- but there's no reason for it to even try).
const NOTIFIER_INITIAL_DELAY: Duration = Duration::from_secs(10 * 60);

/// How often [`spawn_notifier`] re-checks after its first look. Matches gh CLI's own update
/// notifier interval and katamari's [`STALE_AFTER`]-equivalent: frequent enough that a
/// long-running daemon hears about a new release within a day of it shipping, rare enough
/// that it costs one small GET per day, forever, no matter how long `kikimimi agent` stays up.
const NOTIFIER_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// `GET`-timeout for [`fetch_latest_release_tag`] -- see [`SELF_UPDATE_NETWORK_TIMEOUT`]'s
/// docs for why a bound is needed at all. Shorter than that constant because this call
/// carries no payload past a small JSON body (no installer script, no binary), so there's
/// nothing here that legitimately takes 30s.
const NOTIFIER_NETWORK_TIMEOUT: Duration = Duration::from_secs(10);

const RELEASES_URL: &str = "https://api.github.com/repos/isamisushi/kikimimi/releases/latest";

/// Set (to any value, matching this codebase's other `KIKIMIMI_DEV_*`-style boolean env vars
/// -- see `crates/cloud/src/config.rs`'s `KIKIMIMI_DEV_AUTOAPPROVE`) to `"1"` to disable
/// [`spawn_notifier`] entirely: no background task is spawned at all, so a machine with this
/// set never makes the GitHub request and never touches the cache file, not even to leave it
/// stale -- the complete opt-out promised by the milestone task.
const NOTIFIER_DISABLE_ENV: &str = "KIKIMIMI_NO_UPDATE_CHECK";

/// A newer release than the one running now -- the only thing [`available_update`]'s caller
/// (`status_cmd.rs`) needs to know. Carrying just the version string (not the whole
/// [`Cache`]) keeps that call site's job (print one line) obviously matched to what it's
/// given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableUpdate {
    pub latest_version: String,
}

/// The state file's on-disk shape -- `~/.kikimimi/update-check.json`. Field names match the
/// milestone task's spec verbatim (`latest_version`, `checked_at_ms`) rather than
/// katamari's `last_checked`/`latest_version` naming, since this is a fresh file this
/// project owns, not one inheriting an existing format. `checked_at_ms` is retried-on-
/// failure too (see [`refresh_cache`]'s docs): an offline daemon gets asked again the next
/// day, not left silently stale forever.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Cache {
    latest_version: String,
    checked_at_ms: i64,
}

/// Whether [`spawn_notifier`] should refuse to spawn at all -- see [`NOTIFIER_DISABLE_ENV`].
pub fn notifier_disabled() -> bool {
    std::env::var(NOTIFIER_DISABLE_ENV)
        .map(|v| v == "1")
        .unwrap_or(false)
}

/// Starts the daemon's background update check as a detached `tokio` task -- called once
/// from `agent::run`, right alongside its other one-time-at-startup `tokio::spawn` calls
/// (the control-socket accept loop, the OTLP/web servers). Nothing about this task is ever
/// awaited or joined by the caller (true of every long-lived task `agent::run` spawns: the
/// daemon's shutdown path tears down the control socket, OTLP, and web servers explicitly,
/// but this task has nothing to tear down -- it holds no listener, no socket, nothing that
/// would outlive the process in any way that matters), so a slow or unreachable GitHub API
/// can, at worst, make this task keep sleeping past process exit for the instant before the
/// runtime drops it -- it can never block ingestion, a flush, a control-socket reply, or
/// affect `kikimimi agent`'s exit at all. A no-op when [`notifier_disabled`] -- no task, no
/// request, ever, not even a delayed first one.
pub fn spawn_notifier() {
    if notifier_disabled() {
        return;
    }
    tokio::spawn(async {
        tokio::time::sleep(NOTIFIER_INITIAL_DELAY).await;
        loop {
            refresh_cache(&cache_path()).await;
            tokio::time::sleep(NOTIFIER_INTERVAL).await;
        }
    });
}

/// `kikimimi status`'s cache-only read (never a live request -- see the module docs): `None`
/// unless the cached `latest_version` parses as strictly newer than `current` (see
/// [`compare_versions`] for what "parses" covers). A missing cache, an unparseable one, or
/// one that's equal to or older than `current` all mean nothing to show, not an error --
/// `kikimimi status` must never fail or warn just because the daemon hasn't checked yet (or
/// ever, e.g. [`notifier_disabled`]).
pub fn available_update() -> Option<AvailableUpdate> {
    let cache = read_cache(&cache_path());
    available_update_from(cache.as_ref(), env!("CARGO_PKG_VERSION"))
}

/// The pure decision [`available_update`] boils down to -- split out so a test can supply a
/// fabricated [`Cache`] without touching the real filesystem.
fn available_update_from(cache: Option<&Cache>, current: &str) -> Option<AvailableUpdate> {
    let cache = cache?;
    match compare_versions(&cache.latest_version, current) {
        Some(std::cmp::Ordering::Greater) => Some(AvailableUpdate {
            latest_version: cache.latest_version.clone(),
        }),
        _ => None,
    }
}

/// `kikimimi status`'s exact notice line for an [`AvailableUpdate`] -- pulled out of
/// `status_cmd.rs` so the wording lives in one place and a test can check it without
/// capturing stdout.
pub fn status_notice(update: &AvailableUpdate) -> String {
    format!(
        "update available: v{} (run: kikimimi self-update)",
        update.latest_version
    )
}

// --- Background refresh -----------------------------------------------

/// The only network call [`spawn_notifier`]'s task ever makes. All failure -- a timeout, no
/// network, a malformed response, a write that couldn't land -- is silent by design (see the
/// module docs): this runs unattended on a detached task with no channel back to anything,
/// so there is nowhere to report an error *to* even if this raised one.
/// `checked_at_ms` is written unconditionally, success or failure, so a machine with no
/// network access gets retried once a day (per [`NOTIFIER_INTERVAL`]), not spammed with
/// requests, and not left permanently stuck retrying every few seconds either.
async fn refresh_cache(path: &Path) {
    let now = now_ms();
    // A failed fetch keeps whatever version was already cached (if any) rather than
    // blanking it out -- a transient network hiccup shouldn't make a real pending update
    // disappear from `kikimimi status`'s next read.
    let previous_latest = read_cache(path).map(|c| c.latest_version);
    let latest_version = fetch_latest_release_tag()
        .await
        .or(previous_latest)
        .unwrap_or_default();
    let cache = Cache {
        latest_version,
        checked_at_ms: now,
    };
    let _ = write_cache(path, &cache);
}

/// `GET`s the GitHub releases API for the pinned repo and pulls `tag_name` out of the JSON
/// body, stripping a leading `v` -- `None` on any failure at any step (client construction,
/// network, non-2xx, malformed JSON, missing/non-string field), collapsed into one outcome
/// since [`refresh_cache`] treats every failure identically. A `User-Agent` header is set
/// because GitHub's API rejects requests without one; the timeout keeps a slow/unreachable
/// API from leaving this task sleeping on a live request instead of its next scheduled
/// [`NOTIFIER_INTERVAL`] tick (harmless either way -- see [`spawn_notifier`]'s docs -- but
/// not free).
async fn fetch_latest_release_tag() -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(NOTIFIER_NETWORK_TIMEOUT)
        .build()
        .ok()?;
    let response = client
        .get(RELEASES_URL)
        .header(
            reqwest::header::USER_AGENT,
            format!("kikimimi/{}", env!("CARGO_PKG_VERSION")),
        )
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let value: serde_json::Value = response.json().await.ok()?;
    let tag = value.get("tag_name")?.as_str()?;
    Some(tag.trim_start_matches('v').to_owned())
}

// --- Version comparison -------------------------------------------------

/// Parses a `v`-prefixed-or-not `major.minor.patch` tag into a comparable triple, ignoring
/// anything after the patch number (a prerelease suffix like `-beta.1` or build metadata
/// like `+build5`) -- good enough for "is there a newer release," which is all this module
/// needs, without pulling in a full semver dependency for it (mirrors katamari's identical
/// choice, for the identical reason: a pinned tiny comparison function here, not a new
/// dependency). Anything that doesn't have at least three dot-separated, numeric-leading
/// components -- a malformed tag, a non-version string, an empty one -- yields `None` rather
/// than a wrong guess, so a botched release tag can only ever make this module do nothing,
/// never show a bogus "update available."
fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.strip_prefix('v').unwrap_or(s);
    let mut parts = s.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch_field = parts.next()?;
    let patch_digits: String = patch_field
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    if patch_digits.is_empty() {
        return None;
    }
    let patch = patch_digits.parse().ok()?;
    Some((major, minor, patch))
}

/// `None` if either side fails to parse (see [`parse_version`]) -- the caller
/// ([`available_update_from`]) treats that identically to "not newer," the only safe default
/// for a tag this module doesn't understand.
fn compare_versions(a: &str, b: &str) -> Option<std::cmp::Ordering> {
    Some(parse_version(a)?.cmp(&parse_version(b)?))
}

// --- Install receipt detection ------------------------------------------

/// The candidate receipt *directories* for `app_name`, in the exact order and with the exact
/// fallback rule `axoupdater`'s own lookup uses (`axoupdater::receipt::get_config_paths`,
/// mirrored here by hand rather than called into, so `self_update_cmd.rs` can decide *whether*
/// a receipt-managed self-update is even possible -- a cheap fs check -- before constructing
/// an `AxoUpdater` at all).
///
/// `axoupdater` checks two internal env-var overrides *first*, and when either is set it
/// short-circuits to that single candidate, skipping XDG/HOME entirely (its own test harness
/// -- `axoupdater::test::helpers` -- relies on exactly this to sandbox receipt lookups away
/// from a real `$HOME`): `working_dir_override` is `Some` iff `AXOUPDATER_CONFIG_WORKING_DIR`
/// is set (to *any* value, per `axoupdater`'s own `env::var(..).is_ok()` check) and carries
/// the process's actual current directory (what `axoupdater` resolves that override *to*,
/// not the env var's own value); `config_path_override` is `Some` iff `AXOUPDATER_CONFIG_PATH`
/// is set and carries that env var's value verbatim, mirroring `axoupdater` taking it
/// literally with no existence check. Getting this wrong would make this pre-check a false
/// "no receipt" (or false "receipt found," pointed at the wrong file) in exactly the
/// situation `axoupdater`'s own `load_receipt` would resolve differently -- silently steering
/// a real receipt-managed install onto the curl-installer fallback instead of updating it, or
/// the reverse.
///
/// Absent both overrides, falls back to the ordinary XDG/HOME rule: `$XDG_CONFIG_HOME/<app_name>`
/// is a candidate *only* when that directory already exists -- an XDG base that's configured
/// but empty doesn't count -- and `$HOME/.config/<app_name>` is *always* also a candidate,
/// appended unconditionally, even when `XDG_CONFIG_HOME` is set to somewhere else entirely.
/// That "always also," not "else," matters: picking exactly one location based on whether
/// `XDG_CONFIG_HOME` happened to be set would mean a real receipt sitting at the
/// `$HOME/.config` fallback goes unseen whenever `XDG_CONFIG_HOME` points at some other,
/// receipt-less directory -- a false "no receipt" that `axoupdater`'s own `load_receipt`
/// wouldn't produce for the identical filesystem state.
fn receipt_dir_candidates(
    app_name: &str,
    xdg_config_home: Option<&Path>,
    home: Option<&Path>,
    working_dir_override: Option<&Path>,
    config_path_override: Option<&Path>,
) -> Vec<PathBuf> {
    if let Some(cwd) = working_dir_override {
        return vec![cwd.to_path_buf()];
    }
    if let Some(path) = config_path_override {
        return vec![path.to_path_buf()];
    }
    let mut candidates = Vec::new();
    if let Some(dir) = xdg_config_home {
        if !dir.as_os_str().is_empty() {
            let xdg_app_dir = dir.join(app_name);
            if xdg_app_dir.exists() {
                candidates.push(xdg_app_dir);
            }
        }
    }
    if let Some(home) = home {
        candidates.push(home.join(".config").join(app_name));
    }
    candidates
}

/// Whether a cargo-dist install receipt for `app_name` exists in any of
/// [`receipt_dir_candidates`] -- the actual I/O in this pair (plain `Path::exists` calls, no
/// network, aside from the one `std::env::current_dir` call folded in here for
/// [`receipt_dir_candidates`]'s `working_dir_override`). Doesn't parse or validate the
/// receipt's contents -- that's `axoupdater`'s `load_receipt`'s job, in `self_update_cmd.rs`
/// itself; a corrupt receipt here still means "try the receipt path," and that command will
/// report the real reason it can't proceed.
pub(crate) fn has_install_receipt(
    app_name: &str,
    xdg_config_home: Option<&Path>,
    home: Option<&Path>,
) -> bool {
    let working_dir_override = std::env::var_os("AXOUPDATER_CONFIG_WORKING_DIR")
        .is_some()
        .then(|| std::env::current_dir().ok())
        .flatten();
    let config_path_override = std::env::var_os("AXOUPDATER_CONFIG_PATH").map(PathBuf::from);
    receipt_dir_candidates(
        app_name,
        xdg_config_home,
        home,
        working_dir_override.as_deref(),
        config_path_override.as_deref(),
    )
    .into_iter()
    .any(|dir| dir.join(format!("{app_name}-receipt.json")).exists())
}

// --- Homebrew/Linuxbrew detection ---------------------------------------

/// Whether `exe_path` looks like a Homebrew- or Linuxbrew-managed install. Homebrew owns a
/// binary under either prefix outright -- its own `brew upgrade` replaces it directly, and
/// nothing else installs into a Cellar path -- so `self_update_cmd.rs` must send a
/// brew-managed install to [`BREW_UPGRADE_COMMAND`] unconditionally, without even checking
/// for a receipt first (a receipt can never legitimately coexist with a Cellar path, but
/// checking cheaply-first-and-correctly beats relying on that never happening). Extends
/// katamari's `/Cellar/`-only check (macOS Homebrew) with `/linuxbrew/`, since kikimimi ships
/// Linux builds too (`dist-workspace.toml`'s `*-unknown-linux-*` targets) and Linuxbrew
/// installs under `/home/linuxbrew/.linuxbrew` or `/opt/linuxbrew`, both containing
/// `/linuxbrew/`.
pub(crate) fn is_brew_managed(exe_path: &Path) -> bool {
    let s = exe_path.to_string_lossy();
    s.contains("/Cellar/") || s.contains("/linuxbrew/")
}

// --- pid liveness / stop -------------------------------------------------

/// Whether a process with this pid currently exists, via a signal-0 probe (`kill(pid, 0)`
/// sends no actual signal, just checks permission + existence -- POSIX-guaranteed to error
/// `ESRCH` for a pid that doesn't exist). `false` for pid `0` (which would probe the caller's
/// own process group, not a real daemon, per `kill(2)`'s special-case semantics for pid 0)
/// as well as for any pid this process can't signal at all (already exited, or -- far less
/// likely for a `state.json` this same user's `kikimimi agent` wrote -- owned by another
/// user).
pub(crate) fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// Sends `SIGTERM` to `pid` (the same signal `kikimimi agent`'s own graceful-shutdown path
/// already handles -- `agent.rs`'s `sigterm.recv()` arm, which does a final drain + flush
/// before exiting) and polls [`pid_alive`] until it exits or `timeout` elapses. A pid still
/// alive past the timeout gets one `SIGKILL` as a last resort, so a wedged daemon can never
/// leave `kikimimi self-update` hanging indefinitely, or -- worse -- end up with a second
/// daemon spawned alongside a first that never actually died and is still holding the
/// control socket and OTLP/web ports.
pub(crate) fn kill_and_wait(pid: u32, timeout: Duration) -> std::io::Result<()> {
    if unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) } != 0 {
        // Already gone (a race with the process exiting on its own right as this
        // ran) -- nothing left to wait for.
        return Ok(());
    }
    let deadline = std::time::Instant::now() + timeout;
    while pid_alive(pid) {
        if std::time::Instant::now() >= deadline {
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
            std::thread::sleep(Duration::from_millis(100));
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

// --- Cache I/O -------------------------------------------------------------

/// `None` for a missing file (never checked yet) and, deliberately, for one that fails to
/// parse -- this state file is disposable (safe to delete any time, at worst costing one
/// redundant network round trip on the next check), so a corrupt one degrades to "never
/// checked" instead of erroring, the same graceful-degradation rule `config.rs`'s
/// `KikimimiConfig::load` applies to a bad `config.json`.
fn read_cache(path: &Path) -> Option<Cache> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_cache(path: &Path, cache: &Cache) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(cache)?;
    crate::state::write_atomic(path, &bytes)
}

fn cache_path() -> PathBuf {
    kikimimi_schema::paths::kikimimi_dir().join("update-check.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    // --- version comparison ---------------------------------------------

    #[test]
    fn newer_remote_version_compares_greater() {
        assert_eq!(compare_versions("1.3.0", "1.2.9"), Some(Ordering::Greater));
    }

    #[test]
    fn older_remote_version_compares_less() {
        assert_eq!(compare_versions("1.0.0", "1.2.0"), Some(Ordering::Less));
    }

    #[test]
    fn equal_versions_compare_equal() {
        assert_eq!(compare_versions("2.4.1", "2.4.1"), Some(Ordering::Equal));
    }

    #[test]
    fn a_leading_v_is_stripped_on_either_side() {
        assert_eq!(compare_versions("v1.3.0", "1.2.9"), Some(Ordering::Greater));
        assert_eq!(compare_versions("1.3.0", "v1.2.9"), Some(Ordering::Greater));
    }

    #[test]
    fn prerelease_suffixes_are_ignored_past_the_patch_number() {
        assert_eq!(
            compare_versions("1.3.0-beta.1", "1.2.9"),
            Some(Ordering::Greater)
        );
    }

    #[test]
    fn malformed_tags_are_ignored_rather_than_mis_compared() {
        assert_eq!(compare_versions("not-a-version", "1.2.9"), None);
        assert_eq!(compare_versions("1.2.9", "not-a-version"), None);
        assert_eq!(compare_versions("1.2", "1.2.9"), None, "missing patch");
        assert_eq!(compare_versions("", "1.2.9"), None);
    }

    // --- available_update_from (display-gating pure decision) -----------------

    fn cache(latest_version: &str) -> Cache {
        Cache {
            latest_version: latest_version.to_owned(),
            checked_at_ms: 0,
        }
    }

    #[test]
    fn no_cache_means_no_notice() {
        assert_eq!(available_update_from(None, "1.0.0"), None);
    }

    #[test]
    fn a_newer_cached_version_produces_a_notice() {
        assert_eq!(
            available_update_from(Some(&cache("2.0.0")), "1.0.0"),
            Some(AvailableUpdate {
                latest_version: "2.0.0".to_owned()
            })
        );
    }

    #[test]
    fn an_equal_or_older_cached_version_produces_no_notice() {
        assert_eq!(available_update_from(Some(&cache("1.0.0")), "1.0.0"), None);
        assert_eq!(available_update_from(Some(&cache("0.9.0")), "1.0.0"), None);
    }

    #[test]
    fn a_malformed_cached_version_produces_no_notice() {
        assert_eq!(
            available_update_from(Some(&cache("garbage")), "1.0.0"),
            None
        );
    }

    #[test]
    fn status_notice_names_the_version_and_the_command() {
        let text = status_notice(&AvailableUpdate {
            latest_version: "9.9.9".to_owned(),
        });
        assert_eq!(text, "update available: v9.9.9 (run: kikimimi self-update)");
    }

    // --- install receipt detection ------------------------------------------

    fn fixture_dir() -> PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!("kikimimi-update-test-{}-{n}", std::process::id()))
    }

    #[test]
    fn receipt_dir_candidates_skips_a_missing_xdg_app_dir_but_still_offers_home() {
        let xdg = fixture_dir();
        std::fs::create_dir_all(&xdg).unwrap();
        let home = Path::new("/home/someone");
        assert_eq!(
            receipt_dir_candidates("kikimimi", Some(&xdg), Some(home), None, None),
            vec![home.join(".config").join("kikimimi")],
        );
    }

    #[test]
    fn receipt_dir_candidates_includes_xdg_first_when_its_app_dir_exists() {
        let xdg = fixture_dir();
        let xdg_app_dir = xdg.join("kikimimi");
        std::fs::create_dir_all(&xdg_app_dir).unwrap();
        let home = Path::new("/home/someone");
        assert_eq!(
            receipt_dir_candidates("kikimimi", Some(&xdg), Some(home), None, None),
            vec![xdg_app_dir, home.join(".config").join("kikimimi")],
        );
    }

    #[test]
    fn receipt_dir_candidates_falls_back_to_home_config_when_xdg_unset() {
        let home = Path::new("/home/someone");
        assert_eq!(
            receipt_dir_candidates("kikimimi", None, Some(home), None, None),
            vec![home.join(".config").join("kikimimi")],
        );
    }

    #[test]
    fn receipt_dir_candidates_ignores_an_empty_xdg_config_home() {
        let home = Path::new("/home/someone");
        assert_eq!(
            receipt_dir_candidates("kikimimi", Some(Path::new("")), Some(home), None, None),
            vec![home.join(".config").join("kikimimi")],
        );
    }

    #[test]
    fn receipt_dir_candidates_is_empty_without_xdg_or_home() {
        assert_eq!(
            receipt_dir_candidates("kikimimi", None, None, None, None),
            Vec::<PathBuf>::new(),
        );
    }

    // --- receipt_dir_candidates: axoupdater's own env-var overrides -------------

    #[test]
    fn receipt_dir_candidates_uses_only_the_working_dir_override_when_set() {
        // Mirrors axoupdater::receipt::get_config_paths short-circuiting on
        // AXOUPDATER_CONFIG_WORKING_DIR: XDG/HOME must be ignored entirely, not merely
        // deprioritized, once this override is present.
        let xdg = fixture_dir();
        std::fs::create_dir_all(&xdg).unwrap();
        let home = Path::new("/home/someone");
        let cwd = Path::new("/some/working/dir");
        assert_eq!(
            receipt_dir_candidates("kikimimi", Some(&xdg), Some(home), Some(cwd), None),
            vec![cwd.to_path_buf()],
        );
    }

    #[test]
    fn receipt_dir_candidates_uses_only_the_config_path_override_when_set() {
        // Same short-circuit for AXOUPDATER_CONFIG_PATH -- and it wins over the working-dir
        // override too, matching axoupdater's own if/else-if order (working dir checked
        // first, but a real process only ever has at most one of the two set).
        let home = Path::new("/home/someone");
        let config_path = Path::new("/explicit/receipt/dir");
        assert_eq!(
            receipt_dir_candidates("kikimimi", None, Some(home), None, Some(config_path)),
            vec![config_path.to_path_buf()],
        );
    }

    // --- has_install_receipt: the real env vars, end to end ---------------------

    #[test]
    #[serial_test::serial]
    fn has_install_receipt_honors_the_working_dir_override_env_var() {
        // A receipt that only exists under the ordinary XDG/HOME candidates must be invisible
        // once AXOUPDATER_CONFIG_WORKING_DIR is set -- axoupdater's own load_receipt would
        // look only at the current directory in that case, and this pre-check has to agree
        // or it can wrongly tell the user "no receipt" (or the reverse) for an install
        // load_receipt would actually resolve differently.
        std::env::remove_var("AXOUPDATER_CONFIG_WORKING_DIR");
        std::env::remove_var("AXOUPDATER_CONFIG_PATH");
        let xdg = fixture_dir();
        let dir = xdg.join("kikimimi");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("kikimimi-receipt.json"), "{}").unwrap();
        assert!(has_install_receipt("kikimimi", Some(&xdg), None));

        std::env::set_var("AXOUPDATER_CONFIG_WORKING_DIR", "1");
        assert!(!has_install_receipt("kikimimi", Some(&xdg), None));

        std::env::remove_var("AXOUPDATER_CONFIG_WORKING_DIR");
    }

    #[test]
    #[serial_test::serial]
    fn has_install_receipt_honors_the_config_path_override_env_var() {
        std::env::remove_var("AXOUPDATER_CONFIG_WORKING_DIR");
        std::env::remove_var("AXOUPDATER_CONFIG_PATH");
        let xdg = fixture_dir();
        let dir = xdg.join("kikimimi");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("kikimimi-receipt.json"), "{}").unwrap();

        // Pointed at a directory with no receipt in it: must not fall back to XDG/HOME.
        let elsewhere = fixture_dir();
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::env::set_var("AXOUPDATER_CONFIG_PATH", &elsewhere);
        assert!(!has_install_receipt("kikimimi", Some(&xdg), None));

        // Pointed at a directory that does have one: found there instead.
        std::fs::write(elsewhere.join("kikimimi-receipt.json"), "{}").unwrap();
        assert!(has_install_receipt("kikimimi", Some(&xdg), None));

        std::env::remove_var("AXOUPDATER_CONFIG_PATH");
    }

    #[test]
    fn has_install_receipt_is_false_when_the_file_is_missing() {
        let xdg = fixture_dir();
        assert!(!has_install_receipt("kikimimi", Some(&xdg), None));
    }

    #[test]
    fn has_install_receipt_is_true_when_the_file_exists_under_xdg() {
        let xdg = fixture_dir();
        let dir = xdg.join("kikimimi");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("kikimimi-receipt.json"), "{}").unwrap();
        assert!(has_install_receipt("kikimimi", Some(&xdg), None));
    }

    #[test]
    fn has_install_receipt_is_true_via_the_home_fallback_even_with_xdg_config_home_set() {
        // A real receipt sits only at `$HOME/.config/<app>`, while `XDG_CONFIG_HOME` is set
        // to some other, existing-but-unrelated directory. `axoupdater`'s own lookup still
        // finds it via the unconditional home fallback; this must too.
        let xdg = fixture_dir();
        std::fs::create_dir_all(&xdg).unwrap();
        let home = fixture_dir();
        let home_app_dir = home.join(".config").join("kikimimi");
        std::fs::create_dir_all(&home_app_dir).unwrap();
        std::fs::write(home_app_dir.join("kikimimi-receipt.json"), "{}").unwrap();
        assert!(has_install_receipt("kikimimi", Some(&xdg), Some(&home)));
    }

    // --- brew-path classifier -------------------------------------------------

    #[test]
    fn a_macos_cellar_path_is_brew_managed() {
        assert!(is_brew_managed(Path::new(
            "/opt/homebrew/Cellar/kikimimi/0.1.0/bin/kikimimi"
        )));
    }

    #[test]
    fn a_linuxbrew_path_is_brew_managed() {
        assert!(is_brew_managed(Path::new(
            "/home/linuxbrew/.linuxbrew/Cellar/kikimimi/0.1.0/bin/kikimimi"
        )));
        assert!(is_brew_managed(Path::new("/opt/linuxbrew/bin/kikimimi")));
    }

    #[test]
    fn a_cargo_bin_path_is_not_brew_managed() {
        assert!(!is_brew_managed(Path::new(
            "/home/someone/.cargo/bin/kikimimi"
        )));
    }

    #[test]
    fn a_plain_usr_local_bin_path_is_not_brew_managed() {
        assert!(!is_brew_managed(Path::new("/usr/local/bin/kikimimi")));
    }

    // --- cache read/write round trip ----------------------------------------

    fn fixture_cache_path() -> PathBuf {
        fixture_dir().join("update-check.json")
    }

    #[test]
    fn write_then_read_round_trips() {
        let path = fixture_cache_path();
        let written = Cache {
            latest_version: "9.9.9".to_owned(),
            checked_at_ms: 12_345,
        };
        write_cache(&path, &written).unwrap();
        assert_eq!(read_cache(&path), Some(written));
    }

    #[test]
    fn read_cache_is_none_for_a_missing_file() {
        assert_eq!(read_cache(&fixture_cache_path()), None);
    }

    #[test]
    fn read_cache_is_none_for_a_malformed_file() {
        let path = fixture_cache_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(read_cache(&path), None);
    }

    #[test]
    fn write_cache_overwrites_a_previous_value() {
        let path = fixture_cache_path();
        write_cache(
            &path,
            &Cache {
                latest_version: "1.0.0".to_owned(),
                checked_at_ms: 1,
            },
        )
        .unwrap();
        write_cache(
            &path,
            &Cache {
                latest_version: "2.0.0".to_owned(),
                checked_at_ms: 2,
            },
        )
        .unwrap();
        assert_eq!(
            read_cache(&path),
            Some(Cache {
                latest_version: "2.0.0".to_owned(),
                checked_at_ms: 2,
            })
        );
    }

    // --- notifier disable env -------------------------------------------------

    #[test]
    #[serial_test::serial]
    fn notifier_disabled_reads_the_env_var() {
        std::env::remove_var(NOTIFIER_DISABLE_ENV);
        assert!(!notifier_disabled());

        std::env::set_var(NOTIFIER_DISABLE_ENV, "1");
        assert!(notifier_disabled());

        // Only "1" counts, matching this codebase's other boolean env vars
        // (crates/cloud/src/config.rs's KIKIMIMI_DEV_AUTOAPPROVE) -- an unrelated non-empty
        // value must not accidentally disable the check.
        std::env::set_var(NOTIFIER_DISABLE_ENV, "true");
        assert!(!notifier_disabled());

        std::env::remove_var(NOTIFIER_DISABLE_ENV);
    }

    #[tokio::test]
    async fn spawn_notifier_spawns_nothing_when_disabled() {
        // No network, no file I/O should even happen -- there is nothing to directly assert
        // on a fire-and-forget spawn, so this pins the one thing that *is* observable
        // without a network mock: calling it under the disable env must not panic and must
        // return immediately without ever reaching `tokio::spawn`. (A real spawn, if this
        // guard were missing, would itself be harmless in a test -- it just sleeps -- but
        // proving *no* task starts here is what matters for the "complete opt-out" promise.)
        std::env::set_var(NOTIFIER_DISABLE_ENV, "1");
        spawn_notifier();
        std::env::remove_var(NOTIFIER_DISABLE_ENV);
    }

    // --- pid liveness / stop --------------------------------------------------

    #[test]
    fn pid_alive_is_true_for_the_current_process() {
        assert!(pid_alive(std::process::id()));
    }

    #[test]
    fn pid_alive_is_false_for_pid_zero() {
        assert!(!pid_alive(0));
    }

    #[test]
    fn pid_alive_is_false_once_a_child_has_exited_and_been_reaped() {
        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("spawning `true`");
        let pid = child.id();
        child.wait().expect("waiting for `true` to exit");
        assert!(!pid_alive(pid));
    }

    #[test]
    fn kill_and_wait_returns_promptly_for_an_already_dead_pid() {
        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("spawning `true`");
        let pid = child.id();
        child.wait().expect("waiting for `true` to exit");
        // Must not hang for the full timeout when the pid is already gone.
        kill_and_wait(pid, Duration::from_secs(5)).unwrap();
    }

    #[test]
    fn kill_and_wait_terminates_a_live_process_within_the_timeout() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawning `sleep 30`");
        let pid = child.id();
        assert!(pid_alive(pid));
        // Reap it on a background thread the moment it exits -- the same way `init` reaps
        // kikimimi's real daemon (never `kikimimi self-update`'s own child; see the "pid
        // liveness / stop" section of the module docs on why that's true in production).
        // Without this, `kill(pid, 0)` keeps reporting a zombie as "alive" (POSIX: a pid
        // stays valid for `kill` until its parent reaps it) until *this test itself* calls
        // `wait()`, which would defeat the point of asserting death within the timeout.
        let reaper = std::thread::spawn(move || {
            let _ = child.wait();
        });
        kill_and_wait(pid, Duration::from_secs(5)).unwrap();
        assert!(!pid_alive(pid));
        reaper.join().unwrap();
    }
}
