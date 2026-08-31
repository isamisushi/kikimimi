//! `kikimimi agent` の既定 (非 `--foreground`) 動作: 二重 fork でターミナルから切り離す。
//!
//! tokio ランタイムを作る **前** (main はまだ sync) に呼ぶこと。マルチスレッド化した
//! プロセスを fork するのは未定義動作の温床なので、fork はランタイム開始前に済ませる。

use std::io;
use std::os::unix::io::AsRawFd;
use std::path::Path;

/// 親 → exit(0)、中間子 → setsid + exit(0)、孫だけが戻り値を受け取って動き続ける。
/// 失敗した場合は呼び出し元 (`kikimimi agent`) に伝え、`--foreground` 相当にフォールバックできるようにする。
pub fn daemonize(log_path: &Path) -> anyhow::Result<()> {
    // First fork: detach from the shell that launched us.
    match unsafe { libc::fork() } {
        pid if pid < 0 => anyhow::bail!("fork failed: {}", io::Error::last_os_error()),
        pid if pid > 0 => std::process::exit(0), // original parent
        _ => {}
    }

    if unsafe { libc::setsid() } < 0 {
        anyhow::bail!("setsid failed: {}", io::Error::last_os_error());
    }

    // Second fork: guarantee we can never re-acquire a controlling terminal.
    match unsafe { libc::fork() } {
        pid if pid < 0 => anyhow::bail!("second fork failed: {}", io::Error::last_os_error()),
        pid if pid > 0 => std::process::exit(0), // session-leader intermediate child
        _ => {}
    }

    redirect_stdio(log_path)?;
    let _ = std::env::set_current_dir("/");
    Ok(())
}

/// stdin を `/dev/null` に、stdout/stderr を `log_path` (追記) に付け替える。
fn redirect_stdio(log_path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let devnull = std::fs::OpenOptions::new().read(true).open("/dev/null")?;
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;

    unsafe {
        libc::dup2(devnull.as_raw_fd(), 0);
        libc::dup2(log_file.as_raw_fd(), 1);
        libc::dup2(log_file.as_raw_fd(), 2);
    }
    Ok(())
}
