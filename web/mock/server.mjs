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
  acc = { email, githubLogin: null, personalOrgSlug: personalSlug };
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
  console.log(`kikimimi mock web API listening on http://localhost:${PORT}`);
  console.log(`  try: POST /web/login {"email":"you@example.com","invite_code":"KIKIMIMI-DEMO"}`);
});
