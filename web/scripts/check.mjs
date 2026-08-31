#!/usr/bin/env node
// Playwright-free smoke test: boots the mock server on a scratch port,
// walks the full contract (login/me/logout + every /web/q/* endpoint),
// and checks response shape. Run with `npm run check`.

import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const SERVER_PATH = path.join(__dirname, "..", "mock", "server.mjs");
const PORT = 8799;
const BASE = `http://localhost:${PORT}`;

let failures = 0;
function check(cond, label) {
  if (cond) {
    console.log(`  ok   ${label}`);
  } else {
    console.error(`  FAIL ${label}`);
    failures++;
  }
}

function extractCookie(res) {
  const raw = res.headers.get("set-cookie");
  if (!raw) return null;
  return raw.split(";")[0]; // "kikimimi_session=<token>"
}

async function waitForServer() {
  for (let i = 0; i < 50; i++) {
    try {
      await fetch(`${BASE}/web/me`);
      return;
    } catch {
      await new Promise((r) => setTimeout(r, 100));
    }
  }
  throw new Error(`mock server did not come up on ${BASE}`);
}

function checkQueryResult(body, expectedColumns, label) {
  check(Array.isArray(body?.columns), `${label}: columns is an array`);
  check(Array.isArray(body?.rows), `${label}: rows is an array`);
  check(
    JSON.stringify(body?.columns) === JSON.stringify(expectedColumns),
    `${label}: columns === ${JSON.stringify(expectedColumns)} (got ${JSON.stringify(body?.columns)})`,
  );
  check(body?.rows?.length > 0, `${label}: rows is non-empty`);
  if (Array.isArray(body?.rows)) {
    const widthsOk = body.rows.every((r) => Array.isArray(r) && r.length === expectedColumns.length);
    check(widthsOk, `${label}: every row has ${expectedColumns.length} cells`);
  }
}

async function main() {
  console.log(`starting mock server: node ${SERVER_PATH} (PORT=${PORT})`);
  const child = spawn(process.execPath, [SERVER_PATH], {
    env: { ...process.env, PORT: String(PORT) },
    stdio: "pipe",
  });
  child.stdout.on("data", () => {});
  child.stderr.on("data", (d) => process.stderr.write(d));

  try {
    await waitForServer();

    // Unauthenticated /web/me -> 401
    {
      const res = await fetch(`${BASE}/web/me`);
      check(res.status === 401, "GET /web/me without cookie -> 401");
    }

    // Bad invite code -> 403
    {
      const res = await fetch(`${BASE}/web/login`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email: "a@b.com", invite_code: "WRONG" }),
      });
      check(res.status === 403, "POST /web/login with bad invite_code -> 403");
    }

    // Good login -> 200 + Set-Cookie + {email, org_id}
    let cookie;
    {
      const res = await fetch(`${BASE}/web/login`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email: "dev@kikimimi.dev", invite_code: "KIKIMIMI-DEMO" }),
      });
      check(res.status === 200, "POST /web/login with valid invite_code -> 200");
      cookie = extractCookie(res);
      check(!!cookie, "POST /web/login sets kikimimi_session cookie");
      const body = await res.json();
      check(body.email === "dev@kikimimi.dev", "login response echoes email");
      check(typeof body.org_id === "string" && body.org_id.length > 0, "login response has org_id");
    }

    const authed = { headers: { Cookie: cookie } };

    // /web/me with cookie -> 200
    {
      const res = await fetch(`${BASE}/web/me`, authed);
      check(res.status === 200, "GET /web/me with cookie -> 200");
      const body = await res.json();
      check(body.email === "dev@kikimimi.dev", "GET /web/me returns correct email");
    }

    // /web/q/overview
    {
      const res = await fetch(`${BASE}/web/q/overview?days=14`, authed);
      check(res.status === 200, "GET /web/q/overview -> 200");
      const body = await res.json();
      checkQueryResult(
        body,
        ["dt", "events", "tool_calls", "failures", "input_tokens", "output_tokens", "cost_usd"],
        "overview",
      );
      check(body.rows.length === 14, "overview: 14 rows for days=14");
      const hasNullCell = body.rows.some((r) => r.some((c) => c === null));
      check(hasNullCell, "overview: fixture includes at least one null cell (unknown usage_source)");
    }

    // /web/q/machines
    {
      const res = await fetch(`${BASE}/web/q/machines`, authed);
      check(res.status === 200, "GET /web/q/machines -> 200");
      const body = await res.json();
      checkQueryResult(body, ["host_id", "env_kind", "os", "last_event_ts", "events_30d"], "machines");
      check(body.rows.length >= 2, "machines: at least 2 hosts");
    }

    // /web/q/tools
    {
      const res = await fetch(`${BASE}/web/q/tools?days=14`, authed);
      check(res.status === 200, "GET /web/q/tools -> 200");
      const body = await res.json();
      checkQueryResult(
        body,
        ["tool_name", "tool_kind", "calls", "failures", "p50_duration_ms", "p95_duration_ms"],
        "tools",
      );
      check(body.rows.length === 6, "tools: 6 tools in fixture");
      check(
        body.rows.some((r) => r[0].startsWith("mcp__github__")),
        "tools: includes an mcp__github__* tool",
      );
    }

    // /web/q/mcp
    {
      const res = await fetch(`${BASE}/web/q/mcp?days=14`, authed);
      check(res.status === 200, "GET /web/q/mcp -> 200");
      const body = await res.json();
      checkQueryResult(
        body,
        ["mcp_server", "calls", "failures", "distinct_sessions", "last_called_dt"],
        "mcp",
      );
      check(
        body.rows.some((r) => r[1] === 0),
        "mcp: at least one server has 0 calls (unused)",
      );
    }

    // /web/q/sessions
    {
      const res = await fetch(`${BASE}/web/q/sessions?days=14&limit=50`, authed);
      check(res.status === 200, "GET /web/q/sessions -> 200");
      const body = await res.json();
      checkQueryResult(
        body,
        [
          "session_id",
          "agent",
          "host_id",
          "started_at",
          "events",
          "tool_calls",
          "failures",
          "models",
          "input_tokens",
          "output_tokens",
          "cost_usd",
        ],
        "sessions",
      );
      check(body.rows.length <= 50, "sessions: respects limit=50");
      check(
        body.rows.some((r) => r[6] > 0),
        "sessions: fixture includes at least one session with failures > 0",
      );
    }

    // Logout, then /web/me should 401 again
    {
      const res = await fetch(`${BASE}/web/logout`, { method: "POST", ...authed });
      check(res.status === 200, "POST /web/logout -> 200");
      const res2 = await fetch(`${BASE}/web/me`, authed);
      check(res2.status === 401, "GET /web/me after logout -> 401");
    }
  } finally {
    child.kill();
  }

  console.log("");
  if (failures > 0) {
    console.error(`${failures} check(s) failed`);
    process.exit(1);
  } else {
    console.log("all checks passed");
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
