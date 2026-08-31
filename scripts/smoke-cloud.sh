#!/usr/bin/env bash
# kikimimi Stage 0 cloud smoke test (architecture.md §6, §8, §12).
#
# End-to-end, two-host check against a real kikimimi-cloud server + a real
# (throwaway) Postgres database in the dev Docker container:
#
#   1. cargo build --release (workspace)
#   2. fresh test database + kikimimi-cloud on 127.0.0.1:8790 (KIKIMIMI_DEV_AUTOAPPROVE=1)
#   3. host A: kikimimi login (autoapprove) -> kikimimi agent -> 3 hook fixtures
#      (SessionStart, PreToolUse/PostToolUse mcp__github__get_issue) -> kikimimi
#      flush -> stop agent
#   4. host B: same, different KIKIMIMI_DIR/XDG_RUNTIME_DIR/OTLP port, a Bash
#      tool + a distinct session_id, same (autoapprove default) login email
#      so both hosts land in one org
#   5. psql: events from BOTH host_ids, one uniform org_id; host A's exact
#      batch reconstructed from the DB and re-POSTed -> deduped, row count
#      unchanged
#   6. `kikimimi query --cloud tools` (host A) shows both hosts' tools; `kikimimi
#      export`'s parquet row count matches psql (via the duckdb CLI)
#   7. RLS negative check: a second kikimimi-cloud process with
#      KIKIMIMI_DEV_EMAIL=other@local, a fresh login against it -> its
#      `/v1/query/today` returns 0 rows
#   8. cleanup (kill everything, drop the test database) -> PASS
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"

for cmd in psql curl gzip duckdb python3; do
  if ! command -v "$cmd" >/dev/null 2>&1; then
    echo "smoke-cloud.sh: required command '$cmd' not found in PATH" >&2
    exit 1
  fi
done

# --- fixed config (per the task spec) -------------------------------------

PG_HOST=127.0.0.1
PG_PORT=5433
PG_SUPERUSER=postgres
export PGPASSWORD="guru-dev" # shared local dev Postgres container's actual superuser password -- unrelated to the product rename, see README/report

CLOUD_BIND="127.0.0.1:8790"
CLOUD_URL="http://${CLOUD_BIND}"
CLOUD2_BIND="127.0.0.1:8791"
CLOUD2_URL="http://${CLOUD2_BIND}"

TEST_DB="kikimimi_smoke_$(date +%s)_$$"

WORKDIR=/tmp/kikimimi-e2e
KIKIMIMI_DIR_A="$WORKDIR/a"
RT_A="$WORKDIR/a-rt"
KIKIMIMI_DIR_B="$WORKDIR/b"
RT_B="$WORKDIR/b-rt"
KIKIMIMI_DIR_C="$WORKDIR/c"
RT_C="$WORKDIR/c-rt"

OTLP_PORT_A=14319
OTLP_PORT_B=14320

BIN="$REPO_ROOT/target/release/kikimimi"
CLOUD_BIN="$REPO_ROOT/target/release/kikimimi-cloud"

rm -rf "$WORKDIR"
mkdir -p "$KIKIMIMI_DIR_A" "$RT_A" "$KIKIMIMI_DIR_B" "$RT_B" "$KIKIMIMI_DIR_C" "$RT_C"

# --- helpers ---------------------------------------------------------------

psql_test() {
  psql -h "$PG_HOST" -p "$PG_PORT" -U "$PG_SUPERUSER" -d "$TEST_DB" -v ON_ERROR_STOP=1 "$@"
}

psql_admin() {
  psql -h "$PG_HOST" -p "$PG_PORT" -U "$PG_SUPERUSER" -d postgres -v ON_ERROR_STOP=1 "$@"
}

# Reads `.cloud.<field>` out of a saved config.json (`token` / `org_id` / `email`).
cfg_field() {
  python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['cloud'][sys.argv[2]])" "$1" "$2"
}

json_field() {
  python3 -c "import json,sys; d=json.loads(sys.argv[1]); v=d[sys.argv[2]]; print(v if not isinstance(v,(list,dict)) else json.dumps(v))" "$1" "$2"
}

