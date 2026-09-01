//! kikimimi-sink::cloud — "cloud" sink (architecture.md §4 「sink (出口)」, §6, §8, §12 Stage 0)。
//!
//! `push()` はまず本文列 (`tool_input_json` / `tool_output_excerpt` / `prompt_text` /
//! `redaction_applied`) を強制的に `None` にマスクしてからバッファに積む — Stage 0 の
//! kikimimi cloud はメタデータのみ受け付ける契約 (§5.2, cloud API 契約) なので、サーバー側の
//! 防御的 NULL 化を待たず、送信前にクライアント側でも同じマスクをかける。
//!
//! `flush()` はバッファを最大 500 件ずつのバッチに分け、`{"schema":"kikimimi.v1","events":[...]}`
//! を gzip 圧縮して `POST <endpoint>/v1/events` に `Authorization: Bearer <token>` +
//! `Content-Encoding: gzip` で送る (10 秒タイムアウト、429 は `Retry-After` を 1 回だけ
//! 尊重してリトライ — 最大 2 回)。あるバッチが最終的に失敗したら、そのバッチ以降は
//! バッファに残したまま `Err` を返す (FileSink の「取りこぼさない」原則と同じ)。
//!
//! バッファは 50,000 件を上限とし、それを超えた分は古い順に
//! `~/.kikimimi/cloud-pending.jsonl` (JSON Lines, 追記) へ退避する。次にこのプロセスが
//! (再) 起動して `CloudSink::new` を呼ぶと、そのファイルを読み込んでバッファに戻し、
//! ファイルは空にする — cloud が長時間不通でもイベントを失わないための二次退避。

use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use kikimimi_schema::Event;

use crate::EventSink;

/// 1 リクエストで送る最大イベント数 (FileSink::DEFAULT_MAX_ROWS と揃えている。
/// API 契約の上限は 5,000 だが、そこまで溜めずに小刻みに送る)。
const BATCH_SIZE: usize = 500;
/// バッファに保持する最大イベント数。超過分は `cloud-pending.jsonl` へ退避する。
const MAX_BUFFERED: usize = 50_000;
/// `cloud-pending.jsonl` のハードキャップ (バイト)。architecture.md §6 の
/// オフライン退避 Parquet (`local.max_size`, 既定 2 GB, 超過分は古い順に削除
/// して `kikimimi status` に警告) と同じ考え方をこの二次退避ファイルにも適用する
/// — セキュリティレビュー: 上限が無いと、cloud が長時間不通 (障害・トークン
/// 失効・設定ミス) の間このファイルが無制限に伸び続け、ディスクを食い潰し
/// うる。超過したら最も古い行から削除する (`trim_pending_file_if_over_cap`)。
const MAX_PENDING_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// HTTP リクエストのタイムアウト。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// 429 を受け取ったときの最大試行回数 (初回 + リトライ 1 回)。
const MAX_ATTEMPTS_ON_429: u32 = 2;

#[derive(Debug, Clone, Serialize)]
struct EventsEnvelope<'a> {
    schema: &'a str,
    events: &'a [Event],
}

#[derive(Debug, Clone, Deserialize)]
struct PushResponse {
    #[allow(dead_code)]
    accepted: u64,
    #[allow(dead_code)]
    deduped: u64,
}

/// push した順序を保ったまま、バッファ滞留時間 (age 判定用) を追跡する。
/// FileSink の `Buffered` と同じ考え方 (sink/src/lib.rs 参照)。
struct Buffered {
    event: Event,
    pushed_at: Instant,
}

/// cloud への送信バッファ ("cloud" sink)。1 ホストにつき 1 つ、`kikimimi agent` が
/// `FileSink` と並行して保持する (agent.rs)。
pub struct CloudSink {
    endpoint: String,
    token: String,
    host_id: String,
    client: reqwest::blocking::Client,
    buf: VecDeque<Buffered>,
    pending_path: PathBuf,
    last_error: Option<String>,
    last_push_at_ms: Option<i64>,
}

