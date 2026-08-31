//! ローカルのファイル配置。
//! - 状態:   ~/.kikimimi/{host_id, state.json, config.toml}
//! - データ: ~/.kikimimi/data/events/dt=YYYY-MM-DD/*.parquet
//! - spool:  $XDG_RUNTIME_DIR/kikimimi/spool (無ければ ~/.kikimimi/spool) — tmpfs 想定
//! - socket: $XDG_RUNTIME_DIR/kikimimi/agent.sock (無ければ ~/.kikimimi/agent.sock)

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Once;

static LEGACY_DIR_ENV_WARNED: Once = Once::new();
static LEGACY_DIR_MIGRATE_LOGGED: Once = Once::new();

/// guru → kikimimi rename: `KIKIMIMI_DIR` is the current override; the old
/// `GURU_DIR` name still works too (one-time deprecation warning to stderr)
/// so an existing `GURU_DIR=...` in someone's shell profile or CI config
/// doesn't silently start writing to the wrong place. When neither is set,
/// the *default* `$HOME/.guru` (if present) is migrated in place to
/// `$HOME/.kikimimi` -- see [`migrate_legacy_dir`].
pub fn kikimimi_dir() -> PathBuf {
    if let Ok(d) = std::env::var("KIKIMIMI_DIR") {
        return PathBuf::from(d);
    }
    if let Ok(d) = std::env::var("GURU_DIR") {
        LEGACY_DIR_ENV_WARNED.call_once(|| {
            eprintln!(
                "warning: GURU_DIR is deprecated, use KIKIMIMI_DIR instead (guru → kikimimi rename)"
            );
        });
        return PathBuf::from(d);
    }
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()));
    let new_dir = home.join(".kikimimi");
    let old_dir = home.join(".guru");
    migrate_legacy_dir(&old_dir, &new_dir);
    new_dir
}

/// Best-effort, one-time (per process) migration of the default `~/.guru` to
/// `~/.kikimimi`: only runs when `new_dir` doesn't exist yet and `old_dir`
/// does, so it's a no-op on every call after the first successful rename (or
/// on a fresh install with neither directory present yet). Failure (e.g.
/// permissions, cross-device) is logged once and otherwise ignored -- a
/// fresh `new_dir` just gets created from scratch by the caller, same as any
/// other first run.
fn migrate_legacy_dir(old_dir: &Path, new_dir: &Path) {
    if new_dir.exists() || !old_dir.exists() {
        return;
    }
    match fs::rename(old_dir, new_dir) {
        Ok(()) => LEGACY_DIR_MIGRATE_LOGGED.call_once(|| {
            eprintln!(
                "kikimimi: migrated {} to {} (guru → kikimimi rename)",
                old_dir.display(),
                new_dir.display()
            );
        }),
        Err(e) => LEGACY_DIR_MIGRATE_LOGGED.call_once(|| {
            eprintln!(
                "kikimimi: warning: found legacy {} but could not migrate it to {} ({e:#}); \
                 a fresh {} will be used instead",
                old_dir.display(),
                new_dir.display(),
                new_dir.display()
            );
        }),
    }
}

pub fn data_dir() -> PathBuf {
    kikimimi_dir().join("data").join("events")
}

/// `<data_dir()>/dt=*/*.parquet`, single-quote-escaped for embedding directly in a SQL
/// string literal (DuckDB `read_parquet('...')`). Shared by `kikimimi query`
/// (`crates/cli/src/query_cmd.rs`) and `kikimimi agent`'s local web UI (`/web/q/*`,
/// `crates/cli/src/web_query.rs`) so both read the exact same Parquet layout with the
/// same escaping, instead of each re-deriving it.
pub fn events_glob_sql() -> String {
    events_glob_sql_in(&data_dir())
}

/// [`events_glob_sql`], parameterized by `data_dir` (tests point this at a tempdir
/// instead of the real `~/.kikimimi/data/events`).
pub fn events_glob_sql_in(data_dir: &std::path::Path) -> String {
    format!("{}/dt=*/*.parquet", data_dir.display()).replace('\'', "''")
}

fn runtime_base() -> PathBuf {
    std::env::var("XDG_RUNTIME_DIR")
        .map(|d| PathBuf::from(d).join("kikimimi"))
        .unwrap_or_else(|_| kikimimi_dir())
}

