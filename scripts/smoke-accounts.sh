#!/usr/bin/env bash
# kikimimi account-model smoke test (architecture.md §6.1, "2026-09-01 確定").
#
# End-to-end, against a real kikimimi-cloud server + a real (throwaway)
# Postgres database + a real (mock) GitHub, exercising the full contract in
# one pass:
#
#   1. cargo build --release (workspace)
#   2. fresh test database; a tiny stdlib-python mock GitHub OAuth server
#      (/login/oauth/access_token, /user, /user/emails, two fixtures keyed
#      by the `code`); kikimimi-cloud started with GITHUB_CLIENT_ID/_SECRET
#      + GITHUB_API_BASE/GITHUB_OAUTH_BASE pointed at the mock (no
#      KIKIMIMI_DEV_AUTOAPPROVE -- this is the real OAuth code path)
#   3. account A signs in via GitHub OAuth (curl drives GET /auth/github ->
#      GET /auth/github/callback by hand, forwarding the state cookie --
#      same round trip a browser makes), creates team org "acme-smoke",
#      creates a member invite
#   4. account B signs in via GitHub OAuth, joins via the invite link;
#      B (member) is then 403'd listing acme-smoke's invites
#   5. A's device-flow login binds a device to acme-smoke by hand (org
#      dropdown flow via curl: POST /v1/device/code -> GET/POST /activate
#      with A's session cookie -> POST /v1/device/token)
#   6. `kikimimi login --org acme-smoke` (the real CLI binary, polling)
#      against the server -- A approves it via curl exactly like step 5;
#      `kikimimi orgs` / `kikimimi devices` (the new account-model `/v1/orgs`
#      + `/v1/devices` Bearer endpoints) against that device
#   7. `kikimimi repos allow` a glob, then `kikimimi agent --foreground` with
#      two fixture Codex rollout files (one matching repo, one not) ->
#      `kikimimi flush` -> matching event reaches Postgres, non-matching one
#      doesn't (file sink gets both -- unaffected by the org-scoped filter)
#   8. admin (A) switches active org to acme-smoke and hits
#      `GET /web/q/sessions` -> an `audit_log` row is written
#   9. legacy `POST /web/login` 404s now that GITHUB_CLIENT_ID is set
#   10. cleanup -> PASS
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"

for cmd in psql curl gzip duckdb python3; do
  if ! command -v "$cmd" >/dev/null 2>&1; then
    echo "smoke-accounts.sh: required command '$cmd' not found in PATH" >&2
    exit 1
  fi
done

# --- fixed config ------------------------------------------------------------

PG_HOST=127.0.0.1
PG_PORT=5433
PG_SUPERUSER=postgres
export PGPASSWORD="guru-dev" # shared local dev Postgres container's actual superuser password

CLOUD_BIND="127.0.0.1:8794"
CLOUD_URL="http://${CLOUD_BIND}"
GITHUB_MOCK_PORT=8795
GITHUB_MOCK_URL="http://127.0.0.1:${GITHUB_MOCK_PORT}"

TEST_DB="kikimimi_smoke_accounts_$(date +%s)_$$"

WORKDIR=/tmp/kikimimi-e2e-accounts
KIKIMIMI_DIR_D="$WORKDIR/host-d"
RT_D="$WORKDIR/host-d-rt"
CODEX_HOME_D="$WORKDIR/host-d-codex"

OTLP_PORT_D=14330

TEAM_SLUG="acme-smoke"
OWNER_EMAIL="owner-smoke@example.com"
JOINER_EMAIL="joiner-smoke@example.com"
OWNER_CODE="code-owner"
JOINER_CODE="code-joiner"

BIN="$REPO_ROOT/target/release/kikimimi"
CLOUD_BIN="$REPO_ROOT/target/release/kikimimi-cloud"

rm -rf "$WORKDIR"
mkdir -p "$KIKIMIMI_DIR_D" "$RT_D" "$CODEX_HOME_D/sessions"

