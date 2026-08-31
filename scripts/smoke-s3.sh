#!/usr/bin/env bash
# guru BYO S3 sink smoke test (architecture.md §4/§6, §12).
#
# guru never touches S3 credentials: uploads are shelled out to the `aws` CLI. This
# script proves that end to end without any real AWS account by putting a *fake* `aws`
# executable on PATH (a plain shell script that just copies the file it's told to,
# mirroring `s3://bucket/key` as a local directory tree) and pointing the s3 sink at it
# via the default uploader name ("aws") -- exactly what `guru sink add s3` wires up in
# production, just resolved from a fake PATH entry instead of a real install.
#
#   1. cargo build --release (workspace)
#   2. temp GURU_DIR/XDG_RUNTIME_DIR + a fake `aws` on PATH
#   3. `guru sink add s3 s3://fake-bucket/team`
#   4. start `guru agent --foreground`
#   5. feed 3 hook fixtures (SessionStart, PreToolUse Bash, PostToolUse Bash)
#   6. `guru flush`
#   7. assert the fake uploader received exactly one parquet file, and its row count
#      (checked with the duckdb CLI) matches the 3 fixtures fed
#   8. `guru status` shows the s3 sink healthy (pending=0, last_push_at set, no
#      last_error)
#   9. stop the daemon -> PASS
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"

if ! command -v duckdb >/dev/null 2>&1; then
  echo "smoke-s3.sh: the 'duckdb' CLI is required (to check the uploaded parquet's row count) but was not found in PATH" >&2
  exit 1
fi

echo "==> [1/9] building release binary"
cargo build --release -p guru-cli

BIN="$REPO_ROOT/target/release/guru"

WORKDIR="$(mktemp -d)"
export GURU_DIR="$WORKDIR/guru-home"
export XDG_RUNTIME_DIR="$WORKDIR/xdg-runtime"
export GURU_OTLP_PORT="14321"
mkdir -p "$GURU_DIR" "$XDG_RUNTIME_DIR"

FAKE_BIN_DIR="$WORKDIR/fake-bin"
FAKE_BUCKET_ROOT="$WORKDIR/fake-bucket-root"
FAKE_CALL_LOG="$WORKDIR/fake-aws-calls.log"
mkdir -p "$FAKE_BIN_DIR" "$FAKE_BUCKET_ROOT"
: >"$FAKE_CALL_LOG"

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
echo "==> GURU_DIR=$GURU_DIR XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR GURU_OTLP_PORT=$GURU_OTLP_PORT"

# --- [2/9] fake `aws` CLI on PATH -----------------------------------------
#
# Mirrors the real contract `guru-sink::S3Sink` invokes:
#   aws s3 cp <staging-file> <s3://bucket/prefix/...> [--profile P] [--endpoint-url E] --only-show-errors
# and nothing else -- no real AWS SDK, no network, no credentials anywhere.

echo "==> [2/9] installing fake 'aws' on PATH ($FAKE_BIN_DIR)"
cat >"$FAKE_BIN_DIR/aws" <<EOF
#!/usr/bin/env bash
set -euo pipefail
echo "\$@" >>"$FAKE_CALL_LOG"
if [[ "\${1:-}" != "s3" || "\${2:-}" != "cp" ]]; then
  echo "fake aws: unsupported invocation: \$*" >&2
  exit 1
fi
src="\$3"
dst="\$4"
rel="\${dst#s3://}"
out="$FAKE_BUCKET_ROOT/\$rel"
mkdir -p "\$(dirname "\$out")"
cp "\$src" "\$out"
EOF
chmod +x "$FAKE_BIN_DIR/aws"
export PATH="$FAKE_BIN_DIR:$PATH"
if [[ "$(command -v aws)" != "$FAKE_BIN_DIR/aws" ]]; then
  echo "smoke-s3.sh: fake aws did not shadow a real one on PATH (found $(command -v aws))" >&2
  exit 1
