//! `~/.kikimimi/state.json` — daemon の稼働状況を可視化するための小さな状態ファイル。
//! `kikimimi agent` が書き、`kikimimi status` が読む。
//!
//! 書き込みは呼び出し側 (agent.rs) が atomic (tmp+rename) に行う。

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::Context;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct EventsBySource {
    pub hook: u64,
    pub otel: u64,
    /// `source = "log"` (architecture.md §5.1) — 現状は Codex の rollout tailer のみが
    /// ここに計上する。`#[serde(default)]`: Codex tailer 対応前の旧い state.json も読める。
    #[serde(default)]
    pub log: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LastFlush {
    pub at_ms: i64,
    pub files: Vec<String>,
}

/// `kikimimi login` 済み (config.json に `cloud.token` がある) なデーモンの cloud sink の
/// 現況スナップショット。`kikimimi_sink::CloudSink` の getter からそのまま作る (agent.rs
/// `sync_cloud_state`) — state.json 側は薄い写しを持つだけで、真の状態は sink 側にある。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CloudState {
    pub endpoint: String,
    pub pending: usize,
    pub last_push_at: Option<i64>,
    pub last_error: Option<String>,
}

/// `kikimimi sink add s3` 済み (config.json に `s3` がある) なデーモンの BYO S3 sink の
/// 現況スナップショット。`kikimimi_sink::S3Sink` の getter からそのまま作る (agent.rs
/// `sync_s3_state`) — `CloudState` と同じ形。`url` は秘密情報ではないのでそのまま出す
/// (redact しない)。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct S3State {
    pub url: String,
    pub pending: usize,
    pub last_push_at: Option<i64>,
    pub last_error: Option<String>,
}

/// `kikimimi agent` の Codex rollout tailer (architecture.md §4「ログ tailer」, §4.1 Codex
/// 行) の現況スナップショット。`~/.codex` が無い/Codex を使っていないマシンでは
/// `files_watched == 0` のまま (エラーではない — Codex 未インストールは正常系)。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct CodexTailerState {
    /// `~/.codex/sessions` 配下で見つかった `rollout-*.jsonl` の総数 (直近スキャン時点)。
    pub files_watched: u64,
    /// 累計で読み取った (正規化を試みた) 行数。
    pub lines_read: u64,
    /// JSON として壊れていて読めなかった行数 (`agent.rs::drain_spool` の
    /// `malformed_spool` と同じ考え方)。
    pub malformed_lines: u64,
    /// 未対応の envelope/payload/item 種別でスキップした累計件数
    /// (`kikimimi_adapter_codex::CodexNormalizer::skipped()`)。
    pub skipped: u64,
    /// 上記の理由別内訳 (`skipped_by_reason()`)。
    #[serde(default)]
    pub skipped_by_reason: BTreeMap<String, u64>,
}

/// architecture.md §8 (個人ビュー/ローカル): the local web UI's current port and its
/// per-daemon-start auth token. `token` is regenerated every `kikimimi agent` start (not
/// persisted across restarts) — `kikimimi status`/`kikimimi web` read it fresh from here so the
/// URL they print always matches whatever the *running* daemon actually accepts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct WebState {
    pub port: u16,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentState {
    pub pid: u32,
    pub started_at_ms: i64,
    pub events_by_source: EventsBySource,
    pub skipped: u64,
    /// `skipped` の理由別内訳 (kikimimi-adapter-claude::Normalizer::skipped_by_reason に加え、
    /// デーモン側で読めなかった/パースできなかった spool ファイルの件数を
    /// "malformed_spool" キーで含む。agent.rs 参照)。
    /// `#[serde(default)]`: このフィールドが無い旧い state.json も読める。
    #[serde(default)]
    pub skipped_by_reason: BTreeMap<String, u64>,
    pub last_event_ts: Option<i64>,
    pub last_flush: Option<LastFlush>,
    pub otlp_port: u16,
    pub otlp_error: Option<String>,
    /// 直近の sink flush が失敗した場合のエラーメッセージ (`Some` の間は buffered
    /// events がディスクに書かれず溜まっている)。次の flush が成功したら `None` に戻す。
    /// `#[serde(default)]`: 旧い state.json (このフィールドが無い) も読める。
    #[serde(default)]
    pub last_flush_error: Option<String>,
    /// `kikimimi login` していないデーモン (cloud sink 無し) は `None`。
    /// `#[serde(default)]`: cloud 対応前の旧い state.json も読める。
    #[serde(default)]
    pub cloud: Option<CloudState>,
    /// `kikimimi sink add s3` していないデーモン (s3 sink 無し) は `None`。
    /// `#[serde(default)]`: s3 sink 対応前の旧い state.json も読める。
    #[serde(default)]
    pub s3: Option<S3State>,
    /// ローカル web UI の現在のポートとトークン (architecture.md §8)。
    /// `#[serde(default)]`: web UI 対応前の旧い state.json も読める
    /// (その場合 port=0/token="" — `kikimimi status`/`kikimimi web` はこれを「web UI 未起動」
    /// として扱う)。
    #[serde(default)]
    pub web: WebState,
    /// web UI の axum サーバーの bind に失敗した場合のエラーメッセージ
    /// (`otlp_error` と同じ役割)。`Some` の間 `kikimimi status`/`kikimimi web` は URL を出さない。
    #[serde(default)]
    pub web_error: Option<String>,
    /// Codex rollout tailer の現況 (architecture.md §4.1 Codex 行)。
    /// `#[serde(default)]`: Codex tailer 対応前の旧い state.json も読める。
    #[serde(default)]
    pub codex: CodexTailerState,
}