# --- generic helpers ---------------------------------------------------------

psql_test() {
  psql -h "$PG_HOST" -p "$PG_PORT" -U "$PG_SUPERUSER" -d "$TEST_DB" -v ON_ERROR_STOP=1 "$@"
}

psql_admin() {
  psql -h "$PG_HOST" -p "$PG_PORT" -U "$PG_SUPERUSER" -d postgres -v ON_ERROR_STOP=1 "$@"
}

cfg_field() {
  python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['cloud'][sys.argv[2]])" "$1" "$2"
}

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

wait_http_ok() {
  local url="$1" tries="${2:-100}"
  local i=0
  until curl -fsS -o /dev/null --max-time 2 "$url" 2>/dev/null; do
    i=$((i + 1))
    if [[ "$i" -ge "$tries" ]]; then
      echo "smoke-accounts.sh: $url never became reachable" >&2
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
      echo "smoke-accounts.sh: agent (pid $pid, KIKIMIMI_DIR=$dir) died before becoming ready" >&2
      return 1
    fi
    i=$((i + 1))
    if [[ "$i" -ge 100 ]]; then
      echo "smoke-accounts.sh: agent (pid $pid, KIKIMIMI_DIR=$dir) never became reachable on its control socket" >&2
      return 1
    fi
    sleep 0.1
  done
}

# Polls until `events` has at least $2 rows for host_id=$1 (cloud-side flush
# is asynchronous: `kikimimi flush` only guarantees the daemon *received*
# the request, not that it finished POSTing to kikimimi-cloud yet).
wait_for_cloud_count() {
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
      echo "smoke-accounts.sh: host_id=$host_id never reached $expected cloud rows (stuck at $count)" >&2
      echo "$count"
      return 1
    fi
    sleep 0.1
  done
}

local_parquet_count() {
  duckdb -noheader -csv -c \
    "SELECT count(*) FROM read_parquet('${1}/data/events/dt=*/*.parquet') WHERE host_id = '${2}';" \
    2>/dev/null || echo 0
}

wait_for_local_count() {
  local dir="$1" host_id="$2" expected="$3" tries="${4:-100}"
  local i=0 count
  while true; do
    count=$(local_parquet_count "$dir" "$host_id")
    [[ "$count" =~ ^[0-9]+$ ]] || count=0
    if [[ "$count" -ge "$expected" ]]; then
      echo "$count"
      return 0
    fi
    i=$((i + 1))
    if [[ "$i" -ge "$tries" ]]; then
      echo "smoke-accounts.sh: host_id=$host_id never reached $expected local-parquet rows (stuck at $count)" >&2
      echo "$count"
      return 1
    fi
    sleep 0.1
  done
}

# --- tiny HTTP client: captures status/headers/body so we can drive
# redirects + cookies by hand, the same way a browser round trip works
# (github.rs's own test helper, MockGithub::oauth_login, does the identical
# thing with reqwest -- this is the curl/bash translation the task asks
# for). $STATUS/$HDR/$BODY are set by every call; read them out before the
# next call overwrites them. ---------------------------------------------

HDR="$WORKDIR/resp.hdr"
BODY="$WORKDIR/resp.body"

http() { # METHOD URL [curl-args...]
  local method="$1" url="$2"
  shift 2
  STATUS=$(curl -sS -o "$BODY" -D "$HDR" -w '%{http_code}' -X "$method" "$url" "$@")
}

hdr_value() { # name
  grep -i "^$1:" "$HDR" | tail -1 | sed -E 's/^[^:]*:[[:space:]]*//' | tr -d '\r'
}

# Prints "name=value" for the first Set-Cookie in $HDR whose name matches $1.
set_cookie() {
  local want="$1"
  grep -i '^set-cookie:' "$HDR" | sed -E 's/^[Ss]et-[Cc]ookie:[[:space:]]*//' | tr -d '\r' |
    while IFS= read -r line; do
      local pair="${line%%;*}"
      local name="${pair%%=*}"
      if [[ "$name" == "$want" ]]; then
        printf '%s' "$pair"
        return
      fi
    done
}

