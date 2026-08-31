//! guru-sink::s3 — "s3" sink (BYO, architecture.md §4 「sink (出口)」, §6 「BYO sink
//! (任意)」).
//!
//! guru は S3 の認証情報を **一切保持・保存しない**: アップロードは常に `aws` CLI
//! (テストでは `uploader` で任意のバイナリに差し替え可能) にシェルアウトし、
//! ユーザーの既存プロファイル/SSO/IAM ロールをそのまま使わせる。`--endpoint-url` を
//! 渡せば R2/MinIO 等の S3 互換エンドポイントにも書ける。
//!
//! `push()` はイベントをメモリバッファに積むだけ (BYO sink はフル本文を受け取れる —
//! §5.2 — マスクは呼び出し側の責務で、ここでは何もマスクしない)。`flush()` は:
//!
//! 1. バッファ済みイベントを `dt` ごとに [`write_parquet_partition`] で staging
//!    ディレクトリ (`<staging_dir>/dt=<dt>/<host8>-<seq:06>-<uuid8>.parquet`) へ書く
//!    (`FileSink` と全く同じビルダー・命名規則を再利用— parquet 組み立てコードの
//!    重複を避ける)。書き込み失敗時は `FileSink::flush` と同じく、失敗した
//!    パーティションとまだ試していない後続パーティションのイベントをバッファへ戻す。
//! 2. staging ディレクトリの合計サイズが 64MB を超えていたら、古いファイルから
//!    削除して上限内に収める (`cloud-pending.jsonl` の考え方と同じ)。
//! 3. staging ディレクトリに **今現在残っている** 全 Parquet ファイル (今回新しく
//!    書いたものだけでなく、前回以前の flush で失敗して残っていたものも含む) を
//!    `<uploader> s3 cp <staging> <url>/guru.v1/events/dt=<dt>/<file>.parquet
//!    [--profile P] [--endpoint-url E] --only-show-errors` でアップロードする。
//!    S3 オブジェクトキーは staging ファイルの相対パス (`dt=.../file.parquet`) から
//!    決定的に導出するので、アップロードに失敗したファイルをどこにも記録し直す
//!    必要がない — staging ディレクトリ自体がリトライキューを兼ねる。1 ファイルにつき
//!    (アップローダのバイナリが見つからない場合を除き) 最大 3 回まで軽いバックオフを
//!    挟んでリトライする。成功したファイルは削除し、失敗したファイルは次回の
//!    `flush()` (次の tick / `guru flush`) に持ち越す。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Context;

use guru_schema::Event;

use crate::{write_parquet_partition, EventSink};

/// アップローダのバイナリ名の既定値。`S3Config::uploader` が `None` ならこれを使う。
const DEFAULT_UPLOADER: &str = "aws";
/// staging ディレクトリの合計サイズの上限 (バイト)。architecture.md §6 の
/// オフライン退避 Parquet (`local.max_size`) や `cloud-pending.jsonl`
/// (`MAX_PENDING_FILE_BYTES`, cloud.rs) と同じ考え方 — 上限が無いと、S3 側の障害・
/// 認証切れ・設定ミスが続く間ディスクを食い潰しうる。超過したら古いファイルから
/// 削除する。
const MAX_STAGING_BYTES: u64 = 64 * 1024 * 1024;
/// 1 回のアップロード実行 (`<uploader> s3 cp ...`) に許すタイムアウト。
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(60);
/// 1 ファイルにつき、1 回の `flush()` 内で試すアップロードの最大回数
/// (アップローダのバイナリそのものが見つからない場合は 1 回で諦める — リトライしても
/// 直らないエラーなので無駄なバックオフで `flush()` を遅くしない)。
const MAX_UPLOAD_ATTEMPTS: u32 = 3;
/// リトライ間のバックオフの基準値 (`attempt` 回目の待機 = これの `attempt` 倍)。
const UPLOAD_RETRY_BACKOFF: Duration = Duration::from_millis(300);
/// アップローダのバイナリが `PATH` に無い場合の `last_error` メッセージ (固定文言)。
const UPLOADER_NOT_FOUND_MSG: &str = "aws CLI not found";

