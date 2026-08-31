//! guru-spool — hook シムとデーモンの間にある耐久ローカルキュー (architecture.md §4)。
//!
//! hook シムは判定を一切行わず、1 呼び出し 1 ファイルを **atomic rename** で
//! spool ディレクトリに書き、デーモンの unix socket に **50ms タイムアウトの
//! ノンブロッキング接続** で通知して即座に返る。デーモン不在・オフラインでも
//! 失敗せず (fail-open)、書き込みは spool に残るだけなので後で拾える。

use anyhow::Context;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use uuid::Uuid;

/// spool ディレクトリ内の一時ファイル接頭辞。list() 系はこれを無視する
/// (`.` で始まる名前は全て隠しエントリとして無視する。`.poisoned/` 隔離ディレクトリも含む)。
const TMP_PREFIX: &str = ".tmp-";

/// 処理できなかった spool エントリ (壊れた JSON・読み込み失敗など) を退避するサブ
/// ディレクトリ名。`is_completed_entry` の「`.` で始まる名前は無視」に含まれるので、
/// ここに移したエントリが `list()`/再処理ループに戻ってくることはない。
const QUARANTINE_DIR: &str = ".poisoned";

/// デーモンへの接続 (notify_daemon / send_control) に許す合計タイムアウト。
const CONNECT_TIMEOUT: Duration = Duration::from_millis(50);

/// 一時ファイルへの書き込み (create + write_all + fsync) に許す最大時間。
/// これを超えたら (遅い/ネットワークマウントされたディスクなどで) 諦めてエラーを返す —
/// hook シムは絶対にエージェントの実行をハングさせてはならない (設計原則 2)。
/// バックグラウンドスレッドで書き込みを続けさせたまま戻るので、直後にプロセスが
/// 終了すれば書き込みは打ち切られイベントを 1 件失うが、無期限にブロックするより
/// fail-open の設計方針に沿う。
const WRITE_TIMEOUT: Duration = Duration::from_millis(200);

/// kind 文字列を `[A-Za-z0-9_-]` のみに絞る。空になった場合は "unknown" にする。
fn sanitize_kind(kind: &str) -> String {
    let cleaned: String = kind
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    if cleaned.is_empty() {
        "unknown".to_string()
    } else {
        cleaned
    }
}

/// 1 回の hook 呼び出しを `dir` 配下に書く。
///
/// `dir` が無ければ作成する。まず `<dir>/.tmp-<uuid>` にペイロードを書いて
/// fsync し (最大 [`WRITE_TIMEOUT`] だけ待つ。詳細は同定数のドキュメント参照)、
/// その後 `<dir>/<epoch_ms>-<uuid>.<kind>.json` へ **同一ファイルシステム内の
/// rename** で公開する。rename は POSIX 上 atomic なので、デーモン側が途中状態の
/// ファイルを読むことはない。
pub fn write_entry_in(dir: &Path, kind: &str, payload: &[u8]) -> anyhow::Result<PathBuf> {
    fs::create_dir_all(dir).with_context(|| format!("creating spool dir {}", dir.display()))?;

    let kind = sanitize_kind(kind);
    let id = Uuid::new_v4();
    let tmp_path = dir.join(format!("{TMP_PREFIX}{id}"));

    write_tmp_file_bounded(&tmp_path, payload, WRITE_TIMEOUT)?;

    let epoch_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let final_path = dir.join(format!("{epoch_ms}-{id}.{kind}.json"));

    fs::rename(&tmp_path, &final_path).with_context(|| {
        format!(
            "renaming {} -> {}",
            tmp_path.display(),
            final_path.display()
        )
    })?;

    Ok(final_path)
}

/// `create + write_all + fsync` をバックグラウンドスレッドで行い、`timeout` だけ完了を
/// 待つ。通常のローカルディスクではまず一瞬で終わるが、遅い/ハングしたディスク
/// (ネットワークマウント等) では `timeout` で見切りをつけてエラーを返す。
/// タイムアウトした場合、書き込みスレッドは合流(join)せず放棄する — 呼び出し元の
/// プロセスがその直後に終了すればファイルは不完全なまま残るが、`.tmp-` 接頭辞のため
/// `list()`/デーモンから見えることはない (最終的な rename 前なので)。
fn write_tmp_file_bounded(tmp_path: &Path, payload: &[u8], timeout: Duration) -> anyhow::Result<()> {
    let payload = payload.to_vec();
    let tmp_path_owned = tmp_path.to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel::<std::io::Result<()>>();

    std::thread::spawn(move || {
        let result = (|| -> std::io::Result<()> {
            let mut f = fs::File::create(&tmp_path_owned)?;
            f.write_all(&payload)?;
            // 電源断・クラッシュ耐性のため rename 前に確実にディスクへ落とす。
            f.sync_all().ok();
            Ok(())
        })();
        // 受信側が既にタイムアウトして channel を drop していても構わない (fail-open)。
        let _ = tx.send(result);
    });

    match rx.recv_timeout(timeout) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => {
            Err(e).with_context(|| format!("writing temp spool file {}", tmp_path.display()))
        }
        Err(_) => Err(anyhow::anyhow!(
            "timed out after {timeout:?} writing temp spool file {}",
            tmp_path.display()
        )),
    }
}