body_json_field() { # field
  python3 -c "import json,sys; print(json.load(open(sys.argv[1]))[sys.argv[2]])" "$BODY" "$1"
}

# Full GitHub-OAuth round trip (GET /auth/github -> GET
# /auth/github/callback), exactly what a real browser does after a user
# clicks "authorize" on GitHub -- except there's no real GitHub to click
# through, so this jumps straight to the callback with the `state` +
# `kikimimi_oauth_state` cookie the first response handed back, plus a fixed
# `code` that identifies which mock-GitHub fixture to log in as. Prints
# "kikimimi_session=<value>".
github_login() { # code
  local code="$1"
  http GET "$CLOUD_URL/auth/github"
  [[ "$STATUS" == "302" ]] || fail "GET /auth/github expected 302, got $STATUS: $(cat "$BODY")"
  local location state state_cookie
  location=$(hdr_value "Location")
  state=$(printf '%s' "$location" | grep -oE 'state=[^&]+' | head -1 | cut -d= -f2)
  [[ -n "$state" ]] || fail "no state= param in GET /auth/github's Location: $location"
  state_cookie=$(set_cookie "kikimimi_oauth_state")
  [[ -n "$state_cookie" ]] || fail "no Set-Cookie: kikimimi_oauth_state from GET /auth/github"

  http GET "$CLOUD_URL/auth/github/callback?code=${code}&state=${state}" -H "Cookie: ${state_cookie}"
  [[ "$STATUS" == "302" ]] || fail "GET /auth/github/callback expected 302, got $STATUS: $(cat "$BODY")"
  local session_cookie
  session_cookie=$(set_cookie "kikimimi_session")
  [[ -n "$session_cookie" ]] || fail "no Set-Cookie: kikimimi_session from the oauth callback"
  printf '%s' "$session_cookie"
}

# Approves a device's user_code as the account owning `cookie`, for org
# `slug` (the org-dropdown flow: GET /activate lists the account's
# memberships, POST /activate submits the chosen one).
approve_device() { # cookie user_code slug
  local cookie="$1" user_code="$2" slug="$3"
  http GET "$CLOUD_URL/activate?code=${user_code}" -H "Cookie: ${cookie}"
  [[ "$STATUS" == "200" ]] || fail "GET /activate?code=$user_code expected 200, got $STATUS: $(cat "$BODY")"
  grep -q "value=\"${slug}\"" "$BODY" || fail "GET /activate's org dropdown is missing a $slug option: $(cat "$BODY")"

  http POST "$CLOUD_URL/activate" -H "Cookie: ${cookie}" \
    --data-urlencode "code=${user_code}" --data-urlencode "org_slug=${slug}"
  [[ "$STATUS" == "200" ]] || fail "POST /activate expected 200, got $STATUS: $(cat "$BODY")"
}

AGENT_D_PID=""
CLOUD_PID=""
GITHUB_MOCK_PID=""
LOGIN_D_PID=""

cleanup() {
  set +e
  [[ -n "$LOGIN_D_PID" ]] && kill "$LOGIN_D_PID" 2>/dev/null
  [[ -n "$LOGIN_D_PID" ]] && wait "$LOGIN_D_PID" 2>/dev/null
  for pid in "$AGENT_D_PID" "$CLOUD_PID" "$GITHUB_MOCK_PID"; do
    [[ -n "$pid" ]] && kill "$pid" 2>/dev/null
  done
  for pid in "$AGENT_D_PID" "$CLOUD_PID" "$GITHUB_MOCK_PID"; do
    [[ -n "$pid" ]] && wait "$pid" 2>/dev/null
  done
  if [[ -n "${TEST_DB:-}" ]]; then
    psql_admin -c "DROP DATABASE IF EXISTS \"${TEST_DB}\" WITH (FORCE);" >/dev/null 2>&1
  fi
  rm -rf "$WORKDIR"
}
trap cleanup EXIT