/// `S3Sink::new` の設定。
#[derive(Debug, Clone, Default)]
pub struct S3Config {
    /// `s3://bucket/prefix` (末尾の `/` の有無は問わない)。
    pub url: String,
    /// `aws s3 cp --profile <profile>`。
    pub profile: Option<String>,
    /// `aws s3 cp --endpoint-url <endpoint_url>` (R2/MinIO 等の S3 互換エンドポイント用)。
    pub endpoint_url: Option<String>,
    /// アップロードに使うバイナリ名/パス。`None` なら `"aws"` (`PATH` から解決)。
    /// テストは実在しない `aws` を要求せずに済むよう、ここにフェイクスクリプトの
    /// パスを渡す。
    pub uploader: Option<String>,
}

/// push した順序を保ったまま、バッファ滞留時間 (age 判定用) を追跡する。
/// `FileSink`/`CloudSink` の `Buffered` と同じ考え方。
struct Buffered {
    event: Event,
    pushed_at: Instant,
}

/// BYO S3 sink (architecture.md §6)。1 ホストにつき最大 1 つ、`guru agent` が
/// `FileSink`/`CloudSink` と並行して保持する (agent.rs)。
pub struct S3Sink {
    url: String,
    profile: Option<String>,
    endpoint_url: Option<String>,
    uploader: String,
    host_id: String,
    staging_dir: PathBuf,
    buf: Vec<Buffered>,
    seq: u64,
    last_error: Option<String>,
    last_push_at_ms: Option<i64>,
    /// アップローダのバイナリが見つからない旨をすでに一度 stderr に警告したか
    /// ("don't spam" — `last_error` 自体は毎回更新するが、ログはうるさくしない)。
    uploader_missing_warned: bool,
}

impl S3Sink {
    /// FileSink/CloudSink と揃えた既定のフラッシュ閾値 (`maybe_flush` 用)。
    /// タスク仕様: 500 件 / 60 秒。
    pub const DEFAULT_MAX_ROWS: usize = 500;
    pub const DEFAULT_MAX_AGE: Duration = Duration::from_secs(60);

    /// `staging_dir` は `<uploader> s3 cp` に渡す一時 Parquet の置き場
    /// (本番は `~/.guru/s3-staging`、テストは tempdir を渡す)。
    pub fn new(cfg: S3Config, host_id: String, staging_dir: PathBuf) -> Self {
        Self {
            url: cfg.url.trim_end_matches('/').to_string(),
            profile: cfg.profile,
            endpoint_url: cfg.endpoint_url,
            uploader: cfg.uploader.unwrap_or_else(|| DEFAULT_UPLOADER.to_string()),
            host_id,
            staging_dir,
            buf: Vec::new(),
            seq: 0,
            last_error: None,
            last_push_at_ms: None,
            uploader_missing_warned: false,
        }
    }

    /// `pending() >= DEFAULT_MAX_ROWS` か、最も古いバッファ済みイベントが
    /// `DEFAULT_MAX_AGE` を超えていれば flush する。加えて、バッファが空でも
    /// staging ディレクトリに前回以前の flush で失敗して残っているファイルが
    /// あれば flush する (でないと、新しいイベントが来なくなった後にリトライされない
    /// まま永遠に staging に留まってしまう)。
    pub fn maybe_flush(&mut self) -> anyhow::Result<Vec<PathBuf>> {
        let has_leftover_staging = self.has_pending_staging_files();
        if self.buf.is_empty() && !has_leftover_staging {
            return Ok(Vec::new());
        }
        let over_rows = self.buf.len() >= Self::DEFAULT_MAX_ROWS;
        let over_age = self
            .buf
            .first()
            .map(|b| b.pushed_at.elapsed() >= Self::DEFAULT_MAX_AGE)
            .unwrap_or(false);
        if over_rows || over_age || has_leftover_staging {
            EventSink::flush(self)
        } else {
            Ok(Vec::new())
        }
    }

    /// 直近のアップロード試行 (成功/失敗いずれか) が起きた epoch ミリ秒。
    pub fn last_push_at_ms(&self) -> Option<i64> {
        self.last_push_at_ms
    }

    /// 直近の flush 失敗のエラーメッセージ。成功すると `None` に戻る。
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn host_id(&self) -> &str {
        &self.host_id
    }

    fn next_seq(&mut self) -> u64 {
        let s = self.seq;
        self.seq += 1;
        s
    }

    fn has_pending_staging_files(&self) -> bool {
        !self.list_staging_files_by_age().is_empty()
    }