/// `true` なら `XDG_RUNTIME_DIR` が未設定 (または空文字列) で、spool/socket が
/// tmpfs 想定の一時領域ではなく永続ディスク (`kikimimi_dir()`) にフォールバックしている。
/// この場合 fsync/rename が (ネットワークマウントの可能性がある) 実ディスクを叩くため、
/// `kikimimi status` はこれを警告として表示する。
pub fn using_runtime_dir_fallback() -> bool {
    !matches!(std::env::var("XDG_RUNTIME_DIR"), Ok(ref s) if !s.is_empty())
}

pub fn spool_dir() -> PathBuf {
    runtime_base().join("spool")
}

pub fn socket_path() -> PathBuf {
    runtime_base().join("agent.sock")
}

pub fn state_path() -> PathBuf {
    kikimimi_dir().join("state.json")
}

/// Codex rollout tailer (architecture.md §4「ログ tailer」, §4.1 Codex 行) が
/// `~/.codex/sessions/**/rollout-*.jsonl` ごとの読み取りバイト offset を持ち回るための
/// 永続ファイル。`state.json`/`config.json` と同じく `<kikimimi_dir()>` 直下に置く。
/// 中身のフォーマットは `crates/cli/src/codex_tailer.rs` が定義する。
pub fn codex_cursors_path() -> PathBuf {
    kikimimi_dir().join("codex-cursors.json")
}

/// Codex CLI の既定ホーム。`CODEX_HOME` を尊重する (codex 自身と同じ変数名 — 実機の
/// `codex doctor` 出力で確認済み: "CODEX_HOME available", "default `CODEX_HOME` is `~/.codex`")。
/// テスト/smoke 用に上書きできるよう、`CODEX_HOME` が無ければ `$HOME/.codex` を返す。
pub fn codex_home_dir() -> PathBuf {
    if let Ok(d) = std::env::var("CODEX_HOME") {
        return PathBuf::from(d);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".codex")
}

/// Codex の rollout JSONL が置かれるディレクトリ (`$CODEX_HOME/sessions`)。
/// 実機確認 (2026-08-31, codex-cli 0.151.0):
/// `~/.codex/sessions/YYYY/MM/DD/rollout-<RFC3339ライクなタイムスタンプ>-<uuid>.jsonl`。
pub fn codex_sessions_dir() -> PathBuf {
    codex_home_dir().join("sessions")
}

