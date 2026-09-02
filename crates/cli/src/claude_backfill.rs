//! Claude Code transcript の一括バックフィル (architecture.md §4「ログ tailer」、
//! §4.1 Claude Code 行)。
//!
//! # 動機
//!
//! 新規インストールは「今から」のイベントしか集めない。数か月分の
//! `~/.claude/projects` を溜め込んだ既存ユーザーが `kikimimi init` した直後、
//! ダッシュボードは空のまま (§2.3「2 分で最初の気づきを」に反する)。Codex の
//! rollout tailer (`codex_tailer.rs`) は最初から offset 0 (= 発見時点で既にあった
//! ファイルも先頭から) で読むことでこれをやっているが、Claude Code の hook/OTel
//! パイプラインには相当するものが無かった — これがそれ。
//!
//! # overlap guard: なぜ「hooks/OTel 開始前に終わったセッションだけ」なのか
//!
//! 既に hooks/OTel が拾っているセッションの記録を、transcript からも読んでしまうと
//! 二重計上になる。クエリ層は `tool.result` (`correlation_key` = `tool_use_id`) だけ
//! `hook`/`otel` 由来の重複を折りたたむ設計になっていて (docs/design/architecture.md
//! §5.1 の honesty note)、`tool.call`/`api.request` 系の行はソースをまたいで
//! dedup されない。つまり「収集が始まった後にでも 1 行でも記録を残したセッション」は
//! 二重計上のリスクがある。安全に一括取り込みできるのは
//! **収集が始まる前に完全に終わっていたセッションだけ**。
//!
//! そのための基準時刻 (`boundary`) は [`compute_boundary`] が決める:
//! ローカル Parquet が既にあれば、その最古の `dt=` パーティションの開始 (UTC 深夜) —
//! そこより前のセッションはローカルにまだ影も形も無いので安全。ローカルデータが
//! 全く無ければ (真の初回起動)、`state.json` の `first_started_at_ms`
//! (`agent.rs` が daemon の最初の起動時刻として一度だけ確定・永続化する) を使う。
//!
//! **[`compute_boundary`] を呼ぶのは `agent.rs` 側で、daemon の最初の起動時に一度だけ**
//! (レビュー指摘の修正): この関数は `data_dir` の最古の `dt=` パーティションを見るが、
//! それは backfill 自身が書き込む先でもある。もし daemon の起動のたびに呼び直すと、
//! 前回 backfill が作った (より古い) `dt=` パーティションが次の起動の新しい基準に
//! なってしまい、基準がどんどん遡っていく — 本来は安全に backfill できたはずの
//! セッションまで、再起動を重ねるうちに overlap 扱いされて永久にスキップされる。
//! そのため `first_started_at_ms` と全く同じ「一度だけ確定し、`state.json` に
//! 永続化して以降はその値をそのまま使う」パターンにする ([`Boundary`] のドキュメント、
//! `agent.rs::run` 参照)。この関数自体は単なる純粋関数として残す (テスト容易性のため)。
//!
//! ファイルの最後の記録のタイムスタンプが `boundary` **以上**なら
//! [`PlannedOutcome::SkippedOverlap`] としてスキップし、二度と再評価しない —
//! アクティブなセッションはこの先も boundary を超え続けるだけなので、safe になる
//! ことは金輪際無い ([`CursorFile`] に永続記録することで、daemon 再起動のたびに
//! 同じファイルの末尾を読み直す無駄を避ける)。
//!
//! # 実行方式
//!
//! [`spawn`] が `kikimimi agent` の起動時に一度だけ呼ばれ、[`run`] を
//! `tokio::task::spawn_blocking` タスクとして走らせる (`agent.rs`)。[`plan`] で
//! 決めたファイルを古い順に処理し、各行を `TranscriptNormalizer` (行の正規化。
//! `kikimimi-adapter-claude`) に通す。cwd からの repo 解決は
//! [`crate::repo_resolve::RepoResolver`] を使う — `TranscriptNormalizer` は
//! `cwd_hash` しか保持しない (PRIVACY: 生の `cwd` を保持しない設計、
//! `kikimimi-adapter-claude::transcript` 参照) ので、正規化に渡す前にこちら側で
//! `raw["cwd"]` を読んで解決し、返ってきた `Event` にセットする — `agent.rs::drain_spool`
//! が hook イベントの `ev.repo` を埋めるのと全く同じパターン。
//!
//! 正規化した `Event` は **ここでは sink に触れない**。バッチ (既定 500 件) にまとめて
//! `tx` 経由でメインループに送るだけ — 実際の repo filter (§6.1) / FileSink /
//! CloudSink / S3Sink への push は `agent.rs` 側 (`ingest_claude_backfill`) が行う。
//! hook/OTel/Codex と全く同じ経路を通すことで、マスクや filter のロジックを
//! 二重実装しない。
//!
//! スロットル: ファイルごとに小さくスリープし ([`SLEEP_PER_FILE`])、送信チャンネルも
//! 容量を絞ってある (`agent.rs`) — 数千ファイルの backlog がライブの spool drain を
//! 飢えさせないため (`tx.blocking_send` はチャンネルが詰まっていればブロックする
//! ので、これ自体が背圧としても働く)。
//!
//! # クラッシュ再開 (レビュー指摘の修正): ファイル単位ではなくバッチ単位でチェックポイント
//!
//! カーソルファイル ([`CursorFile`]) には 2 種類のエントリがある: `files` (完了した
//! ファイルの最終結果、[`FileOutcome`]) と `progress` (**まだ完了していない** ファイルの
//! 進捗 — 「ここまでは確実に `tx` へ送信済み」というバイト offset)。1 ファイルを
//! 読みながら [`BATCH_SIZE`] 件たまるたびに `tx` へ送り、送信が成功した行の直後の
//! バイト offset を `progress` に書く (`record_progress`)。SIGTERM/Ctrl-C はもちろん、
//! クラッシュ・OOM-kill・強制終了でもこのチェックポイントより後ろの分しか失われない —
//! ファイル全体を毎回先頭から読み直すと (旧実装)、`tool.call`/`api.request` はソースを
//! またいで dedup されないため (`tool.result` だけが dedup 対象、上の「overlap guard」
//! 節参照)、大きな transcript ファイル全体が丸ごと二重計上されかねなかった。
//!
//! 次回このファイルを [`backfill_one_file`] が触るとき、`progress` に記録された offset
//! (`resume_from`) が 0 より大きければ、まず `[0, resume_from)` を「正規化の内部状態
//! (`tool_calls`/`usage_counted`/`current_turn_id`/`session_id` 等) を復元するためだけに」
//! 読み直す — 生成されるイベントは全て捨てる (前回の実行で既に送信済みのため)。これに
//! よって、二重送信を避けつつ (捨てるだけなので) 正規化の文脈は正しく引き継げる。
//! ファイルが完了すると `progress` エントリは消え、`files` に最終結果が入る
//! ([`record_cursor`])。
//!
//! 同じ理由で、1 ファイルの末尾 ([`BATCH_SIZE`] 未満の残り、`finish()` の
//! `session.end` を含む) は無条件で flush してから完了とみなす — 複数ファイルに
//! またがって残りバッファを持ち越すと (旧実装)、「`files` に Backfilled と記録した
//! 時点では、そのファイルの一部イベントがまだ `tx` に一度も送られていない」という
//! ギャップが生まれてしまうため。
//!
//! 進捗 (`crate::state::ClaudeBackfillState`) は [`SharedBackfillState`]
//! (`Arc<Mutex<..>>`) 越しにこのバックグラウンドタスクが直接更新し続け、メインループの
//! 定期 state 保存 (`agent.rs::sync_claude_backfill_state`) がそこから都度写し取る —
//! `otlp_auth`/`otlp_rejected` (agent.rs) と同じ「生きたハンドルを持ち回ってサンプリング
//! する」形。

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kikimimi_adapter_claude::TranscriptNormalizer;
use kikimimi_schema::Event;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::state::ClaudeBackfillState;