/// `write_entry_in` を既定の spool ディレクトリ (`guru_schema::paths::spool_dir()`) に書く。
pub fn write_entry(kind: &str, payload: &[u8]) -> anyhow::Result<PathBuf> {
    write_entry_in(&guru_schema::paths::spool_dir(), kind, payload)
}

/// `path` の unix socket に接続し、1 バイト `byte` を書いて即座に閉じる。
/// 接続 + 送信を合わせた**合計**が `timeout` を超えて待たせない (connect と write
/// それぞれに独立で `timeout` を与えると最悪 2 倍待つことになるため、共通の締切
/// `deadline` から残り時間を引いて write 側に渡す)。失敗は握りつぶし `false` を返す
/// (パニックしない — hook シムは絶対にエージェントを止めない)。
fn connect_and_send(path: &Path, byte: u8, timeout: Duration) -> bool {
    use socket2::{Domain, SockAddr, Socket, Type};

    let deadline = Instant::now() + timeout;

    let addr = match SockAddr::unix(path) {
        Ok(a) => a,
        Err(_) => return false,
    };
    let socket = match Socket::new(Domain::UNIX, Type::STREAM, None) {
        Ok(s) => s,
        Err(_) => return false,
    };
    if socket.connect_timeout(&addr, timeout).is_err() {
        return false;
    }
    // Whatever is left of the original budget after connecting, not a fresh `timeout`.
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return false;
    }
    if socket.set_write_timeout(Some(remaining)).is_err() {
        return false;
    }

    let mut socket = socket;
    socket.write_all(&[byte]).is_ok()
}

/// デーモンに新しいエントリがあることを知らせる (制御バイト `b'n'`)。
/// 50ms 以内に接続できなければ諦めて `false` を返す。デーモンが不在・
/// オフラインでもエージェントを止めないための fail-open な通知。
pub fn notify_daemon() -> bool {
    send_control(b'n')
}

/// 任意の制御バイトをデーモンの socket に送る (例: `b'f'` = flush 要求)。
/// `notify_daemon` と同じ 50ms タイムアウト・fail-open のセマンティクス。
pub fn send_control(byte: u8) -> bool {
    connect_and_send(&guru_schema::paths::socket_path(), byte, CONNECT_TIMEOUT)
}

/// ファイル名が完了済み spool エントリかどうか。`.` で始まる名前
/// (`.tmp-*` の一時ファイル、`.poisoned/` 隔離ディレクトリ) は全て除外し、
/// かつ通常ファイルのみを対象にする (ディレクトリを誤って読もうとしない)。
fn is_completed_entry(path: &Path) -> bool {
    let name_ok = path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| !n.starts_with('.'))
        .unwrap_or(false);
    name_ok && path.is_file()
}

/// spool ディレクトリを読むリーダー。デーモン側で使う。
pub struct SpoolReader {
    dir: PathBuf,
}

impl SpoolReader {
    pub fn new_in(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn new() -> Self {
        Self::new_in(guru_schema::paths::spool_dir())
    }

    /// 完了済みエントリ (`.tmp-*` を除く) をファイル名昇順で列挙する。
    /// ファイル名は `<epoch_ms>-...` で始まるので、これは書き込み順に等しい。
    /// ディレクトリが存在しない場合は空の Vec を返す。
    pub fn list(&self) -> Vec<PathBuf> {
        let mut entries: Vec<PathBuf> = match fs::read_dir(&self.dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| is_completed_entry(p))
                .collect(),
            Err(_) => Vec::new(),
        };
        entries.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
        entries
    }

    pub fn read(&self, p: &Path) -> anyhow::Result<Vec<u8>> {
        fs::read(p).with_context(|| format!("reading spool entry {}", p.display()))
    }

    pub fn remove(&self, p: &Path) -> anyhow::Result<()> {
        fs::remove_file(p).with_context(|| format!("removing spool entry {}", p.display()))
    }