impl CloudSink {
    /// FileSink と揃えた既定のフラッシュ閾値 (`maybe_flush` 用)。
    pub const DEFAULT_MAX_ROWS: usize = 500;
    pub const DEFAULT_MAX_AGE: Duration = Duration::from_secs(30);

    /// `endpoint` は末尾の `/` の有無を問わない (`/v1/events` を付けるときに正規化する)。
    /// 構築時に `~/.kikimimi/cloud-pending.jsonl` (`KIKIMIMI_DIR` があればそちら) を読み込み、
    /// 以前スピルされたイベントをバッファへ戻してファイルを空にする。
    pub fn new(endpoint: String, token: String, host_id: String) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .expect("building reqwest blocking client");

        let mut sink = Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            token,
            host_id,
            client,
            buf: VecDeque::new(),
            pending_path: pending_path(),
            last_error: None,
            last_push_at_ms: None,
        };
        sink.load_pending();
        sink
    }

    /// `pending() >= DEFAULT_MAX_ROWS` か、最も古いバッファ済みイベントが
    /// `DEFAULT_MAX_AGE` を超えていれば flush する。FileSink::maybe_flush と同じ形。
    pub fn maybe_flush(&mut self) -> anyhow::Result<Vec<PathBuf>> {
        if self.buf.is_empty() {
            return Ok(Vec::new());
        }
        let over_rows = self.buf.len() >= Self::DEFAULT_MAX_ROWS;
        let over_age = self
            .buf
            .front()
            .map(|b| b.pushed_at.elapsed() >= Self::DEFAULT_MAX_AGE)
            .unwrap_or(false);
        if over_rows || over_age {
            EventSink::flush(self)
        } else {
            Ok(Vec::new())
        }
    }

    /// 直近の送信 (成功/失敗いずれか) が起きた epoch ミリ秒。まだ一度も送信していなければ `None`。
    pub fn last_push_at_ms(&self) -> Option<i64> {
        self.last_push_at_ms
    }

    /// 直近の flush 失敗のエラーメッセージ。成功すると `None` に戻る。
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn host_id(&self) -> &str {
        &self.host_id
    }

    /// `cloud-pending.jsonl` を読み込んでバッファへ戻し、ファイルを空にする。
    /// ファイルが無い/読めない場合は静かに諦める (補助的な退避なので、これ自体で
    /// コンストラクタを失敗させない)。
    fn load_pending(&mut self) {
        let bytes = match fs::read(&self.pending_path) {
            Ok(b) if !b.is_empty() => b,
            _ => return,
        };
        for line in bytes.split(|&b| b == b'\n') {
            if line.is_empty() {
                continue;
            }
            if let Ok(event) = serde_json::from_slice::<Event>(line) {
                self.buf.push_back(Buffered {
                    event,
                    pushed_at: Instant::now(),
                });
            }
            // Malformed lines are skipped best-effort — this is a secondary spool, not the
            // source of truth, and a single corrupt line must not lose the rest of the file.
        }
        // The events now live in memory again; empty the file so they aren't double-loaded
        // on the next restart. Best-effort: if this fails, `enforce_cap` below will still
        // work (it appends), it'll just also re-write what's already on disk.
        let _ = fs::write(&self.pending_path, b"");
        // Defensive: if the file somehow held more than the in-memory cap (e.g. accumulated
        // across several offline runs), immediately re-spill the overflow instead of just
        // blowing past MAX_BUFFERED.
        if let Err(e) = self.enforce_cap() {
            self.last_error = Some(format!("{e:#}"));
        }
    }

    /// バッファが `MAX_BUFFERED` を超えていれば、古い順に超過分を
    /// `cloud-pending.jsonl` へ追記して in-memory バッファから外す。追記後、
    /// ファイルが `MAX_PENDING_FILE_BYTES` を超えていれば古い行から間引く
    /// (`trim_pending_file_if_over_cap`) — この二次退避ファイル自体を無制限
    /// に伸ばさないための上限 (セキュリティレビュー)。
    fn enforce_cap(&mut self) -> anyhow::Result<()> {
        if self.buf.len() <= MAX_BUFFERED {
            return Ok(());
        }
        let overflow = self.buf.len() - MAX_BUFFERED;
        if let Some(parent) = self.pending_path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        {
            let mut file = self.open_pending_file_append()?;
            for _ in 0..overflow {
                let b = self
                    .buf
                    .pop_front()
                    .expect("buf.len() > MAX_BUFFERED was just checked");
                let line = serde_json::to_string(&b.event).context("serializing spilled event")?;
                writeln!(file, "{line}")
                    .with_context(|| format!("appending to {}", self.pending_path.display()))?;
            }
        }
        self.trim_pending_file_if_over_cap()
    }

    /// `cloud-pending.jsonl` を追記モードで開く。平文のイベント本文 (`args`
    /// オプトイン時など) が乗りうるため `config.json` と同じく owner-only
    /// (`0600`) で開く — 以前は既定の umask 依存モードだった (セキュリティ
    /// レビュー)。ファイルが既に存在する場合 `mode()` は効かない (作成時に
    /// しか適用されない) ので、開いた後で明示的に `set_permissions` もかけて、
    /// このフィックス以前に作られた既存ファイルも次回書き込み時に締め直す。
    fn open_pending_file_append(&self) -> anyhow::Result<std::fs::File> {
        let mut opts = OpenOptions::new();
        opts.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            opts.mode(0o600);
        }
        let file = opts
            .open(&self.pending_path)
            .with_context(|| format!("opening {}", self.pending_path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = fs::set_permissions(&self.pending_path, fs::Permissions::from_mode(0o600));
        }
        Ok(file)
    }

    /// `cloud-pending.jsonl` が `MAX_PENDING_FILE_BYTES` を超えていたら、
    /// 古い行 (ファイル先頭側 = 最初にスピルされたもの) から間引いて上限内に
    /// 収める。architecture.md 原則 7「欠損を隠さない」に倣い、間引きが発生
    /// したことは `last_error` 経由で `kikimimi status` から見えるようにする
    /// (次の実際の送信失敗で上書きされる程度の軽い可視化だが、無音で握り
    /// 潰すよりは良い)。
    fn trim_pending_file_if_over_cap(&mut self) -> anyhow::Result<()> {
        let len = match fs::metadata(&self.pending_path) {
            Ok(m) => m.len(),
            Err(_) => return Ok(()),
        };
        if len <= MAX_PENDING_FILE_BYTES {
            return Ok(());
        }
        let bytes = fs::read(&self.pending_path)
            .with_context(|| format!("reading {}", self.pending_path.display()))?;
        let (out, dropped) = trim_to_cap(&bytes, MAX_PENDING_FILE_BYTES);
        if dropped == 0 {
            return Ok(());
        }

        fs::write(&self.pending_path, &out)
            .with_context(|| format!("rewriting {}", self.pending_path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = fs::set_permissions(&self.pending_path, fs::Permissions::from_mode(0o600));
        }
        self.last_error = Some(format!(
            "cloud-pending.jsonl exceeded {MAX_PENDING_FILE_BYTES} bytes; \
             dropped {dropped} oldest queued event(s) to stay under the cap"
        ));
        Ok(())
    }

    fn flush_impl(&mut self) -> anyhow::Result<Vec<PathBuf>> {
        while !self.buf.is_empty() {
            let take = self.buf.len().min(BATCH_SIZE);
            let batch: Vec<Event> = self
                .buf
                .iter()
                .take(take)
                .map(|b| b.event.clone())
                .collect();
            match self.send_batch(&batch) {
                Ok(_resp) => {
                    for _ in 0..take {
                        self.buf.pop_front();
                    }
                    self.last_push_at_ms = Some(now_ms());
                    self.last_error = None;
                }
                Err(e) => {
                    self.last_push_at_ms = Some(now_ms());
                    self.last_error = Some(format!("{e:#}"));
                    return Err(e);
                }
            }
        }
        Ok(Vec::new())
    }

    /// 1 バッチを送信する。429 は `Retry-After` (秒、無ければ 1 秒) を 1 回だけ待って
    /// リトライする (最大 2 試行)。それ以外の非成功ステータスやネットワークエラーは
    /// そのまま `Err` を返す (呼び出し側 `flush_impl` がバッファをそのまま保持する)。
    fn send_batch(&self, batch: &[Event]) -> anyhow::Result<PushResponse> {
        let envelope = EventsEnvelope {
            schema: kikimimi_schema::SCHEMA_VERSION,
            events: batch,
        };
        let json = serde_json::to_vec(&envelope).context("serializing events envelope")?;
        let gz = gzip_compress(&json).context("gzip-compressing events payload")?;

        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let resp = self
                .client
                .post(format!("{}/v1/events", self.endpoint))
                .bearer_auth(&self.token)
                .header(reqwest::header::CONTENT_ENCODING, "gzip")
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(gz.clone())
                .send()
                .context("sending POST /v1/events")?;

            let status = resp.status();
            if status.is_success() {
                return resp
                    .json::<PushResponse>()
                    .context("parsing POST /v1/events response body");
            }

            if status.as_u16() == 429 && attempt < MAX_ATTEMPTS_ON_429 {
                let retry_after_secs = resp
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(1);
                std::thread::sleep(Duration::from_secs(retry_after_secs));
                continue;
            }

            let body = resp.text().unwrap_or_default();
            anyhow::bail!("POST /v1/events returned {status}: {body}");
        }
    }
}