/// overlap 判定用にファイル末尾から読む最大バイト数 ("last few KB" — 設計メモ参照)。
/// 1 行がどれだけ長くても (assistant の応答は巨大になり得る)、直近数行分の
/// `"timestamp"` フィールドをほぼ確実に含められるサイズ。
const TAIL_BYTES: u64 = 64 * 1024;

/// メインループへ送るバッチの目標サイズ。
pub const BATCH_SIZE: usize = 500;

/// ファイルごとの小さなスリープ (スロットル)。数千ファイルの backlog があっても
/// ライブの spool drain を飢えさせないための「間を空ける」だけの措置 —
/// `spawn_blocking` の専用スレッドで動くのでこの sleep 自体は他のタスクを
/// ブロックしない。
const SLEEP_PER_FILE: Duration = Duration::from_millis(10);

/// 進捗報告用の共有ハンドル。バックグラウンドタスク (`run`) がここを更新し続け、
/// `agent.rs` の定期 state 保存がそこからサンプリングする。
pub type SharedBackfillState = Arc<Mutex<ClaudeBackfillState>>;

fn bump(shared: &SharedBackfillState, f: impl FnOnce(&mut ClaudeBackfillState)) {
    let mut s = shared
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    f(&mut s);
}

// ---------------------------------------------------------------------
// カーソルファイル (~/.kikimimi/claude-backfill.json)
// ---------------------------------------------------------------------

/// 1 ファイルの処理結果として永続化する種別。`SkippedDone` は無い —
/// 「カーソルに記録済みで size/mtime が変わっていない」こと自体が done の意味であり、
/// 別途の outcome 値としては持たない ([`plan`] のドキュメント参照)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileOutcome {
    Backfilled,
    SkippedOverlap,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct CursorEntry {
    size: u64,
    mtime_ms: i64,
    outcome: FileOutcome,
}

/// `~/.kikimimi/claude-backfill.json` の中身。キーは transcript ファイルの絶対パス
/// (文字列)。`codex_tailer.rs::CursorFile` と同じ「tmp + rename で atomic に保存、
/// 壊れていれば黙って空から始める」形。
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct CursorFile {
    #[serde(default)]
    files: HashMap<String, CursorEntry>,
    /// まだ完了していない (= 前回クラッシュ等で中断された) backfill の進捗。
    /// パス → 「ここまでは確実に `tx` へ送信済み」のバイト offset
    /// (モジュール doc の「クラッシュ再開」節参照)。ファイルが完了すると消え、
    /// `files` に最終結果が入る ([`record_cursor`])。
    #[serde(default)]
    progress: HashMap<String, u64>,
}