    /// staging ディレクトリ (`<staging_dir>/dt=*/*.parquet`, `.tmp-*` を除く) を
    /// 更新時刻の昇順 (古い順) で列挙する。`seq` はプロセス再起動で 0 に戻るため
    /// ファイル名では時系列順が保証できず、代わりに mtime を使う
    /// (`CloudSink` の "古い順にトリム" と同じ考え方)。
    fn list_staging_files_by_age(&self) -> Vec<(PathBuf, SystemTime, u64)> {
        let mut out = Vec::new();
        let Ok(rd) = fs::read_dir(&self.staging_dir) else {
            return out;
        };
        for entry in rd.filter_map(|e| e.ok()) {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            if !ft.is_dir() {
                continue;
            }
            if !entry.file_name().to_string_lossy().starts_with("dt=") {
                continue;
            }
            let Ok(sub) = fs::read_dir(&path) else {
                continue;
            };
            for f in sub.filter_map(|e| e.ok()) {
                let fp = f.path();
                if f.file_name().to_string_lossy().starts_with(".tmp-") {
                    continue;
                }
                if fp.extension().map(|e| e == "parquet").unwrap_or(false) {
                    if let Ok(meta) = f.metadata() {
                        let mtime = meta.modified().unwrap_or(UNIX_EPOCH);
                        out.push((fp, mtime, meta.len()));
                    }
                }
            }
        }
        out.sort_by_key(|(_, mtime, _)| *mtime);
        out
    }

    /// staging ディレクトリの合計サイズが `cap` を超えていたら、[`paths_to_trim`]
    /// が選んだ (古い順の) ファイルを削除して上限内に収める。削除が起きたら
    /// `last_error` に理由を残す (`guru status` から見えるようにする — 無音で
    /// 握り潰すよりは良い、`CloudSink::trim_pending_file_if_over_cap` と同じ考え方)。
    fn enforce_staging_cap_with(&mut self, cap: u64) {
        let files = self.list_staging_files_by_age();
        let sized: Vec<(PathBuf, u64)> = files.into_iter().map(|(p, _, sz)| (p, sz)).collect();
        let to_drop = paths_to_trim(&sized, cap);
        if to_drop.is_empty() {
            return;
        }
        let mut dropped = 0usize;
        for path in &to_drop {
            if fs::remove_file(path).is_ok() {
                dropped += 1;
            }
        }
        if dropped > 0 {
            self.last_error = Some(format!(
                "s3 staging dir exceeded {cap} bytes; dropped {dropped} oldest staged file(s) \
                 to stay under the cap"
            ));
        }
    }

    fn enforce_staging_cap(&mut self) {
        self.enforce_staging_cap_with(MAX_STAGING_BYTES);
    }

