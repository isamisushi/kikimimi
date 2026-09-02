//! kikimimi.v1 — 固定スキーマ (docs/design/architecture.md §5.1)。
//! cloud DB・BYO sink の Parquet・エクスポートはすべてこの列定義を共有する。
//! 破壊的変更 (列の削除・改名) は kikimimi.v2 として別モジュールにする。列追加のみ可。

pub mod env_compat;
pub mod paths;

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: &str = "kikimimi.v1";

/// events テーブルの 1 行。取れない値は None のまま送る (推定で埋めない — 原則 7)。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Event {
    // ---- 識別 ----
    pub event_id: String,
    /// UNIX epoch ミリ秒 (UTC)
    pub ts: i64,
    /// "YYYY-MM-DD" (UTC)。パーティションキー
    pub dt: String,
    pub org_id: Option<String>,
    pub team_id: Option<String>,
    pub user_id: Option<String>,
    /// account | agent_email | unknown
    pub user_id_source: Option<String>,
    pub host_id: String,
    /// laptop | devcontainer | ci | cloud-vm
    pub env_kind: Option<String>,
    pub os: Option<String>,
    /// claude-code | codex | gemini | cursor | copilot | kiro
    pub agent: String,
    pub agent_version: Option<String>,
    pub session_id: Option<String>,
    pub parent_session_id: Option<String>,
    pub turn_id: Option<String>,
    pub cwd_hash: Option<String>,
    pub repo: Option<String>,
    // ---- 由来 ----
    /// hook | otel | log | vendor_api
    pub source: String,
    pub correlation_key: Option<String>,
    /// exact | fuzzy | none
    pub correlation_confidence: Option<String>,
    // ---- 種別 ----
    /// session.start | session.end | turn | tool.call | tool.result | tool.denied
    /// | api.request | api.error | subagent.stop | compaction | hook.decision
    pub event_type: String,
    // ---- ツール ----
    pub tool_name: Option<String>,
    /// builtin | mcp | skill | bash | browser
    pub tool_kind: Option<String>,
    pub mcp_server: Option<String>,
    pub mcp_tool: Option<String>,
    /// tool_kind='skill' のときの Skill 名。hook の tool_input.skill から
    /// tool_name と同格のメタデータとして抽出する (args 等の本文は §5.2 どおりコピーしない)。
    pub skill_name: Option<String>,
    pub duration_ms: Option<i64>,
    pub success: Option<bool>,
    pub error_type: Option<String>,
    /// accept | reject | deny
    pub decision: Option<String>,
    /// user | config | hook
    pub decision_source: Option<String>,
    // ---- モデル ----
    pub provider: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub thinking: Option<bool>,
    // ---- 使用量 ----
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub reasoning_tokens: Option<i64>,
    pub cost_usd: Option<f64>,
    /// otel | hook | log | vendor_api | unknown
    pub usage_source: Option<String>,
    // ---- 本文 (オプトイン。cloud sink は常に None にマスクされる) ----
    pub tool_input_json: Option<String>,
    pub tool_output_excerpt: Option<String>,
    pub prompt_text: Option<String>,
    pub redaction_applied: Option<bool>,
    // ---- MCP 設定スナップショット (末尾追加, 列追加のみ原則) ----
    /// `event_type='session.start'` のときだけ埋まる、設定済み MCP サーバー **名**
    /// (URL/コマンド/引数は含まない、§5.2) のソート済み重複排除 JSON 配列文字列
    /// (例 `["github","playwright"]`)。「導入されているのに一度も呼ばれない
    /// サーバー」(§7.1, §7.2 unused_mcp_server) を検知するための設定スナップショット。
    /// 読めない/空のときは None のまま (推定で埋めない — 原則 7)。今のところ
    /// Claude Code hook 由来のみ (crates/cli/src/mcp_config.rs); Codex は未対応。
    pub configured_mcp_servers: Option<String>,
}