impl CursorFile {
    pub fn load_from(path: &Path) -> Self {
        match fs::read_to_string(path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(self)?;
        crate::state::write_atomic(path, &bytes)
    }
}

// ---------------------------------------------------------------------
// boundary (overlap guard の基準時刻)
// ---------------------------------------------------------------------

/// overlap guard の基準時刻。`kikimimi status` は `label` をそのまま表示する。
pub struct Boundary {
    pub ts_ms: i64,
    /// "dt=YYYY-MM-DD" (ローカル Parquet の最古パーティション由来) か、RFC3339
    /// タイムスタンプ (`first_started_at_ms` 由来)。
    pub label: String,
}

/// モジュール doc comment の「overlap guard」節を参照。ローカル Parquet
/// (`data_dir/dt=YYYY-MM-DD/`) が 1 つでもあれば最古の `dt` の UTC 深夜を、
/// 無ければ `first_started_at_ms` をそのまま使う。
pub fn compute_boundary(data_dir: &Path, first_started_at_ms: i64) -> Boundary {
    match earliest_local_dt(data_dir) {
        Some(dt) => Boundary {
            ts_ms: dt_start_ms(&dt).unwrap_or(first_started_at_ms),
            label: format!("dt={dt}"),
        },
        None => Boundary {
            ts_ms: first_started_at_ms,
            label: iso_ms(first_started_at_ms),
        },
    }
}

fn earliest_local_dt(data_dir: &Path) -> Option<String> {
    let entries = fs::read_dir(data_dir).ok()?;
    entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter_map(|name| name.strip_prefix("dt=").map(str::to_string))
        .min()
}

fn dt_start_ms(dt: &str) -> Option<i64> {
    use chrono::NaiveDate;
    let date = NaiveDate::parse_from_str(dt, "%Y-%m-%d").ok()?;
    Some(date.and_hms_opt(0, 0, 0)?.and_utc().timestamp_millis())
}

fn iso_ms(ms: i64) -> String {
    use chrono::TimeZone;
    chrono::Utc
        .timestamp_millis_opt(ms)
        .single()
        .map(|t| t.to_rfc3339())
        .unwrap_or_else(|| ms.to_string())
}

// ---------------------------------------------------------------------
// plan(): どのファイルをどうするか決める (副作用なし、fs メタデータの読み取りのみ)
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlannedOutcome {
    Backfill,
    SkippedOverlap,
    SkippedDone,
}

#[derive(Debug, Clone)]
pub struct PlannedFile {
    pub path: PathBuf,
    pub size: u64,
    pub mtime_ms: i64,
    pub outcome: PlannedOutcome,
}

/// `<projects_dir>/*/*.jsonl` を列挙し、ファイルごとに 1 つの [`PlannedOutcome`] を
/// 決める。副作用は無い (カーソルファイルへの書き込みは呼び出し側 [`run`] の仕事)。
///
/// 判定順序:
/// 1. カーソルに `SkippedOverlap` として記録済みなら、size/mtime に関わらず
///    常にスキップ ("never retried while it keeps changing" — モジュール doc の
///    「overlap guard」節: アクティブなセッションはこの先も overlap し続けるだけなので、
///    毎回ファイル末尾を読み直す意味が無い)。
/// 2. カーソルに記録済みで、かつ現在の size/mtime が記録値と完全一致するなら
///    [`PlannedOutcome::SkippedDone`] (前回から何も変わっていない)。
/// 3. それ以外 (未知のファイル、または記録後に変化したファイル) は、末尾を読んで
///    最後の記録のタイムスタンプを boundary と比較する — 以上なら
///    `SkippedOverlap`、未満なら `Backfill`。
///
/// 戻り値は mtime 昇順 (同値は path 昇順) — [`run`] が「古い順に処理する」ための順序。
pub fn plan(projects_dir: &Path, cursor: &CursorFile, boundary_ms: i64) -> Vec<PlannedFile> {
    let mut out = Vec::new();
    for path in discover_files(projects_dir) {
        let Ok(meta) = fs::metadata(&path) else {
            continue; // vanished mid-scan; next plan() (next daemon start) picks it up if it comes back
        };
        let size = meta.len();
        let mtime_ms = mtime_ms_of(&meta);
        let key = path.to_string_lossy().to_string();

        if let Some(entry) = cursor.files.get(&key) {
            if entry.outcome == FileOutcome::SkippedOverlap {
                out.push(PlannedFile {
                    path,
                    size,
                    mtime_ms,
                    outcome: PlannedOutcome::SkippedOverlap,
                });
                continue;
            }
            if entry.size == size && entry.mtime_ms == mtime_ms {
                out.push(PlannedFile {
                    path,
                    size,
                    mtime_ms,
                    outcome: PlannedOutcome::SkippedDone,
                });
                continue;
            }
        }

        let last_ts = last_record_ts_ms(&path, mtime_ms);
        let outcome = if last_ts >= boundary_ms {
            PlannedOutcome::SkippedOverlap
        } else {
            PlannedOutcome::Backfill
        };
        out.push(PlannedFile {
            path,
            size,
            mtime_ms,
            outcome,
        });
    }
    out.sort_by(|a, b| {
        a.mtime_ms
            .cmp(&b.mtime_ms)
            .then_with(|| a.path.cmp(&b.path))
    });
    out
}

/// `<projects_dir>/*/*.jsonl` (再帰しない — Claude Code の実レイアウトは
/// `<projects>/<url-encoded-cwd>/<session-uuid>.jsonl` の 2 階層固定)。
fn discover_files(projects_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(projects_dir) else {
        return out; // ~/.claude/projects が無い = Claude Code 未使用/未実行。エラーにしない。
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let Ok(sub) = fs::read_dir(&path) else {
            continue;
        };
        for f in sub.filter_map(|e| e.ok()) {
            let fp = f.path();
            if fp.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                out.push(fp);
            }
        }
    }
    out
}

fn mtime_ms_of(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// ファイル末尾 [`TAIL_BYTES`] 分だけ読み、`"timestamp"` フィールドを持つ最後の行の
/// 値を ms へパースして返す。末尾に一切見つからなければ (壊れたファイル、
/// timestamp の無い行しか無い、等) `fallback_mtime_ms` (呼び出し側が渡す、ファイルの
/// mtime) を返す — モジュール doc 冒頭のタスク仕様どおり。
fn last_record_ts_ms(path: &Path, fallback_mtime_ms: i64) -> i64 {
    let Some(tail) = read_tail(path, TAIL_BYTES) else {
        return fallback_mtime_ms;
    };
    let text = String::from_utf8_lossy(&tail);
    for line in text.lines().rev() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue; // 末尾の切れた最初の行、またはたまたま壊れた行 -- 1 つ前を試す
        };
        if let Some(ts) = v.get("timestamp").and_then(Value::as_str) {
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) {
                return dt.timestamp_millis();
            }
        }
    }
    fallback_mtime_ms
}

fn read_tail(path: &Path, max_bytes: u64) -> Option<Vec<u8>> {
    let mut f = fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(max_bytes);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    Some(buf)
}

// ---------------------------------------------------------------------
// 実行: spawn_blocking タスク本体
// ---------------------------------------------------------------------

/// [`run`] を `tokio::task::spawn_blocking` として起動する (design point 3)。
/// `kikimimi agent` の起動時に一度だけ呼ばれる、fire-and-forget な背景タスク
/// (`crate::update::spawn_notifier` と同じ形 — `JoinHandle` を呼び出し側が
/// 待つ必要は無い)。`boundary_ms`/`boundary_label` は呼び出し側 (`agent.rs`) が
/// [`compute_boundary`] を daemon の最初の起動時に一度だけ呼んで `state.json` に
/// 永続化した値をそのまま渡す — この関数の中では二度と計算し直さない
/// (モジュール doc の「boundary」節、レビュー指摘の修正)。
pub fn spawn(
    host_id: String,
    projects_dir: PathBuf,
    cursor_path: PathBuf,
    boundary_ms: i64,
    boundary_label: String,
    tx: tokio::sync::mpsc::Sender<Vec<Event>>,
    shared: SharedBackfillState,
) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        run(
            &host_id,
            &projects_dir,
            &cursor_path,
            boundary_ms,
            &boundary_label,
            &tx,
            &shared,
        );
    })
}