# --- 1. build -----------------------------------------------------------------

echo "==> [1/10] cargo build --release (workspace)"
cargo build --release

# --- 2. mock GitHub + fresh test db + kikimimi-cloud (OAuth-configured) -------

echo "==> [2/10] mock GitHub on $GITHUB_MOCK_URL, test database $TEST_DB, kikimimi-cloud on $CLOUD_BIND (GitHub OAuth configured, no autoapprove)"

cat >"$WORKDIR/mock_github.py" <<'PY'
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs

PORT = int(sys.argv[1])

# code -> {id, login, email, verified}: keyed by the OAuth `code` (a real
# GitHub authorize round trip would tie `code` to whichever human clicked
# "authorize"; here the smoke test picks the human by picking the code).
FIXTURES = {
    "code-owner": {"id": 910001, "login": "acme-owner-gh", "email": "owner-smoke@example.com", "verified": True},
    "code-joiner": {"id": 910002, "login": "acme-joiner-gh", "email": "joiner-smoke@example.com", "verified": True},
}


class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        pass

    def _send_json(self, status, obj):
        body = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _fixture_from_auth(self):
        auth = self.headers.get("Authorization", "")
        prefix = "Bearer token-for-"
        if not auth.startswith(prefix):
            return None
        return FIXTURES.get(auth[len(prefix):])

    def do_GET(self):
        if self.path == "/mock-health":
            self._send_json(200, {"ok": True})
        elif self.path == "/user":
            fx = self._fixture_from_auth()
            if fx is None:
                self._send_json(401, {"error": "bad token"})
                return
            self._send_json(200, {"id": fx["id"], "login": fx["login"]})
        elif self.path == "/user/emails":
            fx = self._fixture_from_auth()
            if fx is None:
                self._send_json(401, {"error": "bad token"})
                return
            self._send_json(200, [{"email": fx["email"], "primary": True, "verified": fx["verified"]}])
        else:
            self._send_json(404, {"error": "not found"})

    def do_POST(self):
        if self.path == "/login/oauth/access_token":
            length = int(self.headers.get("Content-Length", "0"))
            raw = self.rfile.read(length).decode()
            code = parse_qs(raw).get("code", [""])[0]
            if code not in FIXTURES:
                self._send_json(400, {"error": "bad code"})
                return
            self._send_json(200, {"access_token": f"token-for-{code}"})
        else:
            self._send_json(404, {"error": "not found"})


if __name__ == "__main__":
    ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
PY

python3 "$WORKDIR/mock_github.py" "$GITHUB_MOCK_PORT" >"$WORKDIR/mock-github.log" 2>&1 &
GITHUB_MOCK_PID=$!
wait_http_ok "$GITHUB_MOCK_URL/mock-health" || { cat "$WORKDIR/mock-github.log" >&2; exit 1; }
echo "==> mock GitHub is up (pid $GITHUB_MOCK_PID)"

psql_admin -c "CREATE DATABASE \"${TEST_DB}\";"
DATABASE_URL="postgres://${PG_SUPERUSER}:${PGPASSWORD}@${PG_HOST}:${PG_PORT}/${TEST_DB}"

env BIND_ADDR="$CLOUD_BIND" DATABASE_URL="$DATABASE_URL" \
  GITHUB_CLIENT_ID="smoke-client-id" GITHUB_CLIENT_SECRET="smoke-client-secret" \
  GITHUB_OAUTH_BASE="$GITHUB_MOCK_URL" GITHUB_API_BASE="$GITHUB_MOCK_URL" \
  "$CLOUD_BIN" >"$WORKDIR/cloud.log" 2>&1 &
CLOUD_PID=$!
wait_http_ok "$CLOUD_URL/healthz" || { cat "$WORKDIR/cloud.log" >&2; exit 1; }
echo "==> kikimimi-cloud is up (pid $CLOUD_PID, db $TEST_DB)"

