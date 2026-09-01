//! `CodexNormalizer::rollout_line` の正規化テスト。
//!
//! `tests/fixtures/rollout_line_*.json` は **このマシンで実際に codex-cli 0.151.0 が
//! 書いた本物の rollout JSONL** (`~/.codex/sessions/2026/08/31/rollout-*.jsonl`,
//! 2026-08-31 採取) の各行から、セッション ID・リポジトリ URL・コマンド文字列・
//! 標準出力・エージェント応答文などの機微情報だけを `REDACTED...` に置換したもの
//! (構造・キー名・型は一切変えていない)。`hook_*.json` (`tests/hook.rs` 側) とは違い、
//! これらは構成物ではなく実データの redact 版。

use kikimimi_adapter_codex::{CodexNormalizer, RolloutSessionCtx};

fn load(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/rollout_line_{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"))
}

/// Feeds every fixture line, in the same order they appeared in the real rollout file
/// (session_meta → task_started → turn_context → token_count → item_completed(exec) →
/// response_item(custom_tool_call) → response_item(custom_tool_call_output) →
/// world_state → task_complete), through one shared ctx -- exercising the full,
/// realistic per-file sequence rather than each line in isolation.
fn run_full_session() -> (
    CodexNormalizer,
    RolloutSessionCtx,
    Vec<kikimimi_schema::Event>,
) {
    let mut n = CodexNormalizer::new("host-1".into());
    let mut ctx = RolloutSessionCtx::default();
    let mut all = Vec::new();
    for name in [
        "session_meta",
        "task_started",
        "turn_context",
        "token_count",
        "item_completed_command_execution",
        "response_item_custom_tool_call",
        "response_item_custom_tool_call_output",
        "world_state",
        "task_complete",
    ] {
        let line = load(name);
        let events = n.rollout_line(&mut ctx, &line).unwrap();
        all.extend(events);
    }
    (n, ctx, all)
}

#[test]
fn session_meta_line_emits_session_start_and_seeds_ctx() {
    let mut n = CodexNormalizer::new("host-1".into());
    let mut ctx = RolloutSessionCtx::default();
    let events = n.rollout_line(&mut ctx, &load("session_meta")).unwrap();

    assert_eq!(events.len(), 1);
    let ev = &events[0];
    assert_eq!(ev.agent, "codex");
    assert_eq!(ev.source, "log");
    assert_eq!(ev.event_type, kikimimi_schema::event_type::SESSION_START);
    assert_eq!(ev.session_id.as_deref(), Some("REDACTED-session-id-0001"));
    assert_eq!(ev.agent_version.as_deref(), Some("0.151.0"));
    assert_eq!(ev.provider.as_deref(), Some("openai"));
    assert_eq!(
        ev.repo.as_deref(),
        Some("git@github.com:example-org/example-repo.git")
    );
    assert!(ev.cwd_hash.is_some());
    // PRIVACY: base_instructions text must never leak into the Event.
    assert!(ev.prompt_text.is_none());

    assert_eq!(ctx.session_id.as_deref(), Some("REDACTED-session-id-0001"));
    assert_eq!(ctx.provider.as_deref(), Some("openai"));
}

#[test]
fn task_started_and_task_complete_map_to_turn_with_turn_id() {
    let mut n = CodexNormalizer::new("host-1".into());
    let mut ctx = RolloutSessionCtx::default();
    ctx.session_id = Some("sess-1".into());

    let started = n.rollout_line(&mut ctx, &load("task_started")).unwrap();
    assert_eq!(started.len(), 1);
    assert_eq!(started[0].event_type, kikimimi_schema::event_type::TURN);
    assert_eq!(
        started[0].turn_id.as_deref(),
        Some("01a057b2-2768-73e3-826c-83cb99fa2b1f")
    );
    assert_eq!(
        started[0].duration_ms, None,
        "task_started has no duration yet"
    );
    assert_eq!(
        ctx.current_turn_id.as_deref(),
        Some("01a057b2-2768-73e3-826c-83cb99fa2b1f"),
        "ctx must be seeded from task_started for later lines that lack turn_id"
    );

    let complete = n.rollout_line(&mut ctx, &load("task_complete")).unwrap();
    assert_eq!(complete.len(), 1);
    assert_eq!(complete[0].event_type, kikimimi_schema::event_type::TURN);
    assert_eq!(complete[0].duration_ms, Some(47335));
    // Distinct lines (distinct ordinal) -> distinct event_id even though both are "turn".
    assert_ne!(started[0].event_id, complete[0].event_id);
}

#[test]
fn turn_context_updates_ctx_but_emits_no_event() {
    let mut n = CodexNormalizer::new("host-1".into());
    let mut ctx = RolloutSessionCtx::default();
    let events = n.rollout_line(&mut ctx, &load("turn_context")).unwrap();

    assert!(events.is_empty(), "turn_context is context-only");
    assert_eq!(ctx.current_model.as_deref(), Some("gpt-5.6-sol"));
    assert_eq!(
        ctx.current_turn_id.as_deref(),
        Some("01a057b2-2768-73e3-826c-83cb99fa2b1f")
    );
    assert_eq!(n.skipped_by_reason().get("rollout:turn_context"), Some(&1));
}

#[test]
fn token_count_maps_to_api_request_using_last_token_usage() {
    let mut n = CodexNormalizer::new("host-1".into());
    let mut ctx = RolloutSessionCtx::default();
    ctx.session_id = Some("sess-1".into());
    ctx.current_turn_id = Some("turn-1".into());
    ctx.current_model = Some("gpt-5.6-sol".into());

    let events = n.rollout_line(&mut ctx, &load("token_count")).unwrap();
    assert_eq!(events.len(), 1);
    let ev = &events[0];
    assert_eq!(ev.event_type, kikimimi_schema::event_type::API_REQUEST);
    assert_eq!(ev.input_tokens, Some(16230));
    assert_eq!(ev.cache_read_tokens, Some(11008));
    assert_eq!(ev.cache_write_tokens, Some(0));
    assert_eq!(ev.output_tokens, Some(172));
    assert_eq!(ev.reasoning_tokens, Some(0));
    assert_eq!(ev.usage_source.as_deref(), Some("log"));
    // token_count itself carries no turn_id/model -- both come from ctx enrichment.
    assert_eq!(ev.turn_id.as_deref(), Some("turn-1"));
    assert_eq!(ev.model.as_deref(), Some("gpt-5.6-sol"));
}

#[test]
fn command_execution_item_emits_paired_tool_call_and_tool_result() {
    let mut n = CodexNormalizer::new("host-1".into());
    let mut ctx = RolloutSessionCtx::default();
    ctx.session_id = Some("sess-1".into());

    let events = n
        .rollout_line(&mut ctx, &load("item_completed_command_execution"))
        .unwrap();
    assert_eq!(events.len(), 2, "one tool.call + one tool.result");

    let call = &events[0];
    let result = &events[1];
    assert_eq!(call.event_type, kikimimi_schema::event_type::TOOL_CALL);
    assert_eq!(result.event_type, kikimimi_schema::event_type::TOOL_RESULT);
    assert_ne!(call.event_id, result.event_id);

    for ev in [call, result] {
        assert_eq!(ev.tool_name.as_deref(), Some("shell"));
        assert_eq!(ev.tool_kind.as_deref(), Some("bash"));
        assert_eq!(
            ev.turn_id.as_deref(),
            Some("01a057b2-2768-73e3-826c-83cb99fa2b1f")
        );
        // PRIVACY: command/stdout text must never leak into the Event.
        assert!(ev.tool_input_json.is_none());
        assert!(ev.tool_output_excerpt.is_none());
    }

    // A plain exec (no SKILL.md read) must not get a skill_name.
    for ev in [call, result] {
        assert!(ev.skill_name.is_none());
    }

    // duration comes from item.duration {secs:0, nanos:3335} -> 0ms (sub-millisecond
    // real execution), not fabricated.
    assert_eq!(result.duration_ms, Some(0));
    assert_eq!(result.success, Some(true), "exit_code 0 -> success");
}

/// Codex filesystem skills are activated by reading their `SKILL.md` (per the
/// injected skills_instructions); an exec touching `.../<dir>/SKILL.md` therefore
/// records `skill_name = <dir>` as metadata (never the command text itself).
#[test]
fn command_execution_reading_skill_md_records_skill_name() {
    let mut n = CodexNormalizer::new("host-1".into());
    let mut ctx = RolloutSessionCtx::default();
    ctx.session_id = Some("sess-1".into());

    let events = n
        .rollout_line(&mut ctx, &load("item_completed_command_execution_skill"))
        .unwrap();
    assert_eq!(events.len(), 2);
    for ev in &events {
        assert_eq!(ev.skill_name.as_deref(), Some("imagegen"));
        assert_eq!(ev.tool_kind.as_deref(), Some("bash"));
        assert!(ev.tool_input_json.is_none(), "command text must not leak");
    }
}

/// Same underlying shell execution also appears as a `custom_tool_call` /
/// `custom_tool_call_output` response_item pair (correlated by call_id) -- these are
/// intentionally NOT mapped to Events (see `src/rollout.rs` docs: CommandExecution is
/// the higher-fidelity, structured record; mapping both would double-count).
#[test]
fn custom_tool_call_pair_is_skipped_not_double_counted() {
    let mut n = CodexNormalizer::new("host-1".into());
    let mut ctx = RolloutSessionCtx::default();

    let call_events = n
        .rollout_line(&mut ctx, &load("response_item_custom_tool_call"))
        .unwrap();
    let output_events = n
        .rollout_line(&mut ctx, &load("response_item_custom_tool_call_output"))
        .unwrap();

    assert!(call_events.is_empty());
    assert!(output_events.is_empty());
    assert_eq!(
        n.skipped_by_reason()
            .get("rollout:response_item:custom_tool_call"),
        Some(&1)
    );
    assert_eq!(
        n.skipped_by_reason()
            .get("rollout:response_item:custom_tool_call_output"),
        Some(&1)
    );
}

#[test]
fn world_state_is_skipped() {
    let mut n = CodexNormalizer::new("host-1".into());
    let mut ctx = RolloutSessionCtx::default();
    let events = n.rollout_line(&mut ctx, &load("world_state")).unwrap();
    assert!(events.is_empty());
    assert_eq!(n.skipped_by_reason().get("rollout:world_state"), Some(&1));
}

#[test]
fn unknown_envelope_type_is_skipped_by_reason() {
    let mut n = CodexNormalizer::new("host-1".into());
    let mut ctx = RolloutSessionCtx::default();
    let line = serde_json::json!({"timestamp": "2026-08-31T00:00:00Z", "ordinal": 0, "type": "future_envelope_kind", "payload": {}}).to_string();
    let events = n.rollout_line(&mut ctx, &line).unwrap();
    assert!(events.is_empty());
    assert_eq!(
        n.skipped_by_reason().get("rollout:future_envelope_kind"),
        Some(&1)
    );
}

#[test]
fn unknown_item_completed_item_type_is_skipped_not_guessed() {
    let mut n = CodexNormalizer::new("host-1".into());
    let mut ctx = RolloutSessionCtx::default();
    // No real McpToolCall/FileChange sample was available on this machine (no MCP
    // servers configured -- see src/rollout.rs docs); this exercises that unknown
    // item.type values are visibly skipped rather than mis-parsed.
    let line = serde_json::json!({
        "timestamp": "2026-08-31T00:00:00Z",
        "ordinal": 5,
        "type": "event_msg",
        "payload": {
            "type": "item_completed",
            "turn_id": "t1",
            "item": {"type": "McpToolCall", "id": "x"}
        }
    })
    .to_string();
    let events = n.rollout_line(&mut ctx, &line).unwrap();
    assert!(events.is_empty());
    assert_eq!(
        n.skipped_by_reason().get("rollout:item:McpToolCall"),
        Some(&1)
    );
}

#[test]
fn empty_line_is_a_noop_not_an_error() {
    let mut n = CodexNormalizer::new("host-1".into());
    let mut ctx = RolloutSessionCtx::default();
    assert_eq!(n.rollout_line(&mut ctx, "").unwrap(), vec![]);
    assert_eq!(n.rollout_line(&mut ctx, "   \n").unwrap(), vec![]);
    assert_eq!(
        n.skipped(),
        0,
        "a blank line (e.g. a not-yet-flushed tail) is not a skip"
    );
}

#[test]
fn malformed_json_line_is_an_error_the_caller_can_count_separately() {
    let mut n = CodexNormalizer::new("host-1".into());
    let mut ctx = RolloutSessionCtx::default();
    assert!(n.rollout_line(&mut ctx, "{not valid json").is_err());
}

#[test]
fn event_id_is_deterministic_across_a_simulated_restart() {
    // Re-reading the exact same line (e.g. after a daemon restart that persisted the
    // byte offset just *before* the sink flush that would have advanced past it) must
    // reproduce the identical event_id, so cloud's ON CONFLICT DO NOTHING dedups it
    // instead of double-counting -- unlike hook()'s tool_use_id-less fallback keys,
    // which deliberately do NOT do this (see src/rollout.rs's `rollout_primary_key` docs).
    let mut before = CodexNormalizer::new("host-1".into());
    let mut after = CodexNormalizer::new("host-1".into());
    let mut ctx_before = RolloutSessionCtx::default();
    let mut ctx_after = RolloutSessionCtx::default();

    let a = before
        .rollout_line(&mut ctx_before, &load("session_meta"))
        .unwrap();
    let b = after
        .rollout_line(&mut ctx_after, &load("session_meta"))
        .unwrap();
    assert_eq!(a[0].event_id, b[0].event_id);
}

#[test]
fn full_realistic_session_produces_the_expected_row_shape() {
    let (_n, _ctx, events) = run_full_session();
    // session_meta(1) + task_started(1) + turn_context(0) + token_count(1) +
    // command_execution(2: call+result) + custom_tool_call(0) +
    // custom_tool_call_output(0) + world_state(0) + task_complete(1) = 6
    assert_eq!(events.len(), 6);

    let types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
    assert_eq!(
        types,
        vec![
            kikimimi_schema::event_type::SESSION_START,
            kikimimi_schema::event_type::TURN,
            kikimimi_schema::event_type::API_REQUEST,
            kikimimi_schema::event_type::TOOL_CALL,
            kikimimi_schema::event_type::TOOL_RESULT,
            kikimimi_schema::event_type::TURN,
        ]
    );

    // Every event_id in one session must be unique (no accidental collisions across
    // the different event_types/ordinals exercised here).
    let mut ids: Vec<&str> = events.iter().map(|e| e.event_id.as_str()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), events.len());

    // All rows share the same session_id seeded by the session_meta line.
    for ev in &events {
        assert_eq!(ev.session_id.as_deref(), Some("REDACTED-session-id-0001"));
        assert_eq!(ev.agent, "codex");
        assert_eq!(ev.source, "log");
    }
}