wait_http_ok() {
  local url="$1" tries="${2:-100}"
  local i=0
  until curl -fsS -o /dev/null --max-time 2 "$url" 2>/dev/null; do
    i=$((i + 1))
    if [[ "$i" -ge "$tries" ]]; then
      echo "smoke-cloud.sh: $url never became reachable" >&2
      return 1
    fi
    sleep 0.1
  done
}

wait_daemon_ready() {
  local dir="$1" rt="$2" pid="$3"
  local i=0
  while true; do
    if env KIKIMIMI_DIR="$dir" XDG_RUNTIME_DIR="$rt" "$BIN" flush >/dev/null 2>&1; then
      return 0
    fi
    if ! kill -0 "$pid" 2>/dev/null; then
      echo "smoke-cloud.sh: agent (pid $pid, KIKIMIMI_DIR=$dir) died before becoming ready" >&2
      return 1
    fi
    i=$((i + 1))
    if [[ "$i" -ge 100 ]]; then
      echo "smoke-cloud.sh: agent (pid $pid, KIKIMIMI_DIR=$dir) never became reachable on its control socket" >&2
      return 1
    fi
    sleep 0.1
  done
}

# Polls until `events` has at least $3 rows for host_id=$2 (cloud-side flush
# is asynchronous: `kikimimi flush` only guarantees the daemon *received* the
# request, not that it has finished POSTing to kikimimi-cloud yet).
wait_for_row_count() {
  local host_id="$1" expected="$2" tries="${3:-100}"
  local i=0 count
  while true; do
    count=$(psql_test -tAc "SELECT count(*) FROM events WHERE host_id = '${host_id}';")
    if [[ "$count" -ge "$expected" ]]; then
      echo "$count"
      return 0
    fi
    i=$((i + 1))
    if [[ "$i" -ge "$tries" ]]; then
      echo "smoke-cloud.sh: host_id=$host_id never reached $expected rows (stuck at $count)" >&2
      echo "$count"
      return 1
    fi
    sleep 0.1
  done
}

feed_hook() {
  local dir="$1" rt="$2" event="$3" payload="$4"
  local out status
  set +e
  out=$(printf '%s' "$payload" | env KIKIMIMI_DIR="$dir" XDG_RUNTIME_DIR="$rt" "$BIN" hook "$event")
  status=$?
  set -e
  if [[ "$status" -ne 0 ]]; then
    echo "smoke-cloud.sh: kikimimi hook $event (KIKIMIMI_DIR=$dir) exited $status (must always be 0)" >&2
    exit 1
  fi
  if [[ -n "$out" ]]; then
    echo "smoke-cloud.sh: kikimimi hook $event (KIKIMIMI_DIR=$dir) printed output on success: $out" >&2
    exit 1
  fi
  sleep 0.05 # keep ts strictly increasing across fixtures (ms resolution)
}

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

CLOUD_PID=""
CLOUD2_PID=""
AGENT_A_PID=""
AGENT_B_PID=""

cleanup() {
  set +e
  for pid in "$AGENT_A_PID" "$AGENT_B_PID" "$CLOUD_PID" "$CLOUD2_PID"; do
    [[ -n "$pid" ]] && kill "$pid" 2>/dev/null
  done
  for pid in "$AGENT_A_PID" "$AGENT_B_PID" "$CLOUD_PID" "$CLOUD2_PID"; do
    [[ -n "$pid" ]] && wait "$pid" 2>/dev/null
  done
  if [[ -n "${TEST_DB:-}" ]]; then
    psql_admin -c "DROP DATABASE IF EXISTS \"${TEST_DB}\" WITH (FORCE);" >/dev/null 2>&1
  fi
  rm -rf "$WORKDIR"
}
trap cleanup EXIT

# --- 1. build ---------------------------------------------------------------

echo "==> [1/8] cargo build --release (workspace)"
cargo build --release

# --- 2. fresh test db + kikimimi-cloud ------------------------------------------

echo "==> [2/8] creating test database $TEST_DB and starting kikimimi-cloud on $CLOUD_BIND"
psql_admin -c "CREATE DATABASE \"${TEST_DB}\";"
DATABASE_URL="postgres://${PG_SUPERUSER}:${PGPASSWORD}@${PG_HOST}:${PG_PORT}/${TEST_DB}"

