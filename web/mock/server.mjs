#!/usr/bin/env node
// Mock dev server for the kikimimi web API contract (see docs / task description).
// Plain Node core only, no dependencies. The real Rust server implements the
// same contract; this exists so the SPA can be built and demoed standalone.
//
// Usage: node mock/server.mjs   (or `npm run mock` from web/)

import http from "node:http";
import crypto from "node:crypto";
import { URL } from "node:url";

// Not 8787: that's the real kikimimi-cloud server's port (fly.toml
// internal_port). Keep the mock on a distinct port so it never collides
// with a real instance running locally.
const PORT = process.env.PORT ? Number(process.env.PORT) : 8788;
const COOKIE_NAME = "kikimimi_session";

// Any of these invite codes "work"; anything else -> 403.
const VALID_INVITES = new Set(["KIKIMIMI-DEMO", "KIKIMIMI-2026"]);

// This mock never has a real GITHUB_CLIENT_ID/_SECRET to hand out, so it
// always reports the legacy email+invite path as the (only) live one --
// GET /auth/github itself is not implemented here (nothing meaningful to
// mock about an OAuth redirect to a real github.com).
const WEB_CONFIG = { github_oauth: false, legacy_login: true };

// ---------------------------------------------------------------------------
// Account model (architecture.md §6.1): accounts, orgs, memberships,
// invites, devices, and sessions, all in memory. Seeded with a shared demo
// team org ("Acme Inc") plus two synthetic teammates so the Team/Devices
// pages have something to show on the very first login, without needing a
// second browser/account to explore the admin views.
// ---------------------------------------------------------------------------

const ROLE_RANK = { owner: 4, admin: 3, member: 2, viewer: 1 };
function roleAtLeast(role, min) {
  return (ROLE_RANK[role] ?? 0) >= (ROLE_RANK[min] ?? 0);
}

