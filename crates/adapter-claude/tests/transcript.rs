//! kikimimi-adapter-claude 統合テスト: `TranscriptNormalizer` (transcript JSONL →
//! kikimimi_schema::Event, architecture.md §4.1) を、実際の Claude Code transcript
//! 行の形に近いフィクスチャで検証する。
//!
//! `TranscriptNormalizer::line` は既にパース済みの `serde_json::Value` しか
//! 受け取らない (transcript.rs の doc comment参照) ので、ここでは実際のログ
//! tailer と同じように、フィクスチャファイルを 1 行ずつ自前でパースし、壊れた
//! 行は (tailer 側の責務として) この統合テストの中で数える。

use std::collections::HashMap;

use kikimimi_adapter_claude::TranscriptNormalizer;
use kikimimi_schema::{event_type, Event};

const SENTINEL: &str = "SECRET-TEXT-MUST-NOT-LEAK";

struct Run {
    events: Vec<Event>,
    malformed_lines: u64,
    normalizer: TranscriptNormalizer,
}

/// フィクスチャファイルを行ごとに読み、パースできた行だけ `line()` に渡す
/// (パース失敗はログ tailer が数える想定 — transcript.rs の doc comment参照)。
/// 最後に `finish()` を 1 回呼ぶ。
fn run_fixture(name: &str) -> Run {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));

    let mut normalizer = TranscriptNormalizer::new("host-1".into());
    let mut events = Vec::new();
    let mut malformed_lines = 0u64;

    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<serde_json::Value>(line) {
            Ok(raw) => events.extend(normalizer.line(&raw)),
            Err(_) => malformed_lines += 1,
        }
    }
    events.extend(normalizer.finish());

    Run {
        events,
        malformed_lines,
        normalizer,
    }
}

fn events_of<'a>(run: &'a Run, event_type: &str) -> Vec<&'a Event> {
    run.events
        .iter()
        .filter(|e| e.event_type == event_type)
        .collect()
}

/// PRIVACY (architecture.md §5.2): どの Event のどのフィールドにも本文
/// (prompt/tool_input/tool_result/thinking/assistant テキスト) が写っていないこと。
/// フィクスチャは全てのテキスト系フィールドに SENTINEL を埋め込んでいるので、
/// シリアライズした Event に一度でも出現したら PRIVACY 違反。
fn assert_no_leak(run: &Run) {
    for ev in &run.events {
        let json = serde_json::to_string(ev).expect("serialize Event");
        assert!(
            !json.contains(SENTINEL),
            "PRIVACY leak: sentinel found in serialized Event: {json}"
        );
    }
}

#[test]
fn no_event_ever_contains_fixture_body_text() {
    let run = run_fixture("transcript_basic.jsonl");
    assert!(
        !run.events.is_empty(),
        "fixture must produce events to check"
    );
    assert_no_leak(&run);
}

#[test]
fn malformed_json_line_is_counted_by_the_caller_not_the_normalizer() {
    let run = run_fixture("transcript_basic.jsonl");
    assert_eq!(
        run.malformed_lines, 1,
        "fixture has exactly one malformed line"
    );
}

#[test]
fn bookkeeping_lines_are_skipped_and_counted_by_type() {
    let run = run_fixture("transcript_basic.jsonl");
    let by_reason: &HashMap<String, u64> = run.normalizer.skipped_by_reason();

    assert_eq!(by_reason.get("mode"), Some(&1));
    assert_eq!(by_reason.get("permission-mode"), Some(&1));
    assert_eq!(by_reason.get("system"), Some(&1));
    assert_eq!(by_reason.get("attachment"), Some(&1));
    assert_eq!(
        run.normalizer.skipped(),
        4,
        "exactly the 4 bookkeeping lines above must be skipped, nothing else"
    );
}

#[test]
fn session_start_comes_from_the_first_timestamped_line_even_if_its_type_is_skipped() {
    let run = run_fixture("transcript_basic.jsonl");
    let starts = events_of(&run, event_type::SESSION_START);
    assert_eq!(starts.len(), 1, "exactly one session.start");
    let start = starts[0];

    // The 3rd fixture line ("system", subtype local_command) is the first line
    // in the file carrying a "timestamp" (the two bookkeeping lines before it
    // — "mode"/"permission-mode" — have none). session.start must come from
    // *that* line's ts, not from the first turn.
    assert_eq!(start.ts, 1_788_343_200_000, "2026-09-02T10:00:00.000Z");
    assert_eq!(start.session_id.as_deref(), Some("sess-transcript-abc123"));
    assert_eq!(start.source, "log");
    assert_eq!(start.agent, "claude-code");
    assert_eq!(start.correlation_confidence.as_deref(), Some("none"));
    assert!(start.cwd_hash.is_some());
    assert_ne!(
        start.cwd_hash.as_deref(),
        Some("/home/user/project-guru-fixture"),
        "cwd must be hashed, never stored in the clear"
    );
    assert_eq!(start.agent_version.as_deref(), Some("2.1.258"));
}