env BIND_ADDR="$CLOUD_BIND" DATABASE_URL="$DATABASE_URL" KIKIMIMI_DEV_AUTOAPPROVE=1 \
  "$CLOUD_BIN" >"$WORKDIR/cloud.log" 2>&1 &
CLOUD_PID=$!
wait_http_ok "$CLOUD_URL/healthz" || { cat "$WORKDIR/cloud.log" >&2; exit 1; }
echo "==> kikimimi-cloud is up (pid $CLOUD_PID, db $TEST_DB)"

# --- 3. host A ---------------------------------------------------------------

echo "==> [3/8] host A: login, agent, 3 hook fixtures (SessionStart + PreToolUse/PostToolUse mcp__github__get_issue), flush, stop"

env KIKIMIMI_DIR="$KIKIMIMI_DIR_A" XDG_RUNTIME_DIR="$RT_A" "$BIN" login --endpoint "$CLOUD_URL" --no-browser
HOST_A_ID=$(cat "$KIKIMIMI_DIR_A/host_id")
TOKEN_A=$(cfg_field "$KIKIMIMI_DIR_A/config.json" token)
ORG_A=$(cfg_field "$KIKIMIMI_DIR_A/config.json" org_id)
echo "==> host A logged in: host_id=$HOST_A_ID org_id=$ORG_A"

env KIKIMIMI_DIR="$KIKIMIMI_DIR_A" XDG_RUNTIME_DIR="$RT_A" KIKIMIMI_OTLP_PORT="$OTLP_PORT_A" \
  "$BIN" agent --foreground >"$WORKDIR/agent-a.log" 2>&1 &
AGENT_A_PID=$!
wait_daemon_ready "$KIKIMIMI_DIR_A" "$RT_A" "$AGENT_A_PID" || { cat "$WORKDIR/agent-a.log" >&2; exit 1; }
echo "==> host A agent is up (pid $AGENT_A_PID)"

A_SESSION_START=$(cat <<'JSON'
{
  "session_id": "sess-e2e-a",
  "transcript_path": "/tmp/kikimimi-e2e/a/sess-e2e-a.jsonl",
  "cwd": "/tmp/kikimimi-e2e/a",
  "hook_event_name": "SessionStart",
  "source": "startup",
  "model": "claude-opus-4-6-20260805"
}
JSON
)
A_PRETOOLUSE_MCP=$(cat <<'JSON'
{
  "session_id": "sess-e2e-a",
  "transcript_path": "/tmp/kikimimi-e2e/a/sess-e2e-a.jsonl",
  "cwd": "/tmp/kikimimi-e2e/a",
  "hook_event_name": "PreToolUse",
  "tool_name": "mcp__github__get_issue",
  "tool_input": { "owner": "isamisushi", "repo": "kikimimi", "issue_number": 42 },
  "tool_use_id": "toolu_e2e_a1"
}
JSON
)
A_POSTTOOLUSE_MCP=$(cat <<'JSON'
{
  "session_id": "sess-e2e-a",
  "transcript_path": "/tmp/kikimimi-e2e/a/sess-e2e-a.jsonl",
  "cwd": "/tmp/kikimimi-e2e/a",
  "hook_event_name": "PostToolUse",
  "tool_name": "mcp__github__get_issue",
  "tool_input": { "owner": "isamisushi", "repo": "kikimimi", "issue_number": 42 },
  "tool_response": { "success": true },
  "tool_use_id": "toolu_e2e_a1",
  "duration_ms": 890
}
JSON
)

feed_hook "$KIKIMIMI_DIR_A" "$RT_A" "SessionStart" "$A_SESSION_START"
feed_hook "$KIKIMIMI_DIR_A" "$RT_A" "PreToolUse" "$A_PRETOOLUSE_MCP"
feed_hook "$KIKIMIMI_DIR_A" "$RT_A" "PostToolUse" "$A_POSTTOOLUSE_MCP"