/// [`spawn`] の同期本体。`projects_dir` が無い (Claude Code 未使用) 場合も
/// [`discover_files`] が空を返すだけで正常終了する。
fn run(
    host_id: &str,
    projects_dir: &Path,
    cursor_path: &Path,
    boundary_ms: i64,
    boundary_label: &str,
    tx: &tokio::sync::mpsc::Sender<Vec<Event>>,
    shared: &SharedBackfillState,
) {
    bump(shared, |s| s.running = true);
    bump(shared, |s| s.boundary = boundary_label.to_string());

    let mut cursor = CursorFile::load_from(cursor_path);
    let planned = plan(projects_dir, &cursor, boundary_ms);

    let mut repo_resolver = crate::repo_resolve::RepoResolver::default();
    let mut mcp_cache = crate::mcp_config::McpConfigCache::default();

    'files: for pf in &planned {
        bump(shared, |s| s.files_seen += 1);
        match pf.outcome {
            PlannedOutcome::SkippedDone => {
                bump(shared, |s| s.files_skipped_done += 1);
                // カーソルは既に正しい内容で記録済み -- 書き直さない。
            }
            PlannedOutcome::SkippedOverlap => {
                bump(shared, |s| s.files_skipped_overlap += 1);
                record_cursor(&mut cursor, cursor_path, pf, FileOutcome::SkippedOverlap);
            }
            PlannedOutcome::Backfill => {
                let ok = backfill_one_file(
                    host_id,
                    pf,
                    &mut repo_resolver,
                    &mut mcp_cache,
                    tx,
                    shared,
                    &mut cursor,
                    cursor_path,
                );
                if !ok {
                    // 受け手 (メインループ) が居なくなった -- daemon シャットダウン中。
                    // ここまでの進捗は既に `cursor.progress` へ書いてある (バッチ単位の
                    // チェックポイント、モジュール doc 参照) ので、`files` への最終記録は
                    // せずに静かに抜ける -- 次回起動時にこのファイルの続きから再開する。
                    break 'files;
                }
                bump(shared, |s| s.files_backfilled += 1);
                record_cursor(&mut cursor, cursor_path, pf, FileOutcome::Backfilled);
            }
        }
        std::thread::sleep(SLEEP_PER_FILE);
    }

    bump(shared, |s| s.running = false);
}

/// 1 ファイルを最初から最後まで正規化し、行の境界で [`BATCH_SIZE`] 件たまるたびに
/// `tx` へ送る。戻り値 `false` は「受け手が居なくなった (daemon シャットダウン中) ので
/// これ以上続けても無駄」を意味する。前回の実行がこのファイルの途中で中断されていれば
/// (`cursor` の `progress` にバイト offset が残っていれば)、そこまでを状態復元のためだけ
/// に読み直し (生成イベントは捨てる)、続きから改めて送信する
/// (モジュール doc の「クラッシュ再開」節参照)。
#[allow(clippy::too_many_arguments)]
fn backfill_one_file(
    host_id: &str,
    pf: &PlannedFile,
    repo_resolver: &mut crate::repo_resolve::RepoResolver,
    mcp_cache: &mut crate::mcp_config::McpConfigCache,
    tx: &tokio::sync::mpsc::Sender<Vec<Event>>,
    shared: &SharedBackfillState,
    cursor: &mut CursorFile,
    cursor_path: &Path,
) -> bool {
    let path = &pf.path;
    let key = path.to_string_lossy().to_string();
    let mut normalizer = TranscriptNormalizer::new(host_id.to_string());

    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            bump(shared, |s| {
                s.last_error = Some(format!(
                    "opening transcript file {}: {e:#}",
                    file_name_for_error(path)
                ))
            });
            return true; // このファイルだけ諦める。全体は続行する。
        }
    };
    let file_len = file.metadata().map(|m| m.len()).unwrap_or(u64::MAX);

    // 前回この関数がこのファイルの途中で中断されていれば、「ここまでは確実に tx へ
    // 送信済み」の offset が cursor.progress に残っている。file_len でクランプするのは
    // チェックポイント後にファイルが縮んだ/差し替わった場合の防御
    // (`codex_tailer.rs::drain_one_file` の同じガードと同じ発想)。
    let resume_from = cursor
        .progress
        .get(&key)
        .copied()
        .unwrap_or(0)
        .min(file_len);

    let mut reader = BufReader::new(file);
    let mut offset: u64 = 0;
    let mut batch: Vec<Event> = Vec::with_capacity(BATCH_SIZE);

    // フェーズ 1 (resume_from > 0 の時のみ): [0, resume_from) を正規化の内部状態
    // (tool_calls/usage_counted/current_turn_id/session_id 等) を復元するためだけに
    // 読み直す。生成されるイベントは全て捨てる -- 前回の (中断された) 実行で既に
    // tx へ送信済みだから。
    while offset < resume_from {
        let mut line = String::new();
        let n = match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        offset += n as u64;
        if let Ok(raw) = serde_json::from_str::<Value>(line.trim_end()) {
            let _ = process_line(&mut normalizer, repo_resolver, mcp_cache, &raw);
        }
    }

    // フェーズ 2: 実際に処理し、行の境界でまとまった分から順に tx へ送る。
    loop {
        let mut line = String::new();
        let n = match reader.read_line(&mut line) {
            Ok(0) => break, // EOF
            Ok(n) => n,
            Err(_) => break, // 途中の I/O エラー -- ここまでで打ち切り、次のファイルへ
        };
        offset += n as u64;
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            continue;
        }
        bump(shared, |s| s.lines_read += 1);

        let raw: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => {
                bump(shared, |s| s.malformed_lines += 1);
                continue;
            }
        };

        let events = process_line(&mut normalizer, repo_resolver, mcp_cache, &raw);
        batch.extend(events);

        // 行の境界でだけ flush する (1 行が生む複数イベントを 2 つの送信に分割
        // しない) -- 「送信成功 = この行まで確実に届いた」を保証し、チェックポイント
        // (record_progress) と正確に対応させるため。
        if batch.len() >= BATCH_SIZE {
            if !send_batch(&mut batch, tx, shared) {
                return false;
            }
            record_progress(cursor, cursor_path, &key, offset);
        }
    }

    batch.extend(normalizer.finish());

    // このファイルの最終的な skip 内訳をまとめて積算する (行ごとではなく 1 回だけ --
    // TranscriptNormalizer はファイル 1 つにつき 1 インスタンスなので、これが
    // このファイルの全行を通した最終値)。
    let by_reason: Vec<(String, u64)> = normalizer
        .skipped_by_reason()
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    bump(shared, |s| {
        for (k, v) in by_reason {
            *s.skipped_by_type.entry(k).or_insert(0) += v;
        }
    });

    // ファイル末尾は BATCH_SIZE 未満でも無条件で flush する -- これが無いと、
    // 呼び出し側 (run()) が `files` へ Backfilled と記録した時点で、このファイルの
    // 末尾イベント (session.end を含む) がまだ tx に一度も送られていない、という
    // ギャップが生まれてしまう (次のファイルの分と合わせて閾値を超えるまで
    // batch に残り続けるため)。
    if !batch.is_empty() && !send_batch(&mut batch, tx, shared) {
        return false;
    }

    true
}

