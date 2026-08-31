//! `kikimimi init` / `kikimimi uninstall` — architecture.md §4.2, §12 Stage 0。
//!
//! `~/.claude/settings.json` に hooks / env を書く (init) / 消す (uninstall)。
//! 冪等: 既に "kikimimi hook" で始まる command があるイベントには足さない。
//! ユーザー自身の既存 hooks は絶対に削除・並べ替えしない。

use std::fs;

use anyhow::Context;
use serde_json::Value;

use crate::claude_settings as cs;

pub fn init(dry_run: bool) -> anyhow::Result<()> {
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
    let preferred = crate::config::resolve_otlp_port();
    let port = if crate::config::otlp_port_env_override().is_some() {
        preferred
    } else {
        kikimimi_otlp::pick_port(preferred)
    };
    if port != preferred {
        messages.push(format!(
            "WARNING otlp: port {preferred} is already in use; selected alternate port {port} instead (kikimimi agent will use it too)"
        ));
    }
    for (key, expected) in cs::expected_env(port) {
        match value
            .pointer(&format!("/env/{key}"))
            .and_then(Value::as_str)
        {
            Some(current) if current == expected => {
                messages.push(format!("env.{key}: already set correctly"));
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

    // Persist the chosen port so `kikimimi agent` binds the same one next time it starts
    // (without this, agent would default back to 4318 and could re-collide immediately).
    // Load-modify-save (not a fresh default) so re-running `kikimimi init` after `kikimimi login`
    // doesn't clobber the saved cloud token (§6).
    let mut cfg = crate::config::KikimimiConfig::load();
    cfg.otlp_port = Some(port);
    cfg.save().context("saving config.json")?;

    Ok(())
}

pub fn uninstall(purge_data: bool) -> anyhow::Result<()> {
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
        // alternate port persisted in config.json), not blindly recompute the default —
        // otherwise uninstall would think a successfully-written env value "no longer
        // matches" and leave it behind instead of removing it.
        let port = crate::config::resolve_otlp_port();
        for (key, expected) in cs::expected_env(port) {
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

        init(false).unwrap();

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
        init(false).unwrap();
        let first = cs::load_settings(&guard.path).unwrap();
        init(false).unwrap();
        let second = cs::load_settings(&guard.path).unwrap();
        assert_eq!(first, second, "running init twice must not change anything");
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

        init(false).unwrap();

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
        init(false).unwrap();
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

        init(false).unwrap();

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
        let _ = init(false); // may fail (marker true but not object with hooks ok) - ignore result
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

        init(false).unwrap();

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
        init(true).unwrap();
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

        init(false).unwrap();
        uninstall(false).unwrap();

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

        uninstall(false).unwrap();

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
        init(false).unwrap();
        let mut v = cs::load_settings(&guard.path).unwrap();
        cs::set_env(&mut v, "OTEL_EXPORTER_OTLP_PROTOCOL", "http/json").unwrap();
        cs::write_settings_atomic(&guard.path, &v).unwrap();

        uninstall(false).unwrap();

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

        init(false).unwrap();

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

    #[test]
    #[serial]
    fn uninstall_purge_data_removes_kikimimi_dir() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = with_settings_path(&dir);
        let kikimimi_dir = dir.path().join("kikimimi-home");
        std::env::set_var("KIKIMIMI_DIR", &kikimimi_dir);
        fs::create_dir_all(kikimimi_dir.join("data")).unwrap();
        fs::write(kikimimi_dir.join("host_id"), "abc").unwrap();

        uninstall(true).unwrap();

        assert!(!kikimimi_dir.exists());
        std::env::remove_var("KIKIMIMI_DIR");
    }
}