env KIKIMIMI_DIR="$KIKIMIMI_DIR_A" XDG_RUNTIME_DIR="$RT_A" "$BIN" flush
wait_for_row_count "$HOST_A_ID" 3 >/dev/null || { cat "$WORKDIR/agent-a.log" >&2; exit 1; }

kill "$AGENT_A_PID"
wait "$AGENT_A_PID" 2>/dev/null || true
AGENT_A_PID=""
echo "==> host A: 3 events flushed to cloud, agent stopped"

# --- 4. host B ---------------------------------------------------------------

echo "==> [4/8] host B: same flow, distinct session/host/port, a Bash tool, same login email"

env KIKIMIMI_DIR="$KIKIMIMI_DIR_B" XDG_RUNTIME_DIR="$RT_B" "$BIN" login --endpoint "$CLOUD_URL" --no-browser
HOST_B_ID=$(cat "$KIKIMIMI_DIR_B/host_id")
TOKEN_B=$(cfg_field "$KIKIMIMI_DIR_B/config.json" token)
ORG_B=$(cfg_field "$KIKIMIMI_DIR_B/config.json" org_id)
echo "==> host B logged in: host_id=$HOST_B_ID org_id=$ORG_B"

if [[ "$ORG_A" != "$ORG_B" ]]; then
  fail "host A (org $ORG_A) and host B (org $ORG_B) should share one org (same autoapprove email)"
fi

env KIKIMIMI_DIR="$KIKIMIMI_DIR_B" XDG_RUNTIME_DIR="$RT_B" KIKIMIMI_OTLP_PORT="$OTLP_PORT_B" \
  "$BIN" agent --foreground >"$WORKDIR/agent-b.log" 2>&1 &
AGENT_B_PID=$!
wait_daemon_ready "$KIKIMIMI_DIR_B" "$RT_B" "$AGENT_B_PID" || { cat "$WORKDIR/agent-b.log" >&2; exit 1; }
echo "==> host B agent is up (pid $AGENT_B_PID)"

B_SESSION_START=$(cat <<'JSON'
{
  "session_id": "sess-e2e-b",
  "transcript_path": "/tmp/kikimimi-e2e/b/sess-e2e-b.jsonl",
  "cwd": "/tmp/kikimimi-e2e/b",
  "hook_event_name": "SessionStart",
  "source": "startup",
  "model": "claude-opus-4-6-20260805"
}
JSON
)
B_PRETOOLUSE_BASH=$(cat <<'JSON'
{
  "session_id": "sess-e2e-b",
  "transcript_path": "/tmp/kikimimi-e2e/b/sess-e2e-b.jsonl",
  "cwd": "/tmp/kikimimi-e2e/b",
  "hook_event_name": "PreToolUse",
  "tool_name": "Bash",
  "tool_input": { "command": "ls -la", "description": "List files" },
  "tool_use_id": "toolu_e2e_b1"
}
JSON
)
B_POSTTOOLUSE_BASH=$(cat <<'JSON'
{
  "session_id": "sess-e2e-b",
  "transcript_path": "/tmp/kikimimi-e2e/b/sess-e2e-b.jsonl",
  "cwd": "/tmp/kikimimi-e2e/b",
  "hook_event_name": "PostToolUse",
  "tool_name": "Bash",
  "tool_input": { "command": "ls -la", "description": "List files" },
  "tool_response": { "stdout": "file1\nfile2\n", "stderr": "", "success": true },
  "tool_use_id": "toolu_e2e_b1",
  "duration_ms": 142
}
JSON
)

feed_hook "$KIKIMIMI_DIR_B" "$RT_B" "SessionStart" "$B_SESSION_START"
feed_hook "$KIKIMIMI_DIR_B" "$RT_B" "PreToolUse" "$B_PRETOOLUSE_BASH"
feed_hook "$KIKIMIMI_DIR_B" "$RT_B" "PostToolUse" "$B_POSTTOOLUSE_BASH"

env KIKIMIMI_DIR="$KIKIMIMI_DIR_B" XDG_RUNTIME_DIR="$RT_B" "$BIN" flush
wait_for_row_count "$HOST_B_ID" 3 >/dev/null || { cat "$WORKDIR/agent-b.log" >&2; exit 1; }

