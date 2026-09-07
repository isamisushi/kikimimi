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
    let started = std::time::Instant::now();
    let mut buf = Vec::new();
    std::io::stdin()
        .lock()
        .take(MAX_STDIN_BYTES)
        .read_to_end(&mut buf)?;
    kikimimi_spool::write_entry(event, &buf)?;
    // Fail-open: whether or not the daemon is reachable, the shim must return immediately.
    let _ = kikimimi_spool::notify_daemon();
    // KKM-17: record how long the shim itself took (what Claude Code waited for), so
    // `kikimimi status` can print the p50/p99 instead of us guessing. Best effort.
    let _ = shim_latency::record(
        &kikimimi_schema::paths::kikimimi_dir(),
        started.elapsed().as_micros() as u64,
    );
    Ok(())
}

/// `~/.kikimimi/shim-latency.log`: one `"<unix_ms> <elapsed_us>"` line per hook
/// invocation, rotated to `.1` past [`shim_latency::MAX_BYTES`]. Read by
/// `status_cmd`'s `print_shim_latency`.
pub mod shim_latency {
    use std::io::Write as _;
    use std::path::Path;

    pub const FILE: &str = "shim-latency.log";
    /// ~35 bytes a line: 512 KiB keeps the last ~15k invocations, twice that with `.1`.
    pub const MAX_BYTES: u64 = 512 * 1024;
    /// How many of the newest samples `status` summarizes.
    pub const SAMPLE_WINDOW: usize = 2000;

    pub fn record(dir: &Path, elapsed_us: u64) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(FILE);
        if std::fs::metadata(&path)
            .map(|m| m.len() > MAX_BYTES)
            .unwrap_or(false)
        {
            let _ = std::fs::rename(&path, dir.join(format!("{FILE}.1")));
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        writeln!(
            f,
            "{} {}",
            chrono::Utc::now().timestamp_millis(),
            elapsed_us
        )
    }

    /// The newest `SAMPLE_WINDOW` recorded latencies (µs), oldest first. Empty when
    /// nothing was recorded yet.
    pub fn recent(dir: &Path) -> Vec<u64> {
        let mut all: Vec<u64> = Vec::new();
        for name in [format!("{FILE}.1"), FILE.to_string()] {
            if let Ok(text) = std::fs::read_to_string(dir.join(name)) {
                all.extend(text.lines().filter_map(|l| {
                    let mut it = l.split_whitespace();
                    it.next()?;
                    it.next()?.parse::<u64>().ok()
                }));
            }
        }
        if all.len() > SAMPLE_WINDOW {
            all.drain(..all.len() - SAMPLE_WINDOW);
        }
        all
    }

    /// `(p50, p99)` of the samples (nearest-rank), `None` when empty.
    pub fn percentiles(samples: &[u64]) -> Option<(u64, u64)> {
        if samples.is_empty() {
            return None;
        }
        let mut v = samples.to_vec();
        v.sort_unstable();
        let rank = |p: f64| v[((p * v.len() as f64).ceil() as usize).clamp(1, v.len()) - 1];
        Some((rank(0.5), rank(0.99)))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn percentiles_use_nearest_rank() {
            assert_eq!(percentiles(&[]), None);
            assert_eq!(percentiles(&[7]), Some((7, 7)));
            let v: Vec<u64> = (1..=100).collect();
            assert_eq!(percentiles(&v), Some((50, 99)));
        }

        #[test]
        fn record_appends_and_rotates_and_recent_reads_newest_window() {
            let dir = tempfile::tempdir().unwrap();
            for i in 0..5 {
                record(dir.path(), 100 + i).unwrap();
            }
            assert_eq!(recent(dir.path()), vec![100, 101, 102, 103, 104]);

            // Force a rotation: pad the live file past MAX_BYTES, then record once more.
            let path = dir.path().join(FILE);
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            f.write_all(&vec![b'\n'; (MAX_BYTES + 1) as usize]).unwrap();
            drop(f);
            record(dir.path(), 999).unwrap();
            assert!(dir.path().join(format!("{FILE}.1")).exists());
            let r = recent(dir.path());
            assert_eq!(
                r,
                vec![100, 101, 102, 103, 104, 999],
                "rotated samples still count"
            );
        }
    }
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