fi

# --- [3/9] guru sink add s3 -------------------------------------------------

echo "==> [3/9] guru sink add s3 s3://fake-bucket/team"
"$BIN" sink add s3 s3://fake-bucket/team
"$BIN" sink list

# --- [4/9] start the daemon -------------------------------------------------

echo "==> [4/9] starting guru agent --foreground in background"
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
  echo "smoke-s3.sh: daemon (pid $AGENT_PID) never became reachable on its control socket" >&2
  echo "----- agent.log -----" >&2
  cat "$WORKDIR/agent.log" >&2 || true
  exit 1
fi
echo "==> daemon is up (pid $AGENT_PID)"

# --- [5/9] feed 3 hook fixtures, one session_id -----------------------------

feed_hook() {
  local event="$1"
  local payload="$2"
  local out status
  set +e
  out=$(printf '%s' "$payload" | "$BIN" hook "$event")
  status=$?
  set -e
  if [[ "$status" -ne 0 ]]; then
    echo "smoke-s3.sh: guru hook $event exited $status (must always be 0)" >&2
    exit 1
  fi
  if [[ -n "$out" ]]; then
    echo "smoke-s3.sh: guru hook $event printed output on success: $out" >&2
    exit 1
  fi
  sleep 0.05 # keep ts strictly increasing across fixtures (ms resolution)
}

SESSION_START=$(cat <<'JSON'
{
  "session_id": "sess-smoke-s3-1",
  "transcript_path": "/tmp/guru-smoke-s3/sess-smoke-s3-1.jsonl",
  "cwd": "/tmp/guru-smoke-s3",
  "hook_event_name": "SessionStart",
  "source": "startup",
  "model": "claude-opus-4-6-20260805"
}
JSON
)

PRETOOLUSE_BASH=$(cat <<'JSON'
{
  "session_id": "sess-smoke-s3-1",
  "transcript_path": "/tmp/guru-smoke-s3/sess-smoke-s3-1.jsonl",
  "cwd": "/tmp/guru-smoke-s3",
  "hook_event_name": "PreToolUse",
  "tool_name": "Bash",
  "tool_input": { "command": "ls -la", "description": "List files" },
  "tool_use_id": "toolu_smoke_s3_bash_0"
}
JSON
)

POSTTOOLUSE_BASH=$(cat <<'JSON'
{
  "session_id": "sess-smoke-s3-1",
  "transcript_path": "/tmp/guru-smoke-s3/sess-smoke-s3-1.jsonl",
  "cwd": "/tmp/guru-smoke-s3",
  "hook_event_name": "PostToolUse",
  "tool_name": "Bash",
  "tool_input": { "command": "ls -la", "description": "List files" },
  "tool_response": { "stdout": "file1\nfile2\n", "stderr": "", "success": true },
  "tool_use_id": "toolu_smoke_s3_bash_0",
  "duration_ms": 87
}
JSON
)

echo "==> [5/9] feeding 3 hook fixtures (SessionStart, PreToolUse Bash, PostToolUse Bash)"
feed_hook "SessionStart" "$SESSION_START"
feed_hook "PreToolUse" "$PRETOOLUSE_BASH"
feed_hook "PostToolUse" "$POSTTOOLUSE_BASH"

# --- [6/9] flush -------------------------------------------------------------

echo "==> [6/9] sleeping briefly, then flushing"
sleep 0.5
FLUSH_OUT=$("$BIN" flush)
echo "$FLUSH_OUT"
if [[ "$FLUSH_OUT" != *"true"* ]]; then
  echo "smoke-s3.sh: guru flush was not acked by the daemon" >&2
  exit 1
fi
sleep 0.5

# --- [7/9] assert the fake uploader received exactly one parquet, row count matches --