http GET "$CLOUD_URL/web/config"
[[ "$STATUS" == "200" ]] || fail "GET /web/config expected 200, got $STATUS"
[[ "$(body_json_field github_oauth)" == "True" ]] || fail "GET /web/config: github_oauth should be true once GITHUB_CLIENT_ID is set"
[[ "$(body_json_field legacy_login)" == "False" ]] || fail "GET /web/config: legacy_login should be false without KIKIMIMI_LEGACY_INVITE"

# --- 3. account A: GitHub OAuth sign-in, create team org, create invite ------

echo "==> [3/10] account A signs in via GitHub OAuth (curl), creates team org '$TEAM_SLUG', creates a member invite"

OWNER_COOKIE=$(github_login "$OWNER_CODE")
echo "==> account A signed in: $OWNER_COOKIE"

http POST "$CLOUD_URL/web/orgs" -H "Cookie: $OWNER_COOKIE" -H "Content-Type: application/json" \
  -d "$(printf '{"name":"Acme Smoke Co","slug":"%s"}' "$TEAM_SLUG")"
[[ "$STATUS" == "200" ]] || fail "POST /web/orgs expected 200, got $STATUS: $(cat "$BODY")"
[[ "$(body_json_field role)" == "owner" ]] || fail "org creator should be owner, got $(cat "$BODY")"

http POST "$CLOUD_URL/web/orgs/$TEAM_SLUG/invites" -H "Cookie: $OWNER_COOKIE" -H "Content-Type: application/json" \
  -d '{"role":"member"}'