function slugify(s) {
  return s
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

function shortId() {
  return crypto.randomBytes(4).toString("hex");
}

// accounts: email -> { email, githubLogin, personalOrgSlug }
const accounts = new Map();
// orgs: slug -> { slug, name, kind }
const orgs = new Map();
// memberships: "email::slug" -> role
const memberships = new Map();
const membershipKey = (email, slug) => `${email}::${slug}`;
// devices: id -> { id, ownerEmail, orgSlug, hostId, hostname, createdAt, lastSeenAt, revoked }
const devices = new Map();
// invites: token -> { id, orgSlug, role, expiresAt, maxUses, uses, revoked, createdAt }
const invites = new Map();
// sessions: token -> { email, activeOrgSlug }
const sessions = new Map();

const ACME_SLUG = "acme";
orgs.set(ACME_SLUG, { slug: ACME_SLUG, name: "Acme Inc", kind: "team" });
for (const [email, role] of [
  ["taylor@example.com", "admin"],
  ["jordan@example.com", "member"],
]) {
  memberships.set(membershipKey(email, ACME_SLUG), role);
}
function seedDevice({ ownerEmail, orgSlug, hostId, hostname, ageMs, lastSeenAgoMs }) {
  const id = crypto.randomUUID();
  devices.set(id, {
    id,
    ownerEmail,
    orgSlug,
    hostId,
    hostname: hostname ?? null,
    createdAt: new Date(Date.now() - ageMs).toISOString(),
    lastSeenAt: lastSeenAgoMs === null ? null : new Date(Date.now() - lastSeenAgoMs).toISOString(),
    revoked: false,
  });
}
seedDevice({
  ownerEmail: "taylor@example.com",
  orgSlug: ACME_SLUG,
  hostId: "taylor-mbp",
  hostname: "taylor-mbp.local",
  ageMs: 30 * 86_400_000,
  lastSeenAgoMs: 5 * 60_000,
});
seedDevice({
  ownerEmail: "jordan@example.com",
  orgSlug: ACME_SLUG,
  hostId: "ci-runner-01",
  hostname: null,
  ageMs: 10 * 86_400_000,
  lastSeenAgoMs: 2 * 3_600_000,
});

/** First login for `email`: personal org (owner) + auto-joined into the
 * shared demo team org as `admin` (so the Team page's admin-only views --
 * members list, invite creation -- are explorable immediately). Idempotent
 * for repeat logins by the same email within one mock server run. */
function ensureAccount(email) {
  let acc = accounts.get(email);
  if (acc) return acc;
  const personalSlug = `${slugify(email.split("@")[0]) || "user"}-${shortId()}`;
  orgs.set(personalSlug, { slug: personalSlug, name: email.split("@")[0], kind: "personal" });
  memberships.set(membershipKey(email, personalSlug), "owner");
  memberships.set(membershipKey(email, ACME_SLUG), "admin");
  seedDevice({
    ownerEmail: email,
    orgSlug: personalSlug,
    hostId: `${slugify(email.split("@")[0]) || "user"}-laptop`,
    hostname: "this-machine.local",
    ageMs: 3 * 86_400_000,
    lastSeenAgoMs: 60_000,
  });
  // The first account of a mock run is the deployment operator (KKM-21),
  // so the Team page's "All orgs" funnel toggle is explorable.
  acc = { email, githubLogin: null, personalOrgSlug: personalSlug, operator: accounts.size === 0 };
  accounts.set(email, acc);
  return acc;
}

function membershipsFor(email) {
  const prefix = `${email}::`;
  const out = [];
  for (const [key, role] of memberships) {
    if (!key.startsWith(prefix)) continue;
    const slug = key.slice(prefix.length);
    const org = orgs.get(slug);
    if (!org) continue;
    out.push({ slug: org.slug, name: org.name, kind: org.kind, role });
  }
  out.sort((a, b) => (a.kind === "personal" ? -1 : 0) - (b.kind === "personal" ? -1 : 0));
  return out;
}

function meBody(session) {
  const acc = accounts.get(session.email);
  return {
    email: session.email,
    github_login: acc?.githubLogin ?? null,
    operator: acc?.operator === true,
    orgs: membershipsFor(session.email),
    active_org: session.activeOrgSlug,
  };
}

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

// /web/q/unused-mcp fixture (see queries.md's unused-mcp example): the
// currently-configured servers (a mix of the ones above that ARE called,
// plus "notion" -- configured but with no row in MCP_SERVERS at all, i.e.
// truly no events ever, not just zero-in-window) unioned with one server
// that was called in the past but has since been removed from the config.
const MCP_CONFIGURED_SERVERS = ["github", "sentry", "linear", "notion"];
const MCP_SESSIONS_CONFIGURED = { github: 12, sentry: 9, linear: 4, notion: 4 };
const MCP_HISTORICAL_ONLY = { mcp_server: "jira", calls: 6, distinct_sessions: 3, lastCalledDaysAgo: 20 };

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

/** [mcp_server, configured, calls, distinct_sessions, last_called_dt,
 * sessions_configured, configured_from_snapshot] -- see `UnusedMcpRow` in
 * web/src/api/types.ts. `configured_from_snapshot` is always true here
 * (this mock always simulates a real session.start config snapshot, never
 * the observed-in-30-days fallback). */
function generateUnusedMcp(days) {
  const scale = days / 14;
  const now = Date.now();
  const observedByServer = new Map(MCP_SERVERS.map((s) => [s.mcp_server, s]));

  const rows = MCP_CONFIGURED_SERVERS.map((server) => {
    const observed = observedByServer.get(server);
    const calls = observed ? Math.max(0, Math.round(observed.calls * scale)) : 0;
    const distinctSessions = observed ? observed.distinct_sessions : 0;
    const lastCalledDt =
      calls > 0 && observed?.lastCalledDaysAgo != null
        ? dateStr(new Date(now - observed.lastCalledDaysAgo * 86_400_000))
        : null;
    return [server, true, calls, distinctSessions, lastCalledDt, MCP_SESSIONS_CONFIGURED[server] ?? 0, true];
  });

  rows.push([
    MCP_HISTORICAL_ONLY.mcp_server,
    false,
    Math.max(0, Math.round(MCP_HISTORICAL_ONLY.calls * scale)),
    MCP_HISTORICAL_ONLY.distinct_sessions,
    dateStr(new Date(now - MCP_HISTORICAL_ONLY.lastCalledDaysAgo * 86_400_000)),
    0,
    true,
  ]);

  // Never-called-but-configured rows first, then by calls ascending --
  // mirrors the real cloud/local daemon ORDER BY.
  rows.sort((a, b) => {
    const aNeverCalled = a[1] && a[2] === 0 ? 0 : 1;
    const bNeverCalled = b[1] && b[2] === 0 ? 0 : 1;
    if (aNeverCalled !== bNeverCalled) return aNeverCalled - bNeverCalled;
    return a[2] - b[2];
  });
  return rows;
}

/** [session_id, started_at, subagents, agent_types, subagent_tool_calls,
 * tool_calls, subagent_duration_ms, session_duration_ms, duration_share,
 * subagent_api_requests, subagent_tokens_est, session_tokens_est, token_share,
 * subagents_with_usage] -- see `SubagentRow`. Every third session has no
 * usage for its subagents (null, never 0). */
function generateSubagents(days, limit) {
  const now = Date.now();
  const rows = [];
  const windowMs = days * 86_400_000;
  for (let i = 0; i < limit; i++) {
    const startedAt = new Date(now - (i * 9.1 + (i % 4)) * 3_600_000);
    if (now - startedAt.getTime() > windowMs) break;
    const host = HOSTS[i % HOSTS.length];
    const subagents = 1 + ((i * 7) % 6);
    const types = ["Explore", "general-purpose", "Plan"].slice(0, 1 + (i % 3)).join(",");
    const subCalls = subagents * (8 + (i % 5));
    const allCalls = subCalls + 20 + ((i * 13) % 60);
    const sessionMs = 1_800_000 + ((i * 991_000) % 7_200_000);
    const subMs = Math.round(sessionMs * (0.1 + ((i * 17) % 60) / 100));
    const apiRequests = subagents * (3 + (i % 4));
    const unknown = i % 3 === 2;
    const sessionTokens = 120_000 + ((i * 7_919) % 400_000);
    const subTokens = unknown ? null : Math.round(sessionTokens * (0.05 + ((i * 11) % 50) / 100));
    rows.push([
      `sess_${String(i).padStart(4, "0")}_${host.host_id}`,
      startedAt.toISOString(),
      subagents,
      types,
      subCalls,
      allCalls,
      subMs,
      sessionMs,
      Math.round((subMs / sessionMs) * 1000) / 1000,
      apiRequests,
      subTokens,
      sessionTokens,
      subTokens === null ? null : Math.round((subTokens / sessionTokens) * 1000) / 1000,
      unknown ? 0 : subagents,
    ]);
  }
  return rows;
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


/** `/web/q/session?session_id=X` -- one deterministic session drilled down
 * (see `SessionDetail` in types.ts). Ids not of the `sess_NNNN_host` shape
 * the list endpoints hand out are "not found". */
function generateSessionDetail(sessionId) {
  const m = /^sess_(\d{4})_(.+)$/.exec(sessionId);
  if (!m) return null;
  const i = Number(m[1]);
  const host = m[2];
  const agent = AGENTS[i % AGENTS.length];
  const now = Date.now();
  const startedMs = now - (i * 5.3 + (i % 3)) * 3_600_000;
  const durationMs = 900_000 + ((i * 733_000) % 5_400_000); // 15min .. 1h45
  const endedMs = startedMs + durationMs;
  const ended = i % 5 !== 1;
  const iso = (ms) => new Date(ms).toISOString().replace(/\.\d{3}Z$/, "Z");

  // Subagents first: the event list and the tool table attribute to them.
  const subCount = i % 4; // 0..3
  const subagents = [];
  for (let k = 0; k < subCount; k++) {
    const start = startedMs + Math.round(durationMs * (0.2 + k * 0.25));
    const dur = 60_000 + ((i * 31 + k * 17) % 8) * 45_000;
    subagents.push({
      agent_id: `ag_${String(i).padStart(4, "0")}_${k}`,
      agent_type: ["Explore", "general-purpose", "Plan"][k % 3],
      turn_id: `turn_${k + 3}`,
      start,
      dur,
      tool_calls: 4 + ((i + k) % 6),
      failures: (i + k) % 5 === 0 ? 1 : 0,
      api_requests: 3 + ((i + k) % 4),
      tokens_est: (i + k) % 3 === 2 ? null : 8_000 + ((i * 991 + k * 157) % 40_000),
      tools: k % 2 === 0 ? "Grep,Read" : "Bash,Read",
      model: k % 3 === 1 ? "claude-haiku-4-5-20251001" : MODEL_BY_AGENT[agent],
      effort: k % 3 === 1 ? null : ["high", "medium", "low"][k % 3],
    });
  }

  // Events: a repeating turn shape (api.request -> tool.call -> tool.result),
  // with the subagents' rows interleaved at their start times.
  const TOOLS = [
    ["Bash", "bash", null],
    ["Read", "builtin", null],
    ["Edit", "builtin", null],
    ["mcp__github__search_issues", "mcp", "github"],
    ["Grep", "builtin", null],
  ];
  const events = [];
  const totalTurns = 8 + (i % 12);
  const EFFORTS = ["high", "medium", "xhigh"];
  const effort = EFFORTS[i % EFFORTS.length];
  events.push([startedMs, "session.start", "hook", null, null, null, null, null, null, null, null, null, null, null, null, null, null, null, effort]);
  for (let t = 0; t < totalTurns; t++) {
    const ts = startedMs + Math.round((durationMs * (t + 0.5)) / (totalTurns + 1));
    const inTok = 3_000 + ((i * 131 + t * 977) % 20_000);
    const outTok = 200 + ((i * 17 + t * 401) % 2_000);
    events.push([ts, "api.request", "otel", null, null, null, null, null, null, 2_000 + (t % 5) * 900, null, null, null, MODEL_BY_AGENT[agent], inTok, outTok, costFor(inTok, outTok), `turn_${t}`, effort]);
    const [tool, kind, mcp] = TOOLS[(i + t) % TOOLS.length];
    const fail = (i * 7 + t) % 9 === 0;
    const dur = kind === "mcp" ? 900 + (t % 4) * 700 : 40 + (t % 6) * 120;
    events.push([ts + 1_500, "tool.call", "hook", tool, kind, mcp, null, null, null, null, null, null, null, null, null, null, null, `turn_${t}`, effort]);
    events.push([ts + 1_500 + dur, "tool.result", "otel", tool, kind, mcp, null, null, null, dur, !fail, fail ? "timeout" : null, null, null, null, null, null, `turn_${t}`, effort]);
  }
  for (const sa of subagents) {
    for (let c = 0; c < sa.tool_calls; c++) {
      const ts = sa.start + Math.round((sa.dur * (c + 0.5)) / sa.tool_calls);
      const tool = sa.tools.split(",")[c % 2];
      const fail = sa.failures > 0 && c === 0;
      events.push([ts, "tool.call", "hook", tool, "builtin", null, null, sa.agent_id, sa.agent_type, null, null, null, null, null, null, null, null, sa.turn_id, sa.effort]);
      events.push([ts + 80, "tool.result", "hook", tool, "builtin", null, null, sa.agent_id, sa.agent_type, 80, !fail, fail ? "not_found" : null, null, null, null, null, null, sa.turn_id, sa.effort]);
    }
    if (sa.tokens_est !== null) {
      // transcript-sourced api.request rows for the agent, splitting tokens_est 4:1 in/out
      const perReq = Math.round(sa.tokens_est / sa.api_requests);
      for (let q = 0; q < sa.api_requests; q++) {
        const ts = sa.start + Math.round((sa.dur * (q + 0.25)) / sa.api_requests);
        const inTok = Math.round(perReq * 0.8);
        const outTok = perReq - inTok;
        events.push([ts, "api.request", "log", null, null, null, null, sa.agent_id, sa.agent_type, null, null, null, null, sa.model, inTok, outTok, null, sa.turn_id, sa.effort]);
      }
    }
    events.push([sa.start + sa.dur, "subagent.stop", "hook", null, null, null, null, sa.agent_id, sa.agent_type, sa.dur, null, null, null, null, null, null, null, sa.turn_id, sa.effort]);
  }
  if (ended) events.push([endedMs, "session.end", "hook", null, null, null, null, null, null, null, null, null, null, null, null, null, null, null, effort]);
  events.sort((a, b) => a[0] - b[0]);

  // Roll everything up from the same event list so the sections agree.
  const byTool = new Map();
  for (const e of events) {
    if (!e[3]) continue;
    const row = byTool.get(e[3]) ?? { kind: e[4], mcp: e[5], calls: 0, subCalls: 0, failures: 0, durs: [] };
    if (e[1] === "tool.call") {
      row.calls++;
      if (e[7]) row.subCalls++;
    }
    if (e[1] === "tool.result") {
      if (e[10] === false) row.failures++;
      if (e[9] !== null) row.durs.push(e[9]);
    }
    byTool.set(e[3], row);
  }
  const pct = (arr, q) => {
    if (arr.length === 0) return null;
    const s = [...arr].sort((a, b) => a - b);
    return s[Math.min(s.length - 1, Math.floor(q * (s.length - 1)))];
  };
  const tools = [...byTool.entries()]
    .map(([name, r]) => [name, r.kind, r.mcp, r.calls, r.subCalls, r.failures, 0, pct(r.durs, 0.5), pct(r.durs, 0.95), r.durs.reduce((a, b) => a + b, 0)])
    .sort((a, b) => b[3] - a[3]);

  const failures = events.filter((e) => e[10] === false).length;
  const inputTokens = events.reduce((a, e) => a + (e[14] ?? 0), 0);
  const outputTokens = events.reduce((a, e) => a + (e[15] ?? 0), 0);
  const summary = [
    sessionId,
    agent,
    agent === "claude-code" ? "2.1.0" : null,
    host,
    i % 2 === 0 ? "github.com/isamisushi/kikimimi" : null,
    iso(startedMs),
    iso(ended ? endedMs : events[events.length - 1][0]),
    (ended ? endedMs : events[events.length - 1][0]) - startedMs,
    ended,
    events.length,
    totalTurns,
    events.filter((e) => e[1] === "tool.call").length,
    failures,
    0,
    events.filter((e) => e[1] === "api.request").length,
    0,
    i % 6 === 0 ? 1 : 0,
    subagents.length,
    MODEL_BY_AGENT[agent],
    "hook,otel",
    inputTokens,
    outputTokens,
    inputTokens * 4,
    Math.round(inputTokens / 10),
    costFor(inputTokens, outputTokens),
    i % 3 === 0 ? JSON.stringify(["github", "playwright"]) : null,
    i % 3 === 0 ? JSON.stringify(["dataviz", "design"]) : null,
    [...new Set(events.map((e) => e[18]).filter((x) => x))].sort().join(","),
  ];
  // patch `models` (index 18) so it agrees with the event list's models
  summary[18] = [...new Set(events.map((e) => e[13]).filter((x) => x))].sort().join(",");

  // models section: per (model, effort) over api.request rows
  const byModel = new Map();
  for (const e of events) {
    if (e[1] !== "api.request") continue;
    const key = `${e[13]}|${e[18] ?? ""}`;
    const row = byModel.get(key) ?? [e[13], e[18], 0, 0, 0, 0, 0, 0, 0, null, null];
    row[2]++;
    if (e[7]) row[4]++;
    row[5] += e[14] ?? 0;
    row[6] += e[15] ?? 0;
    row[7] += (e[14] ?? 0) * 4;
    row[8] += Math.round((e[14] ?? 0) / 10);
    if (e[2] === "log") row[9] = (row[9] ?? 0) + Math.round((e[15] ?? 0) * 0.6);
    if (e[16] !== null) row[10] = Number(((row[10] ?? 0) + e[16]).toFixed(4));
    byModel.set(key, row);
  }
  const models = [...byModel.values()].sort((a, b) => b[5] + b[6] - (a[5] + a[6]));

  const bucketMs = timelineBucketMs(summary[7]);
  const buckets = new Map();
  for (const e of events) {
    const b = Math.floor(e[0] / bucketMs) * bucketMs;
    const row = buckets.get(b) ?? [b, 0, 0, 0, 0, 0, 0];
    row[1]++;
    if (e[1] === "tool.call") row[2]++;
    if (e[10] === false) row[3]++;
    if (e[1] === "api.request") row[4]++;
    row[5] += (e[14] ?? 0) + (e[15] ?? 0);
    if (e[7]) row[6]++;
    buckets.set(b, row);
  }
  const timeline = [...buckets.values()].sort((a, b) => a[0] - b[0]);

  return {
    summary,
    tools,
    subagents: subagents.map((sa) => [sa.agent_id, sa.agent_type, sa.turn_id, iso(sa.start), sa.dur, sa.tool_calls * 2 + 1, sa.tool_calls, sa.failures, sa.api_requests, sa.tokens_est, sa.tools, sa.model, sa.effort]),
    models,
    timeline,
    events,
    bucket_ms: bucketMs,
  };
}

/** `/web/q/models?days=N` -- model × effort usage over the window (KKM-34).
 * Deterministic: four models, effort null for the Haiku helper and one
 * "unknown" model (a source that carried usage but no model name). */
function generateModels(days) {
  const specs = [
    // [model, effort, requests/day, in/req, out/req, subShare, errRate]
    ["claude-sonnet-4.5", "high", 90, 9_000, 700, 0.35, 0.01],
    ["claude-sonnet-4.5", "medium", 40, 7_000, 500, 0.1, 0.005],
    ["claude-opus-4.5", "xhigh", 25, 14_000, 1_400, 0.5, 0.02],
    ["claude-opus-4.5", "high", 8, 12_000, 1_100, 0.0, 0],
    ["claude-haiku-4-5-20251001", null, 60, 1_500, 80, 0.0, 0],
    ["gpt-5-codex", "medium", 20, 8_000, 900, 0.0, 0.01],
    ["unknown", null, 3, 2_000, 200, 0.0, 0],
  ];
  const models = [];
  const daily = [];
  const now = new Date();
  const dts = [];
  for (let i = days - 1; i >= 0; i--) {
    const d = new Date(now);
    d.setUTCDate(d.getUTCDate() - i);
    dts.push({ dt: d.toISOString().slice(0, 10), weekend: d.getUTCDay() === 0 || d.getUTCDay() === 6, i });
  }
  for (const [model, effort, perDay, inReq, outReq, subShare, errRate] of specs) {
    let requests = 0, inTok = 0, outTok = 0, cost = 0, sub = 0, subTok = 0, errors = 0;
    const byDay = new Map();
    for (const { dt, weekend, i } of dts) {
      const n = Math.round(perDay * (weekend ? 0.3 : 1) * (1 + ((i * 7) % 5) / 10));
      if (n === 0) continue;
      const din = n * inReq + ((i * 131) % 900);
      const dout = n * outReq + ((i * 17) % 90);
      const dc = model === "gpt-5-codex" ? null : costFor(din, dout);
      byDay.set(dt, [din, dout, dc]);
      requests += n;
      inTok += din;
      outTok += dout;
      if (dc !== null) cost += dc;
      const s = Math.round(n * subShare);
      sub += s;
      subTok += Math.round((din + dout) * subShare);
      errors += Math.round(n * errRate);
    }
    const unknownUsage = model === "unknown";
    models.push([
      model, effort, requests, errors, Math.max(1, Math.round(requests / 12)), sub,
      subShare > 0 ? subTok : null,
      unknownUsage ? null : inTok, unknownUsage ? null : outTok,
      unknownUsage ? null : inTok * 4, unknownUsage ? null : Math.round(inTok / 10),
      model.startsWith("claude") && subShare > 0 ? Math.round(outTok * 0.4) : null,
      model === "gpt-5-codex" || unknownUsage ? null : Number(cost.toFixed(4)),
    ]);
    if (!unknownUsage) {
      for (const [dt, [din, dout, dc]] of byDay) daily.push([dt, model, din, dout, dc]);
    }
  }
  models.sort((a, b) => (b[7] ?? 0) + (b[8] ?? 0) - ((a[7] ?? 0) + (a[8] ?? 0)));
  daily.sort((a, b) => a[0].localeCompare(b[0]) || a[1].localeCompare(b[1]));
  return { models, daily };
}

/** Same step table as the Rust handlers: >= 1 minute, ~240 buckets max. */
function timelineBucketMs(durationMs) {
  const MIN = 60_000;
  const steps = [1, 2, 5, 10, 15, 30, 60, 120, 360, 720, 1440].map((m) => m * MIN);
  const target = Math.max(0, durationMs) / 240;
  return steps.find((s) => s >= target) ?? 1440 * MIN;
}

const SESSION_SUMMARY_COLUMNS = [
  "session_id", "agent", "agent_version", "host_id", "repo", "started_at", "ended_at", "duration_ms", "ended",
  "events", "turns", "tool_calls", "failures", "tool_denied", "api_requests", "api_errors", "compactions",
  "subagents", "models", "sources", "input_tokens", "output_tokens", "cache_read_tokens", "cache_write_tokens",
  "cost_usd", "configured_mcp_servers", "configured_skills", "efforts",
];
const SESSION_TOOLS_COLUMNS = [
  "tool_name", "tool_kind", "mcp_server", "calls", "subagent_calls", "failures", "denied",
  "p50_duration_ms", "p95_duration_ms", "total_duration_ms",
];
const SESSION_SUBAGENTS_COLUMNS = [
  "agent_id", "agent_type", "turn_id", "started_at", "duration_ms", "events", "tool_calls", "failures",
  "api_requests", "tokens_est", "tools", "models", "efforts",
];
const SESSION_MODELS_COLUMNS = [
  "model", "effort", "api_requests", "api_errors", "subagent_api_requests", "input_tokens", "output_tokens",
  "cache_read_tokens", "cache_write_tokens", "reasoning_tokens", "cost_usd",
];
const MODELS_COLUMNS = [
  "model", "effort", "api_requests", "api_errors", "sessions", "subagent_api_requests", "subagent_tokens",
  "input_tokens", "output_tokens", "cache_read_tokens", "cache_write_tokens", "reasoning_tokens", "cost_usd",
];
const MODELS_DAILY_COLUMNS = ["dt", "model", "input_tokens", "output_tokens", "cost_usd"];
const SESSION_TIMELINE_COLUMNS = ["bucket_ts", "events", "tool_calls", "failures", "api_requests", "tokens", "subagent_events"];
const SESSION_EVENTS_COLUMNS = [
  "ts", "event_type", "source", "tool_name", "tool_kind", "mcp_server", "skill_name", "agent_id", "agent_type",
  "duration_ms", "success", "error_type", "decision", "model", "input_tokens", "output_tokens", "cost_usd", "turn_id",
  "effort",
];

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

// /web/q/patterns fixture (queries.md "patterns" + architecture.md §7.2):
// the struggle ranking. `unused_mcp_server linear` and `mcp_bypass github`
// on top -- the two rows the product exists to surface -- plus one row with
// an unknown (null) cost so the UI's "unknown, not zero" rendering is
// exercised, and one each of the remaining patterns.
const PATTERN_RANKING = [
  { pattern_id: "mcp_bypass", subject: "github", sessions: 9, incidents: 14, wasted: 184_000, priced: 12, hits: 14 },
  { pattern_id: "unused_mcp_server", subject: "linear", sessions: 12, incidents: 12, wasted: 96_000, priced: 10, hits: 12 },
  { pattern_id: "retry_spiral", subject: "mcp__github__create_pull_request", sessions: 4, incidents: 15, wasted: 41_000, priced: 4, hits: 4 },
  { pattern_id: "context_bloat", subject: "mcp__sentry__get_issue_events", sessions: 3, incidents: 3, wasted: 130_000, priced: 3, hits: 3 },
  { pattern_id: "deny_detour", subject: "WebFetch", sessions: 5, incidents: 6, wasted: 12_500, priced: 5, hits: 6 },
  { pattern_id: "permission_denied_loop", subject: "Bash", sessions: 2, incidents: 5, wasted: null, priced: 0, hits: 2 },
  { pattern_id: "long_tool_tail", subject: "sentry", sessions: 3, incidents: 3, wasted: null, priced: 0, hits: 3 },
];

function generatePatterns(days) {
  const now = Date.now();
  const lastSeen = dateStr(new Date(now - 86_400_000));
  const firstSeen = dateStr(new Date(now - Math.min(days, 30) * 86_400_000));
  return PATTERN_RANKING.map((p) => [
    p.pattern_id,
    p.subject,
    p.sessions,
    p.incidents,
    p.wasted,
    p.priced,
    p.hits,
    p.wasted === null ? null : p.wasted * p.sessions,
    firstSeen,
    lastSeen,
  ]).sort((a, b) => (b[7] ?? -1) - (a[7] ?? -1));
}

function generatePatternHits(patternId, subject, days, limit) {
  const ranking = PATTERN_RANKING.find((p) => p.pattern_id === patternId && p.subject === subject);
  if (!ranking) return [];
  const now = Date.now();
  const detailFor = {
    mcp_bypass: '{"failed_tool": "mcp__github__search_issues", "detour_tool": "Bash"}',
    unused_mcp_server: '{"n_configured": 4, "api_requests": 11, "allocation": "equal_split", "tokens_est": 8000}',
    retry_spiral: '{"mcp_server": "github"}',
    context_bloat: '{"kind": "jump", "ctx_tokens": 61000, "prev_ctx_tokens": 18000, "delta_tokens": 43000}',
    deny_detour: '{"detour_tool": "Bash"}',
    permission_denied_loop: "{}",
    long_tool_tail: '{"tool_name": "mcp__sentry__get_issue_events", "duration_ms": 31000, "median_ms": 1400}',
  };
  const rows = [];
  for (let i = 0; i < Math.min(ranking.hits, limit); i++) {
    const ts = now - (i + 1) * 3_600_000 * 7 - (i % 3) * 86_400_000;
    const dt = dateStr(new Date(ts));
    if (new Date(dt) < new Date(now - days * 86_400_000)) break;
    const priced = i < ranking.priced && ranking.wasted !== null;
    rows.push([
      dt,
      `sess_${(0x1a2b3c + i * 7919).toString(16)}${patternId.slice(0, 2)}`,
      ts,
      ts + 45_000,
      Math.max(1, Math.round(ranking.incidents / ranking.hits)),
      priced ? Math.round(ranking.wasted / ranking.priced) : null,
      detailFor[patternId] ?? null,
    ]);
  }
  return rows;
}

// Improvement marks (KKM-12): the github bypass row already has one so the
// before/after tiles render out of the box.
const MARKS = [
  {
    id: "mark_1",
    pattern_id: "mcp_bypass",
    subject: "github",
    marked_dt: dateStr(new Date(Date.now() - 12 * 86_400_000)),
    note: "added search_issues filters + clearer error text",
    created_at: new Date(Date.now() - 12 * 86_400_000).toISOString(),
  },
];

function generatePatternTimeline(patternId, subject, days) {
  const ranking = PATTERN_RANKING.find((p) => p.pattern_id === patternId && p.subject === subject);
  const now = Date.now();
  const mark = MARKS.find((m) => m.pattern_id === patternId && m.subject === subject);
  const rows = [];
  for (let i = days - 1; i >= 0; i--) {
    const d = new Date(now - i * 86_400_000);
    const dt = dateStr(d);
    const weekend = d.getUTCDay() === 0 || d.getUTCDay() === 6;
    const sessionsTotal = weekend ? 2 + (i % 2) : 8 + (i % 5);
    if (!ranking) {
      rows.push([dt, sessionsTotal, 0, 0, 0, null]);
      continue;
    }
    // Before the mark the pattern hits ~40% of sessions; after, ~8%.
    const after = mark && dt >= mark.marked_dt;
    const baseRate = after ? 0.08 : 0.4;
    const hit = Math.min(sessionsTotal, Math.round(sessionsTotal * baseRate + ((i * 7) % 3) * 0.3));
    const incidents = hit === 0 ? 0 : hit + (i % 2);
    const wasted = ranking.wasted === null || hit === 0 ? null : Math.round((ranking.wasted / ranking.priced) * hit);
    rows.push([dt, sessionsTotal, hit, (100 * hit) / sessionsTotal, incidents, wasted]);
  }
  return rows;
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

/** Matches `pathname` against a `"/web/orgs/:slug/invites/:id"`-style
 * pattern, returning the `:name` captures or `null` if it doesn't match --
 * enough routing for this file without pulling in a router dependency. */
function matchPath(pattern, pathname) {
  const patternParts = pattern.split("/").filter(Boolean);
  const pathParts = pathname.split("/").filter(Boolean);
  if (patternParts.length !== pathParts.length) return null;
  const params = {};
  for (let i = 0; i < patternParts.length; i++) {
    const part = patternParts[i];
    if (part.startsWith(":")) {
      params[part.slice(1)] = decodeURIComponent(pathParts[i]);
    } else if (part !== pathParts[i]) {
      return null;
    }
  }
  return params;
}

function inviteInfo(token) {
  const inv = invites.get(token);
  if (!inv) return null;
  const org = orgs.get(inv.orgSlug);
  const expired = Date.now() > new Date(inv.expiresAt).getTime();
  const exhausted = inv.maxUses !== null && inv.uses >= inv.maxUses;
  return {
    org_name: org?.name ?? inv.orgSlug,
    role: inv.role,
    usable: !inv.revoked && !expired && !exhausted,
    revoked: inv.revoked,
    expired,
    exhausted,
  };
}

const server = http.createServer(async (req, res) => {
  const url = new URL(req.url, `http://${req.headers.host ?? "localhost"}`);
  const { pathname, searchParams } = url;

  try {
    // --- Auth ---
    if (pathname === "/web/config" && req.method === "GET") {
      sendJson(res, 200, WEB_CONFIG);
      return;
    }

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

      const acc = ensureAccount(email);
      const token = crypto.randomBytes(24).toString("hex");
      sessions.set(token, { email, activeOrgSlug: acc.personalOrgSlug });
      res.setHeader(
        "Set-Cookie",
        `${COOKIE_NAME}=${token}; HttpOnly; Path=/; SameSite=Lax; Max-Age=${60 * 60 * 24 * 7}`,
      );
      // Legacy shape (account-model contract): just {email, org_id} -- the
      // SPA follows up with GET /web/me for the full session.
      sendJson(res, 200, { email, org_id: acc.personalOrgSlug });
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
      sendJson(res, 200, meBody(session));
      return;
    }

    // --- Orgs ---
    if (pathname === "/web/orgs" && req.method === "POST") {
      const session = requireSession(req, res);
      if (!session) return;
      let body;
      try {
        body = await readJsonBody(req);
      } catch {
        sendJson(res, 400, { error: "invalid json" });
        return;
      }
      const name = typeof body.name === "string" ? body.name.trim() : "";
      const slug = typeof body.slug === "string" ? body.slug.trim().toLowerCase() : "";
      if (!name) {
        sendJson(res, 400, { error: "name must not be empty" });
        return;
      }
      if (!slug || slug.length > 63 || !/^[a-z0-9-]+$/.test(slug) || slug.startsWith("-") || slug.endsWith("-")) {
        sendJson(res, 400, { error: "slug must be lowercase alphanumeric/hyphen" });
        return;
      }
      if (orgs.has(slug)) {
        sendJson(res, 400, { error: `slug ${JSON.stringify(slug)} is already taken` });
        return;
      }
      orgs.set(slug, { slug, name, kind: "team" });
      memberships.set(membershipKey(session.email, slug), "owner");
      sendJson(res, 200, { slug, name, kind: "team", role: "owner" });
      return;
    }

    if (pathname === "/web/active-org" && req.method === "POST") {
      const session = requireSession(req, res);
      if (!session) return;
      let body;
      try {
        body = await readJsonBody(req);
      } catch {
        sendJson(res, 400, { error: "invalid json" });
        return;
      }
      const slug = typeof body.slug === "string" ? body.slug : "";
      if (!orgs.has(slug)) {
        sendJson(res, 404, { error: `org ${JSON.stringify(slug)} not found` });
        return;
      }
      if (!memberships.has(membershipKey(session.email, slug))) {
        sendJson(res, 403, { error: "not a member of that org" });
        return;
      }
      session.activeOrgSlug = slug;
      sendJson(res, 200, { active_org: slug });
      return;
    }

    {
      const params = matchPath("/web/orgs/:slug/members", pathname);
      if (params && req.method === "GET") {
        const session = requireSession(req, res);
        if (!session) return;
        if (!orgs.has(params.slug)) {
          sendJson(res, 404, { error: "org not found" });
          return;
        }
        const callerRole = memberships.get(membershipKey(session.email, params.slug));
        if (!callerRole) {
          sendJson(res, 404, { error: "org not found" });
          return;
        }
        if (!roleAtLeast(callerRole, "admin")) {
          sendJson(res, 403, { error: "requires role admin or higher" });
          return;
        }
        const prefix = `::${params.slug}`;
        const members = [];
        for (const [key, role] of memberships) {
          if (!key.endsWith(prefix)) continue;
          const email = key.slice(0, -prefix.length);
          const acc = accounts.get(email);
          members.push({
            account_id: email,
            email,
            github_login: acc?.githubLogin ?? null,
            role,
            created_at: new Date(0).toISOString(),
          });
        }
        sendJson(res, 200, { members });
        return;
      }
    }

    // --- Invites ---
    {
      const params = matchPath("/web/orgs/:slug/invites", pathname);
      if (params && req.method === "POST") {
        const session = requireSession(req, res);
        if (!session) return;
        if (!orgs.has(params.slug)) {
          sendJson(res, 404, { error: "org not found" });
          return;
        }
        const callerRole = memberships.get(membershipKey(session.email, params.slug));
        if (!callerRole) {
          sendJson(res, 404, { error: "org not found" });
          return;
        }
        if (!roleAtLeast(callerRole, "admin")) {
          sendJson(res, 403, { error: "requires role admin or higher" });
          return;
        }
        let body;
        try {
          body = await readJsonBody(req);
        } catch {
          sendJson(res, 400, { error: "invalid json" });
          return;
        }
        const role = typeof body.role === "string" ? body.role : "";
        if (!(role in ROLE_RANK)) {
          sendJson(res, 400, { error: "role must be one of owner/admin/member/viewer" });
          return;
        }
        if (ROLE_RANK[role] > ROLE_RANK[callerRole]) {
          sendJson(res, 403, { error: "cannot create an invite for a role higher than your own" });
          return;
        }
        const expiresHours = Math.min(Math.max(Number(body.expires_hours) || 24 * 7, 1), 24 * 90);
        const maxUses = body.max_uses === null || body.max_uses === undefined ? null : Number(body.max_uses);
        const token = crypto.randomBytes(24).toString("hex");
        invites.set(token, {
          id: crypto.randomUUID(),
          orgSlug: params.slug,
          role,
          expiresAt: new Date(Date.now() + expiresHours * 3_600_000).toISOString(),
          maxUses,
          uses: 0,
          revoked: false,
          createdAt: new Date().toISOString(),
        });
        sendJson(res, 200, { url: `/join/${token}` });
        return;
      }
      if (params && req.method === "GET") {
        const session = requireSession(req, res);
        if (!session) return;
        if (!orgs.has(params.slug)) {
          sendJson(res, 404, { error: "org not found" });
          return;
        }
        const callerRole = memberships.get(membershipKey(session.email, params.slug));
        if (!callerRole || !roleAtLeast(callerRole, "admin")) {
          sendJson(res, callerRole ? 403 : 404, { error: "org not found or forbidden" });
          return;
        }
        const list = [...invites.entries()]
          .filter(([, inv]) => inv.orgSlug === params.slug)
          .sort((a, b) => new Date(b[1].createdAt) - new Date(a[1].createdAt))
          .map(([token, inv]) => ({
            id: inv.id,
            role: inv.role,
            expires_at: inv.expiresAt,
            max_uses: inv.maxUses,
            uses: inv.uses,
            revoked: inv.revoked,
            created_at: inv.createdAt,
            // Not part of the real contract (the real server never echoes
            // the plaintext token back out of a list endpoint), but handy
            // for this mock's own /join/:token demo links -- harmless
            // extra field, the SPA doesn't read it.
            _token: token,
          }));
        sendJson(res, 200, { invites: list });
        return;
      }
    }

    {
      const params = matchPath("/web/orgs/:slug/invites/:id", pathname);
      if (params && req.method === "DELETE") {
        const session = requireSession(req, res);
        if (!session) return;
        const callerRole = memberships.get(membershipKey(session.email, params.slug));
        if (!callerRole || !roleAtLeast(callerRole, "admin")) {
          sendJson(res, callerRole ? 403 : 404, { error: "org not found or forbidden" });
          return;
        }
        const entry = [...invites.values()].find((inv) => inv.id === params.id && inv.orgSlug === params.slug);
        if (!entry) {
          sendJson(res, 404, { error: "invite not found" });
          return;
        }
        entry.revoked = true;
        sendJson(res, 200, { ok: true });
        return;
      }
    }

    {
      const params = matchPath("/web/invites/:token", pathname);
      if (params && req.method === "GET") {
        const session = requireSession(req, res);
        if (!session) return;
        const info = inviteInfo(params.token);
        if (!info) {
          sendJson(res, 404, { error: "invite not found" });
          return;
        }
        sendJson(res, 200, info);
        return;
      }
    }

    {
      const params = matchPath("/join/:token", pathname);
      if (params && req.method === "POST") {
        const session = requireSession(req, res);
        if (!session) return;
        const inv = invites.get(params.token);
        if (!inv) {
          sendJson(res, 404, { error: "invite not found" });
          return;
        }
        if (inv.revoked) {
          sendJson(res, 400, { error: "invite has been revoked" });
          return;
        }
        if (Date.now() > new Date(inv.expiresAt).getTime()) {
          sendJson(res, 400, { error: "invite has expired" });
          return;
        }
        if (inv.maxUses !== null && inv.uses >= inv.maxUses) {
          sendJson(res, 400, { error: "invite has reached its use limit" });
          return;
        }
        const key = membershipKey(session.email, inv.orgSlug);
        if (!memberships.has(key)) {
          memberships.set(key, inv.role);
        }
        inv.uses += 1;
        sendJson(res, 200, { joined: true, org_slug: inv.orgSlug, role: inv.role });
        return;
      }
    }

    // --- Devices ---
    if (pathname === "/web/devices" && req.method === "GET") {
      const session = requireSession(req, res);
      if (!session) return;
      const activeRole = memberships.get(membershipKey(session.email, session.activeOrgSlug));
      const isAdmin = roleAtLeast(activeRole ?? "", "admin");
      const rows = [...devices.values()].filter((d) =>
        isAdmin ? d.orgSlug === session.activeOrgSlug : d.ownerEmail === session.email,
      );
      sendJson(res, 200, {
        devices: rows.map((d) => ({
          id: d.id,
          host_id: d.hostId,
          hostname: d.hostname,
          created_at: d.createdAt,
          last_seen_at: d.lastSeenAt,
          revoked: d.revoked,
          account_email: d.ownerEmail,
          org_slug: d.orgSlug,
          org_kind: orgs.get(d.orgSlug)?.kind ?? "team",
        })),
      });
      return;
    }

    {
      const params = matchPath("/web/devices/:id/revoke", pathname);
      if (params && req.method === "POST") {
        const session = requireSession(req, res);
        if (!session) return;
        const device = devices.get(params.id);
        const activeRole = memberships.get(membershipKey(session.email, session.activeOrgSlug));
        const canRevoke =
          device &&
          (device.ownerEmail === session.email ||
            (roleAtLeast(activeRole ?? "", "admin") && device.orgSlug === session.activeOrgSlug));
        if (!canRevoke) {
          sendJson(res, 404, { error: "device not found" });
          return;
        }
        device.revoked = true;
        sendJson(res, 200, { ok: true });
        return;
      }
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

    if (pathname === "/web/q/models" && req.method === "GET") {
      if (!requireSession(req, res)) return;
      const days = Number(searchParams.get("days") ?? "14");
      if (!Number.isInteger(days) || days < 1 || days > 365) {
        sendJson(res, 400, { error: `days must be between 1 and 365, got ${days}` });
        return;
      }
      const m = generateModels(days);
      sendJson(res, 200, {
        models: { columns: MODELS_COLUMNS, rows: m.models },
        daily: { columns: MODELS_DAILY_COLUMNS, rows: m.daily },
        days,
      });
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

    if (pathname === "/web/q/unused-mcp" && req.method === "GET") {
      if (!requireSession(req, res)) return;
      const days = Number(searchParams.get("days") ?? "14") || 14;
      sendQueryResult(
        res,
        [
          "mcp_server",
          "configured",
          "calls",
          "distinct_sessions",
          "last_called_dt",
          "sessions_configured",
          "configured_from_snapshot",
        ],
        generateUnusedMcp(days),
      );
      return;
    }

    if (pathname === "/web/q/patterns" && req.method === "GET") {
      if (!requireSession(req, res)) return;
      const days = Number(searchParams.get("days") ?? "30") || 30;
      sendQueryResult(
        res,
        [
          "pattern_id",
          "subject",
          "sessions",
          "incidents",
          "wasted_tokens_est",
          "priced_hits",
          "hits",
          "priority",
          "first_seen_dt",
          "last_seen_dt",
        ],
        generatePatterns(days),
      );
      return;
    }

    if (pathname === "/web/q/pattern-hits" && req.method === "GET") {
      if (!requireSession(req, res)) return;
      const patternId = searchParams.get("pattern_id");
      const subject = searchParams.get("subject");
      if (!patternId || !subject) {
        sendJson(res, 400, { error: "pattern_id and subject are required" });
        return;
      }
      const days = Number(searchParams.get("days") ?? "30") || 30;
      const limit = Number(searchParams.get("limit") ?? "50") || 50;
      sendQueryResult(
        res,
        ["dt", "session_id", "first_ts", "last_ts", "incidents", "wasted_tokens_est", "detail"],
        generatePatternHits(patternId, subject, days, limit),
      );
      return;
    }

    if (pathname === "/web/q/pattern-timeline" && req.method === "GET") {
      if (!requireSession(req, res)) return;
      const patternId = searchParams.get("pattern_id");
      const subject = searchParams.get("subject");
      if (!patternId || !subject) {
        sendJson(res, 400, { error: "pattern_id and subject are required" });
        return;
      }
      const days = Number(searchParams.get("days") ?? "60") || 60;
      sendQueryResult(
        res,
        ["dt", "sessions_total", "sessions_hit", "rate_pct", "incidents", "wasted_tokens_est"],
        generatePatternTimeline(patternId, subject, days),
      );
      return;
    }

    if (pathname === "/web/marks" && req.method === "GET") {
      if (!requireSession(req, res)) return;
      const patternId = searchParams.get("pattern_id");
      const subject = searchParams.get("subject");
      sendJson(res, 200, {
        marks: MARKS.filter((m) => m.pattern_id === patternId && m.subject === subject),
      });
      return;
    }

    if (pathname === "/web/marks" && req.method === "POST") {
      if (!requireSession(req, res)) return;
      const body = await readJsonBody(req);
      if (!body.pattern_id || !body.subject || !/^\d{4}-\d{2}-\d{2}$/.test(body.marked_dt ?? "")) {
        sendJson(res, 400, { error: "pattern_id, subject and marked_dt (YYYY-MM-DD) are required" });
        return;
      }
      const mark = {
        id: `mark_${MARKS.length + 1}`,
        pattern_id: body.pattern_id,
        subject: body.subject,
        marked_dt: body.marked_dt,
        note: body.note ?? "",
        created_at: new Date().toISOString(),
      };
      MARKS.push(mark);
      sendJson(res, 200, mark);
      return;
    }

    const markDel = pathname.match(/^\/web\/marks\/([^/]+)$/);
    if (markDel && req.method === "DELETE") {
      if (!requireSession(req, res)) return;
      const idx = MARKS.findIndex((m) => m.id === markDel[1]);
      if (idx < 0) {
        sendJson(res, 404, { error: "no such mark" });
        return;
      }
      MARKS.splice(idx, 1);
      sendJson(res, 200, { deleted: markDel[1] });
      return;
    }

    if (pathname === "/web/q/unused-skills" && req.method === "GET") {
      if (!requireSession(req, res)) return;
      sendQueryResult(
        res,
        [
          "skill_name",
          "configured",
          "sessions_configured",
          "calls",
          "distinct_sessions",
          "last_used_dt",
        ],
        [
          ["dataviz", true, 41, 0, 0, null],
          ["artifact-diagramming", true, 41, 0, 0, null],
          ["design", true, 41, 12, 9, "2026-09-05"],
          ["katamari-review", false, 0, 3, 2, "2026-09-02"],
        ],
      );
      return;
    }

    if (pathname === "/web/q/funnel" && req.method === "GET") {
      const session = requireSession(req, res);
      if (!session) return;
      const days = Number(searchParams.get("days") ?? "30") || 30;
      const scope = searchParams.get("scope") ?? "org";
      if (scope !== "org" && scope !== "all") {
        sendJson(res, 400, { error: `scope must be "org" or "all", got ${JSON.stringify(scope)}` });
        return;
      }
      const acc = accounts.get(session.email);
      if (scope === "all" && acc?.operator !== true) {
        sendJson(res, 404, { error: "not found" });
        return;
      }
      if (scope === "org" && (memberships.get(membershipKey(session.email, session.activeOrgSlug)) ?? "viewer") === "member") {
        sendJson(res, 403, { error: "admin or owner required" });
        return;
      }
      // org scope: the active org's seeded devices; all: a wider fixture with a visible drop-off.
      const orgDevices = [...devices.values()].filter((d) => d.orgSlug === session.activeOrgSlug);
      const n = orgDevices.length;
      const steps =
        scope === "all"
          ? [
              { step: "login_started", hosts: 41 },
              { step: "login_done", hosts: 36 },
              { step: "first_events", hosts: 22 },
              { step: "first_insight", hosts: 17 },
            ]
          : [
              { step: "login_started", hosts: n },
              { step: "login_done", hosts: n },
              { step: "first_events", hosts: Math.max(0, n - 1) },
              { step: "first_insight", hosts: Math.max(0, n - 1) },
            ];
      sendJson(res, 200, {
        days,
        scope,
        tracking: true,
        steps,
        median_minutes_login_to_first_events: scope === "all" ? 3.4 : 1.2,
        retention_30d: scope === "all" ? { hosts_eligible: 19, hosts_retained: 12 } : { hosts_eligible: n, hosts_retained: n },
      });
      return;
    }

    if (pathname === "/web/q/coverage" && req.method === "GET") {
      if (!requireSession(req, res)) return;
      sendQueryResult(
        res,
        [
          "events",
          "events_user_id_null",
          "sessions",
          "sessions_without_usage",
          "tool_results_hook",
          "tool_results_otel",
          "tool_results_matched",
          "tool_results_raw",
          "tool_results_deduped",
          "subagents",
          "subagents_with_usage",
          "hosts",
          "hosts_silent_24h",
          "last_event_ts",
        ],
        // 14% of sessions without usage, 91% hook<->otel match, one silent host.
        [[48210, 1930, 212, 30, 9120, 8610, 8300, 17730, 9430, 61, 19, HOSTS.length, 1, new Date().toISOString()]],
      );
      return;
    }

    if (pathname === "/web/q/subagents" && req.method === "GET") {
      if (!requireSession(req, res)) return;
      const days = Number(searchParams.get("days") ?? "14") || 14;
      const limit = Number(searchParams.get("limit") ?? "50") || 50;
      sendQueryResult(
        res,
        [
          "session_id",
          "started_at",
          "subagents",
          "agent_types",
          "subagent_tool_calls",
          "tool_calls",
          "subagent_duration_ms",
          "session_duration_ms",
          "duration_share",
          "subagent_api_requests",
          "subagent_tokens_est",
          "session_tokens_est",
          "token_share",
          "subagents_with_usage",
        ],
        generateSubagents(days, limit),
      );
      return;
    }

    if (pathname === "/web/q/session" && req.method === "GET") {
      if (!requireSession(req, res)) return;
      const sessionId = (searchParams.get("session_id") ?? "").trim();
      if (!sessionId || sessionId.length > 128) {
        sendJson(res, 400, { error: "session_id is required (1..=128 chars)" });
        return;
      }
      const eventsLimit = Number(searchParams.get("events_limit") ?? "500");
      if (!Number.isInteger(eventsLimit) || eventsLimit < 1 || eventsLimit > 2000) {
        sendJson(res, 400, { error: `events_limit must be between 1 and 2000, got ${eventsLimit}` });
        return;
      }
      const d = generateSessionDetail(sessionId);
      if (!d) {
        sendJson(res, 404, { error: "session not found" });
        return;
      }
      sendJson(res, 200, {
        summary: { columns: SESSION_SUMMARY_COLUMNS, rows: [d.summary] },
        tools: { columns: SESSION_TOOLS_COLUMNS, rows: d.tools },
        subagents: { columns: SESSION_SUBAGENTS_COLUMNS, rows: d.subagents },
        models: { columns: SESSION_MODELS_COLUMNS, rows: d.models },
        timeline: { columns: SESSION_TIMELINE_COLUMNS, rows: d.timeline },
        events: { columns: SESSION_EVENTS_COLUMNS, rows: d.events.slice(0, eventsLimit) },
        bucket_ms: d.bucket_ms,
        events_limit: eventsLimit,
      });
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
  console.log(`kikimimi mock web API listening on http://localhost:${PORT}`);
  console.log(`  try: POST /web/login {"email":"you@example.com","invite_code":"KIKIMIMI-DEMO"}`);
});