impl EventSink for CloudSink {
    /// cloud マスク (§5.2) を適用してからバッファに積む。バッファが上限を超えたら
    /// 古い順にディスクへ退避する (`enforce_cap`) — 失敗しても `push` 自体は panic せず、
    /// `last_error` に記録するだけに留める (`EventSink::push` は `Result` を返せない)。
    fn push(&mut self, mut ev: Event) {
        mask_for_cloud(&mut ev);
        self.buf.push_back(Buffered {
            event: ev,
            pushed_at: Instant::now(),
        });
        if let Err(e) = self.enforce_cap() {
            self.last_error = Some(format!("{e:#}"));
        }
    }

    fn flush(&mut self) -> anyhow::Result<Vec<PathBuf>> {
        self.flush_impl()
    }

    fn pending(&self) -> usize {
        self.buf.len()
    }
}

/// Stage 0 の cloud 契約: 本文列は常に `null` で送る (サーバー側の防御的 NULL 化を
/// 待たない — schema.rs のドキュメント参照)。
fn mask_for_cloud(ev: &mut Event) {
    ev.tool_input_json = None;
    ev.tool_output_excerpt = None;
    ev.prompt_text = None;
    ev.redaction_applied = None;
}

fn gzip_compress(bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    enc.write_all(bytes).context("writing to gzip encoder")?;
    enc.finish().context("finishing gzip stream")
}

