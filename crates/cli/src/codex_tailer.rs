//! Codex rollout tailer — daemon component (architecture.md §4「ログ tailer」, §4.1 Codex 行)。
//!
//! `~/.codex/sessions/**/rollout-*.jsonl` (`kikimimi_schema::paths::codex_sessions_dir`) を
//! 再帰的に監視し、新規追記分だけをバイト offset で追跡してテールする。offset は
//! `~/.kikimimi/codex-cursors.json` (`kikimimi_schema::paths::codex_cursors_path`) に
//! 永続化する (Observal のような特定の実装があるわけではない — architecture.md の
//! Observal への言及は競合調査の脚注であり、offset の永続化方式自体はここで新規設計した
//! ものだが、spool 側の「一時ファイル + atomic rename」(`crate::state::write_atomic`) を
//! そのまま再利用する)。
//!
//! # 設計判断
//!
//! - **新規発見したファイルの初期 offset**: デーモン起動時点で **既に存在していた**
//!   ファイルは EOF から始める (既存の大量の履歴を初回起動でまとめて取り込まない —
//!   一般的なログ tailer の「既存ファイルは末尾から、新規ファイルは先頭から」という
//!   定石)。デーモンが動き出した**後**に新規作成されたファイル (= 今まさに動いている
//!   Codex セッション) は先頭 (offset 0) から読む — 実運用ではこちらがほとんどのケース。
//! - **再開時のセッション文脈の復元**: 永続化された offset が 0 より大きいファイルを
//!   このプロセスで初めて触るとき (デーモン再起動直後など)、まず 1 行目 (rollout の
//!   規約上、常に `session_meta` — crate ルート/`kikimimi-adapter-codex` の doc 参照) を
//!   別途読んで [`kikimimi_adapter_codex::RolloutSessionCtx`] の識別系フィールド
//!   (`session_id`/`cwd_hash`/`agent_version`/`provider`/`repo`) を復元してから、永続化
//!   された offset へ実際に seek する。これにより 1 行目自体を二重に emit することなく
//!   (offset はそのまま。1 行目が生む `session.start` イベントは捨てる)、以降の行に
//!   `session_id` 等を正しく付けられる。`current_turn_id`/`current_model` はこの復元では
//!   埋まらず、次の `turn_context`/`task_started` が来るまで一時的に失われる
//!   (原則 7: 推定で埋めない — 実害は「復元直後の数行だけ turn_id/model が空」程度)。
//! - **末尾の未完成行**: 改行で終わっていない行 (書き込み中の Codex プロセスが flush
//!   している途中) は消費しない — offset をその行の手前に残し、次回スキャンで完全な行に
//!   なってから読む。
//! - **ファイルの消失/縮小**: スキャン中にファイルが読めなくなった場合はそのスキャンを
//!   諦めて次回リトライする。永続化済み offset がファイルの現在サイズを超えている場合
//!   (ローテーション等で作り直された可能性) は offset を 0 にリセットする。

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use kikimimi_adapter_codex::{CodexNormalizer, RolloutSessionCtx};
use kikimimi_schema::Event;
use serde::{Deserialize, Serialize};

const ROLLOUT_PREFIX: &str = "rollout-";
const ROLLOUT_SUFFIX: &str = ".jsonl";

/// `~/.kikimimi/codex-cursors.json` の中身。
#[derive(Debug, Default, Serialize, Deserialize, Clone, PartialEq)]
struct CursorFile {
    /// 最初のスキャンが完了した後は true。新規発見したファイルを「デーモン起動前から
    /// 存在していた (EOF から読む)」か「起動後に現れた (先頭から読む)」かを区別するために
    /// 使う (モジュール docs 参照)。
    #[serde(default)]
    initialized: bool,
    /// 発見したファイルの絶対パス (文字列) → 消費済みバイト offset。
    #[serde(default)]
    offsets: HashMap<String, u64>,
}

impl CursorFile {
    fn load_from(path: &Path) -> Self {
        match fs::read_to_string(path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(self)?;
        crate::state::write_atomic(path, &bytes)
    }
}

/// `kikimimi agent` の main loop が保持する、Codex rollout tailer の実行時状態。
pub struct CodexTailer {
    sessions_dir: PathBuf,
    cursors_path: PathBuf,
    cursors: CursorFile,
    /// このプロセス内で見た rollout ファイルごとの正規化文脈。デーモン再起動で失われる
    /// (モジュール docs の「再開時の文脈復元」参照)。
    ctx_by_path: HashMap<PathBuf, RolloutSessionCtx>,
    files_watched: u64,
    lines_read: u64,
    malformed_lines: u64,
}

impl CodexTailer {
    pub fn new() -> Self {
        Self::new_in(
            kikimimi_schema::paths::codex_sessions_dir(),
            kikimimi_schema::paths::codex_cursors_path(),
        )
    }