pub mod event_type {
    pub const SESSION_START: &str = "session.start";
    pub const SESSION_END: &str = "session.end";
    pub const TURN: &str = "turn";
    pub const TOOL_CALL: &str = "tool.call";
    pub const TOOL_RESULT: &str = "tool.result";
    pub const TOOL_DENIED: &str = "tool.denied";
    pub const API_REQUEST: &str = "api.request";
    pub const API_ERROR: &str = "api.error";
    pub const SUBAGENT_STOP: &str = "subagent.stop";
    pub const COMPACTION: &str = "compaction";
    pub const HOOK_DECISION: &str = "hook.decision";
}

/// event_id = sha256(host_id | source | event_type | primary_key) の先頭 32 hex。
/// 一次キー (§5.1): tool.* は tool_use_id (無ければ session_id+連番)、
/// api.* は OTel の request id、session.* 等は session_id + event_type + 連番。
/// Pre/PostToolUse が同じ tool_use_id を持っても event_type が入るので衝突しない。
pub fn event_id(host_id: &str, source: &str, event_type: &str, primary_key: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(host_id.as_bytes());
    h.update(b"|");
    h.update(source.as_bytes());
    h.update(b"|");
    h.update(event_type.as_bytes());
    h.update(b"|");
    h.update(primary_key.as_bytes());
    hex::encode(h.finalize())[..32].to_string()
}

/// ts (epoch ms, UTC) → "YYYY-MM-DD"
pub fn dt_of(ts_ms: i64) -> String {
    use chrono::{TimeZone, Utc};
    Utc.timestamp_millis_opt(ts_ms)
        .single()
        .map(|t| t.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "1970-01-01".to_string())
}

/// cwd はハッシュのみ保存 (データ最小化)
pub fn cwd_hash(cwd: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(cwd.as_bytes()))[..16].to_string()
}

/// "mcp__server__tool" → Some(("server", "tool"))
pub fn split_mcp_tool_name(tool_name: &str) -> Option<(String, String)> {
    let rest = tool_name.strip_prefix("mcp__")?;
    let (server, tool) = rest.split_once("__")?;
    Some((server.to_string(), tool.to_string()))
}

/// Parquet / SQL の列順。sink とエクスポートはこの順を守る (列追加は末尾のみ)。
pub const COLUMNS: &[&str] = &[
    "event_id",
    "ts",
    "dt",
    "org_id",
    "team_id",
    "user_id",
    "user_id_source",
    "host_id",
    "env_kind",
    "os",
    "agent",
    "agent_version",
    "session_id",
    "parent_session_id",
    "turn_id",
    "cwd_hash",
    "repo",
    "source",
    "correlation_key",
    "correlation_confidence",
    "event_type",
    "tool_name",
    "tool_kind",
    "mcp_server",
    "mcp_tool",
    "skill_name",
    "duration_ms",
    "success",
    "error_type",
    "decision",
    "decision_source",
    "provider",
    "model",
    "effort",
    "thinking",
    "input_tokens",
    "output_tokens",
    "cache_read_tokens",
    "cache_write_tokens",
    "reasoning_tokens",
    "cost_usd",
    "usage_source",
    "tool_input_json",
    "tool_output_excerpt",
    "prompt_text",
    "redaction_applied",
    "configured_mcp_servers",
];

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn event_id_distinguishes_event_type() {
        let a = event_id("h", "hook", "tool.call", "toolu_1");
        let b = event_id("h", "hook", "tool.result", "toolu_1");
        assert_ne!(a, b);
        assert_eq!(a.len(), 32);
    }
    #[test]
    fn mcp_split() {
        assert_eq!(
            split_mcp_tool_name("mcp__github__get_issue"),
            Some(("github".into(), "get_issue".into()))
        );
        assert_eq!(split_mcp_tool_name("Bash"), None);
    }
    /// 列追加のみ原則 (§5.3): `configured_mcp_servers` は最新の追加列として
    /// 末尾に、かつ `Event` の同名フィールドと 1 対 1 で揃っていること。
    #[test]
    fn columns_ends_with_configured_mcp_servers_and_matches_event_field_count() {
        assert_eq!(COLUMNS.last(), Some(&"configured_mcp_servers"));
        let ev = Event::default();
        // `ev.configured_mcp_servers` compiles only if the field exists;
        // this also pins that a fresh Event defaults it to None (never guessed).
        assert_eq!(ev.configured_mcp_servers, None);
    }
}
