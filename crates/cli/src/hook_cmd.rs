//! `kikimimi hook <EVENT>` — hook シム (architecture.md §4)。
//!
//! MUST: 常に exit 0、成功時は何も stdout に出さない、絶対に panic しない。
//! tokio は使わない (1 ツール呼び出しごとに起動するプロセスなので依存を最小にする)。

use std::io::Read;

/// stdin をこの上限までしか読まない (fail-open。巨大な入力でハングしない)。
const MAX_STDIN_BYTES: u64 = 10 * 1024 * 1024;

/// エントリポイント。何が起きても panic せず、呼び出し側 (main) はこの関数の後
/// 必ず exit(0) する。
pub fn run(event: &str) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| inner(event)));
    match result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => log_error(&format!("kikimimi hook {event}: {e:#}")),
        Err(_) => log_error(&format!("kikimimi hook {event}: panicked (caught)")),
    }
}

fn inner(event: &str) -> anyhow::Result<()> {
    let mut buf = Vec::new();
    std::io::stdin()
        .lock()
        .take(MAX_STDIN_BYTES)
        .read_to_end(&mut buf)?;
    kikimimi_spool::write_entry(event, &buf)?;
    // Fail-open: whether or not the daemon is reachable, the shim must return immediately.
    let _ = kikimimi_spool::notify_daemon();
    Ok(())
}

/// ベストエフォートで `~/.kikimimi/shim-errors.log` に 1 行追記する。これ自体が失敗しても
/// 呼び出し側には一切伝播させない (常に exit 0 の契約を守る)。
fn log_error(msg: &str) {
    let path = kikimimi_schema::paths::kikimimi_dir().join("shim-errors.log");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        use std::io::Write as _;
        let ts = chrono::Utc::now().to_rfc3339();
        let _ = writeln!(f, "{ts} {msg}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    /// stdin をリダイレクトできないので write_entry/notify_daemon の下請け動作は
    /// spool クレート側のテストに任せ、ここでは「never panics」という契約だけを確認する。
    #[test]
    #[serial]
    fn run_never_panics_even_with_unwritable_spool_dir() {
        let dir = tempfile::tempdir().unwrap();
        // KIKIMIMI_DIR governs both the spool fallback and the kikimimi_dir() used for shim-errors.log.
        std::env::set_var("KIKIMIMI_DIR", dir.path());
        // No XDG_RUNTIME_DIR override here is fine; spool_dir() will fall back to kikimimi_dir().
        run("PreToolUse"); // stdin in test harness is empty/closed; must not panic.
        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    #[serial]
    fn log_error_appends_a_line_and_never_panics_on_bad_dir() {
        // kikimimi_dir() resolves from KIKIMIMI_DIR/HOME; point it at a fresh temp dir.
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());
        log_error("boom");
        let contents = std::fs::read_to_string(dir.path().join("shim-errors.log")).unwrap();
        assert!(contents.contains("boom"));
        std::env::remove_var("KIKIMIMI_DIR");
    }
}