[[ "$STATUS" == "200" ]] || fail "POST /web/orgs/$TEAM_SLUG/invites expected 200, got $STATUS: $(cat "$BODY")"
JOIN_URL=$(body_json_field url)
[[ "$JOIN_URL" == /join/* ]] || fail "invite url doesn't look like /join/<token>: $JOIN_URL"
echo "==> team org created, invite minted: $JOIN_URL"

# --- 4. account B: GitHub OAuth sign-in, join via invite, 403 on invites -----

echo "==> [4/10] account B signs in via GitHub OAuth, joins '$TEAM_SLUG' via the invite; member B is 403'd listing invites"

JOINER_COOKIE=$(github_login "$JOINER_CODE")
echo "==> account B signed in: $JOINER_COOKIE"

http GET "$CLOUD_URL$JOIN_URL" -H "Cookie: $JOINER_COOKIE"
[[ "$STATUS" == "200" ]] || fail "GET $JOIN_URL expected 200, got $STATUS"
grep -qi "<!doctype html" "$BODY" || fail "GET $JOIN_URL should serve the SPA shell, got: $(cat "$BODY")"

http POST "$CLOUD_URL$JOIN_URL" -H "Cookie: $JOINER_COOKIE"
[[ "$STATUS" == "200" ]] || fail "POST $JOIN_URL expected 200, got $STATUS: $(cat "$BODY")"
[[ "$(body_json_field org_slug)" == "$TEAM_SLUG" ]] || fail "join response org_slug mismatch: $(cat "$BODY")"
[[ "$(body_json_field role)" == "member" ]] || fail "join response role should be member: $(cat "$BODY")"
echo "==> account B joined $TEAM_SLUG as member"

http GET "$CLOUD_URL/web/orgs/$TEAM_SLUG/invites" -H "Cookie: $JOINER_COOKIE"
[[ "$STATUS" == "403" ]] || fail "member listing invites should be 403, got $STATUS: $(cat "$BODY")"
echo "==> member B correctly forbidden (403) from listing $TEAM_SLUG's invites"

# --- 5. A's device-flow login binds a device to acme-smoke (curl) -----------

echo "==> [5/10] A's device-flow login binds a device to $TEAM_SLUG by hand (org dropdown flow via curl)"

CODE_RESP=$(curl -fsS -X POST "$CLOUD_URL/v1/device/code" -H "Content-Type: application/json" \
  -d "$(printf '{"host_id":"host-curl-acme","hostname":"curl-host","org_hint":"%s"}' "$TEAM_SLUG")")
DEVICE_CODE=$(printf '%s' "$CODE_RESP" | python3 -c "import json,sys; print(json.load(sys.stdin)['device_code'])")
USER_CODE=$(printf '%s' "$CODE_RESP" | python3 -c "import json,sys; print(json.load(sys.stdin)['user_code'])")

approve_device "$OWNER_COOKIE" "$USER_CODE" "$TEAM_SLUG"

TOKEN_RESP=$(curl -fsS -X POST "$CLOUD_URL/v1/device/token" -H "Content-Type: application/json" \
  -d "$(printf '{"device_code":"%s"}' "$DEVICE_CODE")")
printf '%s' "$TOKEN_RESP" | python3 -c "
import json, sys
d = json.load(sys.stdin)
assert d['status'] == 'ok', d
assert d['org_slug'] == '$TEAM_SLUG', d
assert d['org_kind'] == 'team', d
"
echo "==> curl-driven device bound to $TEAM_SLUG: $(printf '%s' "$TOKEN_RESP" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d['org_slug'], d['org_kind'])")"

# --- 6. real CLI: `kikimimi login --org acme-smoke`, `kikimimi orgs`/`devices` -

echo "==> [6/10] kikimimi login --org $TEAM_SLUG (real CLI, polling) against the server; kikimimi orgs / kikimimi devices"

env KIKIMIMI_DIR="$KIKIMIMI_DIR_D" XDG_RUNTIME_DIR="$RT_D" "$BIN" \
  login --endpoint "$CLOUD_URL" --org "$TEAM_SLUG" --no-browser \
  >"$WORKDIR/login-d.log" 2>&1 &
LOGIN_D_PID=$!

USER_CODE_D=""
for _ in $(seq 1 200); do
  if grep -q "enter code:" "$WORKDIR/login-d.log" 2>/dev/null; then
    USER_CODE_D=$(grep "enter code:" "$WORKDIR/login-d.log" | sed -E 's/^and enter code: *//')
    break
  fi
  if ! kill -0 "$LOGIN_D_PID" 2>/dev/null; then
    break
  fi
  sleep 0.1
done
[[ -n "$USER_CODE_D" ]] || { cat "$WORKDIR/login-d.log" >&2; fail "kikimimi login --org $TEAM_SLUG never printed a user code"; }

approve_device "$OWNER_COOKIE" "$USER_CODE_D" "$TEAM_SLUG"

wait "$LOGIN_D_PID"
LOGIN_D_STATUS=$?
LOGIN_D_PID=""
[[ "$LOGIN_D_STATUS" -eq 0 ]] || { cat "$WORKDIR/login-d.log" >&2; fail "kikimimi login --org $TEAM_SLUG exited $LOGIN_D_STATUS"; }
grep -q "org $TEAM_SLUG \[team\]" "$WORKDIR/login-d.log" || fail "login output missing 'org $TEAM_SLUG [team]': $(cat "$WORKDIR/login-d.log")"
HOST_D_ID=$(cat "$KIKIMIMI_DIR_D/host_id")
echo "==> kikimimi login --org $TEAM_SLUG succeeded: host_id=$HOST_D_ID"
echo "$(cat "$WORKDIR/login-d.log")"

ORGS_OUT=$(env KIKIMIMI_DIR="$KIKIMIMI_DIR_D" XDG_RUNTIME_DIR="$RT_D" "$BIN" orgs)
echo "$ORGS_OUT"
grep -q "$TEAM_SLUG" <<<"$ORGS_OUT" || fail "kikimimi orgs missing $TEAM_SLUG: $ORGS_OUT"
grep -q "role=owner" <<<"$ORGS_OUT" || fail "kikimimi orgs: account A should be owner somewhere: $ORGS_OUT"

DEVICES_OUT=$(env KIKIMIMI_DIR="$KIKIMIMI_DIR_D" XDG_RUNTIME_DIR="$RT_D" "$BIN" devices list)
echo "$DEVICES_OUT"
grep -q "(this device)" <<<"$DEVICES_OUT" || fail "kikimimi devices should flag the calling device: $DEVICES_OUT"

# --- 7. repo filter: team org, matching repo reaches cloud, other doesn't ----

echo "==> [7/10] kikimimi repos allow + kikimimi agent --foreground: matching-repo Codex fixture reaches cloud, non-matching one doesn't (file sink gets both)"

env KIKIMIMI_DIR="$KIKIMIMI_DIR_D" XDG_RUNTIME_DIR="$RT_D" "$BIN" repos allow '*acme-org/widgets*'
REPOS_OUT=$(env KIKIMIMI_DIR="$KIKIMIMI_DIR_D" XDG_RUNTIME_DIR="$RT_D" "$BIN" repos list)
grep -q 'acme-org/widgets' <<<"$REPOS_OUT" || fail "kikimimi repos list missing the pattern just added: $REPOS_OUT"

cat >"$CODEX_HOME_D/sessions/rollout-matching.jsonl" <<'JSONL'
{"timestamp":"2026-09-01T00:00:00.000Z","ordinal":0,"type":"session_meta","payload":{"session_id":"sess-repo-match","cwd":"/tmp/acme-widgets","cli_version":"0.151.0","model_provider":"openai","timestamp":"2026-09-01T00:00:00.000Z","git":{"repository_url":"git@github.com:acme-org/widgets.git"}}}
JSONL
cat >"$CODEX_HOME_D/sessions/rollout-nonmatching.jsonl" <<'JSONL'
{"timestamp":"2026-09-01T00:00:01.000Z","ordinal":0,"type":"session_meta","payload":{"session_id":"sess-repo-nomatch","cwd":"/tmp/other-repo","cli_version":"0.151.0","model_provider":"openai","timestamp":"2026-09-01T00:00:01.000Z","git":{"repository_url":"git@github.com:other-org/other-repo.git"}}}
JSONL

env KIKIMIMI_DIR="$KIKIMIMI_DIR_D" XDG_RUNTIME_DIR="$RT_D" KIKIMIMI_OTLP_PORT="$OTLP_PORT_D" CODEX_HOME="$CODEX_HOME_D" \
  "$BIN" agent --foreground >"$WORKDIR/agent-d.log" 2>&1 &
AGENT_D_PID=$!
wait_daemon_ready "$KIKIMIMI_DIR_D" "$RT_D" "$AGENT_D_PID" || { cat "$WORKDIR/agent-d.log" >&2; exit 1; }
echo "==> host D agent is up (pid $AGENT_D_PID), warning about the (now-configured) repo filter should be absent:"
grep -i "repo filter" "$WORKDIR/agent-d.log" && fail "agent still warned about an unconfigured repo filter after 'kikimimi repos allow'" || true

env KIKIMIMI_DIR="$KIKIMIMI_DIR_D" XDG_RUNTIME_DIR="$RT_D" "$BIN" flush

wait_for_cloud_count "$HOST_D_ID" 1 >/dev/null || { cat "$WORKDIR/agent-d.log" >&2; exit 1; }
wait_for_local_count "$KIKIMIMI_DIR_D" "$HOST_D_ID" 2 >/dev/null || { cat "$WORKDIR/agent-d.log" >&2; exit 1; }

# The matching-repo event should have landed promptly (wait_for_cloud_count
# above); give the (deliberately absent) non-matching one a moment it
# doesn't need, then assert the cloud count never crept past 1.
sleep 1
CLOUD_COUNT_D=$(psql_test -tAc "SELECT count(*) FROM events WHERE host_id = '${HOST_D_ID}';")
[[ "$CLOUD_COUNT_D" -eq 1 ]] || fail "expected exactly 1 cloud event for host D (repo filter should hold the non-matching one back), got $CLOUD_COUNT_D"
CLOUD_REPO_D=$(psql_test -tAc "SELECT repo FROM events WHERE host_id = '${HOST_D_ID}';")
[[ "$CLOUD_REPO_D" == *"acme-org/widgets"* ]] || fail "the one cloud event for host D has an unexpected repo: $CLOUD_REPO_D"
echo "==> cloud: exactly 1 event for host D, repo=$CLOUD_REPO_D"

LOCAL_COUNT_D=$(local_parquet_count "$KIKIMIMI_DIR_D" "$HOST_D_ID")
[[ "$LOCAL_COUNT_D" -eq 2 ]] || fail "expected 2 local-parquet events for host D (file sink is never filtered), got $LOCAL_COUNT_D"
echo "==> local parquet: $LOCAL_COUNT_D events for host D (both repos, file sink unaffected)"

kill "$AGENT_D_PID"
wait "$AGENT_D_PID" 2>/dev/null || true
AGENT_D_PID=""

# --- 8. admin drilldown writes an audit_log row ------------------------------

echo "==> [8/10] admin (A) switches active org to $TEAM_SLUG, GET /web/q/sessions -> audit_log row"

OWNER_ACCOUNT_ID=$(psql_test -tAc "SELECT id FROM accounts WHERE email = '${OWNER_EMAIL}';")
ACME_ORG_ID=$(psql_test -tAc "SELECT id FROM orgs WHERE slug = '${TEAM_SLUG}';")
AUDIT_BEFORE=$(psql_test -tAc "SELECT count(*) FROM audit_log WHERE actor = '${OWNER_ACCOUNT_ID}' AND org_id = '${ACME_ORG_ID}' AND action = 'sessions_drilldown';")

http POST "$CLOUD_URL/web/active-org" -H "Cookie: $OWNER_COOKIE" -H "Content-Type: application/json" \
  -d "$(printf '{"slug":"%s"}' "$TEAM_SLUG")"
[[ "$STATUS" == "200" ]] || fail "POST /web/active-org expected 200, got $STATUS: $(cat "$BODY")"

http GET "$CLOUD_URL/web/q/sessions" -H "Cookie: $OWNER_COOKIE"
[[ "$STATUS" == "200" ]] || fail "GET /web/q/sessions (admin) expected 200, got $STATUS: $(cat "$BODY")"

AUDIT_AFTER=$(psql_test -tAc "SELECT count(*) FROM audit_log WHERE actor = '${OWNER_ACCOUNT_ID}' AND org_id = '${ACME_ORG_ID}' AND action = 'sessions_drilldown';")
[[ "$AUDIT_AFTER" -eq $((AUDIT_BEFORE + 1)) ]] || fail "admin drilldown on /web/q/sessions should write exactly one audit_log row (before=$AUDIT_BEFORE after=$AUDIT_AFTER)"
echo "==> admin drilldown wrote an audit_log row ($AUDIT_BEFORE -> $AUDIT_AFTER)"

# --- 9. legacy login 404s while GITHUB_CLIENT_ID is set ----------------------

echo "==> [9/10] legacy POST /web/login 404s now that GITHUB_CLIENT_ID is set"

http POST "$CLOUD_URL/web/login" -H "Content-Type: application/json" \
  -d '{"email":"someone-legacy@example.com","invite_code":"whatever"}'
[[ "$STATUS" == "404" ]] || fail "legacy POST /web/login should 404 once GITHUB_CLIENT_ID is set, got $STATUS: $(cat "$BODY")"
echo "==> legacy login correctly 404s"

# --- 10. cleanup --------------------------------------------------------------

echo "==> [10/10] cleanup"
kill "$CLOUD_PID"
wait "$CLOUD_PID" 2>/dev/null || true
CLOUD_PID=""
kill "$GITHUB_MOCK_PID"
wait "$GITHUB_MOCK_PID" 2>/dev/null || true
GITHUB_MOCK_PID=""
psql_admin -c "DROP DATABASE IF EXISTS \"${TEST_DB}\" WITH (FORCE);"
TEST_DB=""
rm -rf "$WORKDIR"
trap - EXIT

echo "PASS"