impl AgentState {
    pub fn new(pid: u32, started_at_ms: i64, otlp_port: u16) -> Self {
        Self {
            pid,
            started_at_ms,
            events_by_source: EventsBySource::default(),
            skipped: 0,
            skipped_by_reason: BTreeMap::new(),
            last_event_ts: None,
            last_flush: None,
            otlp_port,
            otlp_error: None,
            last_flush_error: None,
            cloud: None,
            s3: None,
            web: WebState::default(),
            web_error: None,
            codex: CodexTailerState::default(),
        }
    }

    pub fn load_from(path: &Path) -> anyhow::Result<Self> {
        let raw =
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
    }

    /// tmp ファイルに書いてから rename する (途中状態を読ませない)。
    pub fn save(&self) -> anyhow::Result<()> {
        self.save_to(&kikimimi_schema::paths::state_path())
    }

    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let bytes = serde_json::to_vec_pretty(self).context("serializing state.json")?;
        write_atomic(path, &bytes)
    }
}

/// 同一ディレクトリ内の一時ファイルに書いて rename する共通ヘルパー。
///
/// セキュリティレビュー (TOCTOU): 一時ファイルは作成の瞬間から常に owner-only
/// (`0600`) で作る (`OpenOptionsExt::mode`) — `fs::write` (既定の umask 依存の
/// モード、典型的には `0644`) で作ってから rename 後に chmod する実装だと、
/// rename から chmod までの間 `config.json` (cloud のベアラートークンを含む)
/// が world/group-readable な最終パスに一瞬存在してしまう。rename は
/// permission bits をそのまま引き継ぐので、作成時点で 0600 にしておけば
/// その窓は原理的に存在しない。
pub fn write_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = dir.join(format!(
        ".tmp-{}-{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("state"),
        std::process::id()
    ));
    write_tmp_owner_only(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

#[cfg(unix)]
fn write_tmp_owner_only(tmp: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(tmp)?;
    file.write_all(bytes)
}

#[cfg(not(unix))]
fn write_tmp_owner_only(tmp: &Path, bytes: &[u8]) -> std::io::Result<()> {
    fs::write(tmp, bytes)
}

/// state.json が存在しない場合と壊れている場合を区別せず None として扱いたい呼び出し向け。
pub fn load_opt(path: &Path) -> Option<AgentState> {
    match AgentState::load_from(path) {
        Ok(s) => Some(s),
        Err(e) => {
            // NotFound は静かに None。それ以外 (壊れた JSON 等) は呼び出し側の判断に委ねるため
            // ここでは詳細を握りつぶす (status 表示は「state.json なし」として扱う)。
            let _ = e; // keep for potential future logging
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let mut s = AgentState::new(1234, 1_700_000_000_000, 4318);
        s.events_by_source.hook = 5;
        s.events_by_source.otel = 2;
        s.skipped = 1;
        s.skipped_by_reason.insert("PreCompact".into(), 1);
        s.last_event_ts = Some(1_700_000_000_500);
        s.last_flush = Some(LastFlush {
            at_ms: 1_700_000_001_000,
            files: vec!["a.parquet".into()],
        });
        s.otlp_error = Some("port in use".into());
        s.last_flush_error = Some("disk full".into());

        s.save_to(&path).unwrap();
        let loaded = AgentState::load_from(&path).unwrap();
        assert_eq!(loaded, s);
    }

    #[test]
    fn load_tolerates_state_json_from_before_last_flush_error_existed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        // No "last_flush_error" key at all (as an older kikimimi agent would have written).
        let old_json = serde_json::json!({
            "pid": 1,
            "started_at_ms": 0,
            "events_by_source": { "hook": 0, "otel": 0 },
            "skipped": 0,
            "last_event_ts": null,
            "last_flush": null,
            "otlp_port": 4318,
            "otlp_error": null
        });
        fs::write(&path, serde_json::to_vec(&old_json).unwrap()).unwrap();

        let loaded = AgentState::load_from(&path).unwrap();
        assert_eq!(loaded.last_flush_error, None);
    }

    #[test]
    fn load_tolerates_state_json_from_before_cloud_existed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        // No "cloud" key at all (as a pre-`kikimimi login` kikimimi agent would have written).
        let old_json = serde_json::json!({
            "pid": 1,
            "started_at_ms": 0,
            "events_by_source": { "hook": 0, "otel": 0 },
            "skipped": 0,
            "last_event_ts": null,
            "last_flush": null,
            "otlp_port": 4318,
            "otlp_error": null,
            "last_flush_error": null
        });
        fs::write(&path, serde_json::to_vec(&old_json).unwrap()).unwrap();

        let loaded = AgentState::load_from(&path).unwrap();
        assert_eq!(loaded.cloud, None);
    }

    #[test]
    fn save_and_load_roundtrip_with_cloud_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let mut s = AgentState::new(1, 0, 4318);
        s.cloud = Some(CloudState {
            endpoint: "https://cloud.example".into(),
            pending: 3,
            last_push_at: Some(1_700_000_002_000),
            last_error: Some("timed out".into()),
        });

        s.save_to(&path).unwrap();
        let loaded = AgentState::load_from(&path).unwrap();
        assert_eq!(loaded, s);
    }

    /// backward-compat: state.json written before the `s3` BYO sink field existed
    /// (no "s3" key at all) must still load, with `s3: None`.
    #[test]
    fn load_tolerates_state_json_from_before_s3_existed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let old_json = serde_json::json!({
            "pid": 1,
            "started_at_ms": 0,
            "events_by_source": { "hook": 0, "otel": 0 },
            "skipped": 0,
            "last_event_ts": null,
            "last_flush": null,
            "otlp_port": 4318,
            "otlp_error": null,
            "last_flush_error": null,
            "cloud": null
        });
        fs::write(&path, serde_json::to_vec(&old_json).unwrap()).unwrap();

        let loaded = AgentState::load_from(&path).unwrap();
        assert_eq!(loaded.s3, None);
    }

    #[test]
    fn save_and_load_roundtrip_with_s3_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let mut s = AgentState::new(1, 0, 4318);
        s.s3 = Some(S3State {
            url: "s3://my-bucket/team".into(),
            pending: 5,
            last_push_at: Some(1_700_000_003_000),
            last_error: Some("aws CLI not found".into()),
        });

        s.save_to(&path).unwrap();
        let loaded = AgentState::load_from(&path).unwrap();
        assert_eq!(loaded, s);
    }

    #[test]
    fn load_tolerates_state_json_from_before_skipped_by_reason_existed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        // No "skipped_by_reason" key at all (as an older kikimimi agent would have written).
        let old_json = serde_json::json!({
            "pid": 1,
            "started_at_ms": 0,
            "events_by_source": { "hook": 0, "otel": 0 },
            "skipped": 3,
            "last_event_ts": null,
            "last_flush": null,
            "otlp_port": 4318,
            "otlp_error": null,
            "last_flush_error": null
        });
        fs::write(&path, serde_json::to_vec(&old_json).unwrap()).unwrap();

        let loaded = AgentState::load_from(&path).unwrap();
        assert_eq!(loaded.skipped, 3);
        assert!(loaded.skipped_by_reason.is_empty());
    }

    #[test]
    fn save_creates_missing_parent_dir() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("state.json");
        let s = AgentState::new(1, 0, 4318);
        s.save_to(&path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn save_leaves_no_tmp_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        AgentState::new(1, 0, 4318).save_to(&path).unwrap();
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["state.json"]);
    }

    /// TOCTOU regression (security review): the file must be owner-only from
    /// the moment it's created, not just after a later chmod — pinned here by
    /// running under a maximally permissive umask, which only a "chmod after
    /// the fact" implementation could still leak through. `#[serial]`: umask
    /// is process-global.
    #[cfg(unix)]
    #[test]
    #[serial]
    fn write_atomic_creates_the_file_owner_only_regardless_of_umask() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");

        // SAFETY: umask is a per-process setting; `#[serial]` keeps this the
        // only test mutating it at a time. Always restored before returning.
        let original_umask = unsafe { libc::umask(0o000) };
        let result = write_atomic(&path, b"{}");
        unsafe { libc::umask(original_umask) };
        result.unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "write_atomic must produce an owner-only file even under umask 000, got {mode:o}"
        );
    }

    #[test]
    fn save_and_load_roundtrip_with_web_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let mut s = AgentState::new(1, 0, 4318);
        s.web = WebState {
            port: 4319,
            token: "0123456789abcdef0123456789abcdef".into(),
        };
        s.web_error = Some("address in use".into());

        s.save_to(&path).unwrap();
        let loaded = AgentState::load_from(&path).unwrap();
        assert_eq!(loaded, s);
    }

    #[test]
    fn load_tolerates_state_json_from_before_web_existed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        // No "web"/"web_error" keys at all (as a pre-web-UI kikimimi agent would have
        // written).
        let old_json = serde_json::json!({
            "pid": 1,
            "started_at_ms": 0,
            "events_by_source": { "hook": 0, "otel": 0 },
            "skipped": 0,
            "last_event_ts": null,
            "last_flush": null,
            "otlp_port": 4318,
            "otlp_error": null,
            "last_flush_error": null,
            "cloud": null
        });
        fs::write(&path, serde_json::to_vec(&old_json).unwrap()).unwrap();

        let loaded = AgentState::load_from(&path).unwrap();
        assert_eq!(loaded.web, WebState::default());
        assert_eq!(loaded.web_error, None);
    }

    #[test]
    fn save_and_load_roundtrip_with_codex_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let mut s = AgentState::new(1, 0, 4318);
        s.codex = CodexTailerState {
            files_watched: 3,
            lines_read: 120,
            malformed_lines: 1,
            skipped: 4,
            skipped_by_reason: BTreeMap::from([("rollout:world_state".to_string(), 4)]),
        };

        s.save_to(&path).unwrap();
        let loaded = AgentState::load_from(&path).unwrap();
        assert_eq!(loaded, s);
    }

    /// backward-compat: state.json written before the Codex tailer existed (no "codex"
    /// key at all) must still load, with `codex` defaulting to all-zero/empty.
    #[test]
    fn load_tolerates_state_json_from_before_codex_existed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let old_json = serde_json::json!({
            "pid": 1,
            "started_at_ms": 0,
            "events_by_source": { "hook": 0, "otel": 0 },
            "skipped": 0,
            "last_event_ts": null,
            "last_flush": null,
            "otlp_port": 4318,
            "otlp_error": null,
            "last_flush_error": null,
            "cloud": null
        });
        fs::write(&path, serde_json::to_vec(&old_json).unwrap()).unwrap();

        let loaded = AgentState::load_from(&path).unwrap();
        assert_eq!(loaded.codex, CodexTailerState::default());
    }

    /// backward-compat: `EventsBySource` written before the "log" source existed (no
    /// "log" key at all) must still load, defaulting to 0.
    #[test]
    fn load_tolerates_events_by_source_from_before_log_existed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let old_json = serde_json::json!({
            "pid": 1,
            "started_at_ms": 0,
            "events_by_source": { "hook": 3, "otel": 2 },
            "skipped": 0,
            "last_event_ts": null,
            "last_flush": null,
            "otlp_port": 4318,
            "otlp_error": null
        });
        fs::write(&path, serde_json::to_vec(&old_json).unwrap()).unwrap();

        let loaded = AgentState::load_from(&path).unwrap();
        assert_eq!(loaded.events_by_source.hook, 3);
        assert_eq!(loaded.events_by_source.otel, 2);
        assert_eq!(loaded.events_by_source.log, 0);
    }

    #[test]
    fn load_missing_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nope.json");
        assert!(AgentState::load_from(&path).is_err());
        assert!(load_opt(&path).is_none());
    }
}
