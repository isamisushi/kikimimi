#!/usr/bin/env node
// Mock dev server for the guru web API contract (see docs / task description).
// Plain Node core only, no dependencies. The real Rust server implements the
// same contract; this exists so the SPA can be built and demoed standalone.
//
// Usage: node mock/server.mjs   (or `npm run mock` from web/)

import http from "node:http";
import crypto from "node:crypto";
import { URL } from "node:url";

// Not 8787: that's the real guru-cloud server's port (fly.toml
// internal_port). Keep the mock on a distinct port so it never collides
// with a real instance running locally.
const PORT = process.env.PORT ? Number(process.env.PORT) : 8788;
const COOKIE_NAME = "guru_session";
const DEMO_ORG = "org_demo";

// Any of these invite codes "work" against the demo org; anything else -> 403.
const VALID_INVITES = new Set(["GURU-DEMO", "GURU-2026"]);

// token -> { email, org_id }
const sessions = new Map();

// ---------------------------------------------------------------------------
// Fixture data
// ---------------------------------------------------------------------------

const HOSTS = [
  { host_id: "mbp-yuya", env_kind: "laptop", os: "macOS 14.5" },
  { host_id: "ci-runner-03", env_kind: "ci", os: "Ubuntu 22.04" },
];

// A third host that has registered but never sent a heartbeat/event yet -
// exercises the "unknown freshness" / null-events_30d rendering path.
const GHOST_HOST = { host_id: "codespace-tmp-8f2", env_kind: "devcontainer", os: "Ubuntu 24.04" };

const TOOLS = [
  { tool_name: "Bash", tool_kind: "shell", calls: 812, failures: 37, p50: 420, p95: 3800 },
  { tool_name: "Read", tool_kind: "fs", calls: 1190, failures: 2, p50: 18, p95: 60 },
  { tool_name: "Edit", tool_kind: "fs", calls: 640, failures: 9, p50: 22, p95: 95 },
  { tool_name: "mcp__github__create_issue", tool_kind: "mcp", calls: 54, failures: 1, p50: 610, p95: 2100 },
  { tool_name: "mcp__github__search_code", tool_kind: "mcp", calls: 211, failures: 4, p50: 340, p95: 1500 },
  // usage_source unknown for this one -> null durations, never rendered as 0.
  { tool_name: "mcp__playwright__navigate", tool_kind: "browser", calls: 96, failures: 12, p50: null, p95: null },
];

const MCP_SERVERS = [
  { mcp_server: "github", calls: 265, failures: 5, distinct_sessions: 41, lastCalledDaysAgo: 0 },
  { mcp_server: "sentry", calls: 18, failures: 0, distinct_sessions: 6, lastCalledDaysAgo: 3 },
  // The product's core message: a server nobody has called in the window.
  { mcp_server: "linear", calls: 0, failures: 0, distinct_sessions: 0, lastCalledDaysAgo: null },
];

const AGENTS = ["claude-code", "codex-cli"];
const MODEL_BY_AGENT = {
  "claude-code": "claude-sonnet-4.5",
  "codex-cli": "gpt-5-codex",
};

// ---------------------------------------------------------------------------
// Generators
// ---------------------------------------------------------------------------

function dateStr(d) {
  return d.toISOString().slice(0, 10);
}

function costFor(inputTokens, outputTokens) {
  if (inputTokens === null || outputTokens === null) return null;
  return Number((inputTokens * 0.000003 + outputTokens * 0.000015).toFixed(4));
}

function generateOverview(days) {
  const rows = [];
  const now = new Date();
  for (let i = days - 1; i >= 0; i--) {
    const d = new Date(now);
    d.setUTCDate(d.getUTCDate() - i);
    const dow = d.getUTCDay();
    const isWeekend = dow === 0 || dow === 6;
    const base = isWeekend ? 40 : 140;
    const events = base + ((i * 13) % 37);
    const toolCalls = Math.round(events * 2.6);
    const failures = Math.round(toolCalls * (isWeekend ? 0.01 : 0.035)) + (i % 5 === 0 ? 1 : 0);
    // Two days per window simulate an agent whose usage_source is unknown
    // (e.g. a vendor without token reporting) -> nulls, not zeros.
    const unknownUsage = i === 5 || i === 11;
    const inputTokens = unknownUsage ? null : Math.round(toolCalls * 480 + ((i * 733) % 5000));
    const outputTokens = unknownUsage ? null : Math.round(toolCalls * 190 + ((i * 311) % 2000));
    const cost = costFor(inputTokens, outputTokens);
    rows.push([dateStr(d), events, toolCalls, failures, inputTokens, outputTokens, cost]);
  }
  return rows;
}