/// transcript の 1 行を正規化し、`ev.repo` (raw cwd から解決) と、session.start の
/// `configured_mcp_servers` (`agent.rs::drain_spool` が hook イベントに対してやるのと
/// 同じロジック — cwd の `.mcp.json`/`settings.json` から設定済み MCP サーバー名を
/// 引く) を埋めて返す。「状態復元のためだけに読み直す」フェーズと「実際に送る」
/// フェーズの両方から呼ぶ共通ヘルパー (呼び出し側が forward するか捨てるかを決める)。
fn process_line(
    normalizer: &mut TranscriptNormalizer,
    repo_resolver: &mut crate::repo_resolve::RepoResolver,
    mcp_cache: &mut crate::mcp_config::McpConfigCache,
    raw: &Value,
) -> Vec<Event> {
    // TranscriptNormalizer は cwd_hash しか保持しない (PRIVACY: 生の cwd を保持しない
    // 設計)。repo 解決/MCP 設定の参照には生の cwd が要るので、正規化に渡す前に
    // ここで読んでおく -- agent.rs::drain_spool が hook イベントの ev.repo を埋めるのと
    // 同じパターン。
    let cwd = raw.get("cwd").and_then(Value::as_str).map(str::to_string);
    let mut events = normalizer.line(raw);
    if let Some(cwd) = &cwd {
        let repo = repo_resolver.resolve(cwd);
        for ev in &mut events {
            if ev.repo.is_none() {
                ev.repo = repo.clone();
            }
            if ev.event_type == kikimimi_schema::event_type::SESSION_START {
                let servers = mcp_cache.get(cwd);
                ev.configured_mcp_servers =
                    crate::mcp_config::configured_mcp_servers_json(&servers);
            }
        }
    }
    events
}

/// `batch` を丸ごと (`mem::take`) `tx` へ送る。`tx.blocking_send` はチャンネルが
/// 詰まっていればブロックする (= 背圧、モジュール doc の「スロットル」節)。
/// 戻り値 `false` は受け手が居なくなったことを意味する。
fn send_batch(
    batch: &mut Vec<Event>,
    tx: &tokio::sync::mpsc::Sender<Vec<Event>>,
    shared: &SharedBackfillState,
) -> bool {
    let chunk = std::mem::take(batch);
    let n = chunk.len() as u64;
    if tx.blocking_send(chunk).is_err() {
        return false;
    }
    bump(shared, |s| s.events_emitted += n);
    true
}

