#!/usr/bin/env bash
# kikimimi Stage 0 smoke test (architecture.md §4, §12).
#
# Builds the release binary, runs a fully isolated `kikimimi agent`, feeds it a
# bypass-shaped sequence of realistic Claude Code hook payloads plus one OTLP
# logs request, flushes to Parquet, and checks that `kikimimi query tools` /
# `kikimimi query bypass` can see what happened via the duckdb CLI.
set -euo pipefail

# --- setup -------------------------------------------------------------

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"

if ! command -v duckdb >/dev/null 2>&1; then
  echo "smoke.sh: the 'duckdb' CLI is required (kikimimi query shells out to it) but was not found in PATH" >&2
  exit 1
fi

echo "==> building release binary"
cargo build --release -p kikimimi-cli

BIN="$REPO_ROOT/target/release/kikimimi"

WORKDIR="$(mktemp -d)"
export KIKIMIMI_DIR="$WORKDIR/kikimimi-home"
export XDG_RUNTIME_DIR="$WORKDIR/xdg-runtime"
export KIKIMIMI_OTLP_PORT="14318"
mkdir -p "$KIKIMIMI_DIR" "$XDG_RUNTIME_DIR"

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
echo "==> KIKIMIMI_DIR=$KIKIMIMI_DIR XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR KIKIMIMI_OTLP_PORT=$KIKIMIMI_OTLP_PORT"

# --- start the daemon ----------------------------------------------------

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
  echo "smoke.sh: daemon (pid $AGENT_PID) never became reachable on its control socket" >&2
  echo "----- agent.log -----" >&2
  cat "$WORKDIR/agent.log" >&2 || true
  exit 1
fi
echo "==> daemon is up (pid $AGENT_PID)"

# --- feed a bypass-shaped sequence of hook fixtures, one session_id ------
#
# SessionStart -> PreToolUse Bash -> PostToolUse mcp__github__get_issue (ok)
#   -> PostToolUseFailure mcp__github__get_issue (fails) -> PreToolUse Bash curl
# The last two events are exactly the shape `kikimimi query bypass` looks for:
# an MCP tool.result failure immediately followed (same session, within 5
# events) by a bash tool.call.

feed_hook() {
  local event="$1"
  local payload="$2"
  # kikimimi hook must always exit 0 and print nothing on success; check both here
  # rather than letting `set -e` silently abort on a contract violation.
  local out status
  set +e
  out=$(printf '%s' "$payload" | "$BIN" hook "$event")
  status=$?
  set -e
  if [[ "$status" -ne 0 ]]; then
    echo "smoke.sh: kikimimi hook $event exited $status (must always be 0)" >&2
    exit 1
  fi
  if [[ -n "$out" ]]; then
    echo "smoke.sh: kikimimi hook $event printed output on success: $out" >&2
    exit 1
  fi
  sleep 0.05 # keep ts strictly increasing across fixtures (ms resolution)
}

SESSION_START=$(cat <<'JSON'
{
  "session_id": "sess-smoke-1",
  "transcript_path": "/tmp/kikimimi-smoke/sess-smoke-1.jsonl",
  "cwd": "/tmp/kikimimi-smoke",
  "hook_event_name": "SessionStart",
  "source": "startup",
  "model": "claude-opus-4-6-20260805"
}
JSON
)

PRETOOLUSE_BASH=$(cat <<'JSON'
{
  "session_id": "sess-smoke-1",
  "transcript_path": "/tmp/kikimimi-smoke/sess-smoke-1.jsonl",
  "cwd": "/tmp/kikimimi-smoke",
  "hook_event_name": "PreToolUse",
  "tool_name": "Bash",
  "tool_input": { "command": "ls -la", "description": "List files" },
  "tool_use_id": "toolu_smoke_bash_0"
}
JSON
)

POSTTOOLUSE_MCP_OK=$(cat <<'JSON'
{
  "session_id": "sess-smoke-1",
  "transcript_path": "/tmp/kikimimi-smoke/sess-smoke-1.jsonl",
  "cwd": "/tmp/kikimimi-smoke",
  "hook_event_name": "PostToolUse",
  "tool_name": "mcp__github__get_issue",
  "tool_input": { "owner": "isamisushi", "repo": "kikimimi", "issue_number": 1 },
  "tool_response": { "success": true },
  "tool_use_id": "toolu_smoke_mcp_1",
  "duration_ms": 310
}
JSON
)

POSTTOOLUSE_MCP_FAIL=$(cat <<'JSON'
{
  "session_id": "sess-smoke-1",
  "transcript_path": "/tmp/kikimimi-smoke/sess-smoke-1.jsonl",
  "cwd": "/tmp/kikimimi-smoke",
  "hook_event_name": "PostToolUseFailure",
  "tool_name": "mcp__github__get_issue",
  "tool_input": { "owner": "isamisushi", "repo": "kikimimi", "issue_number": 999999 },
  "tool_response": { "success": false, "error": "issue not found" },
  "tool_use_id": "toolu_smoke_mcp_2",
  "duration_ms": 118
}
JSON
)