/// host_id: 初回にランダム UUID を採番して永続化 (machine-id / MAC は使わない)。
/// ゴールデンイメージへの焼き込みを避けるため、ファイルが無ければ必ず新規採番。
pub fn host_id() -> anyhow::Result<String> {
    let p = kikimimi_dir().join("host_id");
    if let Ok(s) = fs::read_to_string(&p) {
        let s = s.trim().to_string();
        if !s.is_empty() {
            return Ok(s);
        }
    }
    fs::create_dir_all(kikimimi_dir())?;
    let id = uuid::Uuid::new_v4().to_string();
    fs::write(&p, &id)?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    #[serial]
    fn codex_home_dir_prefers_codex_home_env_var() {
        std::env::set_var("CODEX_HOME", "/tmp/custom-codex-home");
        assert_eq!(codex_home_dir(), PathBuf::from("/tmp/custom-codex-home"));
        assert_eq!(
            codex_sessions_dir(),
            PathBuf::from("/tmp/custom-codex-home/sessions")
        );
        std::env::remove_var("CODEX_HOME");
    }

    #[test]
    #[serial]
    fn codex_home_dir_falls_back_to_home_dot_codex() {
        std::env::remove_var("CODEX_HOME");
        let _home_guard = HomeGuard::set(std::path::Path::new("/tmp/fake-home"));
        assert_eq!(codex_home_dir(), PathBuf::from("/tmp/fake-home/.codex"));
    }

    #[test]
    #[serial]
    fn codex_cursors_path_lives_under_kikimimi_dir() {
        std::env::set_var("KIKIMIMI_DIR", "/tmp/kikimimi-cursor-test");
        assert_eq!(
            codex_cursors_path(),
            PathBuf::from("/tmp/kikimimi-cursor-test/codex-cursors.json")
        );
        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    #[serial]
    fn using_runtime_dir_fallback_reflects_xdg_runtime_dir() {
        std::env::remove_var("XDG_RUNTIME_DIR");
        assert!(using_runtime_dir_fallback());

        std::env::set_var("XDG_RUNTIME_DIR", "");
        assert!(
            using_runtime_dir_fallback(),
            "empty XDG_RUNTIME_DIR must count as unset"
        );

        std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1000");
        assert!(!using_runtime_dir_fallback());

        std::env::remove_var("XDG_RUNTIME_DIR");
    }

    #[test]
    fn events_glob_sql_in_appends_dt_glob_and_escapes_quotes() {
        assert_eq!(
            events_glob_sql_in(std::path::Path::new("/home/me/.kikimimi/data/events")),
            "/home/me/.kikimimi/data/events/dt=*/*.parquet"
        );
        assert_eq!(
            events_glob_sql_in(std::path::Path::new("/home/o'brien/.kikimimi/data/events")),
            "/home/o''brien/.kikimimi/data/events/dt=*/*.parquet"
        );
    }

    #[test]
    #[serial]
    fn events_glob_sql_uses_data_dir() {
        std::env::set_var("KIKIMIMI_DIR", "/tmp/kikimimi-glob-test");
        assert_eq!(
            events_glob_sql(),
            "/tmp/kikimimi-glob-test/data/events/dt=*/*.parquet"
        );
        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    #[serial]
    fn kikimimi_dir_prefers_new_env_over_legacy() {
        std::env::set_var("KIKIMIMI_DIR", "/tmp/new-dir");
        std::env::set_var("GURU_DIR", "/tmp/old-dir");
        assert_eq!(kikimimi_dir(), PathBuf::from("/tmp/new-dir"));
        std::env::remove_var("KIKIMIMI_DIR");
        std::env::remove_var("GURU_DIR");
    }

    #[test]
    #[serial]
    fn kikimimi_dir_falls_back_to_legacy_guru_dir_env() {
        std::env::remove_var("KIKIMIMI_DIR");
        std::env::set_var("GURU_DIR", "/tmp/legacy-dir");
        assert_eq!(kikimimi_dir(), PathBuf::from("/tmp/legacy-dir"));
        std::env::remove_var("GURU_DIR");
    }

    /// Saves and restores `$HOME` across a test (even on panic) -- these
    /// tests must not leak a deleted tempdir path into `$HOME` for whatever
    /// runs next in this process.
    struct HomeGuard(Option<String>);
    impl HomeGuard {
        fn set(path: &std::path::Path) -> Self {
            let prev = std::env::var("HOME").ok();
            std::env::set_var("HOME", path);
            Self(prev)
        }
    }
    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match &self.0 {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    #[test]
    #[serial]
    fn kikimimi_dir_migrates_default_guru_dir_to_kikimimi_dir() {
        std::env::remove_var("KIKIMIMI_DIR");
        std::env::remove_var("GURU_DIR");
        let home = tempfile::tempdir().unwrap();
        let _home_guard = HomeGuard::set(home.path());

        let old_dir = home.path().join(".guru");
        fs::create_dir_all(&old_dir).unwrap();
        fs::write(old_dir.join("host_id"), "abc-123").unwrap();

        let dir = kikimimi_dir();
        assert_eq!(dir, home.path().join(".kikimimi"));
        assert!(dir.exists(), "new dir must exist after migration");
        assert!(!old_dir.exists(), "old dir must be gone after migration");
        assert_eq!(
            fs::read_to_string(dir.join("host_id")).unwrap(),
            "abc-123",
            "migration must preserve existing contents (rename, not recreate)"
        );
    }

    #[test]
    #[serial]
    fn kikimimi_dir_does_not_touch_legacy_dir_once_new_dir_exists() {
        std::env::remove_var("KIKIMIMI_DIR");
        std::env::remove_var("GURU_DIR");
        let home = tempfile::tempdir().unwrap();
        let _home_guard = HomeGuard::set(home.path());

        // Both present: new_dir already exists, so the (differently
        // populated) old_dir must be left alone -- no silent data loss by
        // clobbering a directory that's already in active use.
        fs::create_dir_all(home.path().join(".kikimimi")).unwrap();
        fs::write(home.path().join(".kikimimi").join("marker"), "new").unwrap();
        fs::create_dir_all(home.path().join(".guru")).unwrap();
        fs::write(home.path().join(".guru").join("marker"), "old").unwrap();

        let dir = kikimimi_dir();
        assert_eq!(dir, home.path().join(".kikimimi"));
        assert_eq!(
            fs::read_to_string(dir.join("marker")).unwrap(),
            "new",
            "existing new dir must not be overwritten"
        );
        assert!(
            home.path().join(".guru").exists(),
            "old dir must be left alone once new dir already exists"
        );
    }
}
