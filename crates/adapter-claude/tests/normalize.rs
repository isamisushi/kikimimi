//! kikimimi-adapter-claude 統合テスト: 実際の hook / OTLP payload 形状に近いフィクスチャで
//! architecture.md §4.1 のマッピング表を検証する。

use kikimimi_adapter_claude::Normalizer;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;

fn fixture(name: &str) -> serde_json::Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {path}: {e}"))
}

fn fixture_text(name: &str) -> String {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

// ---- hooks ----

#[test]
fn pretooluse_maps_to_tool_call_without_copying_body() {
    let mut n = Normalizer::new("host-1".into());
    let raw = fixture("pretooluse_bash.json");
    let events = n.hook(&raw).expect("hook ok");
    assert_eq!(events.len(), 1);
    let ev = &events[0];

    assert_eq!(ev.event_type, "tool.call");
    assert_eq!(ev.agent, "claude-code");
    assert_eq!(ev.source, "hook");
    assert_eq!(ev.session_id.as_deref(), Some("sess-abc123"));
    assert_eq!(ev.tool_name.as_deref(), Some("Bash"));
    assert_eq!(ev.tool_kind.as_deref(), Some("bash"));
    assert_eq!(ev.correlation_key.as_deref(), Some("toolu_01Abc123XYZ"));
    assert_eq!(ev.correlation_confidence.as_deref(), Some("exact"));
    // cwd is hashed, never stored in the clear.
    assert!(ev.cwd_hash.is_some());
    assert_ne!(ev.cwd_hash.as_deref(), Some("/home/user/kikimimi"));

    // PRIVACY: tool_input/tool_response content is never copied into the event.
    assert!(ev.tool_input_json.is_none());
    assert!(ev.tool_output_excerpt.is_none());
    assert!(ev.prompt_text.is_none());
}

#[test]
fn posttooluse_maps_to_tool_result_success_with_duration() {
    let mut n = Normalizer::new("host-1".into());
    let raw = fixture("posttooluse_bash.json");
    let events = n.hook(&raw).unwrap();
    assert_eq!(events.len(), 1);
    let ev = &events[0];

    assert_eq!(ev.event_type, "tool.result");
    assert_eq!(ev.tool_name.as_deref(), Some("Bash"));
    assert_eq!(ev.tool_kind.as_deref(), Some("bash"));
    assert_eq!(ev.success, Some(true));
    assert_eq!(ev.duration_ms, Some(142));
    assert_eq!(ev.correlation_key.as_deref(), Some("toolu_01Abc123XYZ"));
}

#[test]
fn posttooluse_mcp_splits_server_and_tool() {
    let mut n = Normalizer::new("host-1".into());
    let raw = fixture("posttooluse_mcp.json");
    let events = n.hook(&raw).unwrap();
    let ev = &events[0];

    assert_eq!(ev.tool_kind.as_deref(), Some("mcp"));
    assert_eq!(ev.mcp_server.as_deref(), Some("github"));
    assert_eq!(ev.mcp_tool.as_deref(), Some("get_issue"));
    assert_eq!(ev.success, Some(true));
    assert_eq!(ev.duration_ms, Some(890));
}

#[test]
fn posttooluse_failure_sets_success_false_and_error_type() {
    let mut n = Normalizer::new("host-1".into());
    let raw = fixture("posttooluse_failure.json");
    let events = n.hook(&raw).unwrap();
    let ev = &events[0];

    assert_eq!(ev.event_type, "tool.result");
    assert_eq!(ev.success, Some(false));
    assert_eq!(ev.error_type.as_deref(), Some("command not found: fooo"));
}

#[test]
fn posttooluse_success_is_unknown_not_guessed_when_tool_response_success_is_absent() {
    let mut n = Normalizer::new("host-1".into());
    let mut raw = fixture("posttooluse_bash.json");
    // Remove tool_response.success entirely (some tools' payloads may not carry it).
    raw["tool_response"]
        .as_object_mut()
        .unwrap()
        .remove("success");
    let events = n.hook(&raw).unwrap();
    // Must be None (unknown), never guessed true just because this is a PostToolUse
    // (principle 7: never fill an unmeasurable value with an estimate).
    assert_eq!(events[0].success, None);
}

#[test]
fn permissiondenied_maps_to_tool_denied_with_deny_decision() {
    let mut n = Normalizer::new("host-1".into());
    let raw = fixture("permissiondenied.json");
    let events = n.hook(&raw).unwrap();
    let ev = &events[0];

    assert_eq!(ev.event_type, "tool.denied");
    assert_eq!(ev.decision.as_deref(), Some("deny"));
    assert_eq!(ev.decision_source.as_deref(), Some("user"));
    assert_eq!(ev.tool_name.as_deref(), Some("Bash"));
}

#[test]
fn sessionstart_carries_model() {
    let mut n = Normalizer::new("host-1".into());
    let raw = fixture("sessionstart.json");
    let events = n.hook(&raw).unwrap();
    let ev = &events[0];

    assert_eq!(ev.event_type, "session.start");
    assert_eq!(ev.model.as_deref(), Some("claude-opus-4-6-20260805"));
}

#[test]
fn sessionend_maps() {
    let mut n = Normalizer::new("host-1".into());
    let raw = fixture("sessionend.json");
    let events = n.hook(&raw).unwrap();
    assert_eq!(events[0].event_type, "session.end");
}

#[test]
fn subagentstop_extracts_usage_from_tool_response() {
    let mut n = Normalizer::new("host-1".into());
    let raw = fixture("subagentstop.json");
    let events = n.hook(&raw).unwrap();
    let ev = &events[0];

    assert_eq!(ev.event_type, "subagent.stop");
    assert_eq!(ev.input_tokens, Some(1500));
    assert_eq!(ev.output_tokens, Some(320));
    assert_eq!(ev.cache_read_tokens, Some(200));
    assert_eq!(ev.cache_write_tokens, Some(50));
    assert_eq!(ev.usage_source.as_deref(), Some("hook"));
}

#[test]
fn userpromptsubmit_maps_to_turn_without_copying_prompt() {
    let mut n = Normalizer::new("host-1".into());
    let raw = fixture("userpromptsubmit.json");
    let events = n.hook(&raw).unwrap();
    let ev = &events[0];

    assert_eq!(ev.event_type, "turn");
    // The fixture *has* a "prompt" field; kikimimi must not copy it (content opt-in is later stage).
    assert!(ev.prompt_text.is_none());
    // prompt_id (common to every hook payload, hook-telemetry-daemon.md line 21) maps to turn_id.
    assert_eq!(ev.turn_id.as_deref(), Some("prompt-001"));
}

#[test]
fn stop_maps_to_turn() {
    let mut n = Normalizer::new("host-1".into());
    let raw = fixture("stop.json");
    let events = n.hook(&raw).unwrap();
    assert_eq!(events[0].event_type, "turn");
}

#[test]
fn unknown_hook_event_name_is_skipped_and_counted() {
    let mut n = Normalizer::new("host-1".into());
    let raw = fixture("unknown_hook.json");
    let events = n.hook(&raw).unwrap();
    assert!(events.is_empty());
    assert_eq!(n.skipped(), 1);
    assert_eq!(
        n.skipped_by_reason()
            .get("SomeFutureHookNobodyKnowsAboutYet"),
        Some(&1),
        "unknown hook_event_name must be recorded verbatim as the skip reason"
    );

    // A second unknown event keeps counting under the same reason.
    let events2 = n.hook(&raw).unwrap();
    assert!(events2.is_empty());
    assert_eq!(n.skipped(), 2);
    assert_eq!(
        n.skipped_by_reason()
            .get("SomeFutureHookNobodyKnowsAboutYet"),
        Some(&2)
    );
    assert_eq!(n.skipped_by_reason().len(), 1, "must not fan out per-call");
}

#[test]
fn missing_hook_event_name_field_is_skipped() {
    let mut n = Normalizer::new("host-1".into());
    let raw = serde_json::json!({"session_id": "sess-abc123"});
    let events = n.hook(&raw).unwrap();
    assert!(events.is_empty());
    assert_eq!(n.skipped(), 1);
    assert_eq!(n.skipped_by_reason().get("no_hook_event_name"), Some(&1));
}

#[test]
fn webfetch_classified_as_browser_tool() {
    let mut n = Normalizer::new("host-1".into());
    let raw = fixture("pretooluse_webfetch.json");
    let events = n.hook(&raw).unwrap();
    assert_eq!(events[0].tool_kind.as_deref(), Some("browser"));
}

#[test]
fn playwright_mcp_classified_as_browser_but_keeps_mcp_server() {
    // architecture.md §1.1: Playwright MCP is the representative "alternative
    // channel" the bypass / thrash(deny_detour) / reach queries must catch
    // (tool_kind IN ('bash','browser')), so a browser-automation MCP server
    // must classify as tool_kind='browser', not 'mcp' — while mcp_server /
    // mcp_tool stay populated for MCP health / unused-mcp queries.
    let mut n = Normalizer::new("host-1".into());
    let raw = serde_json::json!({
        "session_id": "sess-abc123",
        "transcript_path": "/home/user/.claude/projects/-home-user-kikimimi/sess-abc123.jsonl",
        "cwd": "/home/user/kikimimi",
        "hook_event_name": "PostToolUse",
        "tool_name": "mcp__playwright__navigate",
        "tool_input": {"url": "https://example.com"},
        "tool_response": {"success": true},
        "tool_use_id": "toolu_03Mcp789",
        "duration_ms": 120
    });
    let events = n.hook(&raw).unwrap();
    let ev = &events[0];

    assert_eq!(ev.tool_kind.as_deref(), Some("browser"));
    assert_eq!(ev.mcp_server.as_deref(), Some("playwright"));
    assert_eq!(ev.mcp_tool.as_deref(), Some("navigate"));
}

#[test]
fn skill_tool_classified_as_skill() {
    let mut n = Normalizer::new("host-1".into());
    let raw = fixture("pretooluse_skill.json");
    let events = n.hook(&raw).unwrap();
    assert_eq!(events[0].tool_kind.as_deref(), Some("skill"));
    assert_eq!(events[0].skill_name.as_deref(), Some("code-review"));
}

#[test]
fn event_id_is_stable_for_same_tool_use_id_across_calls() {
    let mut n = Normalizer::new("host-1".into());
    let raw = fixture("pretooluse_bash.json");
    let first = n.hook(&raw).unwrap();
    let second = n.hook(&raw).unwrap();
    assert_eq!(first[0].event_id, second[0].event_id);
    assert_eq!(first[0].event_id.len(), 32);
}

#[test]
fn pre_and_post_tool_use_get_distinct_event_ids_for_same_tool_use_id() {
    let mut n = Normalizer::new("host-1".into());
    let pre = n.hook(&fixture("pretooluse_bash.json")).unwrap();
    let post = n.hook(&fixture("posttooluse_bash.json")).unwrap();
    // Same tool_use_id, but PreToolUse -> tool.call and PostToolUse -> tool.result must not collide.
    assert_eq!(pre[0].correlation_key, post[0].correlation_key);
    assert_ne!(pre[0].event_id, post[0].event_id);
}

#[test]
fn different_hosts_produce_different_event_ids() {
    let mut a = Normalizer::new("host-a".into());
    let mut b = Normalizer::new("host-b".into());
    let raw = fixture("pretooluse_bash.json");
    let ea = a.hook(&raw).unwrap();
    let eb = b.hook(&raw).unwrap();
    assert_ne!(ea[0].event_id, eb[0].event_id);
}

#[test]
fn events_without_tool_use_id_get_distinct_ids_via_session_sequence() {
    let mut n = Normalizer::new("host-1".into());
    let start = n.hook(&fixture("sessionstart.json")).unwrap();
    let end = n.hook(&fixture("sessionend.json")).unwrap();
    assert_ne!(start[0].event_id, end[0].event_id);

    // Calling SessionStart again in the same session yields yet another id (seq keeps advancing).
    let start2 = n.hook(&fixture("sessionstart.json")).unwrap();
    assert_ne!(start[0].event_id, start2[0].event_id);
}

// ---- OTLP logs ----

#[test]
fn otlp_logs_maps_api_request_and_tool_result_and_skips_unknown() {
    let mut n = Normalizer::new("host-1".into());
    let text = fixture_text("otlp_logs.json");
    let req: ExportLogsServiceRequest =
        serde_json::from_str(&text).expect("parse ExportLogsServiceRequest JSON");

    let events = n.otlp_logs(&req).expect("otlp_logs ok");
    assert_eq!(
        events.len(),
        2,
        "the unmapped 3rd record must be skipped, not emitted"
    );
    assert_eq!(n.skipped(), 1);
    assert_eq!(
        n.skipped_by_reason()
            .get("otlp:claude_code.something_unmapped"),
        Some(&1),
        "unknown OTLP record identifier must be recorded with an otlp: prefix"
    );

    let api_request = &events[0];
    assert_eq!(api_request.event_type, "api.request");
    assert_eq!(api_request.source, "otel");
    assert_eq!(api_request.session_id.as_deref(), Some("sess-abc123"));
    assert_eq!(api_request.user_id.as_deref(), Some("dev@example.com"));
    assert_eq!(api_request.user_id_source.as_deref(), Some("agent_email"));
    assert_eq!(api_request.org_id.as_deref(), Some("org-789"));
    assert_eq!(
        api_request.model.as_deref(),
        Some("claude-opus-4-6-20260805")
    );
    assert_eq!(api_request.input_tokens, Some(1520));
    assert_eq!(api_request.output_tokens, Some(340));
    assert_eq!(api_request.cache_read_tokens, Some(200));
    assert_eq!(api_request.cache_write_tokens, Some(0));
    assert_eq!(api_request.duration_ms, Some(2110));
    assert!((api_request.cost_usd.unwrap() - 0.0872).abs() < 1e-9);
    assert_eq!(api_request.usage_source.as_deref(), Some("otel"));
    // No tool_use_id on this record: correlation_confidence must be the explicit
    // sentinel "none" (architecture.md §5.1's 3-value enum), never NULL/None.
    assert_eq!(api_request.correlation_confidence.as_deref(), Some("none"));

    let tool_result = &events[1];
    assert_eq!(tool_result.event_type, "tool.result");
    assert_eq!(
        tool_result.tool_name.as_deref(),
        Some("mcp__github__get_issue")
    );
    assert_eq!(tool_result.tool_kind.as_deref(), Some("mcp"));
    assert_eq!(tool_result.mcp_server.as_deref(), Some("github"));
    assert_eq!(tool_result.mcp_tool.as_deref(), Some("get_issue"));
    assert_eq!(tool_result.success, Some(true));
    assert_eq!(tool_result.duration_ms, Some(890));
    assert_eq!(
        tool_result.correlation_key.as_deref(),
        Some("toolu_02Mcp456")
    );
    assert_eq!(tool_result.correlation_confidence.as_deref(), Some("exact"));
    // session.id/user.email came from resource attributes, inherited by every record.
    assert_eq!(tool_result.session_id.as_deref(), Some("sess-abc123"));
}

#[test]
fn otlp_log_record_without_event_name_is_skipped_under_no_event_name_reason() {
    let mut n = Normalizer::new("host-1".into());
    let json = serde_json::json!({
        "resourceLogs": [{
            "resource": { "attributes": [] },
            "scopeLogs": [{ "logRecords": [{
                "timeUnixNano": "1798675200000000000",
                "attributes": []
            }]}]
        }]
    });
    let req: ExportLogsServiceRequest =
        serde_json::from_value(json).expect("parse ExportLogsServiceRequest JSON");

    let events = n.otlp_logs(&req).expect("otlp_logs ok");
    assert!(events.is_empty());
    assert_eq!(n.skipped(), 1);
    assert_eq!(n.skipped_by_reason().get("otlp:no_event_name"), Some(&1));
}

#[test]
fn skipped_by_reason_sums_to_skipped_across_distinct_reasons() {
    let mut n = Normalizer::new("host-1".into());
    // Two distinct unknown hook_event_name values plus a missing one.
    n.hook(&serde_json::json!({"hook_event_name": "PreCompact"}))
        .unwrap();
    n.hook(&serde_json::json!({"hook_event_name": "Notification"}))
        .unwrap();
    n.hook(&serde_json::json!({"hook_event_name": "PreCompact"}))
        .unwrap();
    n.hook(&serde_json::json!({"session_id": "sess-abc123"}))
        .unwrap();

    assert_eq!(n.skipped(), 4);
    let by_reason = n.skipped_by_reason();
    assert_eq!(by_reason.get("PreCompact"), Some(&2));
    assert_eq!(by_reason.get("Notification"), Some(&1));
    assert_eq!(by_reason.get("no_hook_event_name"), Some(&1));
    assert_eq!(by_reason.values().sum::<u64>(), n.skipped());
}

#[test]
fn otlp_metrics_stage0_returns_empty() {
    let mut n = Normalizer::new("host-1".into());
    let req = ExportMetricsServiceRequest::default();
    let events = n.otlp_metrics(&req).expect("otlp_metrics ok");
    assert!(events.is_empty());
}

// ---- effort / correlation_confidence: none ----

#[test]
fn hook_effort_field_is_mapped_when_present() {
    let mut n = Normalizer::new("host-1".into());
    let mut raw = fixture("pretooluse_bash.json");
    raw["effort"] = serde_json::json!("high");
    let events = n.hook(&raw).unwrap();
    assert_eq!(events[0].effort.as_deref(), Some("high"));
}

#[test]
fn hook_effort_is_none_when_absent() {
    let mut n = Normalizer::new("host-1".into());
    let raw = fixture("pretooluse_bash.json");
    assert!(raw.get("effort").is_none(), "fixture must not have effort");
    let events = n.hook(&raw).unwrap();
    assert_eq!(events[0].effort, None);
}

#[test]
fn hook_correlation_confidence_is_explicit_none_without_tool_use_id() {
    let mut n = Normalizer::new("host-1".into());
    // sessionstart.json has no tool_use_id.
    let raw = fixture("sessionstart.json");
    let events = n.hook(&raw).unwrap();
    assert_eq!(events[0].correlation_key, None);
    assert_eq!(events[0].correlation_confidence.as_deref(), Some("none"));
}

// ---- OTLP claude_code.compaction ----

#[test]
fn otlp_compaction_event_is_mapped_not_skipped() {
    let mut n = Normalizer::new("host-1".into());
    let json = serde_json::json!({
        "resourceLogs": [{
            "resource": { "attributes": [
                { "key": "session.id", "value": { "stringValue": "sess-1" } }
            ]},
            "scopeLogs": [{ "logRecords": [{
                "timeUnixNano": "1798675200000000000",
                "eventName": "claude_code.compaction",
                "attributes": [
                    { "key": "effort", "value": { "stringValue": "medium" } }
                ]
            }]}]
        }]
    });
    let req: ExportLogsServiceRequest =
        serde_json::from_value(json).expect("parse ExportLogsServiceRequest JSON");

    let events = n.otlp_logs(&req).expect("otlp_logs ok");
    assert_eq!(
        events.len(),
        1,
        "claude_code.compaction must not be skipped"
    );
    assert_eq!(n.skipped(), 0);

    let ev = &events[0];
    assert_eq!(ev.event_type, kikimimi_schema::event_type::COMPACTION);
    assert_eq!(ev.session_id.as_deref(), Some("sess-1"));
    assert_eq!(ev.effort.as_deref(), Some("medium"));
    assert_eq!(ev.correlation_confidence.as_deref(), Some("none"));
}