echo "==> [7/9] checking the fake 'aws' received the upload"
if [[ ! -s "$FAKE_CALL_LOG" ]]; then
  echo "smoke-s3.sh: fake aws was never invoked ($FAKE_CALL_LOG is empty)" >&2
  echo "----- agent.log -----" >&2
  cat "$WORKDIR/agent.log" >&2 || true
  exit 1
fi
echo "----- fake aws call log -----"
cat "$FAKE_CALL_LOG"

if ! grep -q -- "--only-show-errors" "$FAKE_CALL_LOG"; then
  echo "smoke-s3.sh: fake aws was not called with --only-show-errors" >&2
  exit 1
fi

UPLOADED=$(find "$FAKE_BUCKET_ROOT/fake-bucket/team/guru.v1/events" -mindepth 2 -maxdepth 2 -type f -name '*.parquet' 2>/dev/null)
UPLOADED_COUNT=$(echo -n "$UPLOADED" | grep -c . || true)
echo "==> uploaded parquet file(s): $UPLOADED_COUNT"
if [[ "$UPLOADED_COUNT" -ne 1 ]]; then
  echo "smoke-s3.sh: expected exactly 1 uploaded parquet file under $FAKE_BUCKET_ROOT/fake-bucket/team/guru.v1/events/dt=*/, got $UPLOADED_COUNT" >&2
  find "$FAKE_BUCKET_ROOT" -type f >&2 || true
  exit 1
fi

if [[ -n "$(find "$GURU_DIR/s3-staging" -type f -name '*.parquet' 2>/dev/null)" ]]; then
  echo "smoke-s3.sh: a staging parquet file survived a successful upload (should have been deleted)" >&2
  exit 1
fi

ROW_COUNT=$(duckdb -noheader -csv -c "SELECT count(*) FROM read_parquet('${UPLOADED}');")
echo "==> uploaded parquet row count (duckdb): $ROW_COUNT"
if [[ "$ROW_COUNT" -ne 3 ]]; then
  echo "smoke-s3.sh: expected 3 rows (one per hook fixture fed) in the uploaded parquet, got $ROW_COUNT" >&2
  exit 1
fi

# --- [8/9] guru status shows the sink healthy --------------------------------

echo "==> [8/9] guru status"
STATUS_OUT=$("$BIN" status)
echo "$STATUS_OUT"

if ! grep -q "s3:" <<<"$STATUS_OUT"; then
  echo "smoke-s3.sh: 'guru status' did not print an s3 sink block" >&2
  exit 1
fi
S3_BLOCK=$(awk '
  /^  s3:/ { found=1 }
  found && /^$/ { exit }
  found { print }
' <<<"$STATUS_OUT")
echo "----- s3 block -----"
echo "$S3_BLOCK"
if ! grep -q "url: s3://fake-bucket/team" <<<"$S3_BLOCK"; then
  echo "smoke-s3.sh: s3 block missing expected url" >&2
  exit 1
fi
if ! grep -q "pending: 0" <<<"$S3_BLOCK"; then
  echo "smoke-s3.sh: s3 sink should show pending: 0 after a successful flush" >&2
  exit 1
fi
if grep -q "last_push_at: -" <<<"$S3_BLOCK"; then
  echo "smoke-s3.sh: s3 sink should have a real last_push_at after a successful upload" >&2
  exit 1
fi
if grep -q "last_error" <<<"$S3_BLOCK"; then
  echo "smoke-s3.sh: s3 sink should be healthy (no last_error) after a successful upload" >&2
  exit 1
fi
if grep -qi "s3 sink push failed" <<<"$STATUS_OUT"; then
  echo "smoke-s3.sh: unexpected s3-related warning in guru status" >&2
  exit 1
fi
echo "==> s3 sink reports healthy: pending=0, last_push_at set, no last_error"

# --- [9/9] shutdown ----------------------------------------------------------

echo "==> [9/9] stopping daemon (pid $AGENT_PID)"
kill "$AGENT_PID"
wait "$AGENT_PID" 2>/dev/null || true
AGENT_PID=""

echo "PASS"