kill "$AGENT_B_PID"
wait "$AGENT_B_PID" 2>/dev/null || true
AGENT_B_PID=""
echo "==> host B: 3 events flushed to cloud, agent stopped"

# --- 5. psql assertions + resend/dedup --------------------------------------

echo "==> [5/8] psql: both host_ids present, org_id uniform; resend host A's batch -> deduped"

HOST_ROWS=$(psql_test -tAc "SELECT host_id || '=' || count(*) FROM events GROUP BY host_id ORDER BY host_id;")
echo "$HOST_ROWS"
grep -q "^${HOST_A_ID}=3$" <<<"$HOST_ROWS" || fail "expected exactly 3 rows for host A ($HOST_A_ID)"
grep -q "^${HOST_B_ID}=3$" <<<"$HOST_ROWS" || fail "expected exactly 3 rows for host B ($HOST_B_ID)"

DISTINCT_ORGS=$(psql_test -tAc "SELECT count(DISTINCT org_id) FROM events;")
[[ "$DISTINCT_ORGS" -eq 1 ]] || fail "expected exactly 1 distinct org_id across events, got $DISTINCT_ORGS"

TOTAL_BEFORE=$(psql_test -tAc "SELECT count(*) FROM events;")
echo "==> total rows before resend: $TOTAL_BEFORE"