/// Drops leading (oldest) newline-delimited, non-empty lines from `bytes`
/// until what remains is at most `cap` bytes (or exactly one line remains —
/// never fully empties a file that has content, so a single pathologically
/// large event doesn't loop forever). Returns the retained bytes (each kept
/// line followed by exactly one `\n`) and how many lines were dropped. Pure
/// (no file I/O) so the trimming logic itself has a fast, deterministic unit
/// test independent of [`MAX_PENDING_FILE_BYTES`]'s real (64 MB) size.
fn trim_to_cap(bytes: &[u8], cap: u64) -> (Vec<u8>, usize) {
    let mut lines: VecDeque<&[u8]> = bytes
        .split(|&b| b == b'\n')
        .filter(|l| !l.is_empty())
        .collect();
    let mut total: u64 = lines.iter().map(|l| l.len() as u64 + 1).sum();
    let mut dropped = 0usize;
    while total > cap && lines.len() > 1 {
        if let Some(l) = lines.pop_front() {
            total = total.saturating_sub(l.len() as u64 + 1);
            dropped += 1;
        }
    }
    let mut out = Vec::with_capacity(total as usize);
    for l in &lines {
        out.extend_from_slice(l);
        out.push(b'\n');
    }
    (out, dropped)
}

fn pending_path() -> PathBuf {
    kikimimi_schema::paths::kikimimi_dir().join("cloud-pending.jsonl")
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
    use httpmock::prelude::*;
    use serde_json::{json, Value};
    use serial_test::serial;
    use std::io::Read as _;
    use std::sync::{Arc, Mutex};

    fn sample_event(event_id: &str) -> Event {
        Event {
            event_id: event_id.to_string(),
            ts: 1_700_000_000_000,
            dt: "2026-08-30".to_string(),
            host_id: "host-abcdef1234567890".to_string(),
            agent: "claude-code".to_string(),
            source: "hook".to_string(),
            event_type: kikimimi_schema::event_type::TOOL_CALL.to_string(),
            tool_name: Some("Bash".to_string()),
            tool_input_json: Some(r#"{"command":"rm -rf /tmp/x"}"#.to_string()),
            tool_output_excerpt: Some("some output".to_string()),
            prompt_text: Some("do the thing".to_string()),
            redaction_applied: Some(true),
            duration_ms: Some(120),
            success: Some(true),
            ..Default::default()
        }
    }

    fn gunzip(bytes: &[u8]) -> Vec<u8> {
        use flate2::read::GzDecoder;
        let mut out = Vec::new();
        GzDecoder::new(bytes).read_to_end(&mut out).unwrap();
        out
    }

    /// Captures what was actually sent to the mock server, decoded, for later assertion
    /// on the test thread (the `respond_with` closure runs on the mock server's own
    /// thread/runtime — mutate shared state instead of asserting inline).
    #[derive(Clone)]
    struct Captured {
        headers: Vec<(String, String)>,
        body: Value,
    }

    fn capture(req: &HttpMockRequest) -> Captured {
        let headers = req
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        let body: Value = serde_json::from_slice(&gunzip(&req.body_vec())).unwrap();
        Captured { headers, body }
    }

    #[test]
    #[serial]
    fn flush_sends_gzip_json_masks_body_and_clears_buffer_on_success() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());

        let server = MockServer::start();
        let captured: Arc<Mutex<Vec<Captured>>> = Arc::new(Mutex::new(Vec::new()));
        let captured2 = captured.clone();
        let mock = server.mock(|when, then| {
            when.method(POST).path("/v1/events");
            then.respond_with(move |req: &HttpMockRequest| {
                captured2.lock().unwrap().push(capture(req));
                HttpMockResponse::builder()
                    .status(200)
                    .header("content-type", "application/json")
                    .body(serde_json::to_vec(&json!({"accepted": 1, "deduped": 0})).unwrap())
                    .build()
            });
        });

        let mut sink = CloudSink::new(server.base_url(), "test-token-123".into(), "host-1".into());
        sink.push(sample_event("e1"));
        assert_eq!(sink.pending(), 1);

        let written = sink.flush().unwrap();
        assert!(written.is_empty(), "CloudSink never writes local files");
        assert_eq!(sink.pending(), 0);
        assert_eq!(sink.last_error(), None);
        assert!(sink.last_push_at_ms().is_some());

        mock.assert_calls(1);
        let calls = captured.lock().unwrap();
        assert_eq!(calls.len(), 1);
        let call = &calls[0];

        let has_header = |name: &str, val: &str| {
            call.headers
                .iter()
                .any(|(k, v)| k.eq_ignore_ascii_case(name) && v == val)
        };
        assert!(has_header("content-encoding", "gzip"));
        assert!(has_header("authorization", "Bearer test-token-123"));

        assert_eq!(call.body["schema"], "kikimimi.v1");
        let events = call.body["events"].as_array().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["event_id"], "e1");
        // The mask must be applied on the wire, even though the pushed Event had Some(..).
        assert!(events[0]["tool_input_json"].is_null());
        assert!(events[0]["tool_output_excerpt"].is_null());
        assert!(events[0]["prompt_text"].is_null());
        assert!(events[0]["redaction_applied"].is_null());

        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    #[serial]
    fn flush_honors_retry_after_on_429_then_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());

        let server = MockServer::start();
        let call_count = Arc::new(Mutex::new(0u32));
        let call_count2 = call_count.clone();
        let mock = server.mock(|when, then| {
            when.method(POST).path("/v1/events");
            then.respond_with(move |_req: &HttpMockRequest| {
                let mut n = call_count2.lock().unwrap();
                *n += 1;
                if *n == 1 {
                    HttpMockResponse::builder()
                        .status(429)
                        .header("retry-after", "0")
                        .build()
                } else {
                    HttpMockResponse::builder()
                        .status(200)
                        .header("content-type", "application/json")
                        .body(serde_json::to_vec(&json!({"accepted": 1, "deduped": 0})).unwrap())
                        .build()
                }
            });
        });

        let mut sink = CloudSink::new(server.base_url(), "tok".into(), "host-1".into());
        sink.push(sample_event("e1"));

        let result = sink.flush();
        assert!(
            result.is_ok(),
            "must retry once after 429 and succeed: {result:?}"
        );
        assert_eq!(sink.pending(), 0);
        mock.assert_calls(2);

        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    #[serial]
    fn flush_gives_up_after_second_429_and_keeps_events_buffered() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());

        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST).path("/v1/events");
            then.status(429).header("retry-after", "0");
        });

        let mut sink = CloudSink::new(server.base_url(), "tok".into(), "host-1".into());
        sink.push(sample_event("e1"));
        sink.push(sample_event("e2"));

        let result = sink.flush();
        assert!(result.is_err(), "must give up after max 2 attempts");
        assert_eq!(
            sink.pending(),
            2,
            "events must remain buffered on persistent failure, not be dropped"
        );
        assert!(sink.last_error().is_some());
        mock.assert_calls(2);

        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    #[serial]
    fn flush_splits_into_batches_of_at_most_500() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());

        let server = MockServer::start();
        let batch_sizes: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(Vec::new()));
        let batch_sizes2 = batch_sizes.clone();
        let mock = server.mock(|when, then| {
            when.method(POST).path("/v1/events");
            then.respond_with(move |req: &HttpMockRequest| {
                let body: Value = serde_json::from_slice(&gunzip(&req.body_vec())).unwrap();
                let n = body["events"].as_array().unwrap().len();
                batch_sizes2.lock().unwrap().push(n);
                HttpMockResponse::builder()
                    .status(200)
                    .body(serde_json::to_vec(&json!({"accepted": n, "deduped": 0})).unwrap())
                    .build()
            });
        });

        let mut sink = CloudSink::new(server.base_url(), "tok".into(), "host-1".into());
        for i in 0..501 {
            sink.push(sample_event(&format!("e{i}")));
        }

        sink.flush().unwrap();
        mock.assert_calls(2);
        let sizes = batch_sizes.lock().unwrap().clone();
        assert_eq!(sizes, vec![500, 1]);

        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    #[serial]
    fn buffer_over_cap_spills_oldest_to_pending_file_and_reloads_on_construction() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());

        {
            let mut sink = CloudSink::new(
                "http://127.0.0.1:1".into(), // never contacted in this test
                "tok".into(),
                "host-1".into(),
            );
            for i in 0..MAX_BUFFERED + 7 {
                sink.push(sample_event(&format!("e{i}")));
            }
            assert_eq!(sink.pending(), MAX_BUFFERED, "buffer must be capped");
            assert!(
                sink.last_error().is_none(),
                "spilling to disk must not itself be an error"
            );

            let pending_file = dir.path().join("cloud-pending.jsonl");
            assert!(pending_file.exists());
            let spilled = fs::read_to_string(&pending_file).unwrap();
            let spilled_lines: Vec<&str> = spilled.lines().collect();
            assert_eq!(
                spilled_lines.len(),
                7,
                "the 7 oldest events must have been spilled"
            );
            let first: Value = serde_json::from_str(spilled_lines[0]).unwrap();
            assert_eq!(first["event_id"], "e0", "oldest event spilled first");
        }

        // A fresh CloudSink (simulating a daemon restart) must recover the spilled events.
        let sink2 = CloudSink::new("http://127.0.0.1:1".into(), "tok".into(), "host-1".into());
        assert_eq!(
            sink2.pending(),
            7,
            "spilled events must be reloaded into the buffer"
        );

        let pending_file = dir.path().join("cloud-pending.jsonl");
        let contents = fs::read_to_string(&pending_file).unwrap();
        assert!(
            contents.is_empty(),
            "pending file must be truncated after reload"
        );

        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    #[serial]
    fn flush_on_empty_buffer_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());

        let mut sink = CloudSink::new("http://127.0.0.1:1".into(), "tok".into(), "host-1".into());
        assert!(EventSink::flush(&mut sink).unwrap().is_empty());
        assert!(sink.maybe_flush().unwrap().is_empty());

        std::env::remove_var("KIKIMIMI_DIR");
    }

    // -----------------------------------------------------------------
    // Security review: unbounded `cloud-pending.jsonl` growth + perms
    // -----------------------------------------------------------------

    #[test]
    fn trim_to_cap_drops_oldest_lines_first_until_under_cap() {
        let bytes = b"aaaa\nbb\ncccccc\nd\n".to_vec();
        // Total is 4+1 + 2+1 + 6+1 + 1+1 = 17 bytes. Cap tight enough to force
        // dropping the two oldest (first) lines but keep the newest two.
        let (out, dropped) = trim_to_cap(&bytes, 9);
        assert_eq!(
            dropped, 2,
            "must drop exactly the oldest lines needed to fit"
        );
        assert_eq!(out, b"cccccc\nd\n");
    }

    #[test]
    fn trim_to_cap_is_a_noop_when_already_under_cap() {
        let bytes = b"one\ntwo\n".to_vec();
        let (out, dropped) = trim_to_cap(&bytes, 1024);
        assert_eq!(dropped, 0);
        assert_eq!(out, bytes);
    }

    #[test]
    fn trim_to_cap_never_drops_the_last_line_even_if_it_alone_exceeds_cap() {
        // A single pathologically large "line" must not be dropped to reach
        // zero lines — that would silently destroy the newest event instead
        // of just failing to shrink further.
        let bytes = b"this-one-line-is-already-bigger-than-the-cap\n".to_vec();
        let (out, dropped) = trim_to_cap(&bytes, 5);
        assert_eq!(dropped, 0);
        assert_eq!(out, bytes);
    }

    #[test]
    fn trim_to_cap_ignores_blank_lines() {
        let bytes = b"\na\n\nb\n\n".to_vec();
        let (out, dropped) = trim_to_cap(&bytes, 1024);
        assert_eq!(dropped, 0);
        assert_eq!(out, b"a\nb\n");
    }

    #[test]
    #[serial]
    fn pending_file_is_created_owner_only_regardless_of_umask() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());

        // SAFETY: umask is process-global; `#[serial]` makes this the only
        // test touching it at a time. Always restored before returning.
        let original_umask = unsafe { libc::umask(0o000) };
        let mut sink = CloudSink::new(
            "http://127.0.0.1:1".into(), // never contacted: nothing here flushes
            "tok".into(),
            "host-1".into(),
        );
        for i in 0..MAX_BUFFERED + 1 {
            sink.push(sample_event(&format!("e{i}")));
        }
        unsafe { libc::umask(original_umask) };

        let pending_file = dir.path().join("cloud-pending.jsonl");
        assert!(pending_file.exists());
        let mode = fs::metadata(&pending_file).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "cloud-pending.jsonl (may hold masked-but-still-sensitive metadata) \
             must be owner-only even under umask 000, got {mode:o}"
        );

        std::env::remove_var("KIKIMIMI_DIR");
    }
}