#[test]
fn session_end_uses_the_last_timestamp_seen_at_finish() {
    let run = run_fixture("transcript_basic.jsonl");
    let ends = events_of(&run, event_type::SESSION_END);
    assert_eq!(ends.len(), 1);
    let end = ends[0];

    // Last successfully-parsed line's ts (the isApiErrorMessage line,
    // 2026-09-02T10:00:09.000Z) — the malformed 13th line never reaches
    // finish()'s bookkeeping since it's never handed to line().
    assert_eq!(end.ts, 1_788_343_209_000);
    assert_eq!(end.session_id.as_deref(), Some("sess-transcript-abc123"));

    let starts = events_of(&run, event_type::SESSION_START);
    assert!(end.ts > starts[0].ts);
    assert_ne!(end.event_id, starts[0].event_id);
}

#[test]
fn real_prompt_becomes_a_turn_without_copying_the_prompt_text() {
    let run = run_fixture("transcript_basic.jsonl");
    let turns = events_of(&run, event_type::TURN);
    assert_eq!(turns.len(), 1);
    let turn = turns[0];

    assert_eq!(turn.turn_id.as_deref(), Some("prompt-1"));
    assert_eq!(turn.session_id.as_deref(), Some("sess-transcript-abc123"));
    assert_eq!(turn.source, "log");
    assert_eq!(turn.correlation_confidence.as_deref(), Some("none"));
    assert!(turn.prompt_text.is_none());
}

#[test]
fn assistant_response_split_across_two_records_counts_usage_once() {
    let run = run_fixture("transcript_basic.jsonl");
    let api_requests = events_of(&run, event_type::API_REQUEST);
    // req_001 (split across 2 lines) + req_002 + req_003 = 3, not 4.
    assert_eq!(api_requests.len(), 3);

    let req1 = api_requests
        .iter()
        .find(|e| {
            e.model.as_deref() == Some("claude-fable-5") && e.effort.as_deref() == Some("high")
        })
        .expect("req_001's api.request");
    assert_eq!(req1.input_tokens, Some(100));
    assert_eq!(req1.output_tokens, Some(20));
    assert_eq!(req1.cache_read_tokens, Some(5));
    assert_eq!(req1.cache_write_tokens, Some(10));
    assert_eq!(req1.reasoning_tokens, Some(8));
    assert_eq!(req1.cost_usd, None);
    assert_eq!(req1.usage_source.as_deref(), Some("log"));
    assert_eq!(req1.source, "log");
    // ts must be the *first* of the two lines sharing req_001 (the thinking
    // block), not the second (tool_use) — usage is counted once, on first sight.
    assert_eq!(req1.ts, 1_788_343_203_000, "2026-09-02T10:00:03.000Z");
}

#[test]
fn tool_use_and_success_tool_result_correlate_with_duration() {
    let run = run_fixture("transcript_basic.jsonl");
    let calls = events_of(&run, event_type::TOOL_CALL);
    let results = events_of(&run, event_type::TOOL_RESULT);
    assert_eq!(calls.len(), 3, "toolu_A, toolu_B, toolu_C");
    assert_eq!(results.len(), 2, "toolu_A, toolu_B only (toolu_C has none)");

    let call_a = calls
        .iter()
        .find(|e| e.correlation_key.as_deref() == Some("toolu_A"))
        .expect("toolu_A call");
    assert_eq!(call_a.tool_name.as_deref(), Some("mcp__github__get_issue"));
    assert_eq!(call_a.tool_kind.as_deref(), Some("mcp"));
    assert_eq!(call_a.mcp_server.as_deref(), Some("github"));
    assert_eq!(call_a.mcp_tool.as_deref(), Some("get_issue"));
    assert_eq!(call_a.turn_id.as_deref(), Some("prompt-1"));
    assert_eq!(call_a.correlation_confidence.as_deref(), Some("exact"));

    let result_a = results
        .iter()
        .find(|e| e.correlation_key.as_deref() == Some("toolu_A"))
        .expect("toolu_A result");
    assert_eq!(result_a.success, Some(true), "is_error absent => success");
    assert_eq!(
        result_a.tool_name.as_deref(),
        Some("mcp__github__get_issue")
    );
    assert_eq!(result_a.tool_kind.as_deref(), Some("mcp"));
    assert_eq!(
        result_a.duration_ms,
        Some(700),
        "10:00:03.500 -> 10:00:04.200"
    );
    assert_eq!(result_a.correlation_confidence.as_deref(), Some("exact"));
}

