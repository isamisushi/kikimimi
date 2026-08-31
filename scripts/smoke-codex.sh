#!/usr/bin/env bash
# kikimimi Codex rollout tailer smoke test (architecture.md §4「ログ tailer」, §4.1 Codex 行).
#
# Builds the release binary, runs a fully isolated `kikimimi agent` with a fake
# CODEX_HOME, appends to a fake `~/.codex/sessions/**/rollout-*.jsonl` file *while
# the daemon is running* (the realistic "file created after the watcher started"
# path -- see crates/cli/src/codex_tailer.rs), and checks that events with
# `agent = 'codex'` land in the local Parquet.
#
# The rollout line shapes here mirror the REAL shapes captured on 2026-08-31 from
# codex-cli 0.151.0 (see crates/adapter-codex/tests/fixtures/rollout_line_*.json,
# redacted from an actual rollout file on this machine) -- not the stale shapes in
# internal/research/hook-telemetry-daemon.md.
set -euo pipefail

# --- setup -------------------------------------------------------------

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"

if ! command -v duckdb >/dev/null 2>&1; then
  echo "smoke-codex.sh: the 'duckdb' CLI is required (kikimimi query shells out to it) but was not found in PATH" >&2
  exit 1
fi

echo "==> building release binary"
cargo build --release -p kikimimi

BIN="$REPO_ROOT/target/release/kikimimi"

WORKDIR="$(mktemp -d)"
export KIKIMIMI_DIR="$WORKDIR/kikimimi-home"
export XDG_RUNTIME_DIR="$WORKDIR/xdg-runtime"
export KIKIMIMI_OTLP_PORT="14328"
export CODEX_HOME="$WORKDIR/codex-home"
mkdir -p "$KIKIMIMI_DIR" "$XDG_RUNTIME_DIR" "$CODEX_HOME/sessions/2026/08/31"

AGENT_PID=""
cleanup() {
  if [[ -n "$AGENT_PID" ]] && kill -0 "$AGENT_PID" 2>/dev/null; then
    kill "$AGENT_PID" 2>/dev/null || true
    wait "$AGENT_PID" 2>/dev/null || true
  fi
  rm -rf "$WORKDIR"
}
trap cleanup EXIT

echo "==> workdir: $WORKDIR"
echo "==> KIKIMIMI_DIR=$KIKIMIMI_DIR XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR CODEX_HOME=$CODEX_HOME KIKIMIMI_OTLP_PORT=$KIKIMIMI_OTLP_PORT"

# --- start the daemon (sessions dir exists but is empty at this point) ---

echo "==> starting kikimimi agent --foreground in background"
"$BIN" agent --foreground >"$WORKDIR/agent.log" 2>&1 &
AGENT_PID=$!

READY=0
for _ in $(seq 1 50); do
  if "$BIN" flush >/dev/null 2>&1; then
    READY=1
    break
  fi
  sleep 0.1
done
if [[ "$READY" -ne 1 ]]; then
  echo "smoke-codex.sh: daemon (pid $AGENT_PID) never became reachable on its control socket" >&2
  echo "----- agent.log -----" >&2
  cat "$WORKDIR/agent.log" >&2 || true
  exit 1
fi
echo "==> daemon is up (pid $AGENT_PID)"

# Let the daemon's Codex tailer complete at least one scan of the (still empty)
# sessions dir first, so it marks itself "initialized" and the rollout file we
# create next is treated as "appeared after the watcher started" (read from byte
# 0) -- not as historical backlog seeded at EOF. See codex_tailer.rs docs.
sleep 3

# --- fake rollout file, appended to incrementally while the daemon runs -----
#
# Real shape (envelope): {"timestamp": <RFC3339>, "ordinal": <u64>, "type": ..., "payload": {...}}.

ROLLOUT_FILE="$CODEX_HOME/sessions/2026/08/31/rollout-2026-08-31T00-00-00-smoke0000-0000-0000-0000-000000000000.jsonl"
SESSION_ID="sess-smoke-codex-0001"
TURN_ID="turn-smoke-codex-0001"

append_line() {
  printf '%s\n' "$1" >>"$ROLLOUT_FILE"
  sleep 0.2 # give the 2s-cadence tailer a moment; also keeps ts strictly increasing
}

echo "==> creating rollout file and appending session_meta"
append_line "{\"timestamp\":\"2026-08-31T00:00:00.000Z\",\"ordinal\":0,\"type\":\"session_meta\",\"payload\":{\"session_id\":\"$SESSION_ID\",\"id\":\"$SESSION_ID\",\"timestamp\":\"2026-08-31T00:00:00.000Z\",\"cwd\":\"/tmp/kikimimi-smoke-codex\",\"originator\":\"codex-tui\",\"cli_version\":\"0.151.0\",\"source\":\"cli\",\"thread_source\":\"user\",\"model_provider\":\"openai\",\"git\":{\"commit_hash\":\"deadbeef\",\"branch\":\"main\",\"repository_url\":\"git@github.com:example-org/example-repo.git\"}}}"

echo "==> appending task_started"
append_line "{\"timestamp\":\"2026-08-31T00:00:01.000Z\",\"ordinal\":1,\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\",\"turn_id\":\"$TURN_ID\",\"started_at\":1788000001,\"model_context_window\":258400,\"collaboration_mode_kind\":\"default\"}}"

echo "==> appending turn_context (model)"
append_line "{\"timestamp\":\"2026-08-31T00:00:01.500Z\",\"ordinal\":2,\"type\":\"turn_context\",\"payload\":{\"turn_id\":\"$TURN_ID\",\"cwd\":\"/tmp/kikimimi-smoke-codex\",\"model\":\"gpt-5.6-sol\"}}"

