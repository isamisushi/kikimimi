//! kikimimi-sink — ローカル Parquet ライター ("file" sink, architecture.md §4 「sink (出口)」, §5.3)。
//!
//! バッファされた [`kikimimi_schema::Event`] を `dt` ごとにグループ化し、
//! `kikimimi_schema::COLUMNS` の列順で zstd 圧縮 Parquet ファイルとして書き出す。
//! `file` sink はオフライン/エアギャップ用の恒久保存先であり、レイアウトは
//! BYO sink (`s3`) やエクスポートと共通 (§5.3)。
//!
//! PRIVACY: 本文列 (`tool_input_json` / `tool_output_excerpt` / `prompt_text`) の
//! マスクは呼び出し側 (正規化 → sink ごとのマスク) の責務。本クレートは
//! Event に入っている値をそのまま列に書く。

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use arrow_array::builder::{BooleanBuilder, Float64Builder, Int64Builder, StringBuilder};
use arrow_array::{ArrayRef, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;

use kikimimi_schema::{Event, COLUMNS};

mod cloud;
pub use cloud::CloudSink;

mod s3;
pub use s3::{S3Config, S3Sink};

/// イベントの出口 (sink)。architecture.md §4 「sink (出口)」: cloud (既定) / s3 (BYO) /
/// file (ローカルのみ) はすべてこの trait を実装し、同じ列定義 (`kikimimi_schema::COLUMNS`) で書く。
pub trait EventSink {
    /// イベントをバッファに積む (即座には書き出さない)。
    fn push(&mut self, ev: Event);
    /// バッファに溜まっている全イベントを書き出す。空でも Ok(vec![]) を返す。
    fn flush(&mut self) -> anyhow::Result<Vec<PathBuf>>;
    /// バッファに溜まっている未送信件数。
    fn pending(&self) -> usize;
}

/// push した順序を保ったまま、バッファに滞留している時間 (age 判定用) を追跡する。
/// ここでの「古さ」はイベントの `ts` ではなく push されてからの経過時間
/// (`Instant`) — 送信バッファの N件/T秒バッチ送信 (§4) と同じ考え方。
struct Buffered {
    event: Event,
    pushed_at: Instant,
}

/// ローカル Parquet ライター ("file" sink)。
///
/// レイアウト: `<data_dir>/dt=<dt>/<host8>-<seq:06>-<uuid8>.parquet` (§5.3)。
/// `host8` は `host_id` の先頭 8 文字、`seq` はこの `FileSink` インスタンス内で
/// 単調増加するプロセスローカルなカウンタ (dt をまたいでも共有)、`uuid8` は
/// 衝突回避のためのランダムな 8 文字。
pub struct FileSink {
    data_dir: PathBuf,
    host_id: String,
    max_rows: usize,
    max_age: Duration,
    buf: Vec<Buffered>,
    seq: u64,
}

impl FileSink {
    /// 既定のフラッシュ閾値: 500 件。
    pub const DEFAULT_MAX_ROWS: usize = 500;
    /// 既定のフラッシュ閾値: 30 秒。
    pub const DEFAULT_MAX_AGE: Duration = Duration::from_secs(30);

    pub fn new(data_dir: PathBuf, host_id: String, max_rows: usize, max_age: Duration) -> Self {
        Self {
            data_dir,
            host_id,
            max_rows,
            max_age,
            buf: Vec::new(),
            seq: 0,
        }
    }

    /// `pending() >= max_rows` か、最も古いバッファ済みイベントが `max_age` を
    /// 超えていれば [`flush`](EventSink::flush) する。どちらでもなければ何もせず
    /// `Ok(vec![])` を返す (ファイルは作られない)。
    pub fn maybe_flush(&mut self) -> anyhow::Result<Vec<PathBuf>> {
        if self.buf.is_empty() {
            return Ok(Vec::new());
        }
        let over_rows = self.buf.len() >= self.max_rows;
        let over_age = self
            .buf
            .first()
            .map(|b| b.pushed_at.elapsed() >= self.max_age)
            .unwrap_or(false);
        if over_rows || over_age {
            EventSink::flush(self)
        } else {
            Ok(Vec::new())
        }
    }

    fn next_seq(&mut self) -> u64 {
        let s = self.seq;
        self.seq += 1;
        s
    }

    /// 1 つの `dt` に属するイベント群を 1 ファイルとして書き出す
    /// ([`write_parquet_partition`] に委譲。atomic rename の理由等はそちらのドキュメント参照)。
    fn write_partition(&mut self, dt: &str, events: &[Event]) -> anyhow::Result<PathBuf> {
        let dir = self.data_dir.join(format!("dt={dt}"));
        let seq = self.next_seq();
        write_parquet_partition(&dir, &self.host_id, seq, events)
    }
}

/// `partition_dir` (呼び出し側が `dt=YYYY-MM-DD` まで組み立て済み) に `events` を 1 つの
/// Parquet ファイルとして atomic に書き出す共通ヘルパー。`FileSink` と `S3Sink` の両方が
/// これを使う (タスク要件: parquet 組み立てコードを重複させない — `S3Sink` は「BYO S3
/// sink」の staging ファイルをこれで書き、`FileSink` と全く同じ列順・圧縮・命名規則になる)。
///
/// ファイル名は `<host8>-<seq:06>-<uuid8>.parquet` (`host8` = `host_id` の先頭 8 文字、
/// `seq` は呼び出し側が渡す単調増加カウンタ、`uuid8` は衝突回避用のランダム 8 文字)。
///
/// `kikimimi query` は DuckDB で `dt=*/*.parquet` を直接 glob するため (query_cmd.rs)、
/// 他の永続化 (spool の tmp+rename, state.json の tmp+rename) と同じく、まず隠し
/// 一時ファイルにフルで書いてから同一ディレクトリ内で **atomic rename** して初めて
/// 最終ファイル名を公開する。途中状態の (中途半端な) Parquet ファイルを絶対に
/// 最終ファイル名で読ませない — architecture.md §4 の「1 呼び出し 1 ファイル +
/// atomic rename」原則は sink の出力ファイルにも同様に適用される。
pub(crate) fn write_parquet_partition(
    partition_dir: &std::path::Path,
    host_id: &str,
    seq: u64,
    events: &[Event],
) -> anyhow::Result<PathBuf> {
    fs::create_dir_all(partition_dir)
        .with_context(|| format!("creating partition dir {}", partition_dir.display()))?;

    let host8: String = host_id.chars().take(8).collect();
    let uuid8: String = uuid::Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(8)
        .collect();
    let file_name = format!("{host8}-{seq:06}-{uuid8}.parquet");
    // NOTE: must NOT end in ".parquet" — readers glob dt=*/*.parquet (dotfiles
    // included), and an in-flight temp file matching the glob makes concurrent
    // queries fail with "too small to be a Parquet file".
    let tmp_path = partition_dir.join(format!("{file_name}.tmp"));
    let path = partition_dir.join(&file_name);

    let batch = build_record_batch(events)?;
    let write_result: anyhow::Result<()> = (|| {
        let file = fs::File::create(&tmp_path)
            .with_context(|| format!("creating temp parquet file {}", tmp_path.display()))?;
        let props = WriterProperties::builder()
            .set_compression(Compression::ZSTD(Default::default()))
            .build();
        let mut writer = ArrowWriter::try_new(file, batch.schema(), Some(props))
            .context("creating parquet arrow writer")?;
        writer
            .write(&batch)
            .context("writing record batch to parquet")?;
        writer.close().context("closing parquet writer")?;
        Ok(())
    })();
    if let Err(e) = write_result {
        let _ = fs::remove_file(&tmp_path); // best-effort: don't leak a partial .tmp- file
        return Err(e);
    }

    fs::rename(&tmp_path, &path)
        .with_context(|| format!("renaming {} -> {}", tmp_path.display(), path.display()))?;

    Ok(path)
}

impl EventSink for FileSink {
    fn push(&mut self, ev: Event) {
        self.buf.push(Buffered {
            event: ev,
            pushed_at: Instant::now(),
        });
    }

    /// バッファを `dt` ごとにパーティション分けして書き出す。
    ///
    /// 途中の `dt` パーティションの書き込みが失敗した場合 (ディスクフル、権限エラー、
    /// `dt=` であるべき場所に別のファイルがある等)、**その** パーティションと、まだ
    /// 書いていない後続パーティションのイベントはバッファに戻す。以前はバッファを
    /// 呼び出し冒頭で空にしてから書いていたため、途中で失敗すると成功済みでない分の
    /// イベントが `Err` を返しつつも `pending()` からは消え、永久に失われていた
    /// (次の flush でリトライされない) — architecture.md §4 の「sink は取りこぼさない」
    /// 前提に反する。
    fn flush(&mut self) -> anyhow::Result<Vec<PathBuf>> {
        if self.buf.is_empty() {
            return Ok(Vec::new());
        }

        // dt でグループ化 (BTreeMap で決定的な順序にする)。1 dt = 1 ファイル (§5.3)。
        // `Buffered` (pushed_at 込み) のまま保持するので、失敗時にそのまま buf へ戻せる。
        let mut by_dt: BTreeMap<String, Vec<Buffered>> = BTreeMap::new();
        for b in std::mem::take(&mut self.buf) {
            by_dt.entry(b.event.dt.clone()).or_default().push(b);
        }

        let mut written = Vec::with_capacity(by_dt.len());
        let mut groups = by_dt.into_iter();
        for (dt, group) in groups.by_ref() {
            let events: Vec<Event> = group.iter().map(|b| b.event.clone()).collect();
            match self.write_partition(&dt, &events) {
                Ok(path) => written.push(path),
                Err(e) => {
                    // Put this partition's (unwritten) events, plus every partition not yet
                    // attempted, back into the buffer so the next flush retries them instead
                    // of silently dropping them.
                    self.buf.extend(group);
                    for (_, remaining) in groups {
                        self.buf.extend(remaining);
                    }
                    return Err(e);
                }
            }
        }
        Ok(written)
    }

    fn pending(&self) -> usize {
        self.buf.len()
    }
}

/// 列名 → Arrow の DataType。COLUMNS の列順を正としてこの関数で型を引く
/// (仕様: ts/duration_ms/トークン数系は Int64、cost_usd は Float64、
/// success/thinking/redaction_applied は Boolean、それ以外は Utf8)。
fn column_data_type(col: &str) -> DataType {
    match col {
        "ts" | "duration_ms" | "input_tokens" | "output_tokens" | "cache_read_tokens"
        | "cache_write_tokens" | "reasoning_tokens" => DataType::Int64,
        "cost_usd" => DataType::Float64,
        "success" | "thinking" | "redaction_applied" => DataType::Boolean,
        _ => DataType::Utf8,
    }
}

/// event_id/ts/dt/host_id/agent/source/event_type は non-null (仕様)。
fn column_nullable(col: &str) -> bool {
    !matches!(
        col,
        "event_id" | "ts" | "dt" | "host_id" | "agent" | "source" | "event_type"
    )
}

fn utf8_value(ev: &Event, col: &str) -> Option<String> {
    match col {
        "event_id" => Some(ev.event_id.clone()),
        "dt" => Some(ev.dt.clone()),
        "org_id" => ev.org_id.clone(),
        "team_id" => ev.team_id.clone(),
        "user_id" => ev.user_id.clone(),
        "user_id_source" => ev.user_id_source.clone(),
        "host_id" => Some(ev.host_id.clone()),
        "env_kind" => ev.env_kind.clone(),
        "os" => ev.os.clone(),
        "agent" => Some(ev.agent.clone()),
        "agent_version" => ev.agent_version.clone(),
        "session_id" => ev.session_id.clone(),
        "parent_session_id" => ev.parent_session_id.clone(),
        "turn_id" => ev.turn_id.clone(),
        "cwd_hash" => ev.cwd_hash.clone(),
        "repo" => ev.repo.clone(),
        "source" => Some(ev.source.clone()),
        "correlation_key" => ev.correlation_key.clone(),
        "correlation_confidence" => ev.correlation_confidence.clone(),
        "event_type" => Some(ev.event_type.clone()),
        "tool_name" => ev.tool_name.clone(),
        "tool_kind" => ev.tool_kind.clone(),
        "mcp_server" => ev.mcp_server.clone(),
        "mcp_tool" => ev.mcp_tool.clone(),
        "skill_name" => ev.skill_name.clone(),
        "error_type" => ev.error_type.clone(),
        "decision" => ev.decision.clone(),
        "decision_source" => ev.decision_source.clone(),
        "provider" => ev.provider.clone(),
        "model" => ev.model.clone(),
        "effort" => ev.effort.clone(),
        "usage_source" => ev.usage_source.clone(),
        "tool_input_json" => ev.tool_input_json.clone(),
        "tool_output_excerpt" => ev.tool_output_excerpt.clone(),
        "prompt_text" => ev.prompt_text.clone(),
        "configured_mcp_servers" => ev.configured_mcp_servers.clone(),
        _ => None,
    }
}

fn i64_value(ev: &Event, col: &str) -> Option<i64> {
    match col {
        "ts" => Some(ev.ts),
        "duration_ms" => ev.duration_ms,
        "input_tokens" => ev.input_tokens,
        "output_tokens" => ev.output_tokens,
        "cache_read_tokens" => ev.cache_read_tokens,
        "cache_write_tokens" => ev.cache_write_tokens,
        "reasoning_tokens" => ev.reasoning_tokens,
        _ => None,
    }
}

fn f64_value(ev: &Event, col: &str) -> Option<f64> {
    match col {
        "cost_usd" => ev.cost_usd,
        _ => None,
    }
}

fn bool_value(ev: &Event, col: &str) -> Option<bool> {
    match col {
        "success" => ev.success,
        "thinking" => ev.thinking,
        "redaction_applied" => ev.redaction_applied,
        _ => None,
    }
}

/// `events` を `kikimimi_schema::COLUMNS` の列順の Arrow RecordBatch に変換する。
///
/// `pub(crate)`: `s3.rs` (`S3Sink`) も同じ列定義で staging Parquet を書くため、
/// このビルダーを再利用する (parquet 組み立てコードの重複を避ける、タスク要件)。
pub(crate) fn build_record_batch(events: &[Event]) -> anyhow::Result<RecordBatch> {
    let fields: Vec<Field> = COLUMNS
        .iter()
        .map(|&name| Field::new(name, column_data_type(name), column_nullable(name)))
        .collect();
    let schema = Arc::new(Schema::new(fields));

    let mut arrays: Vec<ArrayRef> = Vec::with_capacity(COLUMNS.len());
    for &col in COLUMNS {
        let array: ArrayRef = match column_data_type(col) {
            DataType::Int64 => {
                let mut b = Int64Builder::with_capacity(events.len());
                for ev in events {
                    b.append_option(i64_value(ev, col));
                }
                Arc::new(b.finish())
            }
            DataType::Float64 => {
                let mut b = Float64Builder::with_capacity(events.len());
                for ev in events {
                    b.append_option(f64_value(ev, col));
                }
                Arc::new(b.finish())
            }
            DataType::Boolean => {
                let mut b = BooleanBuilder::with_capacity(events.len());
                for ev in events {
                    b.append_option(bool_value(ev, col));
                }
                Arc::new(b.finish())
            }
            _ => {
                let mut b = StringBuilder::new();
                for ev in events {
                    match utf8_value(ev, col) {
                        Some(v) => b.append_value(v),
                        None => b.append_null(),
                    }
                }
                Arc::new(b.finish())
            }
        };
        arrays.push(array);
    }

    RecordBatch::try_new(schema, arrays).context("building arrow record batch")
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{Array, BooleanArray, Int64Array, StringArray};
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    use std::fs::File;

    fn sample_event(dt: &str, event_id: &str, tool_name: &str) -> Event {
        Event {
            event_id: event_id.to_string(),
            ts: 1_700_000_000_000,
            dt: dt.to_string(),
            host_id: "host-abcdef1234567890".to_string(),
            agent: "claude-code".to_string(),
            source: "hook".to_string(),
            event_type: kikimimi_schema::event_type::TOOL_CALL.to_string(),
            tool_name: Some(tool_name.to_string()),
            duration_ms: Some(120),
            success: Some(true),
            cost_usd: Some(0.0042),
            ..Default::default()
        }
    }

    #[test]
    fn flush_writes_one_file_per_dt_with_expected_schema_and_values() {
        let dir = tempfile::tempdir().unwrap();
        let mut sink = FileSink::new(
            dir.path().to_path_buf(),
            "host-abcdef1234567890".into(),
            500,
            Duration::from_secs(30),
        );

        sink.push(sample_event("2026-08-30", "e1", "Bash"));
        sink.push(sample_event("2026-08-30", "e2", "mcp__github__get_issue"));
        sink.push(sample_event("2026-08-31", "e3", "Read"));
        assert_eq!(sink.pending(), 3);

        let written = sink.flush().unwrap();
        assert_eq!(written.len(), 2, "expected one file per dt");
        assert_eq!(sink.pending(), 0);
        for p in &written {
            assert!(p.exists());
            assert!(p.extension().map(|e| e == "parquet").unwrap_or(false));
        }

        let path_30 = written
            .iter()
            .find(|p| p.to_string_lossy().contains("dt=2026-08-30"))
            .expect("dt=2026-08-30 partition file");

        let file = File::open(path_30).unwrap();
        let builder = ParquetRecordBatchReaderBuilder::try_new(file).unwrap();

        // Column order must match kikimimi_schema::COLUMNS exactly.
        let field_names: Vec<&str> = builder
            .schema()
            .fields()
            .iter()
            .map(|f| f.name().as_str())
            .collect();
        assert_eq!(field_names, COLUMNS);

        let mut reader = builder.build().unwrap();
        let batch = reader.next().unwrap().unwrap();
        assert_eq!(batch.num_rows(), 2, "dt=2026-08-30 should have 2 rows");
        assert!(reader.next().is_none(), "expected a single row group/batch");

        let event_id_col = batch
            .column_by_name("event_id")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(event_id_col.value(0), "e1");
        assert_eq!(event_id_col.value(1), "e2");

        let duration_col = batch
            .column_by_name("duration_ms")
            .unwrap()
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(duration_col.value(0), 120);

        let success_col = batch
            .column_by_name("success")
            .unwrap()
            .as_any()
            .downcast_ref::<BooleanArray>()
            .unwrap();
        assert!(success_col.value(0));

        // org_id was never set on the sample events: must come back all-null, not "".
        let org_col = batch.column_by_name("org_id").unwrap();
        assert_eq!(org_col.null_count(), 2);
    }

    /// `configured_mcp_servers` (末尾追加列, §5.1) は他の Utf8 列と同じく
    /// `utf8_value` 経由で Parquet に書かれる — マッチアーム漏れの回帰テスト。
    #[test]
    fn flush_writes_configured_mcp_servers_column() {
        let dir = tempfile::tempdir().unwrap();
        let mut sink = FileSink::new(
            dir.path().to_path_buf(),
            "host-abcdef1234567890".into(),
            500,
            Duration::from_secs(30),
        );
        let mut ev = sample_event("2026-09-02", "e1", "Bash");
        ev.event_type = kikimimi_schema::event_type::SESSION_START.to_string();
        ev.configured_mcp_servers = Some(r#"["github","playwright"]"#.to_string());
        sink.push(ev);
        let written = sink.flush().unwrap();

        let file = File::open(&written[0]).unwrap();
        let builder = ParquetRecordBatchReaderBuilder::try_new(file).unwrap();
        let mut reader = builder.build().unwrap();
        let batch = reader.next().unwrap().unwrap();
        let col = batch
            .column_by_name("configured_mcp_servers")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(col.value(0), r#"["github","playwright"]"#);
    }

    #[test]
    fn maybe_flush_honors_max_rows() {
        let dir = tempfile::tempdir().unwrap();
        let mut sink = FileSink::new(
            dir.path().to_path_buf(),
            "hosthosthost".into(),
            3,
            Duration::from_secs(3600),
        );

        sink.push(sample_event("2026-08-30", "a", "Bash"));
        sink.push(sample_event("2026-08-30", "b", "Bash"));
        assert!(
            sink.maybe_flush().unwrap().is_empty(),
            "below max_rows, no flush yet"
        );
        assert_eq!(sink.pending(), 2);

        sink.push(sample_event("2026-08-30", "c", "Bash"));
        let written = sink.maybe_flush().unwrap();
        assert_eq!(written.len(), 1, "hit max_rows, should flush");
        assert_eq!(sink.pending(), 0);
    }

    #[test]
    fn maybe_flush_honors_max_age() {
        let dir = tempfile::tempdir().unwrap();
        let mut sink = FileSink::new(
            dir.path().to_path_buf(),
            "hosthosthost".into(),
            500,
            Duration::from_millis(20),
        );

        sink.push(sample_event("2026-08-30", "a", "Bash"));
        assert!(sink.maybe_flush().unwrap().is_empty(), "too young to flush");

        std::thread::sleep(Duration::from_millis(30));
        let written = sink.maybe_flush().unwrap();
        assert_eq!(written.len(), 1, "oldest buffered event exceeded max_age");
    }

    #[test]
    fn file_naming_uses_host8_zero_padded_seq_and_uuid8() {
        let dir = tempfile::tempdir().unwrap();
        let mut sink = FileSink::new(
            dir.path().to_path_buf(),
            "abcdefgh-rest-of-a-uuid".into(),
            500,
            Duration::from_secs(30),
        );
        sink.push(sample_event("2026-08-30", "a", "Bash"));
        let written = sink.flush().unwrap();
        assert_eq!(written.len(), 1);

        let name = written[0].file_name().unwrap().to_str().unwrap();
        // host8 = first 8 chars of host_id, seq zero-padded to 6 digits, then uuid8.
        let rest = name
            .strip_prefix("abcdefgh-000000-")
            .expect("host8-seq prefix");
        let uuid8 = rest.strip_suffix(".parquet").expect(".parquet suffix");
        assert_eq!(uuid8.len(), 8);
        assert!(uuid8.chars().all(|c| c.is_ascii_hexdigit()));

        assert!(written[0].to_string_lossy().contains(&format!(
            "{}dt=2026-08-30{}",
            std::path::MAIN_SEPARATOR,
            std::path::MAIN_SEPARATOR
        )));
    }

    #[test]
    fn seq_increments_per_file_across_flushes_and_dt_partitions() {
        let dir = tempfile::tempdir().unwrap();
        let mut sink = FileSink::new(
            dir.path().to_path_buf(),
            "hosthosthost".into(),
            500,
            Duration::from_secs(30),
        );

        sink.push(sample_event("2026-08-30", "a", "Bash"));
        sink.push(sample_event("2026-08-31", "b", "Bash"));
        let mut first = sink.flush().unwrap();
        first.sort();

        sink.push(sample_event("2026-09-01", "c", "Bash"));
        let second = sink.flush().unwrap();

        let seq_of = |p: &PathBuf| -> u64 {
            let name = p.file_name().unwrap().to_str().unwrap();
            name.split('-').nth(1).unwrap().parse().unwrap()
        };

        let mut seqs: Vec<u64> = first
            .iter()
            .map(seq_of)
            .chain(second.iter().map(seq_of))
            .collect();
        seqs.sort();
        assert_eq!(
            seqs,
            vec![0, 1, 2],
            "seq must be unique and monotonic across dt and flushes"
        );
    }

    #[test]
    fn write_partition_leaves_no_tmp_file_behind_on_success() {
        let dir = tempfile::tempdir().unwrap();
        let mut sink = FileSink::new(
            dir.path().to_path_buf(),
            "host".into(),
            500,
            Duration::from_secs(30),
        );
        sink.push(sample_event("2026-08-30", "a", "Bash"));
        let written = sink.flush().unwrap();
        assert_eq!(written.len(), 1);

        let partition_dir = dir.path().join("dt=2026-08-30");
        let names: Vec<String> = fs::read_dir(&partition_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(
            names,
            vec![written[0]
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string()]
        );
        assert!(
            names.iter().all(|n| !n.ends_with(".tmp")),
            "no leftover temp parquet file, got {names:?}"
        );
    }

    #[test]
    fn flush_keeps_unwritten_events_buffered_on_partition_write_failure() {
        let dir = tempfile::tempdir().unwrap();
        // Sabotage the dt=2026-08-31 partition: a regular file sits where a directory
        // must go, so `write_partition`'s `create_dir_all` fails for that dt only.
        let sabotage = dir.path().join("dt=2026-08-31");
        fs::write(&sabotage, b"not a directory").unwrap();

        let mut sink = FileSink::new(
            dir.path().to_path_buf(),
            "host".into(),
            500,
            Duration::from_secs(30),
        );
        sink.push(sample_event("2026-08-30", "a", "Bash")); // dt sorts before the sabotaged one
        sink.push(sample_event("2026-08-31", "b", "Bash")); // will fail
        sink.push(sample_event("2026-09-01", "c", "Bash")); // never attempted (sorts after)
        assert_eq!(sink.pending(), 3);

        let result = sink.flush();
        assert!(result.is_err(), "flush must surface the partition failure");

        // The failed partition's event AND the not-yet-attempted later partition's event
        // must both still be pending — not lost — so the next flush can retry them.
        assert_eq!(
            sink.pending(),
            2,
            "events for the failed + not-yet-attempted partitions must remain buffered"
        );

        // The dt=2026-08-30 partition (before the sabotaged one) was already durably
        // written to disk before the failure was hit.
        let good_dir = dir.path().join("dt=2026-08-30");
        assert!(good_dir.is_dir());
        assert_eq!(fs::read_dir(&good_dir).unwrap().count(), 1);

        // Fix the sabotage and retry: the previously-buffered events must still flush.
        fs::remove_file(&sabotage).unwrap();
        let written = sink.flush().unwrap();
        assert_eq!(written.len(), 2, "both remaining partitions now succeed");
        assert_eq!(sink.pending(), 0);
    }

    #[test]
    fn flush_on_empty_buffer_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut sink = FileSink::new(
            dir.path().to_path_buf(),
            "host".into(),
            500,
            Duration::from_secs(30),
        );
        assert!(EventSink::flush(&mut sink).unwrap().is_empty());
        assert!(sink.maybe_flush().unwrap().is_empty());
    }
}
