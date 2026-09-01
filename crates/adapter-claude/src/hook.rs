//! Claude Code hooks stdin JSON → kikimimi_schema::Event (architecture.md §4 正規化, §4.1)。
//! フィールド名は docs/research/hook-telemetry-daemon.md の hooks セクションに準拠する。

use kikimimi_schema::{cwd_hash, dt_of, event_id, event_type, Event};
use serde_json::Value;

use crate::classify::classify_tool;
use crate::util::{as_i64, extract_hook_ts, now_ms};
use crate::Normalizer;

impl Normalizer {
    /// `kikimimi hook <event>` の stdin JSON を正規化する。
    ///
    /// 未知の hook_event_name (未対応の 30+ フックのいずれか) は Ok(vec![]) を返し `skipped()` を進める
    /// (fail-open。判定はしない)。
    ///
    /// PRIVACY: tool_input / tool_response / prompt の中身は Event にコピーしない
    /// (`tool_input_json` 等は None のまま。本文オプトインは後続ステージ、§5.2)。
    /// 唯一の例外: `tool_input.skill` (Skill 名) は tool_name と同格のメタデータとして
    /// `skill_name` 列に抽出する。
    pub fn hook(&mut self, raw: &Value) -> anyhow::Result<Vec<Event>> {
        let Some(name) = raw.get("hook_event_name").and_then(Value::as_str) else {
            self.mark_skipped("no_hook_event_name");
            return Ok(vec![]);
        };

        let event_type_str: &str = match name {
            "PreToolUse" => event_type::TOOL_CALL,
            "PostToolUse" => event_type::TOOL_RESULT,
            "PostToolUseFailure" => event_type::TOOL_RESULT,
            "PermissionDenied" => event_type::TOOL_DENIED,
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
        // prompt_id is common to every hook payload (hook-telemetry-daemon.md line 21) and is
        // Claude Code's per-turn identifier, so it maps to kikimimi.v1's turn_id column.
        let turn_id = raw
            .get("prompt_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        // effort is likewise common to every hook payload but not tool/session specific.
        let effort = raw
            .get("effort")
            .and_then(Value::as_str)
            .map(str::to_string);
        let ts = extract_hook_ts(raw).unwrap_or_else(now_ms);
        let dt = dt_of(ts);

        let primary_key = self.primary_key(tool_use_id.as_deref(), session_id.as_deref());
        let eid = event_id(&self.host_id, "hook", event_type_str, &primary_key);
        // exact | fuzzy | none (architecture.md §5.1) — always an explicit value, never NULL,
        // so cloud-side dedup/correlation reporting doesn't have to special-case NULL vs "none".
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
            agent: "claude-code".to_string(),
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
        // Skill 名は tool_name と同格のメタデータとして tool_input.skill から抽出する
        // (上の PRIVACY 方針の唯一の例外。args 等の本文は引き続きコピーしない)。
        if ev.tool_kind.as_deref() == Some("skill") {
            ev.skill_name = raw
                .get("tool_input")
                .and_then(|ti| ti.get("skill"))
                .and_then(Value::as_str)
                .map(str::to_string);
        }

        match name {
            "PostToolUse" | "PostToolUseFailure" => {
                ev.duration_ms = raw.get("duration_ms").and_then(as_i64);
                // Do NOT guess success from the hook_event_name (PostToolUse vs
                // PostToolUseFailure) when tool_response.success is simply absent: an
                // undocumented/unconfirmed field being missing is "unknown", not "succeeded"
                // (principle 7 — never fill an unmeasurable value with a guess).
                ev.success = raw
                    .get("tool_response")
                    .and_then(|tr| tr.get("success"))
                    .and_then(Value::as_bool);
                ev.error_type = extract_error_type(raw);
            }
            "PermissionDenied" => {
                ev.decision = Some("deny".to_string());
                ev.decision_source = raw
                    .get("decision_source")
                    .or_else(|| raw.get("source"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
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

/// PostToolUseFailure (および失敗した PostToolUse) の error_type を payload から探す。
/// フィールド名は公式ドキュメント未確定のため、いくつかありそうな場所を順に見る。
fn extract_error_type(raw: &Value) -> Option<String> {
    raw.get("tool_response")
        .and_then(|tr| tr.get("error").or_else(|| tr.get("error_type")))
        .and_then(Value::as_str)
        .or_else(|| raw.get("error_type").and_then(Value::as_str))
        .or_else(|| raw.get("error").and_then(Value::as_str))
        .map(str::to_string)
}

/// SubagentStop の tool_response.usage (無ければ usage) からトークン内訳を読む。
/// 見つからなければ何もしない (usage_source は unknown のまま = None)。
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