    pub fn new_in(sessions_dir: PathBuf, cursors_path: PathBuf) -> Self {
        let cursors = CursorFile::load_from(&cursors_path);
        Self {
            sessions_dir,
            cursors_path,
            cursors,
            ctx_by_path: HashMap::new(),
            files_watched: 0,
            lines_read: 0,
            malformed_lines: 0,
        }
    }

    pub fn files_watched(&self) -> u64 {
        self.files_watched
    }

    pub fn lines_read(&self) -> u64 {
        self.lines_read
    }

    pub fn malformed_lines(&self) -> u64 {
        self.malformed_lines
    }

    /// `~/.codex/sessions` を再帰的に走査し、各 `rollout-*.jsonl` の未読分を正規化して
    /// 返す。呼び出し側 (agent.rs) がこれを sink へ push する — spool 側の `drain_spool`
    /// と違い、sink 参照をここでは取り回さない (Codex tailer は複数ファイルにまたがる
    /// ループの中で 1 行ずつ小さく処理するため、戻り値をまとめて返す方が呼び出し側の
    /// 「source ごとにカウンタを分ける」ロジックと素直に噛み合う — `agent.rs::ingest_otlp`
    /// と同じ形)。
    ///
    /// `~/.codex` 自体が無い (Codex 未インストール) 場合は空を返すだけで、エラーには
    /// しない (正常系)。
    pub fn scan_and_drain(&mut self, normalizer: &mut CodexNormalizer) -> anyhow::Result<Vec<Event>> {
        let discovered = self.discover_files();
        self.files_watched = discovered.len() as u64;

        let mut out = Vec::new();
        for path in &discovered {
            self.drain_one_file(path, normalizer, &mut out);
        }

        self.cursors.initialized = true;
        self.cursors.save_to(&self.cursors_path)?;
        Ok(out)
    }

    fn drain_one_file(&mut self, path: &Path, normalizer: &mut CodexNormalizer, out: &mut Vec<Event>) {
        let key = path.to_string_lossy().to_string();
        let Ok(metadata) = fs::metadata(path) else {
            return; // vanished mid-scan; retry next tick
        };
        let current_len = metadata.len();

        if !self.cursors.offsets.contains_key(&key) {
            // Always start from 0: pre-existing rollout files are exactly the
            // backfill we want (a session that ran before the daemon started).
            // Offsets persist in codex-cursors.json, so nothing is read twice.
            self.cursors.offsets.insert(key.clone(), 0);
        }

        let mut offset = *self.cursors.offsets.get(&key).unwrap_or(&0);
        if offset > current_len {
            // File shrank/was replaced since we last looked; start over.
            offset = 0;
            self.ctx_by_path.remove(path);
        }

        if offset > 0 && !self.ctx_by_path.contains_key(path) {
            self.rehydrate_ctx(path, normalizer);
        }

        if current_len <= offset {
            self.cursors.offsets.insert(key, offset);
            return;
        }

        let Ok(mut file) = fs::File::open(path) else { return };
        if file.seek(SeekFrom::Start(offset)).is_err() {
            return;
        }
        let mut reader = BufReader::new(file);
        let ctx = self.ctx_by_path.entry(path.to_path_buf()).or_default();

        let mut new_offset = offset;
        loop {
            let mut line = String::new();
            let bytes_read = match reader.read_line(&mut line) {
                Ok(n) => n,
                Err(_) => break,
            };
            if bytes_read == 0 {
                break; // EOF
            }
            if !line.ends_with('\n') {
                // Partial line still being written; leave it for the next scan.
                break;
            }
            new_offset += bytes_read as u64;
            self.lines_read += 1;
            match normalizer.rollout_line(ctx, &line) {
                Ok(events) => out.extend(events),
                Err(_) => self.malformed_lines += 1,
            }
        }
        self.cursors.offsets.insert(key, new_offset);
    }

    /// 永続化された offset が 0 より大きいファイルをこのプロセスで初めて触るときに、
    /// 1 行目 (`session_meta`) を読んで `RolloutSessionCtx` の識別系フィールドだけを
    /// 復元する。1 行目が生む `session.start` イベント自体は捨てる (offset はそのまま
    /// なので、既に処理済みの 1 行目を再度 emit してはいけない)。
    fn rehydrate_ctx(&mut self, path: &Path, normalizer: &mut CodexNormalizer) {
        let Some(first_line) = read_first_line(path) else {
            return; // couldn't read it (yet) -- try again next scan
        };
        let mut ctx = RolloutSessionCtx::default();
        let _ = normalizer.rollout_line(&mut ctx, &first_line);
        self.ctx_by_path.insert(path.to_path_buf(), ctx);
    }