    /// staging ファイルのパス (`<staging_dir>/dt=<dt>/<file>.parquet`) から、対応する
    /// S3 オブジェクト URL (`<url>/guru.v1/events/dt=<dt>/<file>.parquet`, §5.3) を
    /// 決定的に導出する。
    fn object_url_for(&self, staging_path: &Path) -> anyhow::Result<String> {
        let file_name = staging_path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| {
                anyhow::anyhow!("staging path {} has no file name", staging_path.display())
            })?;
        let dt_dir = staging_path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "staging path {} has no parent dt= directory",
                    staging_path.display()
                )
            })?;
        let dt = dt_dir.strip_prefix("dt=").ok_or_else(|| {
            anyhow::anyhow!("staging parent directory {dt_dir:?} is not a dt= partition")
        })?;
        Ok(format!("{}/guru.v1/events/dt={dt}/{file_name}", self.url))
    }

    /// 1 つの staging ファイルを最大 [`MAX_UPLOAD_ATTEMPTS`] 回までリトライしつつ
    /// アップロードする。成功したら `Ok(())` (削除は呼び出し側 `flush_impl` が行う)。
    /// アップローダのバイナリが見つからない場合はリトライせず 1 回で諦める。
    fn upload_with_retry(&mut self, staging_path: &Path) -> anyhow::Result<()> {
        let object_url = self.object_url_for(staging_path)?;
        let mut args: Vec<String> = vec![
            "s3".to_string(),
            "cp".to_string(),
            staging_path.display().to_string(),
            object_url,
        ];
        if let Some(p) = &self.profile {
            args.push("--profile".to_string());
            args.push(p.clone());
        }
        if let Some(e) = &self.endpoint_url {
            args.push("--endpoint-url".to_string());
            args.push(e.clone());
        }
        args.push("--only-show-errors".to_string());

        let mut last_err: Option<anyhow::Error> = None;
        for attempt in 1..=MAX_UPLOAD_ATTEMPTS {
            match self.run_uploader(&args) {
                Ok(()) => {
                    self.last_push_at_ms = Some(now_ms());
                    return Ok(());
                }
                Err(e) => {
                    if is_uploader_missing(&e) {
                        self.last_push_at_ms = Some(now_ms());
                        self.last_error = Some(UPLOADER_NOT_FOUND_MSG.to_string());
                        if !self.uploader_missing_warned {
                            eprintln!(
                                "guru-sink s3: {UPLOADER_NOT_FOUND_MSG} (looked for {:?} in PATH)",
                                self.uploader
                            );
                            self.uploader_missing_warned = true;
                        }
                        return Err(e);
                    }
                    last_err = Some(e);
                    if attempt < MAX_UPLOAD_ATTEMPTS {
                        std::thread::sleep(UPLOAD_RETRY_BACKOFF * attempt);
                    }
                }
            }
        }
        let e = last_err.expect("loop only exits here after at least one non-missing failure");
        self.last_push_at_ms = Some(now_ms());
        self.last_error = Some(format!("{e:#}"));
        Err(e)
    }

    /// `<uploader> <args...>` を stdin=null で実行し、[`UPLOAD_TIMEOUT`] 以内に完了
    /// しなければタイムアウトとして扱う。
    ///
    /// タイムアウト検知は `guru-spool::write_tmp_file_bounded` と同じ形 (バック
    /// グラウンドスレッドで `wait_with_output` し、`recv_timeout` で待つ) — `Child` は
    /// スレッドへ move するので、タイムアウト時にこちら側から直接 `kill()` できない
    /// ぶん、先に取得しておいた pid へ `SIGKILL` を送る。スレッド自体は join せず
    /// 放棄する (fail-open。プロセス全体は `guru agent` が生きている限り単なる
    /// ゾンビ待ちスレッド 1 本を抱えるだけで、実害はない)。
    fn run_uploader(&self, args: &[String]) -> anyhow::Result<()> {
        let mut cmd = Command::new(&self.uploader);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());

        let child = cmd
            .spawn()
            .with_context(|| format!("spawning uploader {:?}", self.uploader))?;
        let pid = child.id();

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = child.wait_with_output();
            let _ = tx.send(result);
        });

        match rx.recv_timeout(UPLOAD_TIMEOUT) {
            Ok(Ok(output)) => {
                if output.status.success() {
                    Ok(())
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    anyhow::bail!(
                        "{} exited with {}: {}",
                        self.uploader,
                        output.status,
                        stderr.trim()
                    );
                }
            }
            Ok(Err(e)) => Err(anyhow::Error::new(e).context("waiting for uploader")),
            Err(_) => {
                #[cfg(unix)]
                unsafe {
                    libc::kill(pid as libc::pid_t, libc::SIGKILL);
                }
                #[cfg(not(unix))]
                let _ = pid;
                anyhow::bail!("{} timed out after {UPLOAD_TIMEOUT:?}", self.uploader);
            }
        }
    }

    fn flush_impl(&mut self) -> anyhow::Result<Vec<PathBuf>> {
        if !self.buf.is_empty() {
            // dt でグループ化 (FileSink::flush と同じ形): 失敗したパーティションと
            // まだ試していない後続パーティションのイベントはバッファへ戻す。
            let mut by_dt: BTreeMap<String, Vec<Buffered>> = BTreeMap::new();
            for b in std::mem::take(&mut self.buf) {
                by_dt.entry(b.event.dt.clone()).or_default().push(b);
            }
            let mut groups = by_dt.into_iter();
            for (dt, group) in groups.by_ref() {
                let events: Vec<Event> = group.iter().map(|b| b.event.clone()).collect();
                let dir = self.staging_dir.join(format!("dt={dt}"));
                let seq = self.next_seq();
                match write_parquet_partition(&dir, &self.host_id, seq, &events) {
                    Ok(_path) => {}
                    Err(e) => {
                        self.buf.extend(group);
                        for (_, remaining) in groups {
                            self.buf.extend(remaining);
                        }
                        self.last_error = Some(format!("{e:#}"));
                        return Err(e);
                    }
                }
            }
        }

        self.enforce_staging_cap();

        // Upload everything currently sitting in staging — not just what this call
        // just wrote. A file left over from a previous flush's failed upload is
        // retried here automatically (the staging dir itself is the retry queue).
        let mut uploaded = Vec::new();
        let mut last_err: Option<anyhow::Error> = None;
        for (path, _mtime, _bytes) in self.list_staging_files_by_age() {
            match self.upload_with_retry(&path) {
                Ok(()) => {
                    let _ = fs::remove_file(&path);
                    uploaded.push(path);
                }
                Err(e) => {
                    last_err = Some(e);
                }
            }
        }

        match last_err {
            Some(e) => Err(e),
            None => {
                if !uploaded.is_empty() {
                    self.last_error = None;
                }
                Ok(uploaded)
            }
        }
    }
}