PRETOOLUSE_BASH_CURL=$(cat <<'JSON'
{
  "session_id": "sess-smoke-1",
  "transcript_path": "/tmp/kikimimi-smoke/sess-smoke-1.jsonl",
  "cwd": "/tmp/kikimimi-smoke",
  "hook_event_name": "PreToolUse",
  "tool_name": "Bash",
  "tool_input": {
    "command": "curl -s https://api.github.com/repos/isamisushi/kikimimi/issues/999999",
    "description": "MCP lookup failed, falling back to curl"
  },
  "tool_use_id": "toolu_smoke_bash_1"
}
JSON
)

echo "==> feeding hook fixtures (SessionStart, PreToolUse Bash, PostToolUse mcp ok, PostToolUseFailure mcp, PreToolUse Bash curl)"
feed_hook "SessionStart" "$SESSION_START"
feed_hook "PreToolUse" "$PRETOOLUSE_BASH"
feed_hook "PostToolUse" "$POSTTOOLUSE_MCP_OK"
feed_hook "PostToolUseFailure" "$POSTTOOLUSE_MCP_FAIL"
feed_hook "PreToolUse" "$PRETOOLUSE_BASH_CURL"

# --- one OTLP logs request (api_request with model + tokens) ------------

echo "==> posting one OTLP JSON logs request (claude_code.api_request)"
NOW_NS="$(( $(date +%s%N) ))"
curl -sS -o /dev/null -w '%{http_code}\n' \
  -X POST "http://127.0.0.1:${KIKIMIMI_OTLP_PORT}/v1/logs" \
  -H 'content-type: application/json' \
  -d "{
    \"resourceLogs\": [{
      \"resource\": { \"attributes\": [
        { \"key\": \"session.id\", \"value\": { \"stringValue\": \"sess-smoke-1\" } },
        { \"key\": \"user.email\", \"value\": { \"stringValue\": \"smoke@example.com\" } },
        { \"key\": \"organization.id\", \"value\": { \"stringValue\": \"org-smoke\" } }
      ] },
      \"scopeLogs\": [{ \"logRecords\": [{
        \"timeUnixNano\": \"${NOW_NS}\",
        \"eventName\": \"claude_code.api_request\",
        \"attributes\": [
          { \"key\": \"model\", \"value\": { \"stringValue\": \"claude-opus-4-6-20260805\" } },
          { \"key\": \"input_tokens\", \"value\": { \"intValue\": \"1520\" } },
          { \"key\": \"output_tokens\", \"value\": { \"intValue\": \"340\" } },
          { \"key\": \"cost_usd\", \"value\": { \"doubleValue\": 0.0872 } }
        ]
      }] }]
    }]
  }"

# --- flush and assert a parquet file was written -------------------------

echo "==> sleeping briefly, then flushing"
sleep 1
FLUSH_OUT=$("$BIN" flush)
echo "$FLUSH_OUT"
if [[ "$FLUSH_OUT" != *"true"* ]]; then
  echo "smoke.sh: kikimimi flush was not acked by the daemon" >&2
  exit 1
fi
sleep 0.5

PARQUET_COUNT=$(find "$KIKIMIMI_DIR/data/events" -mindepth 2 -maxdepth 2 -type f -name '*.parquet' 2>/dev/null | wc -l | tr -d ' ')
echo "==> parquet files under dt=*/: $PARQUET_COUNT"
if [[ "$PARQUET_COUNT" -lt 1 ]]; then
  echo "smoke.sh: expected at least one dt=*/*.parquet file under $KIKIMIMI_DIR/data/events" >&2
  exit 1
fi

# --- query -----------------------------------------------------------

echo "==> kikimimi query tools"
TOOLS_OUT=$("$BIN" query tools)
echo "$TOOLS_OUT"
if ! grep -q "mcp__github__get_issue" <<<"$TOOLS_OUT"; then
  echo "smoke.sh: 'kikimimi query tools' output did not mention mcp__github__get_issue" >&2
  exit 1
fi
if ! grep -q "Bash" <<<"$TOOLS_OUT"; then
  echo "smoke.sh: 'kikimimi query tools' output did not mention Bash" >&2
  exit 1
fi

echo "==> kikimimi query bypass"
BYPASS_OUT=$("$BIN" query bypass --show-sql)
echo "$BYPASS_OUT"
if ! grep -q "github" <<<"$BYPASS_OUT"; then
  echo "smoke.sh: 'kikimimi query bypass' output did not mention the github mcp_server" >&2
  exit 1
fi
if ! grep -q "Bash" <<<"$BYPASS_OUT"; then
  echo "smoke.sh: 'kikimimi query bypass' output did not mention the following Bash tool_call" >&2
  exit 1
fi

# --- status ----------------------------------------------------------

echo "==> kikimimi status"
"$BIN" status

# --- shutdown ----------------------------------------------------------

echo "==> stopping daemon (pid $AGENT_PID)"
kill "$AGENT_PID"
wait "$AGENT_PID" 2>/dev/null || true
AGENT_PID=""

echo "PASS"
