//! `CodexNormalizer::hook` の正規化テスト。
//!
//! IMPORTANT: `tests/fixtures/hook_*.json` は **実機で採取した生の hook 呼び出し JSON
//! ではない** — このマシンでは承認プロンプト等 hook を実際に発火させる操作を行って
//! いないため、Codex の生 hook payload を採取できなかった。これらのフィクスチャは
//! `crates/adapter-codex/src/hook.rs` のモジュール doc に書いた「バイナリ文字列解析で
//! 確認済みのフィールド名」(`session_id`/`turn_id`/`cwd`/`hook_event_name`/`tool_name`/
//! `tool_input`/`tool_use_id`/`tool_response` 等) と、architecture.md が明記する
//! 「Codex の hooks エンジンは Claude Code 互換」という設計方針を根拠に**構成した**もの。
//! 実際の hook 発火ログが採れ次第、Stage 1 でこのフィクスチャを実データに置き換えること
//! (`rollout_line` 側のフィクスチャは対照的に実機で採取・redact 済みの本物 — `tests/rollout.rs`
//! 参照)。

use kikimimi_adapter_codex::CodexNormalizer;
use serde_json::Value;

fn load(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parsing {path}: {e}"))
}

#[test]
fn pretooluse_maps_to_tool_call_with_shell_alias_classification() {
    let mut n = CodexNormalizer::new("host-1".into());
    let raw = load("hook_pretooluse_exec.json");
    let events = n.hook(&raw).unwrap();
    assert_eq!(events.len(), 1);
    let ev = &events[0];

    assert_eq!(ev.agent, "codex");
    assert_eq!(ev.source, "hook");
    assert_eq!(ev.event_type, kikimimi_schema::event_type::TOOL_CALL);
    assert_eq!(ev.tool_name.as_deref(), Some("exec"));
    assert_eq!(ev.tool_kind.as_deref(), Some("bash"), "\"exec\" must classify as bash");
    assert_eq!(ev.turn_id.as_deref(), Some("01a057b2-2768-73e3-826c-83cb99fa2b1f"));
    assert_eq!(
        ev.correlation_key.as_deref(),
        Some("call_vqfciNiRdPLyz4cXdOFY3FlQ")
    );
    assert_eq!(ev.correlation_confidence.as_deref(), Some("exact"));
    // PRIVACY: raw command text must never leak into the Event.
    assert!(ev.tool_input_json.is_none());
}

#[test]
fn posttooluse_success_maps_to_tool_result() {
    let mut n = CodexNormalizer::new("host-1".into());
    let raw = load("hook_posttooluse_exec.json");
    let events = n.hook(&raw).unwrap();
    assert_eq!(events.len(), 1);
    let ev = &events[0];

    assert_eq!(ev.event_type, kikimimi_schema::event_type::TOOL_RESULT);
    assert_eq!(ev.duration_ms, Some(220));
    assert_eq!(ev.success, Some(true));
    assert_eq!(ev.error_type, None);
}

#[test]
fn posttooluse_failure_reads_mcp_split_and_error() {
    let mut n = CodexNormalizer::new("host-1".into());
    let raw = load("hook_posttooluse_failure.json");
    let events = n.hook(&raw).unwrap();
    let ev = &events[0];

    assert_eq!(ev.tool_kind.as_deref(), Some("mcp"));
    assert_eq!(ev.mcp_server.as_deref(), Some("ologs"));
    assert_eq!(ev.mcp_tool.as_deref(), Some("get_profile"));
    assert_eq!(ev.success, Some(false));
    assert_eq!(ev.error_type.as_deref(), Some("connector unavailable"));
}

#[test]
fn sessionstart_reads_model() {
    let mut n = CodexNormalizer::new("host-1".into());
    let raw = load("hook_sessionstart.json");
    let events = n.hook(&raw).unwrap();
    let ev = &events[0];
    assert_eq!(ev.event_type, kikimimi_schema::event_type::SESSION_START);
    assert_eq!(ev.model.as_deref(), Some("gpt-5.6-sol"));
}

#[test]
fn sessionend_maps() {
    let mut n = CodexNormalizer::new("host-1".into());
    let raw = load("hook_sessionend.json");
    let events = n.hook(&raw).unwrap();
    assert_eq!(events[0].event_type, kikimimi_schema::event_type::SESSION_END);
}

#[test]
fn userpromptsubmit_and_stop_both_map_to_turn() {
    let mut n = CodexNormalizer::new("host-1".into());
    let ups = n.hook(&load("hook_userpromptsubmit.json")).unwrap();
    let stop = n.hook(&load("hook_stop.json")).unwrap();
    assert_eq!(ups[0].event_type, kikimimi_schema::event_type::TURN);
    assert_eq!(stop[0].event_type, kikimimi_schema::event_type::TURN);
    // Distinct source lines -> distinct event_ids even though both map to "turn".
    assert_ne!(ups[0].event_id, stop[0].event_id);
}

#[test]
fn subagentstop_reads_usage_from_top_level_usage_field() {
    let mut n = CodexNormalizer::new("host-1".into());
    let raw = load("hook_subagentstop.json");
    let events = n.hook(&raw).unwrap();
    let ev = &events[0];
    assert_eq!(ev.event_type, kikimimi_schema::event_type::SUBAGENT_STOP);
    assert_eq!(ev.input_tokens, Some(500));
    assert_eq!(ev.output_tokens, Some(120));
    assert_eq!(ev.cache_read_tokens, Some(300));
    assert_eq!(ev.usage_source.as_deref(), Some("hook"));
}

/// Confirmed real difference from Claude Code (see `src/hook.rs` module docs):
/// Codex's PermissionRequest fires when a decision is *requested*, not when it has
/// been *denied* (unlike Claude's PermissionDenied) -- and no real payload sample was
/// available to confirm an outcome field, so it is intentionally left unmapped
/// (skip_by_reason) instead of guessing.
#[test]
fn permissionrequest_is_intentionally_skipped_not_guessed_as_denied() {
    let mut n = CodexNormalizer::new("host-1".into());
    let raw = load("hook_permissionrequest.json");
    let events = n.hook(&raw).unwrap();
    assert!(events.is_empty());
    assert_eq!(n.skipped(), 1);
    assert_eq!(n.skipped_by_reason().get("PermissionRequest"), Some(&1));
}

#[test]
fn unknown_hook_event_name_is_skipped() {
    let mut n = CodexNormalizer::new("host-1".into());
    let raw = load("hook_unknown.json");
    let events = n.hook(&raw).unwrap();
    assert!(events.is_empty());
    assert_eq!(n.skipped_by_reason().get("PreCompact"), Some(&1));
}

#[test]
fn missing_hook_event_name_is_skipped_distinctly() {
    let mut n = CodexNormalizer::new("host-1".into());
    let events = n.hook(&serde_json::json!({"session_id": "s"})).unwrap();
    assert!(events.is_empty());
    assert_eq!(n.skipped_by_reason().get("no_hook_event_name"), Some(&1));
}

#[test]
fn event_id_is_deterministic_for_the_same_input() {
    let mut n1 = CodexNormalizer::new("host-1".into());
    let mut n2 = CodexNormalizer::new("host-1".into());
    let raw = load("hook_pretooluse_exec.json");
    let a = n1.hook(&raw).unwrap();
    let b = n2.hook(&raw).unwrap();
    assert_eq!(a[0].event_id, b[0].event_id);
}