impl EventSink for S3Sink {
    /// BYO sink はフル本文を受け取れる (§5.2) ので、`CloudSink::push` と違いここでは
    /// 何もマスクしない — マスクは呼び出し側 (sink ごとのポリシー) の責務。
    fn push(&mut self, ev: Event) {
        self.buf.push(Buffered {
            event: ev,
            pushed_at: Instant::now(),
        });
    }

    fn flush(&mut self) -> anyhow::Result<Vec<PathBuf>> {
        self.flush_impl()
    }

    fn pending(&self) -> usize {
        self.buf.len()
    }
}

/// [`S3Sink::enforce_staging_cap_with`] の中身にあたる純粋関数: 古い順に並んだ
/// `(path, size)` のリストから、合計サイズが `cap` 以下になるまで削除すべきパスを
/// 選ぶ。実ファイル I/O をしないので、実際の 64MB という上限値とは独立に軽い単体
/// テストができる (`CloudSink::trim_to_cap` と同じ考え方)。
fn paths_to_trim(files_oldest_first: &[(PathBuf, u64)], cap: u64) -> Vec<PathBuf> {
    let mut total: u64 = files_oldest_first.iter().map(|(_, sz)| sz).sum();
    if total <= cap {
        return Vec::new();
    }
    let mut to_drop = Vec::new();
    for (path, sz) in files_oldest_first {
        if total <= cap {
            break;
        }
        to_drop.push(path.clone());
        total = total.saturating_sub(*sz);
    }
    to_drop
}