    /// 処理できなかったエントリ (読み込み失敗・壊れた JSON・正規化エラーなど) を
    /// `<dir>/.poisoned/<元のファイル名>` へ **削除せず** 退避する。
    ///
    /// `list()` は `.` で始まる名前を無視するので、退避したエントリが再処理ループに
    /// 戻ってきて無限リトライになることはない。同時に、単純に消してしまうより
    /// フォレンジック用途で元データを残せる (将来 Claude Code の hook スキーマが
    /// 変わって一括で拒否され始めた場合の原因調査など)。
    ///
    /// `p` が既に存在しない・rename が失敗する等で退避自体に失敗した場合は、
    /// 呼び出し元が退避ループに陥らないよう best-effort で `p` を直接削除してから
    /// エラーを返す (無限リトライより「1 件失っても前に進む」を優先する — fail-open)。
    pub fn quarantine(&self, p: &Path) -> anyhow::Result<PathBuf> {
        let qdir = self.dir.join(QUARANTINE_DIR);
        let result = fs::create_dir_all(&qdir)
            .with_context(|| format!("creating quarantine dir {}", qdir.display()))
            .and_then(|()| {
                let name = p
                    .file_name()
                    .ok_or_else(|| anyhow::anyhow!("spool entry {} has no filename", p.display()))?;
                let dest = qdir.join(name);
                fs::rename(p, &dest)
                    .with_context(|| format!("quarantining {} -> {}", p.display(), dest.display()))?;
                Ok(dest)
            });

        if result.is_err() {
            // Best-effort: never let a poisoned entry get stuck forever just because
            // quarantining it also failed. A file or a directory (e.g. the reviewer's
            // "unreadable entry" repro: a directory sitting where a spool file should be)
            // are both possible here.
            let _ = fs::remove_file(p);
            let _ = fs::remove_dir_all(p);
        }
        result
    }
}

impl Default for SpoolReader {
    fn default() -> Self {
        Self::new()
    }
}

/// `dir` に溜まっている未処理エントリ数。
pub fn backlog_in(dir: &Path) -> usize {
    SpoolReader::new_in(dir.to_path_buf()).list().len()
}