/// `last_error` に出すファイル名。親ディレクトリ (Claude Code が URL-encode した
/// cwd そのもの) は落とし、ファイル名だけにする -- state.json/`kikimimi status` に
/// プロジェクトパスをそのまま漏らさないため (レビュー指摘の修正)。
fn file_name_for_error(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

/// 中断されず完了した (`Backfilled` または `SkippedOverlap` に決着した) ファイルの
/// 最終結果を書く。`progress` にこのファイルの途中経過が残っていれば消す
/// (このファイルはもう「未完了」ではない)。
fn record_cursor(
    cursor: &mut CursorFile,
    cursor_path: &Path,
    pf: &PlannedFile,
    outcome: FileOutcome,
) {
    let key = pf.path.to_string_lossy().to_string();
    cursor.progress.remove(&key);
    cursor.files.insert(
        key,
        CursorEntry {
            size: pf.size,
            mtime_ms: pf.mtime_ms,
            outcome,
        },
    );
    if let Err(e) = cursor.save_to(cursor_path) {
        eprintln!(
            "kikimimi agent: claude backfill: failed to save cursor file {}: {e:#}",
            cursor_path.display()
        );
    }
}

/// まだ完了していないファイルの途中経過 (バイト offset) を書く
/// (モジュール doc の「クラッシュ再開」節参照)。
fn record_progress(cursor: &mut CursorFile, cursor_path: &Path, key: &str, offset: u64) {
    cursor.progress.insert(key.to_string(), offset);
    if let Err(e) = cursor.save_to(cursor_path) {
        eprintln!(
            "kikimimi agent: claude backfill: failed to save cursor file {}: {e:#}",
            cursor_path.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_file(dir: &Path, rel: &str, contents: &str) -> PathBuf {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, contents).unwrap();
        path
    }

    fn line(ts: &str, extra: &str) -> String {
        format!(
            r#"{{"type":"system","sessionId":"sess-1","cwd":"/tmp/proj","timestamp":"{ts}"{extra}}}"#
        )
    }

    // ---- compute_boundary ----

    #[test]
    fn compute_boundary_uses_earliest_local_dt_partition_when_data_exists() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("data").join("events");
        fs::create_dir_all(data_dir.join("dt=2026-08-15")).unwrap();
        fs::create_dir_all(data_dir.join("dt=2026-07-01")).unwrap();
        fs::create_dir_all(data_dir.join("dt=2026-09-01")).unwrap();

        let boundary = compute_boundary(&data_dir, 9_999_999_999_999);
        assert_eq!(boundary.label, "dt=2026-07-01");
        assert_eq!(boundary.ts_ms, dt_start_ms("2026-07-01").unwrap());
    }

    #[test]
    fn compute_boundary_falls_back_to_first_started_at_when_no_local_data() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("data").join("events"); // never created
        let boundary = compute_boundary(&data_dir, 1_700_000_000_000);
        assert_eq!(boundary.ts_ms, 1_700_000_000_000);
        assert_eq!(boundary.label, iso_ms(1_700_000_000_000));
    }

    // ---- plan() ----

    #[test]
    fn plan_backfills_a_file_finished_before_the_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let projects = dir.path().join("projects");
        write_file(
            &projects,
            "proj1/old-session.jsonl",
            &(line("2026-01-01T00:00:00.000Z", "") + "\n"),
        );

        let boundary_ms = dt_start_ms("2026-06-01").unwrap();
        let planned = plan(&projects, &CursorFile::default(), boundary_ms);

        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].outcome, PlannedOutcome::Backfill);
    }

    #[test]
    fn plan_skips_overlap_when_last_record_is_at_or_after_the_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let projects = dir.path().join("projects");
        write_file(
            &projects,
            "proj1/live-session.jsonl",
            &(line("2026-08-01T00:00:00.000Z", "")
                + "\n"
                + &line("2026-09-01T00:00:00.000Z", "")
                + "\n"),
        );

        let boundary_ms = dt_start_ms("2026-06-01").unwrap();
        let planned = plan(&projects, &CursorFile::default(), boundary_ms);

        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].outcome, PlannedOutcome::SkippedOverlap);
    }

    #[test]
    fn plan_skips_done_when_cursor_already_has_matching_size_and_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let projects = dir.path().join("projects");
        let path = write_file(
            &projects,
            "proj1/already-done.jsonl",
            &(line("2026-01-01T00:00:00.000Z", "") + "\n"),
        );
        let meta = fs::metadata(&path).unwrap();
        let key = path.to_string_lossy().to_string();
        let cursor = CursorFile {
            files: HashMap::from([(
                key,
                CursorEntry {
                    size: meta.len(),
                    mtime_ms: mtime_ms_of(&meta),
                    outcome: FileOutcome::Backfilled,
                },
            )]),
            ..Default::default()
        };

        let boundary_ms = dt_start_ms("2026-06-01").unwrap();
        let planned = plan(&projects, &cursor, boundary_ms);

        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].outcome, PlannedOutcome::SkippedDone);
    }

    #[test]
    fn plan_treats_a_cursor_recorded_overlap_as_permanent_even_if_the_file_grows() {
        let dir = tempfile::tempdir().unwrap();
        let projects = dir.path().join("projects");
        let path = write_file(
            &projects,
            "proj1/growing.jsonl",
            &(line("2026-01-01T00:00:00.000Z", "") + "\n"),
        );
        let key = path.to_string_lossy().to_string();
        // Recorded overlap from an earlier plan(), with a *different* size/mtime than
        // the file currently has (it kept growing since) -- must still be permanent.
        let cursor = CursorFile {
            files: HashMap::from([(
                key,
                CursorEntry {
                    size: 1,
                    mtime_ms: 1,
                    outcome: FileOutcome::SkippedOverlap,
                },
            )]),
            ..Default::default()
        };

        let boundary_ms = dt_start_ms("2026-06-01").unwrap();
        let planned = plan(&projects, &cursor, boundary_ms);

        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].outcome, PlannedOutcome::SkippedOverlap);
    }

    #[test]
    fn plan_orders_files_oldest_mtime_first() {
        let dir = tempfile::tempdir().unwrap();
        let projects = dir.path().join("projects");
        let older = write_file(
            &projects,
            "proj1/older.jsonl",
            &(line("2026-01-01T00:00:00.000Z", "") + "\n"),
        );
        std::thread::sleep(Duration::from_millis(50));
        let newer = write_file(
            &projects,
            "proj1/newer.jsonl",
            &(line("2026-01-02T00:00:00.000Z", "") + "\n"),
        );

        let boundary_ms = dt_start_ms("2026-06-01").unwrap();
        let planned = plan(&projects, &CursorFile::default(), boundary_ms);

        assert_eq!(planned.len(), 2);
        assert_eq!(planned[0].path, older);
        assert_eq!(planned[1].path, newer);
    }

    // ---- cursor file round-trip ----

    #[test]
    fn cursor_file_round_trips_through_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("claude-backfill.json");
        let mut cursor = CursorFile::default();
        cursor.files.insert(
            "/tmp/a.jsonl".to_string(),
            CursorEntry {
                size: 123,
                mtime_ms: 456,
                outcome: FileOutcome::Backfilled,
            },
        );
        cursor.files.insert(
            "/tmp/b.jsonl".to_string(),
            CursorEntry {
                size: 789,
                mtime_ms: 1011,
                outcome: FileOutcome::SkippedOverlap,
            },
        );

        cursor.save_to(&path).unwrap();
        let loaded = CursorFile::load_from(&path);
        assert_eq!(loaded, cursor);
    }

    #[test]
    fn cursor_file_missing_or_corrupt_loads_as_default() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            CursorFile::load_from(&dir.path().join("nope.json")),
            CursorFile::default()
        );

        let corrupt = dir.path().join("corrupt.json");
        fs::write(&corrupt, b"not json").unwrap();
        assert_eq!(CursorFile::load_from(&corrupt), CursorFile::default());
    }

    // ---- end-to-end: plan + normalizer + FileSink lands under the OLD dt partition ----

    #[test]
    fn end_to_end_backfill_lands_events_under_the_old_dt_partition() {
        let dir = tempfile::tempdir().unwrap();
        let projects = dir.path().join("projects");
        // A finished session, safely before any conceivable boundary: one real prompt
        // (-> turn) plus the implicit session.start/session.end (both from this
        // record's own timestamp, since it's the only line).
        write_file(
            &projects,
            "proj1/old-session.jsonl",
            r#"{"type":"user","sessionId":"sess-1","cwd":"/tmp/proj","timestamp":"2026-01-01T00:00:00.000Z","promptId":"p1","message":{"role":"user","content":"hi"}}"#,
        );

        let cursor_path = dir.path().join("claude-backfill.json");
        let data_dir = dir.path().join("data").join("events"); // no local data yet

        let boundary = compute_boundary(&data_dir, dt_start_ms("2026-06-01").unwrap());
        let planned = plan(
            &projects,
            &CursorFile::load_from(&cursor_path),
            boundary.ts_ms,
        );
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].outcome, PlannedOutcome::Backfill);

        let mut normalizer = TranscriptNormalizer::new("host-1".to_string());
        let mut repo_resolver = crate::repo_resolve::RepoResolver::default();
        let mut events = Vec::new();
        for l in BufReader::new(fs::File::open(&planned[0].path).unwrap()).lines() {
            let raw: Value = serde_json::from_str(&l.unwrap()).unwrap();
            let cwd = raw.get("cwd").and_then(Value::as_str).map(str::to_string);
            let mut evs = normalizer.line(&raw);
            if let Some(cwd) = &cwd {
                let repo = repo_resolver.resolve(cwd);
                for ev in &mut evs {
                    if ev.repo.is_none() {
                        ev.repo = repo.clone();
                    }
                }
            }
            events.extend(evs);
        }
        events.extend(normalizer.finish());
        assert!(
            events.iter().any(|e| e.event_type == "session.start"),
            "expected a session.start event, got {events:?}"
        );
        assert!(events.iter().all(|e| e.dt == "2026-01-01"));

        use kikimimi_sink::{EventSink, FileSink};
        let mut sink = FileSink::new(
            data_dir.clone(),
            "host-1".to_string(),
            FileSink::DEFAULT_MAX_ROWS,
            FileSink::DEFAULT_MAX_AGE,
        );
        for ev in events {
            sink.push(ev);
        }
        let written = sink.flush().unwrap();
        assert_eq!(written.len(), 1);
        assert!(
            written[0].to_string_lossy().contains("dt=2026-01-01"),
            "expected the OLD dt partition, got {}",
            written[0].display()
        );
        assert!(written[0].exists());
    }

    // ---- discover_files / mtime_ms_of / last_record_ts_ms small units ----

    #[test]
    fn discover_files_finds_only_direct_children_jsonl_files() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "proj1/a.jsonl", "");
        write_file(dir.path(), "proj1/nested/b.jsonl", ""); // one level too deep, ignored
        write_file(dir.path(), "proj1/not-jsonl.txt", "");

        let found = discover_files(dir.path());
        assert_eq!(found.len(), 1);
        assert!(found[0].to_string_lossy().ends_with("proj1/a.jsonl"));
    }

    #[test]
    fn discover_files_on_missing_projects_dir_is_empty_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let found = discover_files(&dir.path().join("does-not-exist"));
        assert!(found.is_empty());
    }

    #[test]
    fn last_record_ts_ms_reads_the_last_timestamped_line_in_the_tail() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(
            dir.path(),
            "t.jsonl",
            &(line("2026-01-01T00:00:00.000Z", "")
                + "\n"
                + &line("2026-03-15T12:30:00.000Z", "")
                + "\n"),
        );
        let ts = last_record_ts_ms(&path, 0);
        assert_eq!(
            ts,
            dt_start_ms("2026-03-15").unwrap() + 12 * 3_600_000 + 30 * 60_000
        );
    }

    #[test]
    fn last_record_ts_ms_falls_back_to_mtime_when_no_timestamp_found() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(dir.path(), "no-ts.jsonl", "{\"type\":\"mode\"}\n");
        let ts = last_record_ts_ms(&path, 424242);
        assert_eq!(ts, 424242);
    }

    /// A single line longer than `TAIL_BYTES` must not break `last_record_ts_ms` --
    /// the truncated first (partial) line in the tail is skipped, but a shorter later
    /// line with a real timestamp is still found.
    #[test]
    fn last_record_ts_ms_handles_a_line_longer_than_the_tail_window() {
        let dir = tempfile::tempdir().unwrap();
        let huge_padding = "x".repeat(TAIL_BYTES as usize + 1024);
        let contents = format!(
            "{}\n{}\n",
            line(
                "2026-01-01T00:00:00.000Z",
                &format!(r#","pad":"{huge_padding}""#)
            ),
            line("2026-02-02T00:00:00.000Z", "")
        );
        let path = write_file(dir.path(), "huge.jsonl", &contents);
        let ts = last_record_ts_ms(&path, 0);
        assert_eq!(ts, dt_start_ms("2026-02-02").unwrap());
    }

    #[test]
    fn record_cursor_persists_across_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let cursor_path = dir.path().join("claude-backfill.json");
        let mut cursor = CursorFile::default();
        let pf = PlannedFile {
            path: PathBuf::from("/tmp/x.jsonl"),
            size: 42,
            mtime_ms: 1000,
            outcome: PlannedOutcome::Backfill,
        };
        record_cursor(&mut cursor, &cursor_path, &pf, FileOutcome::Backfilled);

        let reloaded = CursorFile::load_from(&cursor_path);
        let entry = reloaded.files.get("/tmp/x.jsonl").unwrap();
        assert_eq!(entry.size, 42);
        assert_eq!(entry.mtime_ms, 1000);
        assert_eq!(entry.outcome, FileOutcome::Backfilled);
    }

    // ---- crash-safe checkpointing (blocker fix) ----

    #[test]
    fn record_cursor_clears_any_in_progress_checkpoint_for_that_file() {
        let dir = tempfile::tempdir().unwrap();
        let cursor_path = dir.path().join("claude-backfill.json");
        let mut cursor = CursorFile::default();
        let pf = PlannedFile {
            path: PathBuf::from("/tmp/x.jsonl"),
            size: 42,
            mtime_ms: 1000,
            outcome: PlannedOutcome::Backfill,
        };
        record_progress(&mut cursor, &cursor_path, "/tmp/x.jsonl", 123);
        assert_eq!(cursor.progress.get("/tmp/x.jsonl"), Some(&123));

        record_cursor(&mut cursor, &cursor_path, &pf, FileOutcome::Backfilled);
        assert_eq!(cursor.progress.get("/tmp/x.jsonl"), None);
        let reloaded = CursorFile::load_from(&cursor_path);
        assert_eq!(reloaded.progress.get("/tmp/x.jsonl"), None);
    }

    #[test]
    fn file_name_for_error_drops_the_parent_directory() {
        // The parent directory is Claude Code's URL-encoded project cwd -- must not
        // leak into last_error/state.json/`kikimimi status` (reviewer minor finding).
        let path = Path::new("/home/yuya/.claude/projects/-home-yuya-secret-project/abc-123.jsonl");
        let shown = file_name_for_error(path);
        assert_eq!(shown, "abc-123.jsonl");
        assert!(!shown.contains("secret-project"));
    }

    #[test]
    #[serial_test::serial]
    fn process_line_populates_configured_mcp_servers_on_session_start() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("proj");
        fs::create_dir_all(&cwd).unwrap();
        fs::write(cwd.join(".mcp.json"), r#"{"mcpServers":{"playwright":{}}}"#).unwrap();
        // Isolate from this machine's real ~/.claude/settings.json + ~/.claude.json
        // (crate::mcp_config's env overrides, same pattern as mcp_config.rs's own
        // #[serial] tests) -- point them at files that don't exist, so only the
        // <cwd>/.mcp.json written above contributes any servers.
        std::env::set_var(
            "KIKIMIMI_CLAUDE_SETTINGS_PATH",
            dir.path().join("no-settings.json"),
        );
        std::env::set_var(
            "KIKIMIMI_CLAUDE_JSON_PATH",
            dir.path().join("no-claude.json"),
        );

        let raw = serde_json::json!({
            "type": "system",
            "sessionId": "sess-1",
            "cwd": cwd.to_string_lossy(),
            "timestamp": "2026-01-01T00:00:00.000Z",
        });
        let mut normalizer = TranscriptNormalizer::new("host-1".to_string());
        let mut repo_resolver = crate::repo_resolve::RepoResolver::default();
        let mut mcp_cache = crate::mcp_config::McpConfigCache::default();
        let events = process_line(&mut normalizer, &mut repo_resolver, &mut mcp_cache, &raw);

        let start = events
            .iter()
            .find(|e| e.event_type == kikimimi_schema::event_type::SESSION_START)
            .expect("expected a session.start event");
        assert_eq!(
            start.configured_mcp_servers.as_deref(),
            Some(r#"["playwright"]"#)
        );

        std::env::remove_var("KIKIMIMI_CLAUDE_SETTINGS_PATH");
        std::env::remove_var("KIKIMIMI_CLAUDE_JSON_PATH");
    }

    /// The blocker this module was reviewed for: a daemon crash/OOM-kill mid-file (the
    /// receiver disappearing partway through a large transcript) must not cause a later
    /// restart to either re-emit already-sent events (double counting -- `tool.call`/
    /// `api.request` aren't deduped across sources, only `tool.result` is) or lose any.
    #[test]
    fn crash_mid_file_resume_neither_duplicates_nor_loses_events() {
        let dir = tempfile::tempdir().unwrap();
        let projects = dir.path().join("projects");
        // 501 "user" turn lines (one real prompt each, distinct promptId/uuid) --
        // comfortably more than BATCH_SIZE so a real mid-file checkpoint fires.
        let mut contents = String::new();
        for i in 0..501 {
            contents.push_str(&format!(
                r#"{{"type":"user","sessionId":"sess-1","cwd":"/tmp/proj","timestamp":"2026-01-01T00:00:{:02}.000Z","promptId":"p{i}","uuid":"u{i}","message":{{"role":"user","content":"hi"}}}}"#,
                i % 60
            ));
            contents.push('\n');
        }
        let path = write_file(&projects, "proj1/big-session.jsonl", &contents);
        let cursor_path = dir.path().join("claude-backfill.json");
        let key = path.to_string_lossy().to_string();

        let boundary_ms = dt_start_ms("2026-06-01").unwrap();
        let mut cursor = CursorFile::load_from(&cursor_path);
        let planned = plan(&projects, &cursor, boundary_ms);
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].outcome, PlannedOutcome::Backfill);
        let pf = planned[0].clone();

        // "Crash": a channel whose receiver takes exactly one batch and disappears,
        // simulating the main loop dying mid-file (an ungraceful shutdown, not the
        // graceful try_recv() drain agent.rs does on SIGTERM/Ctrl-C).
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<Event>>(1);
        let receiver = std::thread::spawn(move || rx.blocking_recv());
        let mut repo_resolver = crate::repo_resolve::RepoResolver::default();
        let mut mcp_cache = crate::mcp_config::McpConfigCache::default();
        let shared: SharedBackfillState = Arc::new(Mutex::new(ClaudeBackfillState::default()));

        let ok = backfill_one_file(
            "host-1",
            &pf,
            &mut repo_resolver,
            &mut mcp_cache,
            &tx,
            &shared,
            &mut cursor,
            &cursor_path,
        );
        assert!(
            !ok,
            "the receiver disappearing mid-file must be reported as failure"
        );
        let first_batch = receiver
            .join()
            .unwrap()
            .expect("the first full batch must have been sent before the crash");
        assert_eq!(first_batch.len(), BATCH_SIZE);

        // Not yet marked done (run() only calls record_cursor on success, which we
        // deliberately didn't get here) -- but a checkpoint must have been persisted.
        let checkpoint = cursor.progress.get(&key).copied();
        assert!(
            matches!(checkpoint, Some(n) if n > 0),
            "expected a persisted progress checkpoint, got {checkpoint:?}"
        );
        assert!(!cursor.files.contains_key(&key));

        // "Restart": re-plan (still Backfill -- nothing in `files` yet) and this time
        // let it run to completion with a receiver that drains everything.
        let planned2 = plan(&projects, &cursor, boundary_ms);
        assert_eq!(planned2.len(), 1);
        assert_eq!(planned2[0].outcome, PlannedOutcome::Backfill);
        let pf2 = planned2[0].clone();

        let (tx2, mut rx2) = tokio::sync::mpsc::channel::<Vec<Event>>(8);
        let drain = std::thread::spawn(move || {
            let mut all = Vec::new();
            while let Some(batch) = rx2.blocking_recv() {
                all.push(batch);
            }
            all
        });
        let mut repo_resolver2 = crate::repo_resolve::RepoResolver::default();
        let mut mcp_cache2 = crate::mcp_config::McpConfigCache::default();
        let ok2 = backfill_one_file(
            "host-1",
            &pf2,
            &mut repo_resolver2,
            &mut mcp_cache2,
            &tx2,
            &shared,
            &mut cursor,
            &cursor_path,
        );
        assert!(ok2, "the resumed run must complete successfully");
        drop(tx2);
        let resumed_events: Vec<Event> = drain.join().unwrap().into_iter().flatten().collect();

        // No event sent before the crash is ever sent again after the resume.
        let before_ids: std::collections::HashSet<_> =
            first_batch.iter().map(|e| e.event_id.clone()).collect();
        for ev in &resumed_events {
            assert!(
                !before_ids.contains(&ev.event_id),
                "duplicate event across crash+resume: {ev:?}"
            );
        }
        // A session.start must appear exactly once across both runs, not once per run
        // (the sharpest symptom of the naive "re-read from line 1" bug this replaces).
        let session_starts = resumed_events
            .iter()
            .chain(first_batch.iter())
            .filter(|e| e.event_type == kikimimi_schema::event_type::SESSION_START)
            .count();
        assert_eq!(session_starts, 1);

        // Nothing is lost either: together the two runs cover every event a single,
        // uninterrupted pass over this file would have produced (501 turns +
        // session.start + session.end).
        assert_eq!(first_batch.len() + resumed_events.len(), 501 + 2);

        // What run() itself would do next: finalize the cursor and clear the checkpoint.
        record_cursor(&mut cursor, &cursor_path, &pf2, FileOutcome::Backfilled);
        assert!(!cursor.progress.contains_key(&key));
        assert!(cursor.files.contains_key(&key));
    }
}