/// `run_uploader` のエラーが「アップローダのバイナリ自体が `PATH` に無い」
/// (`std::io::ErrorKind::NotFound`) かどうかを、anyhow のエラーチェーンを辿って判定する。
fn is_uploader_missing(e: &anyhow::Error) -> bool {
    e.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .map(|io| io.kind() == std::io::ErrorKind::NotFound)
            .unwrap_or(false)
    })
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    use std::fs::File;
    use std::os::unix::fs::PermissionsExt as _;

    fn sample_event(dt: &str, event_id: &str, tool_name: &str) -> Event {
        Event {
            event_id: event_id.to_string(),
            ts: 1_700_000_000_000,
            dt: dt.to_string(),
            host_id: "host-abcdef1234567890".to_string(),
            agent: "claude-code".to_string(),
            source: "hook".to_string(),
            event_type: guru_schema::event_type::TOOL_CALL.to_string(),
            tool_name: Some(tool_name.to_string()),
            tool_input_json: Some(r#"{"command":"rm -rf /tmp/x"}"#.to_string()),
            duration_ms: Some(120),
            success: Some(true),
            ..Default::default()
        }
    }

    // -----------------------------------------------------------------
    // Fake `aws` script generation — no real AWS credentials or network
    // involved; the "uploader" override just points at a local executable.
    // -----------------------------------------------------------------

    const SUCCEED_TEMPLATE: &str = r#"#!/usr/bin/env bash
set -euo pipefail
{
  printf 'CALL'
  for a in "$@"; do printf '\x1f%s' "$a"; done
  printf '\n'
} >> "%%CALL_LOG%%"
# argv is "s3" "cp" <src> <dst> ... (mirrors the real `aws s3 cp <src> <dst>`
# invocation, where "aws" itself is argv[0] i.e. $0 here, not counted in $1.. ).
src="$3"
dst="$4"
rel="${dst#s3://}"
out="%%BUCKET_ROOT%%/$rel"
mkdir -p "$(dirname "$out")"
cp "$src" "$out"
"#;

    const ALWAYS_FAIL_TEMPLATE: &str = r#"#!/usr/bin/env bash
{
  printf 'CALL'
  for a in "$@"; do printf '\x1f%s' "$a"; done
  printf '\n'
} >> "%%CALL_LOG%%"
echo "simulated persistent failure" >&2
exit 1
"#;

    fn write_script(path: &Path, body: &str, call_log: &Path, bucket_root: &Path) {
        let rendered = body
            .replace("%%CALL_LOG%%", &call_log.display().to_string())
            .replace("%%BUCKET_ROOT%%", &bucket_root.display().to_string());
        fs::write(path, rendered).unwrap();
        let mut perms = fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).unwrap();
    }

    /// Parses the call log's lines back into `Vec<Vec<String>>` (one entry per
    /// invocation, split on the 0x1f unit separator the fake script writes).
    fn read_calls(call_log: &Path) -> Vec<Vec<String>> {
        let Ok(contents) = fs::read_to_string(call_log) else {
            return Vec::new();
        };
        contents
            .lines()
            .filter_map(|line| line.strip_prefix("CALL"))
            .map(|rest| {
                rest.split('\u{1f}')
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect()
            })
            .collect()
    }

    fn parquet_row_count(path: &Path) -> usize {
        let file = File::open(path).unwrap();
        let builder = ParquetRecordBatchReaderBuilder::try_new(file).unwrap();
        let mut reader = builder.build().unwrap();
        let mut total = 0usize;
        while let Some(batch) = reader.next() {
            total += batch.unwrap().num_rows();
        }
        total
    }

    #[test]
    fn flush_uploads_object_naming_and_layout_match_spec() {
        let dir = tempfile::tempdir().unwrap();
        let staging_dir = dir.path().join("staging");
        let bucket_root = dir.path().join("bucket");
        let call_log = dir.path().join("calls.log");
        let script = dir.path().join("fake-aws.sh");
        write_script(&script, SUCCEED_TEMPLATE, &call_log, &bucket_root);

        let mut sink = S3Sink::new(
            S3Config {
                url: "s3://fake-bucket/team".to_string(),
                profile: None,
                endpoint_url: None,
                uploader: Some(script.display().to_string()),
            },
            "host-abcdef1234567890".to_string(),
            staging_dir.clone(),
        );

        sink.push(sample_event("2026-08-30", "e1", "Bash"));
        sink.push(sample_event("2026-08-30", "e2", "mcp__github__get_issue"));
        assert_eq!(sink.pending(), 2);

        let uploaded = sink.flush().unwrap();
        assert_eq!(uploaded.len(), 1, "both events share one dt -> one file");
        assert_eq!(sink.pending(), 0);
        assert_eq!(sink.last_error(), None);
        assert!(sink.last_push_at_ms().is_some());

        // Staging file must be gone after a successful upload.
        assert!(
            !uploaded[0].exists(),
            "staging file must be deleted after a successful upload"
        );

        let calls = read_calls(&call_log);
        assert_eq!(calls.len(), 1);
        let call = &calls[0];
        assert_eq!(call[0], "s3");
        assert_eq!(call[1], "cp");
        let dst = &call[3];
        assert!(
            dst.starts_with("s3://fake-bucket/team/guru.v1/events/dt=2026-08-30/"),
            "unexpected dst: {dst}"
        );
        let file_name = dst.rsplit('/').next().unwrap();
        let rest = file_name
            .strip_prefix("host-abc-") // host8 = first 8 chars of "host-abcdef1234567890"
            .expect("host8 prefix");
        let rest = rest.strip_prefix("000000-").expect("zero-padded seq"); // first flush, seq=0
        let uuid8 = rest.strip_suffix(".parquet").expect(".parquet suffix");
        assert_eq!(uuid8.len(), 8);
        assert!(uuid8.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(call.contains(&"--only-show-errors".to_string()));

        // The uploaded file landed under the fake "bucket" at exactly the derived key.
        let landed = bucket_root
            .join("fake-bucket/team/guru.v1/events/dt=2026-08-30")
            .join(file_name);
        assert!(landed.exists(), "expected {} to exist", landed.display());
        assert_eq!(parquet_row_count(&landed), 2);
    }

    #[test]
    fn flush_passes_profile_and_endpoint_url_when_set() {
        let dir = tempfile::tempdir().unwrap();
        let staging_dir = dir.path().join("staging");
        let bucket_root = dir.path().join("bucket");
        let call_log = dir.path().join("calls.log");
        let script = dir.path().join("fake-aws.sh");
        write_script(&script, SUCCEED_TEMPLATE, &call_log, &bucket_root);

        let mut sink = S3Sink::new(
            S3Config {
                url: "s3://fake-bucket/team/".to_string(), // trailing slash must be trimmed
                profile: Some("myprofile".to_string()),
                endpoint_url: Some("http://127.0.0.1:9000".to_string()),
                uploader: Some(script.display().to_string()),
            },
            "hosthosthost".to_string(),
            staging_dir,
        );
        sink.push(sample_event("2026-08-30", "e1", "Bash"));
        sink.flush().unwrap();

        let calls = read_calls(&call_log);
        assert_eq!(calls.len(), 1);
        let call = &calls[0];
        assert!(
            !call[3].contains("//guru.v1"),
            "trailing slash on url must be trimmed: {call:?}"
        );

        let profile_idx = call
            .iter()
            .position(|a| a == "--profile")
            .expect("--profile present");
        assert_eq!(call[profile_idx + 1], "myprofile");
        let endpoint_idx = call
            .iter()
            .position(|a| a == "--endpoint-url")
            .expect("--endpoint-url present");
        assert_eq!(call[endpoint_idx + 1], "http://127.0.0.1:9000");
    }

    #[test]
    fn flush_omits_profile_and_endpoint_url_when_unset() {
        let dir = tempfile::tempdir().unwrap();
        let staging_dir = dir.path().join("staging");
        let bucket_root = dir.path().join("bucket");
        let call_log = dir.path().join("calls.log");
        let script = dir.path().join("fake-aws.sh");
        write_script(&script, SUCCEED_TEMPLATE, &call_log, &bucket_root);

        let mut sink = S3Sink::new(
            S3Config {
                url: "s3://fake-bucket/team".to_string(),
                profile: None,
                endpoint_url: None,
                uploader: Some(script.display().to_string()),
            },
            "hosthosthost".to_string(),
            staging_dir,
        );
        sink.push(sample_event("2026-08-30", "e1", "Bash"));
        sink.flush().unwrap();

        let calls = read_calls(&call_log);
        let call = &calls[0];
        assert!(!call.contains(&"--profile".to_string()));
        assert!(!call.contains(&"--endpoint-url".to_string()));
    }

    #[test]
    fn failed_upload_keeps_staging_file_and_retries_on_next_flush() {
        let dir = tempfile::tempdir().unwrap();
        let staging_dir = dir.path().join("staging");
        let bucket_root = dir.path().join("bucket");
        let call_log = dir.path().join("calls.log");
        let script = dir.path().join("fake-aws.sh");
        write_script(&script, ALWAYS_FAIL_TEMPLATE, &call_log, &bucket_root);

        let mut sink = S3Sink::new(
            S3Config {
                url: "s3://fake-bucket/team".to_string(),
                profile: None,
                endpoint_url: None,
                uploader: Some(script.display().to_string()),
            },
            "hosthosthost".to_string(),
            staging_dir.clone(),
        );
        sink.push(sample_event("2026-08-30", "e1", "Bash"));

        let result = sink.flush();
        assert!(
            result.is_err(),
            "persistent upload failure must surface as Err"
        );
        assert!(sink.last_error().is_some());

        let staged: Vec<_> = fs::read_dir(staging_dir.join("dt=2026-08-30"))
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(
            staged.len(),
            1,
            "the staging file must survive a failed upload"
        );
        let staged_path = staged[0].path();

        // MAX_UPLOAD_ATTEMPTS retries must have happened within that one flush().
        assert_eq!(
            read_calls(&call_log).len(),
            MAX_UPLOAD_ATTEMPTS as usize,
            "must retry up to MAX_UPLOAD_ATTEMPTS times within a single flush()"
        );

        // Simulate the outage clearing: swap the same uploader path to a script
        // that now succeeds, then flush again with no new events pushed at all —
        // the staging directory itself must be the retry queue.
        write_script(&script, SUCCEED_TEMPLATE, &call_log, &bucket_root);
        assert_eq!(sink.pending(), 0, "no new events buffered for this retry");

        let uploaded = sink.flush().unwrap();
        assert_eq!(uploaded, vec![staged_path.clone()]);
        assert!(
            !staged_path.exists(),
            "must be deleted after the retried upload succeeds"
        );
        assert_eq!(
            sink.last_error(),
            None,
            "success must clear the prior error"
        );
    }

    #[test]
    fn missing_uploader_binary_sets_last_error_without_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let staging_dir = dir.path().join("staging");

        let mut sink = S3Sink::new(
            S3Config {
                url: "s3://fake-bucket/team".to_string(),
                profile: None,
                endpoint_url: None,
                uploader: Some(
                    dir.path()
                        .join("does-not-exist-anywhere")
                        .display()
                        .to_string(),
                ),
            },
            "hosthosthost".to_string(),
            staging_dir.clone(),
        );
        sink.push(sample_event("2026-08-30", "e1", "Bash"));

        let result = sink.flush();
        assert!(result.is_err());
        assert_eq!(sink.last_error(), Some(UPLOADER_NOT_FOUND_MSG));

        // Must not have burned MAX_UPLOAD_ATTEMPTS retries on a binary that will
        // never be found -- one attempt is enough to know retrying won't help.
        let staged: Vec<_> = fs::read_dir(staging_dir.join("dt=2026-08-30"))
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(staged.len(), 1, "staging file must survive too");

        // Calling flush() again (still missing) must not panic and must not spam:
        // last_error stays set, no crash.
        let result2 = sink.flush();
        assert!(result2.is_err());
        assert_eq!(sink.last_error(), Some(UPLOADER_NOT_FOUND_MSG));
    }

    #[test]
    fn paths_to_trim_drops_oldest_first_until_under_cap() {
        let files = vec![
            (PathBuf::from("a"), 10u64),
            (PathBuf::from("b"), 10u64),
            (PathBuf::from("c"), 10u64),
        ];
        let dropped = paths_to_trim(&files, 15);
        assert_eq!(dropped, vec![PathBuf::from("a"), PathBuf::from("b")]);
    }

    #[test]
    fn paths_to_trim_is_a_noop_when_already_under_cap() {
        let files = vec![(PathBuf::from("a"), 10u64), (PathBuf::from("b"), 10u64)];
        assert!(paths_to_trim(&files, 1024).is_empty());
    }

    #[test]
    fn enforce_staging_cap_with_deletes_oldest_files_on_disk_and_warns() {
        let dir = tempfile::tempdir().unwrap();
        let staging_dir = dir.path().join("staging");
        let mut sink = S3Sink::new(
            S3Config {
                url: "s3://fake-bucket/team".to_string(),
                ..Default::default()
            },
            "hosthosthost".to_string(),
            staging_dir.clone(),
        );

        let dt_dir = staging_dir.join("dt=2026-08-30");
        fs::create_dir_all(&dt_dir).unwrap();
        let old = dt_dir.join("hosthost-000000-aaaaaaaa.parquet");
        let newer = dt_dir.join("hosthost-000001-bbbbbbbb.parquet");
        fs::write(&old, vec![0u8; 10]).unwrap();
        // Ensure `newer` has a strictly later mtime than `old` on filesystems with
        // coarse mtime resolution.
        std::thread::sleep(Duration::from_millis(20));
        fs::write(&newer, vec![0u8; 10]).unwrap();

        sink.enforce_staging_cap_with(15);

        assert!(!old.exists(), "the older file must be trimmed");
        assert!(newer.exists(), "the newer file must survive");
        assert!(sink.last_error().unwrap().contains("dropped 1 oldest"));
    }

    #[test]
    fn maybe_flush_honors_max_rows_and_max_age() {
        let dir = tempfile::tempdir().unwrap();
        let staging_dir = dir.path().join("staging");
        let bucket_root = dir.path().join("bucket");
        let call_log = dir.path().join("calls.log");
        let script = dir.path().join("fake-aws.sh");
        write_script(&script, SUCCEED_TEMPLATE, &call_log, &bucket_root);

        let mut sink = S3Sink::new(
            S3Config {
                url: "s3://fake-bucket/team".to_string(),
                profile: None,
                endpoint_url: None,
                uploader: Some(script.display().to_string()),
            },
            "hosthosthost".to_string(),
            staging_dir,
        );

        sink.push(sample_event("2026-08-30", "a", "Bash"));
        assert!(
            sink.maybe_flush().unwrap().is_empty(),
            "below max_rows and not old enough, no flush yet"
        );
        assert_eq!(sink.pending(), 1);

        for i in 0..S3Sink::DEFAULT_MAX_ROWS - 1 {
            sink.push(sample_event("2026-08-30", &format!("e{i}"), "Bash"));
        }
        let uploaded = sink.maybe_flush().unwrap();
        assert_eq!(uploaded.len(), 1, "hit max_rows, should flush and upload");
        assert_eq!(sink.pending(), 0);
    }

    #[test]
    fn flush_on_empty_buffer_with_no_leftover_staging_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        let staging_dir = dir.path().join("staging");
        let mut sink = S3Sink::new(
            S3Config {
                url: "s3://fake-bucket/team".to_string(),
                ..Default::default()
            },
            "hosthosthost".to_string(),
            staging_dir,
        );
        assert!(EventSink::flush(&mut sink).unwrap().is_empty());
        assert!(sink.maybe_flush().unwrap().is_empty());
    }
}
