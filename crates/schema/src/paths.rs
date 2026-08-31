//! ローカルのファイル配置。
//! - 状態:   ~/.guru/{host_id, state.json, config.toml}
//! - データ: ~/.guru/data/events/dt=YYYY-MM-DD/*.parquet
//! - spool:  $XDG_RUNTIME_DIR/guru/spool (無ければ ~/.guru/spool) — tmpfs 想定
//! - socket: $XDG_RUNTIME_DIR/guru/agent.sock (無ければ ~/.guru/agent.sock)

use std::fs;
use std::path::PathBuf;

pub fn guru_dir() -> PathBuf {
    if let Ok(d) = std::env::var("GURU_DIR") {
        return PathBuf::from(d);
    }
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".guru")
}

pub fn data_dir() -> PathBuf {
    guru_dir().join("data").join("events")
}

/// `<data_dir()>/dt=*/*.parquet`, single-quote-escaped for embedding directly in a SQL
/// string literal (DuckDB `read_parquet('...')`). Shared by `guru query`
/// (`crates/cli/src/query_cmd.rs`) and `guru agent`'s local web UI (`/web/q/*`,
/// `crates/cli/src/web_query.rs`) so both read the exact same Parquet layout with the
/// same escaping, instead of each re-deriving it.
pub fn events_glob_sql() -> String {
    events_glob_sql_in(&data_dir())
}

/// [`events_glob_sql`], parameterized by `data_dir` (tests point this at a tempdir
/// instead of the real `~/.guru/data/events`).
pub fn events_glob_sql_in(data_dir: &std::path::Path) -> String {
    format!("{}/dt=*/*.parquet", data_dir.display()).replace('\'', "''")
}

fn runtime_base() -> PathBuf {
    std::env::var("XDG_RUNTIME_DIR")
        .map(|d| PathBuf::from(d).join("guru"))
        .unwrap_or_else(|_| guru_dir())
}

/// `true` なら `XDG_RUNTIME_DIR` が未設定 (または空文字列) で、spool/socket が
/// tmpfs 想定の一時領域ではなく永続ディスク (`guru_dir()`) にフォールバックしている。
/// この場合 fsync/rename が (ネットワークマウントの可能性がある) 実ディスクを叩くため、
/// `guru status` はこれを警告として表示する。
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
    guru_dir().join("state.json")
}

/// host_id: 初回にランダム UUID を採番して永続化 (machine-id / MAC は使わない)。
/// ゴールデンイメージへの焼き込みを避けるため、ファイルが無ければ必ず新規採番。
pub fn host_id() -> anyhow::Result<String> {
    let p = guru_dir().join("host_id");
    if let Ok(s) = fs::read_to_string(&p) {
        let s = s.trim().to_string();
        if !s.is_empty() {
            return Ok(s);
        }
    }
    fs::create_dir_all(guru_dir())?;
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
            events_glob_sql_in(std::path::Path::new("/home/me/.guru/data/events")),
            "/home/me/.guru/data/events/dt=*/*.parquet"
        );
        assert_eq!(
            events_glob_sql_in(std::path::Path::new("/home/o'brien/.guru/data/events")),
            "/home/o''brien/.guru/data/events/dt=*/*.parquet"
        );
    }

    #[test]
    #[serial]
    fn events_glob_sql_uses_data_dir() {
        std::env::set_var("GURU_DIR", "/tmp/guru-glob-test");
        assert_eq!(
            events_glob_sql(),
            "/tmp/guru-glob-test/data/events/dt=*/*.parquet"
        );
        std::env::remove_var("GURU_DIR");
    }
}