/// 既定の spool ディレクトリに溜まっている未処理エントリ数。
pub fn backlog() -> usize {
    SpoolReader::new().list().len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::time::Instant;

    #[test]
    fn write_entry_in_produces_no_tmp_files_and_is_readable() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_entry_in(dir.path(), "PreToolUse", b"{\"a\":1}").unwrap();

        assert!(path.exists());
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(!name.starts_with(TMP_PREFIX));
        assert!(name.ends_with(".PreToolUse.json"));

        // No leftover temp files in the directory.
        let all: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert!(all.iter().all(|n| !n.starts_with(TMP_PREFIX)));

        let reader = SpoolReader::new_in(dir.path());
        let listed = reader.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0], path);
        assert!(listed.iter().all(|p| is_completed_entry(p)));

        let content = reader.read(&path).unwrap();
        assert_eq!(content, b"{\"a\":1}");
    }

    #[test]
    fn write_entry_in_creates_missing_dir() {
        let base = tempfile::tempdir().unwrap();
        let nested = base.path().join("nested").join("spool");
        assert!(!nested.exists());
        let path = write_entry_in(&nested, "PostToolUse", b"x").unwrap();
        assert!(path.exists());
    }

    #[test]
    fn kind_is_sanitized() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_entry_in(dir.path(), "Weird/Kind Name!!", b"x").unwrap();
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(name.contains("WeirdKindName"));
        assert!(!name.contains('/'));
        assert!(!name.contains(' '));
        assert!(!name.contains('!'));
    }

    #[test]
    fn kind_falls_back_to_unknown_when_empty_after_sanitizing() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_entry_in(dir.path(), "!!! ///", b"x").unwrap();
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(name.ends_with(".unknown.json"));
    }

    #[test]
    fn list_is_sorted_and_ignores_tmp_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut written = Vec::new();
        for i in 0..5 {
            let p = write_entry_in(dir.path(), "PreToolUse", format!("{i}").as_bytes()).unwrap();
            written.push(p);
            // Ensure distinct epoch_ms ordering even on very fast filesystems/clocks.
            std::thread::sleep(Duration::from_millis(2));
        }
        // Leave a stray temp file behind; list() must ignore it.
        fs::write(dir.path().join(".tmp-stray"), b"partial").unwrap();

        let listed = SpoolReader::new_in(dir.path()).list();
        assert_eq!(listed.len(), 5);
        assert_eq!(listed, written, "entries must be returned in write order");
        assert!(listed.iter().all(|p| is_completed_entry(p)));
    }

    #[test]
    fn read_and_remove_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_entry_in(dir.path(), "Notification", b"payload").unwrap();
        let reader = SpoolReader::new_in(dir.path());
        assert_eq!(reader.read(&path).unwrap(), b"payload");
        reader.remove(&path).unwrap();
        assert!(!path.exists());
        assert!(reader.list().is_empty());
    }

    #[test]
    fn backlog_in_counts_completed_entries_only() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(backlog_in(dir.path()), 0);
        write_entry_in(dir.path(), "PreToolUse", b"1").unwrap();
        write_entry_in(dir.path(), "PreToolUse", b"2").unwrap();
        fs::write(dir.path().join(".tmp-stray"), b"partial").unwrap();
        assert_eq!(backlog_in(dir.path()), 2);
    }

    #[test]
    fn notify_daemon_style_call_returns_false_quickly_with_no_listener() {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("agent.sock");

        let start = Instant::now();
        let ok = connect_and_send(&sock_path, b'n', CONNECT_TIMEOUT);
        let elapsed = start.elapsed();

        assert!(!ok);
        assert!(
            elapsed < Duration::from_millis(200),
            "took too long: {elapsed:?}"
        );
    }

    #[test]
    fn notify_daemon_style_call_returns_true_when_listener_present() {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("agent.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();

        let handle = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut buf = [0u8; 1];
            use std::io::Read;
            conn.read_exact(&mut buf).unwrap();
            buf[0]
        });

        let start = Instant::now();
        let ok = connect_and_send(&sock_path, b'n', CONNECT_TIMEOUT);
        let elapsed = start.elapsed();

        assert!(ok);
        assert!(elapsed < Duration::from_millis(200), "took: {elapsed:?}");
        assert_eq!(handle.join().unwrap(), b'n');
    }

    #[test]
    fn send_control_flush_byte_reaches_listener() {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("agent.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();

        let handle = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut buf = [0u8; 1];
            use std::io::Read;
            conn.read_exact(&mut buf).unwrap();
            buf[0]
        });

        assert!(connect_and_send(&sock_path, b'f', CONNECT_TIMEOUT));
        assert_eq!(handle.join().unwrap(), b'f');
    }

    /// Exercises the real public API (default socket path, no env mutation) to make
    /// sure it never panics and never blocks noticeably beyond the connect timeout,
    /// regardless of whether a real daemon happens to be listening in this environment.
    #[test]
    fn public_notify_daemon_and_send_control_never_panic_or_block() {
        let start = Instant::now();
        let _ = notify_daemon();
        let _ = send_control(b'f');
        assert!(start.elapsed() < Duration::from_millis(500));
    }

    /// The connect+write budget must stay within a small constant multiple of the
    /// configured timeout, not ~2x it (a slow-but-not-dead peer used to be able to
    /// consume the full CONNECT_TIMEOUT on connect *and then again* on write).
    #[test]
    fn connect_and_send_never_exceeds_roughly_one_timeout_budget_with_no_listener() {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("agent.sock");
        let budget = Duration::from_millis(80);

        let start = Instant::now();
        let ok = connect_and_send(&sock_path, b'n', budget);
        let elapsed = start.elapsed();

        assert!(!ok);
        assert!(
            elapsed < budget * 2,
            "took {elapsed:?}, expected well under 2x the {budget:?} budget"
        );
    }

    #[test]
    fn quarantine_moves_entry_aside_and_it_is_never_listed_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_entry_in(dir.path(), "PreToolUse", b"not valid json").unwrap();
        let reader = SpoolReader::new_in(dir.path());
        assert_eq!(reader.list().len(), 1);

        let quarantined = reader.quarantine(&path).unwrap();
        assert!(!path.exists(), "original path must be gone");
        assert!(quarantined.exists(), "quarantined copy must exist");
        assert!(quarantined.starts_with(dir.path().join(QUARANTINE_DIR)));

        // Not picked up by list() again — no infinite retry loop.
        assert!(reader.list().is_empty());

        // The forensic copy still has the original bytes.
        assert_eq!(fs::read(&quarantined).unwrap(), b"not valid json");
    }

    #[test]
    fn quarantine_handles_an_unreadable_directory_entry_without_looping() {
        // Reproduces the "poisoned entry retried forever" failure mode: something that
        // is not a plain file sitting where a completed spool entry's name pattern would
        // be. Before the fix, `SpoolReader::read` would fail on this every single pass
        // and the caller (agent.rs::drain_spool) would just `continue`, leaving it in
        // place forever.
        let dir = tempfile::tempdir().unwrap();
        let poisoned = dir.path().join("1700000000000-deadbeef.PreToolUse.json");
        fs::create_dir(&poisoned).unwrap(); // a directory, not a file: reader.read() will fail

        let reader = SpoolReader::new_in(dir.path());
        // `list()` must not even offer up a directory as a "completed entry" any more.
        assert!(reader.list().is_empty());

        // But even if something upstream still hands us the path directly, quarantining
        // it must succeed (or at minimum remove it) rather than getting stuck.
        assert!(reader.quarantine(&poisoned).is_ok());
        assert!(!poisoned.exists());
    }
}