echo "==> appending item_completed CommandExecution (exec, exit 0)"
append_line "{\"timestamp\":\"2026-08-31T00:00:02.000Z\",\"ordinal\":3,\"type\":\"event_msg\",\"payload\":{\"type\":\"item_completed\",\"thread_id\":\"$SESSION_ID\",\"turn_id\":\"$TURN_ID\",\"item\":{\"type\":\"CommandExecution\",\"id\":\"exec-smoke-0001\",\"command\":[\"/bin/sh\",\"-lc\",\"echo hi\"],\"cwd\":\"file:///tmp/kikimimi-smoke-codex\",\"status\":\"completed\",\"exit_code\":0,\"duration\":{\"secs\":0,\"nanos\":5000000}},\"started_at_ms\":1788000002000,\"completed_at_ms\":1788000002005}}"

echo "==> appending token_count"
append_line "{\"timestamp\":\"2026-08-31T00:00:02.500Z\",\"ordinal\":4,\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":100,\"cached_input_tokens\":10,\"cache_write_input_tokens\":0,\"output_tokens\":20,\"reasoning_output_tokens\":0,\"total_tokens\":120},\"last_token_usage\":{\"input_tokens\":100,\"cached_input_tokens\":10,\"cache_write_input_tokens\":0,\"output_tokens\":20,\"reasoning_output_tokens\":0,\"total_tokens\":120},\"model_context_window\":258400}}}"

echo "==> appending task_complete"
append_line "{\"timestamp\":\"2026-08-31T00:00:03.000Z\",\"ordinal\":5,\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\",\"turn_id\":\"$TURN_ID\",\"last_agent_message\":\"done\",\"started_at\":1788000001,\"completed_at\":1788000003,\"duration_ms\":2000,\"time_to_first_token_ms\":500}}"

# --- give the tailer a couple of ticks, then flush and assert parquet ----

echo "==> sleeping for the tailer's tick cadence, then flushing"
sleep 3
FLUSH_OUT=$("$BIN" flush)
echo "$FLUSH_OUT"
if [[ "$FLUSH_OUT" != *"true"* ]]; then
  echo "smoke-codex.sh: kikimimi flush was not acked by the daemon" >&2
  exit 1
fi
sleep 0.5

PARQUET_COUNT=$(find "$KIKIMIMI_DIR/data/events" -mindepth 2 -maxdepth 2 -type f -name '*.parquet' 2>/dev/null | wc -l | tr -d ' ')
echo "==> parquet files under dt=*/: $PARQUET_COUNT"
if [[ "$PARQUET_COUNT" -lt 1 ]]; then
  echo "smoke-codex.sh: expected at least one dt=*/*.parquet file under $KIKIMIMI_DIR/data/events" >&2
  exit 1
fi

# --- query: agent='codex' rows must be present with the expected shape ----

GLOB="$KIKIMIMI_DIR/data/events/dt=*/*.parquet"
echo "==> querying codex rows via duckdb"
CODEX_ROWS=$("$BIN" query --sql "SELECT event_type, source, tool_name, tool_kind, success, duration_ms, model, session_id FROM read_parquet('$GLOB') WHERE agent = 'codex' ORDER BY ts;")
echo "$CODEX_ROWS"

if ! grep -q "session.start" <<<"$CODEX_ROWS"; then
  echo "smoke-codex.sh: expected a session.start row for agent='codex'" >&2
  exit 1
fi
if ! grep -q "$SESSION_ID" <<<"$CODEX_ROWS"; then
  echo "smoke-codex.sh: expected rows to carry session_id=$SESSION_ID" >&2
  exit 1
fi
if ! grep -qE 'tool\.(call|result).*shell.*bash' <<<"$CODEX_ROWS"; then
  echo "smoke-codex.sh: expected a tool.call/tool.result row with tool_name=shell tool_kind=bash" >&2
  exit 1
fi
if ! grep -q "api.request" <<<"$CODEX_ROWS"; then
  echo "smoke-codex.sh: expected an api.request row (from token_count)" >&2
  exit 1
fi
if ! grep -qw "turn" <<<"$CODEX_ROWS"; then
  echo "smoke-codex.sh: expected at least one turn row (task_started/task_complete)" >&2
  exit 1
fi

TOKEN_ROW=$("$BIN" query --sql "SELECT input_tokens, output_tokens, cache_read_tokens, usage_source FROM read_parquet('$GLOB') WHERE agent = 'codex' AND event_type = 'api.request';")
echo "$TOKEN_ROW"
if ! grep -q "100" <<<"$TOKEN_ROW" || ! grep -q "log" <<<"$TOKEN_ROW"; then
  echo "smoke-codex.sh: expected api.request row with input_tokens=100 usage_source=log" >&2
  exit 1
fi

# --- status: files_watched / lines_read must reflect the tailed file -----

echo "==> kikimimi status"
STATUS_OUT=$("$BIN" status)
echo "$STATUS_OUT"
if ! grep -q "codex (rollout tailer):" <<<"$STATUS_OUT"; then
  echo "smoke-codex.sh: 'kikimimi status' did not print the codex tailer section" >&2
  exit 1
fi
if ! grep -q "files_watched: 1" <<<"$STATUS_OUT"; then
  echo "smoke-codex.sh: expected files_watched: 1 in 'kikimimi status'" >&2
  exit 1
fi

# --- shutdown ----------------------------------------------------------

echo "==> stopping daemon (pid $AGENT_PID)"
kill "$AGENT_PID"
wait "$AGENT_PID" 2>/dev/null || true
AGENT_PID=""

echo "PASS"