#[test]
fn tool_use_and_failed_tool_result_computes_duration_from_call_to_result() {
    let run = run_fixture("transcript_basic.jsonl");
    let calls = events_of(&run, event_type::TOOL_CALL);
    let results = events_of(&run, event_type::TOOL_RESULT);

    let call_b = calls
        .iter()
        .find(|e| e.correlation_key.as_deref() == Some("toolu_B"))
        .expect("toolu_B call");
    assert_eq!(call_b.tool_name.as_deref(), Some("Bash"));
    assert_eq!(call_b.tool_kind.as_deref(), Some("bash"));

    let result_b = results
        .iter()
        .find(|e| e.correlation_key.as_deref() == Some("toolu_B"))
        .expect("toolu_B result");
    assert_eq!(result_b.success, Some(false));
    assert_eq!(
        result_b.duration_ms,
        Some(2500),
        "10:00:05.000 -> 10:00:07.500"
    );
}

#[test]
fn skill_tool_use_extracts_skill_name_from_input() {
    let run = run_fixture("transcript_basic.jsonl");
    let calls = events_of(&run, event_type::TOOL_CALL);
    let call_c = calls
        .iter()
        .find(|e| e.correlation_key.as_deref() == Some("toolu_C"))
        .expect("toolu_C call");

    assert_eq!(call_c.tool_name.as_deref(), Some("Skill"));
    assert_eq!(call_c.tool_kind.as_deref(), Some("skill"));
    assert_eq!(call_c.skill_name.as_deref(), Some("code-review"));
    assert_eq!(call_c.effort.as_deref(), Some("low"));
}

#[test]
fn is_api_error_message_maps_to_api_error_with_numeric_status_as_string() {
    let run = run_fixture("transcript_basic.jsonl");
    let errors = events_of(&run, event_type::API_ERROR);
    assert_eq!(errors.len(), 1);
    let err = errors[0];

    // apiErrorStatus (JSON number 429) must become the *string* "429" — never
    // the "error" field's text ("rate_limit") -- the status code is metadata, the text is not.
    assert_eq!(err.error_type.as_deref(), Some("429"));
    assert_eq!(err.model.as_deref(), Some("<synthetic>"));
    assert_eq!(err.source, "log");
    assert_eq!(err.correlation_confidence.as_deref(), Some("none"));

    // This isApiErrorMessage record must not also have produced an api.request.
    let api_requests = events_of(&run, event_type::API_REQUEST);
    assert!(
        api_requests
            .iter()
            .all(|r| r.model.as_deref() != Some("<synthetic>")),
        "the error record's requestId (req_004) must not leak into api.request"
    );
}

#[test]
fn event_ids_are_stable_across_a_second_pass_over_the_same_fixture() {
    let run1 = run_fixture("transcript_basic.jsonl");
    let run2 = run_fixture("transcript_basic.jsonl");
    assert_eq!(run1.events.len(), run2.events.len());

    let ids1: Vec<&str> = run1.events.iter().map(|e| e.event_id.as_str()).collect();
    let ids2: Vec<&str> = run2.events.iter().map(|e| e.event_id.as_str()).collect();
    assert_eq!(
        ids1, ids2,
        "re-reading the same transcript file (e.g. after a daemon restart) must reproduce identical event_ids — no epoch_nonce/seq needed here (transcript.rs doc comment)"
    );
}

#[test]
fn total_event_count_matches_the_fixture_shape() {
    let run = run_fixture("transcript_basic.jsonl");
    // session.start(1) + turn(1) + api.request(3) + tool.call(3) + tool.result(2)
    // + api.error(1) + session.end(1) = 12.
    assert_eq!(run.events.len(), 12);
}
