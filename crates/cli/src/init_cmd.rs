//! `kikimimi init` / `kikimimi uninstall` — architecture.md §4.2, §12 Stage 0。
//!
//! `~/.claude/settings.json` に hooks / env を書く (init) / 消す (uninstall)。
//! 冪等: 既に "kikimimi hook" で始まる command があるイベントには足さない。
//! ユーザー自身の既存 hooks は絶対に削除・並べ替えしない。

use std::fs;

use anyhow::Context;
use serde_json::Value;

use crate::claude_settings as cs;

pub fn init(dry_run: bool, no_service: bool) -> anyhow::Result<()> {
    let path = cs::settings_path();
    let existed = path.exists();
    let mut value = cs::load_settings(&path)?;
    if !value.is_object() {
        anyhow::bail!(
            "{} does not contain a JSON object at the root; refusing to touch it",
            path.display()
        );
    }

    let mut messages = Vec::new();

    for (event, timeout) in cs::HOOK_EVENTS {
        if cs::has_kikimimi_hook(&value, event) {
            messages.push(format!(
                "hooks.{event}: already has a \"kikimimi hook\" entry, skipping"
            ));
            continue;
        }
        // guru → kikimimi upgrade path: a legacy "guru hook" entry from an older
        // install gets replaced (not left alongside) by the new "kikimimi hook" one
        // added just below -- otherwise every hook would fire twice.
        if cs::has_legacy_guru_hook(&value, event) {
            if let Some(arr) = value
                .pointer_mut(&format!("/hooks/{event}"))
                .and_then(Value::as_array_mut)
            {
                let before = arr.len();
                arr.retain(|entry| !cs::entry_has_legacy_guru_hook(entry));
                let removed = before - arr.len();
                messages.push(format!(
                    "hooks.{event}: removed {removed} legacy \"guru hook\" entry(ies) (upgrading to kikimimi)"
                ));
            }
        }
        cs::add_hook_entry(&mut value, event, *timeout)?;
        messages.push(format!(
            "hooks.{event}: added \"kikimimi hook {event}\" (timeout {timeout}s)"
        ));
    }

    // architecture.md §4 「OTLP レシーバ」: "kikimimi init はポート使用状況を検査し、衝突時は
    // 別ポートを選んで影響する全エージェント設定を一括更新する". An explicit
    // KIKIMIMI_OTLP_PORT (or legacy GURU_OTLP_PORT) override is always honored verbatim
    // (used by tests/smoke.sh and by operators who already know which port they want);
    // otherwise probe the currently preferred port (a prior `kikimimi init`'s choice, or
    // the 4318 default) and, if it's occupied, pick a free one instead.
    //
    // The probe must not mistake kikimimi's *own* running daemon for a foreign occupant:
    // a 0.4.x-era `kikimimi agent &` (or the already-installed service) is still bound to
    // `preferred` at this point -- `report_service_install` below stops the manual one only
    // *after* the settings write -- so a naive bind test would always report "in use",
    // persist a random alternate port, and (because `OTEL_EXPORTER_OTLP_ENDPOINT` is left
    // alone when it differs) leave Claude Code exporting to a port nobody listens on. When
    // the daemon behind the control socket reports the same port in `state.json`, that
    // port is kikimimi's to keep.
    let preferred = crate::config::resolve_otlp_port();
    let port =
        if crate::config::otlp_port_env_override().is_some() || own_daemon_holds_port(preferred) {
            preferred
        } else {
            kikimimi_otlp::pick_port(preferred)
        };
    if port != preferred {
        messages.push(format!(
            "WARNING otlp: port {preferred} is already in use; selected alternate port {port} instead (kikimimi agent will use it too)"
        ));
    }

    // OTLP レシーバの per-install bearer token (§4「認証」): 既存の config.json に
    // otlp_token があればそれを使い回す (既に動いている Claude Code セッションは古い env
    // のまま生き続けるので、re-run のたびに新しいトークンを発行すると、そのセッションの
    // OTel テレメトリだけがずっと 401 で拒否され続けてしまう -- 設計上、絶対に再発行しない)。
    // 無ければここで一度だけ発行し、下の cfg.save() で port と一緒に永続化する。
    // `crate::web::generate_local_token` (ローカル web UI トークンと同じ CSPRNG) を再利用する。
    let mut cfg = crate::config::KikimimiConfig::load();
    let otlp_token = cfg
        .otlp_token
        .clone()
        .unwrap_or_else(crate::web::generate_local_token);
    // When the port changes (a foreign process took the previous one for good), the
    // endpoint a *previous* `kikimimi init` wrote for the previous port is kikimimi's own
    // value, not a user customization -- leaving it "unchanged" would keep Claude Code
    // exporting to the old port. Only that exact stale value is rewritten; anything else a
    // user put there is still reported and left alone.
    let stale_endpoint = cs::previous_endpoint(cfg.otlp_port, port);
    for (key, expected) in cs::expected_env(port, Some(&otlp_token)) {
        match value
            .pointer(&format!("/env/{key}"))
            .and_then(Value::as_str)
            .map(str::to_owned)
        {
            Some(current) if current == expected => {
                messages.push(format!("env.{key}: already set correctly"));
            }
            Some(current)
                if key == "OTEL_EXPORTER_OTLP_ENDPOINT"
                    && stale_endpoint.as_deref() == Some(current.as_str()) =>
            {
                cs::set_env(&mut value, key, &expected)?;
                messages.push(format!(
                    "env.{key}: updated from {current:?} to {expected:?} (OTLP port changed)"
                ));
            }
            Some(current) => {
                messages.push(format!(
                    "WARNING env.{key}: existing value {current:?} differs from kikimimi's {expected:?}; leaving unchanged"
                ));
            }
            None => {
                cs::set_env(&mut value, key, &expected)?;
                messages.push(format!("env.{key}: set to {expected:?}"));
            }
        }
    }

    report_codex(&mut messages);
    report_duckdb(&mut messages);

    for m in &messages {
        println!("{m}");
    }

    if dry_run {
        println!(
            "\n--dry-run: no changes written. Resulting {}:",
            path.display()
        );
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    if existed {
        let backup = cs::backup_path(&path);
        if !backup.exists() {
            fs::copy(&path, &backup).with_context(|| {
                format!("backing up {} to {}", path.display(), backup.display())
            })?;
            println!("backed up existing settings to {}", backup.display());
        }
    }

    cs::write_settings_atomic(&path, &value)?;
    println!("wrote {}", path.display());

    // Persist the chosen port (and the otlp bearer token computed above, freshly generated
    // or reused) so `kikimimi agent` binds the same port and honors the same token next time
    // it starts (without this, agent would default back to port 4318 / no auth). `cfg` was
    // loaded once, up front (load-modify-save), so this never clobbers a saved cloud token (§6)
    // or any other field set since then.
    cfg.otlp_port = Some(port);
    cfg.otlp_token = Some(otlp_token);
    cfg.save().context("saving config.json")?;

    // Best-effort: wake a running daemon so it picks up the (possibly just-generated) token
    // without a restart (agent.rs's 'r' control byte reload, same one `kikimimi sink add s3`/
    // `kikimimi repos allow` already use). No daemon listening yet (first-ever `kikimimi
    // init`) is the common, harmless case -- `send_control` just returns false then, and the
    // token is picked up naturally on the next `kikimimi agent` start (it loads config.json
    // fresh at startup).
    kikimimi_spool::send_control(b'r');

    if no_service {
        println!("service: skipped (--no-service)");
    } else {
        report_service_install();
    }

    Ok(())
}

/// Daemon survives reboots/crashes: after writing hooks/env, register `kikimimi
/// agent` as a user-level service (macOS LaunchAgent / Linux systemd --user; see
/// `crate::service`) so it starts at login and restarts itself after a crash, without anyone
/// needing to remember `kikimimi agent &`. Fail-open (architecture.md §2.2): whatever happens
/// here is only ever *reported*, never turned into an `init` failure -- a service-manager
/// quirk on one machine must never undo the hooks/env write that already succeeded above.
fn report_service_install() {
    // A daemon started by hand (`kikimimi agent &`, the pre-0.5 Quickstart) may already be
    // holding the control socket. `service::install` never kills anything, and a service
    // started next to that process would only crash-loop on the liveness check (agent.rs)
    // until it exits. So when nothing is registered with the service manager yet, stop the
    // manual process first (the same SIGTERM + bounded wait `kikimimi self-update` uses) and
    // let the service take over. When the running daemon *is* the service, leave it alone.
    if kikimimi_spool::send_control(b'n') && !crate::service::status().installed {
        match stop_manual_daemon() {
            Ok(Some(pid)) => println!(
                "service: stopped the manually started kikimimi agent (pid {pid}); the service \
                 takes over from here"
            ),
            Ok(None) => println!(
                "service: a kikimimi agent process is already running outside the service and \
                 state.json does not name its pid -- the service will take over once that \
                 process exits (it is not being killed)"
            ),
            Err(e) => println!(
                "WARNING service: could not stop the manually started kikimimi agent ({e:#}) -- \
                 the service will take over once that process exits"
            ),
        }
    }

    let outcome = crate::service::install();
    let prefix = if outcome.is_failure() {
        "WARNING "
    } else if outcome.is_not_supported() {
        "NOTE "
    } else {
        ""
    };
    println!("{prefix}service: {}", outcome.summary());
}

/// Whether the process currently bound to `preferred` is kikimimi's own daemon: something
/// answers on the control socket *and* the `state.json` that daemon maintains names
/// `preferred` as its OTLP port. Either signal alone is not enough -- a stale `state.json`
/// outlives a crashed daemon (then the port is genuinely free or foreign, and the plain
/// probe decides), and a live daemon on some *other* port says nothing about who holds this
/// one.
fn own_daemon_holds_port(preferred: u16) -> bool {
    kikimimi_spool::send_control(b'n')
        && crate::state::load_opt(&kikimimi_schema::paths::state_path())
            .is_some_and(|state| state.otlp_port == preferred)
}

/// SIGTERM the daemon whose pid `state.json` records and wait (bounded) for it to exit.
/// `Ok(None)` when there is no readable `state.json` or its pid is already dead -- the
/// caller then falls back to "not killed, the service takes over later".
fn stop_manual_daemon() -> anyhow::Result<Option<u32>> {
    let Some(state) = crate::state::load_opt(&kikimimi_schema::paths::state_path()) else {
        return Ok(None);
    };
    if !crate::update::pid_alive(state.pid) {
        return Ok(None);
    }
    crate::update::kill_and_wait(state.pid, crate::update::DAEMON_STOP_TIMEOUT)
        .map_err(|e| anyhow::anyhow!("stopping pid {}: {e}", state.pid))?;
    Ok(Some(state.pid))
}

/// architecture.md §4.1 Codex 行: `~/.codex` を検出したときの案内メッセージを追加する。
/// `~/.codex` が無ければ何もしない (Codex 未インストールは正常系。メッセージも出さない —
/// 使っていないエージェントについて毎回ノイズを出さない)。
///
/// **意図的に Codex 側の設定ファイル (`config.toml`) には一切書き込まない** — このマシンで
/// `codex --help` / `codex doctor` / `codex features list` を調べた限り、hooks は
/// stable 機能として有効だが (`hooks  stable  true`)、hooks/`[otel]` 用の
/// `config.toml` の具体的なキー名/テーブル構造をドキュメント/`--help` 出力から確証できな
/// かった (`codex features list` に hooks 有効の表示はあっても、hooks を設定する TOML
/// キーそのものの一覧は出てこない。バイナリの文字列解析からは `HooksToml`/`HooksFile`
/// という Rust 型が存在すること、hooks に「trust」という別の承認機構があり
/// `--dangerously-bypass-hook-trust` 無しでは対話的な承認が要ることまでは分かったが、
/// 正しいキー名を書ける確信が持てなかった)。誤ったキーを書き込んで `config.toml` を
/// 壊す/静かに無視されるよりは、**何も書かず rollout tailer だけに頼る**方を選ぶ
/// (タスク指示の「hooks config unsupported in installed version の場合は rely on
/// rollout tailer only」に従う判断)。`kikimimi agent` の Codex rollout tailer は
/// この設定と無関係に動くので、収集自体は `kikimimi init` を待たずに機能する。
fn report_codex(messages: &mut Vec<String>) {
    let codex_home = kikimimi_schema::paths::codex_home_dir();
    if !codex_home.exists() {
        return;
    }
    messages.push(format!("codex: {} detected", codex_home.display()));
    messages.push(format!(
        "codex hooks/[otel]: not written to config.toml (Stage 0 — the installed codex-cli's \
         hooks/[otel] config.toml schema could not be verified via `codex --help`/`codex \
         doctor` on this machine; kikimimi relies on the rollout tailer \
         ({}/**/rollout-*.jsonl) instead, which `kikimimi agent` starts automatically -- \
         no config.toml write needed for it to work)",
        kikimimi_schema::paths::codex_sessions_dir().display()
    ));
}

/// The duckdb CLI powers `kikimimi query` and the local dashboard's
/// `/web/q/*` widgets (see `crate::web_query`), but `init` writes hooks/OTel
/// env only -- it never needed duckdb before now. Surface that dependency
/// once, right here, instead of leaving it to be discovered later as a
/// `kikimimi query`/`kikimimi web` failure or a `kikimimi status` warning
/// (`status_cmd::run`, which checks the same `web_query::duckdb_available()`).
/// Silent when duckdb is present -- `init`'s output should stay quiet when
/// there's nothing to report.
const DUCKDB_MISSING_MESSAGE: &str = "NOTE duckdb: not found on PATH -- `kikimimi query` and the \
     `kikimimi web` dashboard need the duckdb CLI (brew install duckdb, or https://duckdb.org). \
     Hooks, the daemon and cloud sync work without it.";

fn report_duckdb(messages: &mut Vec<String>) {
    if !crate::web_query::duckdb_available() {
        messages.push(DUCKDB_MISSING_MESSAGE.to_string());
    }
}

pub fn uninstall(purge_data: bool) -> anyhow::Result<()> {
    uninstall_impl(purge_data, false)
}

/// Shared by the public `uninstall()` and the test suite. `skip_service`, mirroring `init()`'s
/// `no_service`, exists purely so tests never touch the real service manager: unlike `init()`'s
/// `service::install()` (a no-op-ish "register alongside whatever's running" call), plain
/// `service::uninstall()` is destructive -- on any machine that actually has a real user-level
/// service installed (i.e. any machine where `kikimimi init` was ever run for real, which is
/// exactly the kind of machine a developer iterating on this code is likely to be using),
/// calling it here would run `launchctl bootout`/`systemctl --user disable --now` against that
/// real installed service and delete its unit file. `service::*_path_in()` reads `$HOME`
/// directly and isn't redirected by `with_settings_path`'s `KIKIMIMI_CLAUDE_SETTINGS_PATH`/
/// `KIKIMIMI_DIR` env overrides, so there is no other way to make this safe for tests.
fn uninstall_impl(purge_data: bool, skip_service: bool) -> anyhow::Result<()> {
    // Service registration first, and reported rather than propagated: whatever happened to
    // it (removed fine, was never installed, this OS/host doesn't support one, or the service
    // manager itself errored) must never block the settings.json cleanup below -- that cleanup
    // is the part `kikimimi uninstall`'s contract (its own doc comment, and `README.md`)
    // actually promises.
    if skip_service {
        println!("service: skipped (test)");
    } else {
        let service_outcome = crate::service::uninstall();
        println!(
            "{}service: {}",
            if service_outcome.is_failure() {
                "WARNING "
            } else {
                ""
            },
            service_outcome.summary()
        );
    }

    let path = cs::settings_path();
    let mut removed: Vec<String> = Vec::new();

    if path.exists() {
        let mut value = cs::load_settings(&path)?;

        for (event, _timeout) in cs::HOOK_EVENTS {
            let ptr = format!("/hooks/{event}");
            if let Some(arr) = value.pointer_mut(&ptr).and_then(Value::as_array_mut) {
                let before = arr.len();
                // Removes both current ("kikimimi hook") and legacy ("guru hook",
                // pre-rename) entries -- an uninstall must clean up regardless of
                // whether `init` ever ran again to upgrade them.
                arr.retain(|entry| !cs::entry_has_kikimimi_or_legacy_guru_hook(entry));
                let removed_n = before - arr.len();
                if removed_n > 0 {
                    removed.push(format!(
                        "hooks.{event}: removed {removed_n} kikimimi entry(ies)"
                    ));
                }
            }
            // Tidy up: drop the event key entirely if it is now an empty array.
            if let Some(hooks_obj) = value.get_mut("hooks").and_then(Value::as_object_mut) {
                let now_empty = hooks_obj
                    .get(*event)
                    .and_then(Value::as_array)
                    .map(|a| a.is_empty())
                    .unwrap_or(false);
                if now_empty {
                    hooks_obj.remove(*event);
                }
            }
        }

        // Must match whatever `init` actually wrote (which may be a conflict-avoidance
        // alternate port persisted in config.json, and/or a bearer token init generated),
        // not blindly recompute the default — otherwise uninstall would think a
        // successfully-written env value "no longer matches" and leave it behind instead of
        // removing it.
        let port = crate::config::resolve_otlp_port();
        let otlp_token = crate::config::KikimimiConfig::load().otlp_token;
        for (key, expected) in cs::expected_env(port, otlp_token.as_deref()) {
            let matches = value
                .pointer(&format!("/env/{key}"))
                .and_then(Value::as_str)
                == Some(expected.as_str());
            if matches {
                if let Some(env_obj) = value.get_mut("env").and_then(Value::as_object_mut) {
                    env_obj.remove(key);
                    removed.push(format!("env.{key}: removed"));
                }
            } else if value.pointer(&format!("/env/{key}")).is_some() {
                removed.push(format!(
                    "env.{key}: left unchanged (value no longer matches what kikimimi init writes)"
                ));
            }
        }

        cs::write_settings_atomic(&path, &value)?;
    } else {
        removed.push(format!(
            "{} does not exist, nothing to remove",
            path.display()
        ));
    }

    for r in &removed {
        println!("{r}");
    }

    if purge_data {
        let dir = kikimimi_schema::paths::kikimimi_dir();
        if dir.exists() {
            fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
            println!("purged {}", dir.display());
        } else {
            println!("{} does not exist, nothing to purge", dir.display());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::path::PathBuf;

    struct EnvGuard {
        path: PathBuf,
    }

    /// Points both `~/.claude/settings.json` (via `KIKIMIMI_CLAUDE_SETTINGS_PATH`) AND
    /// `~/.kikimimi` (via `KIKIMIMI_DIR`, since `init()` now also writes `config.json` there
    /// for the OTLP port — see `crate::config`) at a fresh tempdir, so tests never touch
    /// the real `$HOME/.kikimimi` on whatever machine runs the test suite.
    fn with_settings_path(dir: &tempfile::TempDir) -> EnvGuard {
        let path = dir.path().join("settings.json");
        std::env::set_var("KIKIMIMI_CLAUDE_SETTINGS_PATH", &path);
        std::env::set_var("KIKIMIMI_DIR", dir.path().join("kikimimi-home"));
        EnvGuard { path }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            std::env::remove_var("KIKIMIMI_CLAUDE_SETTINGS_PATH");
            std::env::remove_var("KIKIMIMI_DIR");
        }
    }

    #[test]
    #[serial]
    fn init_on_missing_file_creates_all_hooks_and_env_no_backup() {
        let dir = tempfile::tempdir().unwrap();
        let guard = with_settings_path(&dir);

        init(false, true).unwrap();

        assert!(guard.path.exists());
        assert!(
            !cs::backup_path(&guard.path).exists(),
            "no backup needed for a file that didn't exist"
        );

        let v = cs::load_settings(&guard.path).unwrap();
        for (event, timeout) in cs::HOOK_EVENTS {
            assert!(cs::has_kikimimi_hook(&v, event), "missing hook for {event}");
            let arr = v
                .pointer(&format!("/hooks/{event}"))
                .unwrap()
                .as_array()
                .unwrap();
            assert_eq!(arr.len(), 1);
            assert_eq!(
                arr[0].pointer("/hooks/0/timeout").unwrap().as_u64(),
                Some(*timeout)
            );
        }
        assert_eq!(
            v.pointer("/env/CLAUDE_CODE_ENABLE_TELEMETRY")
                .unwrap()
                .as_str(),
            Some("1")
        );
        assert_eq!(
            v.pointer("/env/OTEL_EXPORTER_OTLP_PROTOCOL")
                .unwrap()
                .as_str(),
            Some("http/protobuf")
        );
    }

    #[test]
    #[serial]
    fn init_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let guard = with_settings_path(&dir);
        init(false, true).unwrap();
        let first = cs::load_settings(&guard.path).unwrap();
        init(false, true).unwrap();
        let second = cs::load_settings(&guard.path).unwrap();
        assert_eq!(first, second, "running init twice must not change anything");
    }

    /// architecture.md §4「認証」: `init` must both write the bearer-token env header into
    /// settings.json and persist the same token into config.json (so `kikimimi agent` can
    /// enforce it too -- see agent.rs's `otlp_auth` handle).
    #[test]
    #[serial]
    fn init_writes_otlp_headers_env_and_persists_the_token() {
        let dir = tempfile::tempdir().unwrap();
        let guard = with_settings_path(&dir);

        init(false, true).unwrap();

        let v = cs::load_settings(&guard.path).unwrap();
        let header = v
            .pointer("/env/OTEL_EXPORTER_OTLP_HEADERS")
            .and_then(Value::as_str)
            .expect("OTEL_EXPORTER_OTLP_HEADERS must be written");
        let token = crate::config::KikimimiConfig::load()
            .otlp_token
            .expect("otlp_token must be persisted to config.json");
        assert_eq!(header, format!("Authorization=Bearer {token}"));
        assert_eq!(
            token.len(),
            32,
            "expected a 32-hex-char token, got {token:?}"
        );
    }

    /// Re-running `init` must never mint a new token, since an already-running Claude Code
    /// session keeps the old one in its env until restarted -- rotating on every re-run
    /// would leave that session's OTel export permanently 401ing until it's restarted.
    #[test]
    #[serial]
    fn init_a_second_time_keeps_the_same_token() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = with_settings_path(&dir);

        init(false, true).unwrap();
        let first_token = crate::config::KikimimiConfig::load().otlp_token.unwrap();

        init(false, true).unwrap();
        let second_token = crate::config::KikimimiConfig::load().otlp_token.unwrap();

        assert_eq!(
            first_token, second_token,
            "re-running init must reuse the existing token, not regenerate it"
        );
    }

    #[test]
    #[serial]
    fn init_upgrades_legacy_guru_hook_entries_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let guard = with_settings_path(&dir);
        let existing = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    { "hooks": [ { "type": "command", "command": "my-linter", "timeout": 3 } ] },
                    { "hooks": [ { "type": "command", "command": "guru hook PreToolUse", "timeout": 5 } ] }
                ]
            }
        });
        fs::write(&guard.path, serde_json::to_vec_pretty(&existing).unwrap()).unwrap();

        init(false, true).unwrap();

        let v = cs::load_settings(&guard.path).unwrap();
        let arr = v.pointer("/hooks/PreToolUse").unwrap().as_array().unwrap();
        assert_eq!(
            arr.len(),
            2,
            "legacy guru entry replaced in place, not left alongside the new one: {arr:?}"
        );
        assert_eq!(
            arr[0].pointer("/hooks/0/command").unwrap().as_str(),
            Some("my-linter"),
            "user's own hook must be untouched"
        );
        assert_eq!(
            arr[1].pointer("/hooks/0/command").unwrap().as_str(),
            Some("kikimimi hook PreToolUse")
        );
        assert!(cs::has_kikimimi_hook(&v, "PreToolUse"));
        assert!(!cs::has_legacy_guru_hook(&v, "PreToolUse"));

        // Idempotent afterwards: running init again must not add a second entry.
        init(false, true).unwrap();
        let v2 = cs::load_settings(&guard.path).unwrap();
        assert_eq!(v, v2);
    }

    #[test]
    #[serial]
    fn init_preserves_existing_user_hooks_and_backs_up_once() {
        let dir = tempfile::tempdir().unwrap();
        let guard = with_settings_path(&dir);
        let existing = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    { "hooks": [ { "type": "command", "command": "my-linter", "timeout": 3 } ] }
                ]
            },
            "env": { "MY_OWN_VAR": "keep-me" }
        });
        fs::write(&guard.path, serde_json::to_vec_pretty(&existing).unwrap()).unwrap();

        init(false, true).unwrap();

        let backup_path = cs::backup_path(&guard.path);
        assert!(backup_path.exists());
        let backup: Value =
            serde_json::from_str(&fs::read_to_string(&backup_path).unwrap()).unwrap();
        assert_eq!(backup, existing, "backup must be the pre-init content");

        let v = cs::load_settings(&guard.path).unwrap();
        let pre_arr = v.pointer("/hooks/PreToolUse").unwrap().as_array().unwrap();
        assert_eq!(pre_arr.len(), 2, "user hook kept, kikimimi hook appended");
        assert_eq!(
            pre_arr[0].pointer("/hooks/0/command").unwrap().as_str(),
            Some("my-linter")
        );
        assert_eq!(
            v.pointer("/env/MY_OWN_VAR").unwrap().as_str(),
            Some("keep-me")
        );

        // Running init again must not overwrite the backup with post-init content.
        fs::write(&guard.path, b"{\"marker\": true}").unwrap();
        // (simulate a corrupted-looking but still valid settings file, then re-run)
        let _ = init(false, true); // may fail (marker true but not object with hooks ok) - ignore result
        let backup_after: Value =
            serde_json::from_str(&fs::read_to_string(&backup_path).unwrap()).unwrap();
        assert_eq!(backup_after, existing, "backup is only ever taken once");
    }

    #[test]
    #[serial]
    fn init_warns_and_does_not_overwrite_differing_env_value() {
        let dir = tempfile::tempdir().unwrap();
        let guard = with_settings_path(&dir);
        let existing = serde_json::json!({ "env": { "OTEL_EXPORTER_OTLP_PROTOCOL": "http/json" } });
        fs::write(&guard.path, serde_json::to_vec_pretty(&existing).unwrap()).unwrap();

        init(false, true).unwrap();

        let v = cs::load_settings(&guard.path).unwrap();
        assert_eq!(
            v.pointer("/env/OTEL_EXPORTER_OTLP_PROTOCOL")
                .unwrap()
                .as_str(),
            Some("http/json"),
            "must not overwrite an existing different value"
        );
    }

    #[test]
    #[serial]
    fn dry_run_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let guard = with_settings_path(&dir);
        init(true, true).unwrap();
        assert!(!guard.path.exists(), "dry-run must not create the file");
    }

    #[test]
    #[serial]
    fn uninstall_removes_exactly_what_init_added_and_keeps_user_hooks() {
        let dir = tempfile::tempdir().unwrap();
        let guard = with_settings_path(&dir);
        let existing = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    { "hooks": [ { "type": "command", "command": "my-linter", "timeout": 3 } ] }
                ]
            },
            "env": { "MY_OWN_VAR": "keep-me" }
        });
        fs::write(&guard.path, serde_json::to_vec_pretty(&existing).unwrap()).unwrap();

        init(false, true).unwrap();
        uninstall_impl(false, true).unwrap();

        let v = cs::load_settings(&guard.path).unwrap();
        let pre_arr = v.pointer("/hooks/PreToolUse").unwrap().as_array().unwrap();
        assert_eq!(pre_arr.len(), 1, "only the kikimimi entry is removed");
        assert_eq!(
            pre_arr[0].pointer("/hooks/0/command").unwrap().as_str(),
            Some("my-linter")
        );
        for (event, _) in cs::HOOK_EVENTS {
            if *event != "PreToolUse" {
                assert!(
                    v.pointer(&format!("/hooks/{event}")).is_none(),
                    "{event} array fully removed"
                );
            }
        }
        assert!(v.pointer("/env/CLAUDE_CODE_ENABLE_TELEMETRY").is_none());
        assert!(
            v.pointer("/env/OTEL_EXPORTER_OTLP_HEADERS").is_none(),
            "the otlp bearer-token header init wrote must be removed too"
        );
        assert_eq!(
            v.pointer("/env/MY_OWN_VAR").unwrap().as_str(),
            Some("keep-me")
        );
    }

    #[test]
    #[serial]
    fn uninstall_removes_a_lingering_legacy_guru_hook_entry_too() {
        let dir = tempfile::tempdir().unwrap();
        let guard = with_settings_path(&dir);
        // Simulates a settings.json still carrying a pre-upgrade "guru hook"
        // entry (e.g. `init` never got re-run since the guru → kikimimi
        // rename) -- uninstall must clean it up all the same.
        let existing = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    { "hooks": [ { "type": "command", "command": "my-linter", "timeout": 3 } ] },
                    { "hooks": [ { "type": "command", "command": "guru hook PreToolUse", "timeout": 5 } ] }
                ]
            }
        });
        fs::write(&guard.path, serde_json::to_vec_pretty(&existing).unwrap()).unwrap();

        uninstall_impl(false, true).unwrap();

        let v = cs::load_settings(&guard.path).unwrap();
        let arr = v.pointer("/hooks/PreToolUse").unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 1, "only the legacy guru entry is removed");
        assert_eq!(
            arr[0].pointer("/hooks/0/command").unwrap().as_str(),
            Some("my-linter")
        );
    }

    #[test]
    #[serial]
    fn uninstall_leaves_env_value_alone_if_it_no_longer_matches() {
        let dir = tempfile::tempdir().unwrap();
        let guard = with_settings_path(&dir);
        init(false, true).unwrap();
        let mut v = cs::load_settings(&guard.path).unwrap();
        cs::set_env(&mut v, "OTEL_EXPORTER_OTLP_PROTOCOL", "http/json").unwrap();
        cs::write_settings_atomic(&guard.path, &v).unwrap();

        uninstall_impl(false, true).unwrap();

        let after = cs::load_settings(&guard.path).unwrap();
        assert_eq!(
            after
                .pointer("/env/OTEL_EXPORTER_OTLP_PROTOCOL")
                .unwrap()
                .as_str(),
            Some("http/json"),
            "value was modified since init, so uninstall must leave it alone"
        );
    }

    #[test]
    #[serial]
    fn init_picks_alternate_port_on_conflict_and_persists_it() {
        let dir = tempfile::tempdir().unwrap();
        let guard = with_settings_path(&dir);
        std::env::remove_var("KIKIMIMI_OTLP_PORT");
        std::env::remove_var("GURU_OTLP_PORT");

        // Occupy the default OTLP port so `init` must detect the conflict and pick
        // something else (architecture.md §4: "kikimimi init はポート使用状況を検査し...").
        let default_port = kikimimi_otlp::default_addr().port();
        let listener = std::net::TcpListener::bind(("127.0.0.1", default_port));
        let Ok(listener) = listener else {
            // The default port happens to already be busy in this environment for an
            // unrelated reason; skip rather than produce a false failure/pass.
            return;
        };

        init(false, true).unwrap();

        let v = cs::load_settings(&guard.path).unwrap();
        let endpoint = v
            .pointer("/env/OTEL_EXPORTER_OTLP_ENDPOINT")
            .unwrap()
            .as_str()
            .unwrap();
        assert!(
            !endpoint.ends_with(&format!(":{default_port}")),
            "must not write the occupied default port into settings.json, got {endpoint:?}"
        );

        // The picked port must be persisted so `kikimimi agent` binds the same one.
        let cfg = crate::config::KikimimiConfig::load();
        let persisted = cfg.otlp_port.expect("otlp_port must be persisted");
        assert!(endpoint.ends_with(&format!(":{persisted}")));
        assert_ne!(persisted, default_port);

        drop(listener);
    }

    /// Points the daemon control socket (`XDG_RUNTIME_DIR/kikimimi/agent.sock`) at the
    /// tempdir too, so a test can stand in for a running daemon by binding a Unix listener
    /// there -- and never talks to a real daemon on the machine running the suite.
    struct RuntimeDirGuard(Option<String>);
    impl RuntimeDirGuard {
        fn set(path: &std::path::Path) -> Self {
            let prev = std::env::var("XDG_RUNTIME_DIR").ok();
            std::env::set_var("XDG_RUNTIME_DIR", path);
            Self(prev)
        }
    }
    impl Drop for RuntimeDirGuard {
        fn drop(&mut self) {
            match &self.0 {
                Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
                None => std::env::remove_var("XDG_RUNTIME_DIR"),
            }
        }
    }

    #[test]
    #[serial]
    fn init_keeps_the_port_its_own_daemon_holds() {
        // The 0.4.x -> 0.5.0 upgrade shape: `kikimimi agent &` is still running and bound
        // to 4318 when `kikimimi init` re-runs. The port must stay 4318 (the daemon that
        // holds it is ours, and the service takes it over right after) -- not be probed as
        // "in use" and swapped for a random alternate that settings.json never learns of.
        let dir = tempfile::tempdir().unwrap();
        let guard = with_settings_path(&dir);
        let _runtime = RuntimeDirGuard::set(dir.path());
        std::env::remove_var("KIKIMIMI_OTLP_PORT");
        std::env::remove_var("GURU_OTLP_PORT");

        let default_port = kikimimi_otlp::default_addr().port();
        let Ok(tcp) = std::net::TcpListener::bind(("127.0.0.1", default_port)) else {
            return; // busy for an unrelated reason on this machine; see the sibling test
        };
        let sock = kikimimi_schema::paths::socket_path();
        fs::create_dir_all(sock.parent().unwrap()).unwrap();
        let _control = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let state_path = kikimimi_schema::paths::state_path();
        fs::create_dir_all(state_path.parent().unwrap()).unwrap();
        crate::state::AgentState::new(std::process::id(), 0, default_port)
            .save_to(&state_path)
            .unwrap();

        init(false, true).unwrap();

        let v = cs::load_settings(&guard.path).unwrap();
        let endpoint = v
            .pointer("/env/OTEL_EXPORTER_OTLP_ENDPOINT")
            .unwrap()
            .as_str()
            .unwrap();
        assert_eq!(endpoint, format!("http://localhost:{default_port}"));
        assert_eq!(
            crate::config::KikimimiConfig::load().otlp_port,
            Some(default_port)
        );
        drop(tcp);
    }

    #[test]
    #[serial]
    fn init_does_not_trust_a_stale_state_json_without_a_live_daemon() {
        // state.json names the port but nothing answers on the control socket (the daemon
        // crashed and something else took 4318): the plain probe must still pick another.
        let dir = tempfile::tempdir().unwrap();
        let guard = with_settings_path(&dir);
        let _runtime = RuntimeDirGuard::set(dir.path());
        std::env::remove_var("KIKIMIMI_OTLP_PORT");
        std::env::remove_var("GURU_OTLP_PORT");

        let default_port = kikimimi_otlp::default_addr().port();
        let Ok(tcp) = std::net::TcpListener::bind(("127.0.0.1", default_port)) else {
            return;
        };
        let state_path = kikimimi_schema::paths::state_path();
        fs::create_dir_all(state_path.parent().unwrap()).unwrap();
        crate::state::AgentState::new(1, 0, default_port)
            .save_to(&state_path)
            .unwrap();

        init(false, true).unwrap();

        let v = cs::load_settings(&guard.path).unwrap();
        let endpoint = v
            .pointer("/env/OTEL_EXPORTER_OTLP_ENDPOINT")
            .unwrap()
            .as_str()
            .unwrap();
        assert!(
            !endpoint.ends_with(&format!(":{default_port}")),
            "{endpoint}"
        );
        drop(tcp);
    }

    #[test]
    #[serial]
    fn init_rewrites_its_own_stale_endpoint_when_the_port_changes() {
        // A previous init wrote http://localhost:4318; now 4318 belongs to a foreign
        // process. The alternate port must reach settings.json, not just config.json --
        // otherwise Claude Code keeps exporting to a port nobody listens on.
        let dir = tempfile::tempdir().unwrap();
        let guard = with_settings_path(&dir);
        let _runtime = RuntimeDirGuard::set(dir.path());
        std::env::remove_var("KIKIMIMI_OTLP_PORT");
        std::env::remove_var("GURU_OTLP_PORT");

        let default_port = kikimimi_otlp::default_addr().port();
        let Ok(tcp) = std::net::TcpListener::bind(("127.0.0.1", default_port)) else {
            return;
        };
        let existing = serde_json::json!({
            "env": { "OTEL_EXPORTER_OTLP_ENDPOINT": format!("http://localhost:{default_port}") }
        });
        fs::write(&guard.path, serde_json::to_vec_pretty(&existing).unwrap()).unwrap();

        init(false, true).unwrap();

        let persisted = crate::config::KikimimiConfig::load().otlp_port.unwrap();
        assert_ne!(persisted, default_port);
        let v = cs::load_settings(&guard.path).unwrap();
        assert_eq!(
            v.pointer("/env/OTEL_EXPORTER_OTLP_ENDPOINT")
                .unwrap()
                .as_str(),
            Some(format!("http://localhost:{persisted}").as_str())
        );
        drop(tcp);
    }

    #[test]
    #[serial]
    fn init_leaves_a_custom_endpoint_alone_even_when_the_port_changes() {
        let dir = tempfile::tempdir().unwrap();
        let guard = with_settings_path(&dir);
        let _runtime = RuntimeDirGuard::set(dir.path());
        std::env::remove_var("KIKIMIMI_OTLP_PORT");
        std::env::remove_var("GURU_OTLP_PORT");

        let default_port = kikimimi_otlp::default_addr().port();
        let Ok(tcp) = std::net::TcpListener::bind(("127.0.0.1", default_port)) else {
            return;
        };
        let existing = serde_json::json!({
            "env": { "OTEL_EXPORTER_OTLP_ENDPOINT": "http://otel-collector:4317" }
        });
        fs::write(&guard.path, serde_json::to_vec_pretty(&existing).unwrap()).unwrap();

        init(false, true).unwrap();

        let v = cs::load_settings(&guard.path).unwrap();
        assert_eq!(
            v.pointer("/env/OTEL_EXPORTER_OTLP_ENDPOINT")
                .unwrap()
                .as_str(),
            Some("http://otel-collector:4317")
        );
        drop(tcp);
    }

    struct CodexHomeGuard(Option<String>);
    impl CodexHomeGuard {
        fn set(path: &std::path::Path) -> Self {
            let prev = std::env::var("CODEX_HOME").ok();
            std::env::set_var("CODEX_HOME", path);
            Self(prev)
        }
    }
    impl Drop for CodexHomeGuard {
        fn drop(&mut self) {
            match &self.0 {
                Some(v) => std::env::set_var("CODEX_HOME", v),
                None => std::env::remove_var("CODEX_HOME"),
            }
        }
    }

    #[test]
    #[serial]
    fn init_reports_codex_detection_without_writing_to_codex_home() {
        let dir = tempfile::tempdir().unwrap();
        let guard = with_settings_path(&dir);
        let codex_home = dir.path().join("codex-home");
        fs::create_dir_all(&codex_home).unwrap();
        fs::write(codex_home.join("config.toml"), b"# untouched\n").unwrap();
        let _codex_guard = CodexHomeGuard::set(&codex_home);

        init(false, true).unwrap();

        // Non-destructive: kikimimi must not have written/modified anything under
        // ~/.codex (Stage 0 relies on the rollout tailer only -- see report_codex docs).
        assert_eq!(
            fs::read_to_string(codex_home.join("config.toml")).unwrap(),
            "# untouched\n"
        );
        let entries: Vec<_> = fs::read_dir(&codex_home)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name())
            .collect();
        assert_eq!(entries, vec![std::ffi::OsString::from("config.toml")]);

        // Claude settings.json must still have been written normally alongside this.
        assert!(guard.path.exists());
    }

    #[test]
    #[serial]
    fn init_says_nothing_about_codex_when_codex_home_absent() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = with_settings_path(&dir);
        let codex_home = dir.path().join("no-such-codex-home");
        let _codex_guard = CodexHomeGuard::set(&codex_home);

        let mut messages = Vec::new();
        report_codex(&mut messages);
        assert!(messages.is_empty());
    }

    #[test]
    fn duckdb_missing_message_mentions_duckdb() {
        // `report_duckdb` itself calls the real `web_query::duckdb_available()`, which
        // shells out to whatever `duckdb` happens to be on this machine's PATH -- not
        // fakeable here (task A2 note), so this pins the message-formatting path instead:
        // the const `report_duckdb` pushes when duckdb is absent.
        assert!(DUCKDB_MISSING_MESSAGE.contains("duckdb"));
        assert!(DUCKDB_MISSING_MESSAGE.starts_with("NOTE duckdb:"));
    }

    #[test]
    #[serial]
    fn uninstall_purge_data_removes_kikimimi_dir() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = with_settings_path(&dir);
        let kikimimi_dir = dir.path().join("kikimimi-home");
        std::env::set_var("KIKIMIMI_DIR", &kikimimi_dir);
        fs::create_dir_all(kikimimi_dir.join("data")).unwrap();
        fs::write(kikimimi_dir.join("host_id"), "abc").unwrap();

        uninstall_impl(true, true).unwrap();

        assert!(!kikimimi_dir.exists());
        std::env::remove_var("KIKIMIMI_DIR");
    }
}
