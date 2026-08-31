//! Codex CLI hooks stdin JSON → kikimimi_schema::Event (architecture.md §4 正規化, §4.1 Codex 行)。
//!
//! # 実機調査 (2026-08-31, codex-cli 0.151.0) で確認できたこと・できなかったこと
//!
//! `codex features list` は `hooks  stable  true` — hook エンジンは安定 (有効) 機能。
//! インストール済みバイナリの文字列解析 (`strings`) から、hook payload の wire フィールド
//! 名として以下が実在することを確認した:
//! `session_id`, `turn_id`, `agent_type`, `transcript_path`, `hook_event_name`, `model`,
//! `permission_mode`, `trigger`, `tool_name`, `tool_input`, `tool_use_id`, `tool_response`。
//! hook_event_name の値 (PascalCase, `hookSpecificOutput` 系の Wire 構造体名から復元) は
//! `PreToolUse` / `PostToolUse` / `PermissionRequest` / `PreCompact` / `PostCompact` /
//! `SessionStart` / `SessionEnd` / `UserPromptSubmit` / `SubagentStart` / `SubagentStop` /
//! `Stop` / `Interrupt` の 12 種。
//!
//! Claude Code との確認済みの **差分**:
//! - Claude は `prompt_id` を使うが、Codex は turn の識別に `turn_id` を直接使う
//!   (adapter-claude の `hook.rs` はここを `prompt_id` から読んでいる)。本アダプタは
//!   `turn_id` を読む。
//! - Claude の `PermissionDenied` (「拒否された」ことが確定した通知) に対し、Codex の
//!   名前は `PermissionRequest`(「承認を求めている」段階) — 意味が違う可能性が高く、
//!   実際の payload に許可/拒否の結果を示すフィールドがあるかをこのマシンでは
//!   確認できなかった (このマシンでは承認プロンプトが発火しなかった)。誤って
//!   `tool.denied` として扱うと「要求されただけ」を「拒否された」と誤集計しかねないため、
//!   本アダプタは `PermissionRequest` を意図的に未対応 (`skip_by_reason`) のままにする。
//! - Codex には Claude の `PostToolUseFailure` に相当する別イベントが見当たらない
//!   (`PostToolUse` 1 種のみ) — 成否は Claude と同じく `tool_response.success` から読む。
//!
//! 未確認 (このマシンでは実際の hook 呼び出し JSON を採取できなかった —
//! 承認フローが発火しない設定だったため。以下は Claude Code 互換という設計方針
//! (architecture.md §4.1) を根拠にベストエフォートで読むだけで、無くても実害はない):
//! `cwd` (hook wire フィールドの文字列一覧には現れなかったが、他の構造体と文字列を
//! 共有していておかしくない短い定数名のため、確認は取れていない)、`effort`。
//!
//! PRIVACY: tool_input / tool_response / prompt の中身は Event にコピーしない
//! (`kikimimi-adapter-claude::hook` と同じ方針)。

use kikimimi_schema::{cwd_hash, dt_of, event_id, event_type, Event};
use serde_json::Value;

use crate::classify::classify_tool;
use crate::util::{as_i64, extract_hook_ts, now_ms};
use crate::CodexNormalizer;