function generateMachines() {
  const now = Date.now();
  return [
    [HOSTS[0].host_id, HOSTS[0].env_kind, HOSTS[0].os, new Date(now - 4 * 60_000).toISOString(), 4820],
    [HOSTS[1].host_id, HOSTS[1].env_kind, HOSTS[1].os, new Date(now - 260 * 60_000).toISOString(), 1190],
    [GHOST_HOST.host_id, GHOST_HOST.env_kind, GHOST_HOST.os, null, null],
  ];
}

function generateTools(days) {
  const scale = days / 14;
  return TOOLS.map((t) => [
    t.tool_name,
    t.tool_kind,
    Math.max(0, Math.round(t.calls * scale)),
    Math.max(0, Math.round(t.failures * scale)),
    t.p50,
    t.p95,
  ]);
}

function generateMcp(days) {
  const scale = days / 14;
  const now = Date.now();
  return MCP_SERVERS.map((s) => [
    s.mcp_server,
    Math.max(0, Math.round(s.calls * scale)),
    Math.max(0, Math.round(s.failures * scale)),
    s.distinct_sessions,
    s.lastCalledDaysAgo === null ? null : dateStr(new Date(now - s.lastCalledDaysAgo * 86_400_000)),
  ]);
}

function generateSessions(days, limit) {
  const now = Date.now();
  const rows = [];
  const windowMs = days * 86_400_000;
  for (let i = 0; i < limit; i++) {
    const startedAt = new Date(now - (i * 5.3 + (i % 3)) * 3_600_000);
    if (now - startedAt.getTime() > windowMs) break;

    const host = HOSTS[i % HOSTS.length];
    const agent = AGENTS[i % AGENTS.length];
    const events = 20 + ((i * 17) % 180);
    const toolCalls = Math.round(events * 0.55);
    const failures = i % 7 === 0 ? 1 + (i % 3) : 0;
    const unknownUsage = agent === "codex-cli" && i % 4 === 0;
    const inputTokens = unknownUsage ? null : 1200 + ((i * 977) % 40000);
    const outputTokens = unknownUsage ? null : 300 + ((i * 401) % 12000);
    const cost = costFor(inputTokens, outputTokens);

    rows.push([
      `sess_${String(i).padStart(4, "0")}_${host.host_id}`,
      agent,
      host.host_id,
      startedAt.toISOString(),
      events,
      toolCalls,
      failures,
      MODEL_BY_AGENT[agent],
      inputTokens,
      outputTokens,
      cost,
    ]);
  }
  return rows;
}

// ---------------------------------------------------------------------------
// Tiny HTTP plumbing (no framework)
// ---------------------------------------------------------------------------

function parseCookies(header) {
  const out = {};
  if (!header) return out;
  for (const part of header.split(";")) {
    const idx = part.indexOf("=");
    if (idx === -1) continue;
    const k = part.slice(0, idx).trim();
    const v = part.slice(idx + 1).trim();
    out[k] = decodeURIComponent(v);
  }
  return out;
}

function sessionFromRequest(req) {
  const cookies = parseCookies(req.headers.cookie);
  const token = cookies[COOKIE_NAME];
  if (!token) return null;
  return sessions.get(token) ?? null;
}

function sendJson(res, status, body) {
  const data = JSON.stringify(body);
  res.writeHead(status, {
    "Content-Type": "application/json; charset=utf-8",
    "Content-Length": Buffer.byteLength(data),
  });
  res.end(data);
}

function sendQueryResult(res, columns, rows) {
  sendJson(res, 200, { columns, rows });
}

function readJsonBody(req) {
  return new Promise((resolve, reject) => {
    let data = "";
    req.on("data", (chunk) => {
      data += chunk;
      if (data.length > 1_000_000) {
        reject(new Error("body too large"));
        req.destroy();
      }
    });
    req.on("end", () => {
      if (!data) return resolve({});
      try {
        resolve(JSON.parse(data));
      } catch (err) {
        reject(err);
      }
    });
    req.on("error", reject);
  });
}