    fn discover_files(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        walk_dir(&self.sessions_dir, &mut out);
        out.sort();
        out
    }
}

impl Default for CodexTailer {
    fn default() -> Self {
        Self::new()
    }
}

fn walk_dir(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        match entry.file_type() {
            Ok(ft) if ft.is_dir() => walk_dir(&path, out),
            Ok(ft) if ft.is_file() && is_rollout_file(&path) => out.push(path),
            _ => {}
        }
    }
}

fn is_rollout_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.starts_with(ROLLOUT_PREFIX) && n.ends_with(ROLLOUT_SUFFIX))
        .unwrap_or(false)
}

fn read_first_line(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    let n = reader.read_line(&mut line).ok()?;
    if n == 0 || !line.ends_with('\n') {
        return None;
    }
    Some(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn write_file(dir: &Path, rel: &str, contents: &str) -> PathBuf {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, contents).unwrap();
        path
    }

    fn fixture(name: &str) -> String {
        let path = format!(
            "{}/../adapter-codex/tests/fixtures/rollout_line_{name}.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let raw = fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"));
        // Fixtures are pretty-printed; rollout JSONL is one JSON value per line.
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        format!("{}\n", serde_json::to_string(&v).unwrap())
    }

    #[test]
    fn discover_files_finds_nested_rollout_files_only() {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "2026/08/31/rollout-a.jsonl", "");
        write_file(dir.path(), "2026/08/31/rollout-b.jsonl", "");
        write_file(dir.path(), "2026/08/31/not-a-rollout.jsonl", "");
        write_file(dir.path(), "2026/08/31/rollout-c.txt", "");

        let cursors = dir.path().join("cursors.json");
        let tailer = CodexTailer::new_in(dir.path().to_path_buf(), cursors);
        let found = tailer.discover_files();
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|p| p
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("rollout-")));
    }

    #[test]
    fn missing_sessions_dir_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = dir.path().join("does-not-exist");
        let cursors = dir.path().join("cursors.json");
        let mut tailer = CodexTailer::new_in(sessions, cursors);
        let mut n = CodexNormalizer::new("host-1".into());
        let events = tailer.scan_and_drain(&mut n).unwrap();
        assert!(events.is_empty());
        assert_eq!(tailer.files_watched(), 0);
    }

        #[test]
    fn file_created_after_watcher_started_is_read_from_the_start() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = dir.path().join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        let cursors = dir.path().join("cursors.json");

        let mut tailer = CodexTailer::new_in(sessions.clone(), cursors);
        let mut n = CodexNormalizer::new("host-1".into());

        // First scan with no files yet marks the tailer "initialized".
        let first = tailer.scan_and_drain(&mut n).unwrap();
        assert!(first.is_empty());

        // A new session starts *after* the watcher is already running.
        write_file(&sessions, "rollout-new.jsonl", &fixture("session_meta"));
        let second = tailer.scan_and_drain(&mut n).unwrap();

        assert_eq!(second.len(), 1);
        assert_eq!(second[0].event_type, kikimimi_schema::event_type::SESSION_START);
        assert_eq!(tailer.lines_read(), 1);
    }

    #[test]
    fn a_pre_existing_rollout_is_backfilled_from_offset_zero() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = dir.path().join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        let cursors = dir.path().join("cursors.json");
        // File exists BEFORE the tailer ever runs (daemon started after the session).
        write_file(&sessions, "rollout-old.jsonl", &fixture("session_meta"));
        let mut tailer = CodexTailer::new_in(sessions.clone(), cursors);
        let mut n = CodexNormalizer::new("host-1".into());
        let events = tailer.scan_and_drain(&mut n).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, kikimimi_schema::event_type::SESSION_START);
    }

    #[test]
    fn incremental_appends_are_picked_up_across_scans() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = dir.path().join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        let cursors = dir.path().join("cursors.json");
        let mut tailer = CodexTailer::new_in(sessions.clone(), cursors);
        let mut n = CodexNormalizer::new("host-1".into());
        tailer.scan_and_drain(&mut n).unwrap(); // establishes "initialized"

        let path = write_file(&sessions, "rollout-live.jsonl", &fixture("session_meta"));
        let events1 = tailer.scan_and_drain(&mut n).unwrap();
        assert_eq!(events1.len(), 1);

        // Codex appends a new line to the same (still-open) file.
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(fixture("task_started").as_bytes()).unwrap();
        drop(f);

        let events2 = tailer.scan_and_drain(&mut n).unwrap();
        assert_eq!(events2.len(), 1);
        assert_eq!(events2[0].event_type, kikimimi_schema::event_type::TURN);
        assert_eq!(tailer.lines_read(), 2);

        // Nothing new: a third scan must not re-read anything.
        let events3 = tailer.scan_and_drain(&mut n).unwrap();
        assert!(events3.is_empty());
        assert_eq!(tailer.lines_read(), 2);
    }

    #[test]
    fn a_trailing_partial_line_is_not_consumed_until_complete() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = dir.path().join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        let cursors = dir.path().join("cursors.json");
        let mut tailer = CodexTailer::new_in(sessions.clone(), cursors);
        let mut n = CodexNormalizer::new("host-1".into());
        tailer.scan_and_drain(&mut n).unwrap();

        let full_line = fixture("session_meta");
        let partial = &full_line[..full_line.len() - 5]; // no trailing '\n'
        let path = write_file(&sessions, "rollout-live.jsonl", partial);
        let events = tailer.scan_and_drain(&mut n).unwrap();
        assert!(events.is_empty(), "partial line must not be consumed yet");
        assert_eq!(tailer.lines_read(), 0);

        // The writer finishes the line.
        fs::write(&path, &full_line).unwrap();
        let events2 = tailer.scan_and_drain(&mut n).unwrap();
        assert_eq!(events2.len(), 1);
        assert_eq!(tailer.lines_read(), 1);
    }

    /// Simulates a daemon restart mid-session: cursors.json already has a non-zero
    /// offset for a file this *new* CodexTailer instance has never touched -- it must
    /// re-read line 0 to seed session_id/etc without double-emitting it.
    #[test]
    fn resume_after_restart_rehydrates_session_identity_without_double_emitting_line_zero() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = dir.path().join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        let cursors_path = dir.path().join("cursors.json");

        let session_meta = fixture("session_meta");
        let task_started = fixture("task_started");
        let path;

        // First "process": the watcher starts with an empty sessions dir, then the
        // session's file appears (and is therefore read from the start) and both of
        // its lines get consumed.
        {
            let mut tailer = CodexTailer::new_in(sessions.clone(), cursors_path.clone());
            let mut n = CodexNormalizer::new("host-1".into());
            tailer.scan_and_drain(&mut n).unwrap(); // establishes "initialized"

            path = write_file(
                &sessions,
                "rollout-live.jsonl",
                &format!("{session_meta}{task_started}"),
            );
            let events = tailer.scan_and_drain(&mut n).unwrap();
            assert_eq!(events.len(), 2);
        }

        // Append a third line (token_count) as if the session continued.
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(fixture("token_count").as_bytes()).unwrap();
        drop(f);

        // "Restart": a fresh CodexTailer/CodexNormalizer, offset persisted from before.
        let mut tailer2 = CodexTailer::new_in(sessions, cursors_path);
        let mut n2 = CodexNormalizer::new("host-1".into());
        let events = tailer2.scan_and_drain(&mut n2).unwrap();

        assert_eq!(
            events.len(),
            1,
            "must only emit the new line, not re-emit session_meta/task_started"
        );
        assert_eq!(events[0].event_type, kikimimi_schema::event_type::API_REQUEST);
        assert_eq!(
            events[0].session_id.as_deref(),
            Some("REDACTED-session-id-0001"),
            "session_id must be recovered via line-0 rehydration after the restart"
        );
    }

    #[test]
    fn cursors_persist_to_disk_and_reload() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = dir.path().join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        let cursors_path = dir.path().join("cursors.json");

        {
            let mut tailer = CodexTailer::new_in(sessions.clone(), cursors_path.clone());
            let mut n = CodexNormalizer::new("host-1".into());
            tailer.scan_and_drain(&mut n).unwrap(); // establishes "initialized" with no files yet

            // File appears *after* the watcher started -> read from the start and
            // actually consumed (a real, non-trivial offset to persist).
            write_file(&sessions, "rollout-live.jsonl", &fixture("session_meta"));
            let events = tailer.scan_and_drain(&mut n).unwrap();
            assert_eq!(events.len(), 1);
        }

        let raw = fs::read_to_string(&cursors_path).unwrap();
        let parsed: CursorFile = serde_json::from_str(&raw).unwrap();
        assert!(parsed.initialized);
        assert_eq!(parsed.offsets.len(), 1);
        let (_, &offset) = parsed.offsets.iter().next().unwrap();
        assert!(offset > 0, "the consumed line's bytes must be reflected in the persisted offset");

        // A second scan with a fresh tailer must not re-read the already-consumed line.
        let mut tailer2 = CodexTailer::new_in(sessions, cursors_path);
        let mut n2 = CodexNormalizer::new("host-1".into());
        let events = tailer2.scan_and_drain(&mut n2).unwrap();
        assert!(events.is_empty());
    }
}