impl CodexNormalizer {
    /// `kikimimi hook <event>` の stdin JSON (Codex 側) を正規化する。
    ///
    /// 未知の hook_event_name (`PermissionRequest`/`PreCompact`/`PostCompact`/
    /// `SubagentStart`/`Interrupt`、および将来の未対応イベント) は Ok(vec![]) を返し
    /// `skipped()` を進める (fail-open。判定はしない)。
    pub fn hook(&mut self, raw: &Value) -> anyhow::Result<Vec<Event>> {
        let Some(name) = raw.get("hook_event_name").and_then(Value::as_str) else {
            self.mark_skipped("no_hook_event_name");
            return Ok(vec![]);
        };

        let event_type_str: &str = match name {
            "PreToolUse" => event_type::TOOL_CALL,
            "PostToolUse" => event_type::TOOL_RESULT,
            "SessionStart" => event_type::SESSION_START,
            "SessionEnd" => event_type::SESSION_END,
            "SubagentStop" => event_type::SUBAGENT_STOP,
            "UserPromptSubmit" => event_type::TURN,
            "Stop" => event_type::TURN,
            _ => {
                self.mark_skipped(name);
                return Ok(vec![]);
            }
        };

        let session_id = raw
            .get("session_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        let cwd = raw.get("cwd").and_then(Value::as_str).map(cwd_hash);
        let tool_use_id = raw
            .get("tool_use_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        // Confirmed difference from Claude Code (see module docs): Codex's hook wire
        // struct uses "turn_id" directly, not "prompt_id".
        let turn_id = raw
            .get("turn_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        let effort = raw.get("effort").and_then(Value::as_str).map(str::to_string);
        let ts = extract_hook_ts(raw).unwrap_or_else(now_ms);
        let dt = dt_of(ts);

        let primary_key = self.primary_key(tool_use_id.as_deref(), session_id.as_deref());
        let eid = event_id(&self.host_id, "hook", event_type_str, &primary_key);
        let correlation_confidence = Some(if tool_use_id.is_some() {
            "exact".to_string()
        } else {
            "none".to_string()
        });

        let mut ev = Event {
            event_id: eid,
            ts,
            dt,
            host_id: self.host_id.clone(),
            agent: "codex".to_string(),
            source: "hook".to_string(),
            session_id,
            turn_id,
            cwd_hash: cwd,
            correlation_key: tool_use_id.clone(),
            correlation_confidence,
            event_type: event_type_str.to_string(),
            effort,
            ..Default::default()
        };

        let tool_name = raw
            .get("tool_name")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(tn) = &tool_name {
            let class = classify_tool(tn);
            ev.tool_kind = Some(class.kind.to_string());
            ev.mcp_server = class.mcp_server;
            ev.mcp_tool = class.mcp_tool;
        }
        ev.tool_name = tool_name;

        match name {
            "PostToolUse" => {
                ev.duration_ms = raw.get("duration_ms").and_then(as_i64);
                ev.success = raw
                    .get("tool_response")
                    .and_then(|tr| tr.get("success"))
                    .and_then(Value::as_bool);
                ev.error_type = extract_error_type(raw);
            }
            "SessionStart" => {
                ev.model = raw.get("model").and_then(Value::as_str).map(str::to_string);
            }
            "SubagentStop" => {
                apply_subagent_usage(&mut ev, raw);
            }
            _ => {}
        }

        Ok(vec![ev])
    }
}

/// 失敗した PostToolUse の error_type を payload から探す (adapter-claude と同じ探索順)。
fn extract_error_type(raw: &Value) -> Option<String> {
    raw.get("tool_response")
        .and_then(|tr| tr.get("error").or_else(|| tr.get("error_type")))
        .and_then(Value::as_str)
        .or_else(|| raw.get("error_type").and_then(Value::as_str))
        .or_else(|| raw.get("error").and_then(Value::as_str))
        .map(str::to_string)
}

/// SubagentStop の tool_response.usage (無ければ usage) からトークン内訳を読む
/// (adapter-claude と同じロジック)。見つからなければ何もしない。
fn apply_subagent_usage(ev: &mut Event, raw: &Value) {
    let Some(usage) = raw
        .get("usage")
        .or_else(|| raw.get("tool_response").and_then(|tr| tr.get("usage")))
    else {
        return;
    };

    let input = usage.get("input_tokens").and_then(as_i64);
    let output = usage.get("output_tokens").and_then(as_i64);
    let cache_read = usage
        .get("cache_read_input_tokens")
        .or_else(|| usage.get("cache_read_tokens"))
        .and_then(as_i64);
    let cache_write = usage
        .get("cache_creation_input_tokens")
        .or_else(|| usage.get("cache_write_tokens"))
        .and_then(as_i64);

    if input.is_none() && output.is_none() && cache_read.is_none() && cache_write.is_none() {
        return;
    }

    ev.input_tokens = input;
    ev.output_tokens = output;
    ev.cache_read_tokens = cache_read;
    ev.cache_write_tokens = cache_write;
    ev.usage_source = Some("hook".to_string());
}