function requireSession(req, res) {
  const session = sessionFromRequest(req);
  if (!session) {
    sendJson(res, 401, { error: "unauthorized" });
    return null;
  }
  return session;
}

const server = http.createServer(async (req, res) => {
  const url = new URL(req.url, `http://${req.headers.host ?? "localhost"}`);
  const { pathname, searchParams } = url;

  try {
    // --- Auth ---
    if (pathname === "/web/login" && req.method === "POST") {
      let body;
      try {
        body = await readJsonBody(req);
      } catch {
        sendJson(res, 400, { error: "invalid json" });
        return;
      }
      const email = typeof body.email === "string" ? body.email.trim() : "";
      const inviteCode = typeof body.invite_code === "string" ? body.invite_code.trim() : "";

      if (!email || !VALID_INVITES.has(inviteCode)) {
        sendJson(res, 403, { error: "invalid email or invite code" });
        return;
      }

      const token = crypto.randomBytes(24).toString("hex");
      sessions.set(token, { email, org_id: DEMO_ORG });
      res.setHeader(
        "Set-Cookie",
        `${COOKIE_NAME}=${token}; HttpOnly; Path=/; SameSite=Lax; Max-Age=${60 * 60 * 24 * 7}`,
      );
      sendJson(res, 200, { email, org_id: DEMO_ORG });
      return;
    }

    if (pathname === "/web/logout" && req.method === "POST") {
      const cookies = parseCookies(req.headers.cookie);
      const token = cookies[COOKIE_NAME];
      if (token) sessions.delete(token);
      res.setHeader("Set-Cookie", `${COOKIE_NAME}=; HttpOnly; Path=/; SameSite=Lax; Max-Age=0`);
      sendJson(res, 200, { ok: true });
      return;
    }

    if (pathname === "/web/me" && req.method === "GET") {
      const session = sessionFromRequest(req);
      if (!session) {
        sendJson(res, 401, { error: "unauthorized" });
        return;
      }
      sendJson(res, 200, session);
      return;
    }

    // --- Data endpoints (all require a session) ---
    if (pathname === "/web/q/overview" && req.method === "GET") {
      if (!requireSession(req, res)) return;
      const days = Number(searchParams.get("days") ?? "14") || 14;
      sendQueryResult(
        res,
        ["dt", "events", "tool_calls", "failures", "input_tokens", "output_tokens", "cost_usd"],
        generateOverview(days),
      );
      return;
    }

    if (pathname === "/web/q/machines" && req.method === "GET") {
      if (!requireSession(req, res)) return;
      sendQueryResult(res, ["host_id", "env_kind", "os", "last_event_ts", "events_30d"], generateMachines());
      return;
    }

    if (pathname === "/web/q/tools" && req.method === "GET") {
      if (!requireSession(req, res)) return;
      const days = Number(searchParams.get("days") ?? "14") || 14;
      sendQueryResult(
        res,
        ["tool_name", "tool_kind", "calls", "failures", "p50_duration_ms", "p95_duration_ms"],
        generateTools(days),
      );
      return;
    }

    if (pathname === "/web/q/mcp" && req.method === "GET") {
      if (!requireSession(req, res)) return;
      const days = Number(searchParams.get("days") ?? "14") || 14;
      sendQueryResult(
        res,
        ["mcp_server", "calls", "failures", "distinct_sessions", "last_called_dt"],
        generateMcp(days),
      );
      return;
    }

    if (pathname === "/web/q/sessions" && req.method === "GET") {
      if (!requireSession(req, res)) return;
      const days = Number(searchParams.get("days") ?? "14") || 14;
      const limit = Number(searchParams.get("limit") ?? "50") || 50;
      sendQueryResult(
        res,
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
        generateSessions(days, limit),
      );
      return;
    }

    sendJson(res, 404, { error: "not found" });
  } catch (err) {
    sendJson(res, 500, { error: String(err?.message ?? err) });
  }
});

server.listen(PORT, () => {
  console.log(`guru mock web API listening on http://localhost:${PORT}`);
  console.log(`  try: POST /web/login {"email":"you@example.com","invite_code":"GURU-DEMO"}`);
});