# Reconstruct host A's exact 3 events straight from the DB (same event_ids)
# and re-POST them as a fresh batch, exercising ON CONFLICT (event_id) DO
# NOTHING end to end rather than assuming anything about the client.
EVENTS_JSON=$(psql_test -tAc "
  SELECT coalesce(jsonb_agg(row_to_json(e)), '[]'::jsonb) FROM (
    SELECT event_id, ts, dt, org_id::text AS org_id, team_id, user_id, user_id_source,
           host_id, env_kind, os, agent, agent_version, session_id, parent_session_id,
           turn_id, cwd_hash, repo, source, correlation_key, correlation_confidence,
           event_type, tool_name, tool_kind, mcp_server, mcp_tool, duration_ms, success,
           error_type, decision, decision_source, provider, model, effort, thinking,
           input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
           reasoning_tokens, cost_usd, usage_source, tool_input_json, tool_output_excerpt,
           prompt_text, redaction_applied
    FROM events WHERE host_id = '${HOST_A_ID}' ORDER BY ts
  ) e;
")

RESEND_JSON="$WORKDIR/resend.json"
printf '{"schema":"kikimimi.v1","events":%s}' "$EVENTS_JSON" >"$RESEND_JSON"
gzip -c "$RESEND_JSON" >"$RESEND_JSON.gz"

RESEND_RESP=$(curl -fsS --max-time 10 -X POST "$CLOUD_URL/v1/events" \
  -H "Authorization: Bearer $TOKEN_A" \
  -H "Content-Encoding: gzip" \
  -H "Content-Type: application/json" \
  --data-binary @"$RESEND_JSON.gz")
echo "==> resend response: $RESEND_RESP"

RESEND_ACCEPTED=$(json_field "$RESEND_RESP" accepted)
RESEND_DEDUPED=$(json_field "$RESEND_RESP" deduped)
[[ "$RESEND_ACCEPTED" -eq 0 ]] || fail "resend of host A's batch should have accepted=0, got $RESEND_ACCEPTED"
[[ "$RESEND_DEDUPED" -eq 3 ]] || fail "resend of host A's batch should have deduped=3, got $RESEND_DEDUPED"

TOTAL_AFTER=$(psql_test -tAc "SELECT count(*) FROM events;")
[[ "$TOTAL_AFTER" -eq "$TOTAL_BEFORE" ]] || fail "row count changed after resend ($TOTAL_BEFORE -> $TOTAL_AFTER)"
echo "==> resend deduped ($RESEND_DEDUPED events), row count unchanged at $TOTAL_AFTER"

# --- 6. kikimimi query --cloud + kikimimi export ------------------------------------

echo "==> [6/8] kikimimi query --cloud tools (host A) + kikimimi export vs psql (duckdb)"

TOOLS_OUT=$(env KIKIMIMI_DIR="$KIKIMIMI_DIR_A" XDG_RUNTIME_DIR="$RT_A" "$BIN" query --cloud tools)
echo "$TOOLS_OUT"
grep -q "mcp__github__get_issue" <<<"$TOOLS_OUT" || fail "cloud 'tools' query missing mcp__github__get_issue (host A's session)"
grep -q "Bash" <<<"$TOOLS_OUT" || fail "cloud 'tools' query missing Bash (host B's session) -- cross-host aggregation broken"

EXPORT_PATH="$WORKDIR/export.parquet"
env KIKIMIMI_DIR="$KIKIMIMI_DIR_A" XDG_RUNTIME_DIR="$RT_A" "$BIN" export -o "$EXPORT_PATH"
[[ -s "$EXPORT_PATH" ]] || fail "kikimimi export did not write a non-empty file at $EXPORT_PATH"

DUCKDB_COUNT=$(duckdb -noheader -csv -c "SELECT count(*) FROM read_parquet('${EXPORT_PATH}');")
PSQL_TOTAL=$(psql_test -tAc "SELECT count(*) FROM events;")
[[ "$DUCKDB_COUNT" -eq "$PSQL_TOTAL" ]] || fail "export row count ($DUCKDB_COUNT) != psql row count ($PSQL_TOTAL)"
echo "==> export row count ($DUCKDB_COUNT) matches psql ($PSQL_TOTAL)"

# --- 7. RLS negative check ---------------------------------------------------

echo "==> [7/8] RLS negative check: second org (KIKIMIMI_DEV_EMAIL=other@local) sees 0 rows"

env BIND_ADDR="$CLOUD2_BIND" DATABASE_URL="$DATABASE_URL" KIKIMIMI_DEV_AUTOAPPROVE=1 KIKIMIMI_DEV_EMAIL="other@local" \
  "$CLOUD_BIN" >"$WORKDIR/cloud2.log" 2>&1 &
CLOUD2_PID=$!
wait_http_ok "$CLOUD2_URL/healthz" || { cat "$WORKDIR/cloud2.log" >&2; exit 1; }

env KIKIMIMI_DIR="$KIKIMIMI_DIR_C" XDG_RUNTIME_DIR="$RT_C" "$BIN" login --endpoint "$CLOUD2_URL" --no-browser
TOKEN_C=$(cfg_field "$KIKIMIMI_DIR_C/config.json" token)
ORG_C=$(cfg_field "$KIKIMIMI_DIR_C/config.json" org_id)
EMAIL_C=$(cfg_field "$KIKIMIMI_DIR_C/config.json" email)
echo "==> host C logged in as $EMAIL_C: org_id=$ORG_C"

[[ "$EMAIL_C" == "other@local" ]] || fail "host C should have autoapproved as other@local, got $EMAIL_C"
[[ "$ORG_C" != "$ORG_A" ]] || fail "host C (other@local) must not land in host A/B's org"

RLS_RESP=$(curl -fsS --max-time 10 "$CLOUD2_URL/v1/query/today" -H "Authorization: Bearer $TOKEN_C")
echo "==> other org's /v1/query/today: $RLS_RESP"
RLS_ROW_COUNT=$(python3 -c "import json,sys; print(len(json.loads(sys.argv[1])['rows']))" "$RLS_RESP")
[[ "$RLS_ROW_COUNT" -eq 0 ]] || fail "other@local's org should see 0 rows via /v1/query/today, got $RLS_ROW_COUNT"
echo "==> RLS negative check passed: cross-tenant leak would have shown up here"

kill "$CLOUD2_PID"
wait "$CLOUD2_PID" 2>/dev/null || true
CLOUD2_PID=""

# --- 8. cleanup --------------------------------------------------------------

echo "==> [8/8] cleanup"
kill "$CLOUD_PID"
wait "$CLOUD_PID" 2>/dev/null || true
CLOUD_PID=""
psql_admin -c "DROP DATABASE IF EXISTS \"${TEST_DB}\" WITH (FORCE);"
TEST_DB=""
rm -rf "$WORKDIR"
trap - EXIT

echo "PASS"
