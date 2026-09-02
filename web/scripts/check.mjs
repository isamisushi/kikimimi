#!/usr/bin/env node
// Playwright-free smoke test: boots the mock server on a scratch port,
// walks the full contract (login/me/logout, every /web/q/* endpoint, and
// the account-model surface -- config, orgs, active-org switch, members,
// invites, join, devices), and checks response shape. Run with `npm run check`.

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

    // GET /web/config (unauthenticated, needed by the login page before any session)
    {
      const res = await fetch(`${BASE}/web/config`);
      check(res.status === 200, "GET /web/config -> 200 with no cookie");
      const body = await res.json();
      check(typeof body.github_oauth === "boolean", "config: github_oauth is a boolean");
      check(typeof body.legacy_login === "boolean", "config: legacy_login is a boolean");
      check(body.legacy_login === true, "mock config: legacy_login is true (no real GitHub client configured)");
    }

    // /web/me with cookie -> 200, account-model shape (orgs[]/active_org/github_login)
    let personalSlug;
    {
      const res = await fetch(`${BASE}/web/me`, authed);
      check(res.status === 200, "GET /web/me with cookie -> 200");
      const body = await res.json();
      check(body.email === "dev@kikimimi.dev", "GET /web/me returns correct email");
      check(body.github_login === null, "GET /web/me: github_login is null for an email-login account");
      check(Array.isArray(body.orgs), "GET /web/me: orgs is an array");
      check(body.orgs.length >= 2, "GET /web/me: has a personal org plus the seeded demo team org");
      const personal = body.orgs.find((o) => o.kind === "personal");
      check(!!personal && personal.role === "owner", "GET /web/me: owner of own personal org");
      personalSlug = personal?.slug;
      const acme = body.orgs.find((o) => o.slug === "acme");
      check(!!acme && acme.kind === "team" && acme.role === "admin", "GET /web/me: auto-joined the demo team org as admin");
      check(body.active_org === personalSlug, "GET /web/me: active_org defaults to the personal org");
    }

    // POST /web/orgs -> create a team org, caller becomes owner
    {
      const res = await fetch(`${BASE}/web/orgs`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ name: "Check Co", slug: "check-co" }),
      });
      check(res.status === 401, "POST /web/orgs without a session -> 401");

      const res2 = await fetch(`${BASE}/web/orgs`, {
        method: "POST",
        ...authed,
        headers: { ...authed.headers, "Content-Type": "application/json" },
        body: JSON.stringify({ name: "Check Co", slug: "check-co" }),
      });
      check(res2.status === 200, "POST /web/orgs -> 200");
      const body = await res2.json();
      check(body.slug === "check-co" && body.role === "owner" && body.kind === "team", "created org response shape");

      const dup = await fetch(`${BASE}/web/orgs`, {
        method: "POST",
        ...authed,
        headers: { ...authed.headers, "Content-Type": "application/json" },
        body: JSON.stringify({ name: "Dup", slug: "check-co" }),
      });
      check(dup.status === 400, "POST /web/orgs with a taken slug -> 400");
    }

    // POST /web/active-org -> switch, reflected on /web/me
    {
      const res = await fetch(`${BASE}/web/active-org`, {
        method: "POST",
        ...authed,
        headers: { ...authed.headers, "Content-Type": "application/json" },
        body: JSON.stringify({ slug: "check-co" }),
      });
      check(res.status === 200, "POST /web/active-org -> 200");
      const me = await (await fetch(`${BASE}/web/me`, authed)).json();
      check(me.active_org === "check-co", "active_org reflects the switch on /web/me");

      const forbidden = await fetch(`${BASE}/web/active-org`, {
        method: "POST",
        ...authed,
        headers: { ...authed.headers, "Content-Type": "application/json" },
        body: JSON.stringify({ slug: "acme" }),
      });
      // dev@kikimimi.dev *is* a member of acme (auto-joined), so this
      // should succeed -- switch back to the personal org to prove a
      // genuinely-foreign slug is what actually gets rejected.
      check(forbidden.status === 200, "switching to a org the caller IS a member of -> 200");

      const notMember = await fetch(`${BASE}/web/active-org`, {
        method: "POST",
        ...authed,
        headers: { ...authed.headers, "Content-Type": "application/json" },
        body: JSON.stringify({ slug: "some-slug-nobody-is-in" }),
      });
      check(notMember.status === 404, "switching to an unknown org -> 404");

      // Switch back to the personal org for the rest of the checks below
      // (devices, etc. assume the personal org is active).
      await fetch(`${BASE}/web/active-org`, {
        method: "POST",
        ...authed,
        headers: { ...authed.headers, "Content-Type": "application/json" },
        body: JSON.stringify({ slug: personalSlug }),
      });
    }

    // Members + invites lifecycle on the seeded "acme" team org (dev@kikimimi.dev is admin there)
    {
      const res = await fetch(`${BASE}/web/orgs/acme/members`, authed);
      check(res.status === 200, "GET /web/orgs/acme/members (as admin) -> 200");
      const body = await res.json();
      check(body.members.length >= 3, "acme members: seeded teammates + this account");
      check(
        body.members.some((m) => m.email === "taylor@example.com" && m.role === "admin"),
        "acme members includes seeded admin teammate",
      );

      const createRes = await fetch(`${BASE}/web/orgs/acme/invites`, {
        method: "POST",
        ...authed,
        headers: { ...authed.headers, "Content-Type": "application/json" },
        body: JSON.stringify({ role: "member" }),
      });
      check(createRes.status === 200, "POST /web/orgs/acme/invites -> 200");
      const createBody = await createRes.json();
      check(typeof createBody.url === "string" && createBody.url.startsWith("/join/"), "invite response has a /join/ url");

      const listRes = await fetch(`${BASE}/web/orgs/acme/invites`, authed);
      check(listRes.status === 200, "GET /web/orgs/acme/invites -> 200");
      const listBody = await listRes.json();
      check(listBody.invites.length >= 1, "invites list is non-empty after creating one");
      const created = listBody.invites[0];
      check(created.role === "member" && created.uses === 0 && created.revoked === false, "created invite fields");

      const delRes = await fetch(`${BASE}/web/orgs/acme/invites/${created.id}`, { method: "DELETE", ...authed });
      check(delRes.status === 200, "DELETE /web/orgs/acme/invites/:id -> 200");
    }

    // GET /web/invites/:token (preview) + POST /join/:token (accept), from
    // a second account so it actually gains a NEW membership. Targets
    // "check-co" (created above, dev@kikimimi.dev is its owner) rather than
    // the seeded "acme" -- every fresh mock account auto-joins acme as
    // admin already (see ensureAccount), which would make "joining" it a
    // no-op re-join instead of actually exercising the grant.
    {
      const createRes = await fetch(`${BASE}/web/orgs/check-co/invites`, {
        method: "POST",
        ...authed,
        headers: { ...authed.headers, "Content-Type": "application/json" },
        body: JSON.stringify({ role: "viewer", max_uses: 1 }),
      });
      const { url } = await createRes.json();
      const token = url.replace("/join/", "");

      const anonPreview = await fetch(`${BASE}/web/invites/${token}`);
      check(anonPreview.status === 401, "GET /web/invites/:token without a session -> 401");

      const preview = await fetch(`${BASE}/web/invites/${token}`, authed);
      check(preview.status === 200, "GET /web/invites/:token -> 200");
      const previewBody = await preview.json();
      check(previewBody.org_name === "Check Co" && previewBody.usable === true, "invite preview shape");

      const loginRes = await fetch(`${BASE}/web/login`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ email: "joiner@example.com", invite_code: "KIKIMIMI-2026" }),
      });
      const joinerCookie = extractCookie(loginRes);
      const joinerAuthed = { headers: { Cookie: joinerCookie } };

      const joinRes = await fetch(`${BASE}/join/${token}`, { method: "POST", ...joinerAuthed });
      check(joinRes.status === 200, "POST /join/:token -> 200");
      const joinBody = await joinRes.json();
      check(joinBody.org_slug === "check-co" && joinBody.role === "viewer", "join response reports org + granted role");

      const joinerMe = await (await fetch(`${BASE}/web/me`, joinerAuthed)).json();
      check(
        joinerMe.orgs.some((o) => o.slug === "check-co" && o.role === "viewer"),
        "joiner's /web/me now lists membership in check-co",
      );

      const missing = await fetch(`${BASE}/web/invites/not-a-real-token`, authed);
      check(missing.status === 404, "GET /web/invites/:token for an unknown token -> 404");
    }

    // GET /web/devices — admin of active org sees the whole org; non-admin
    // sees only their own (across orgs). "acme" is currently NOT active for
    // `authed` (switched back to personal above), but dev@kikimimi.dev *is*
    // admin of acme -- switch there to check the admin branch, then switch
    // back.
    {
      const own = await fetch(`${BASE}/web/devices`, authed);
      check(own.status === 200, "GET /web/devices (personal org active) -> 200");
      const ownBody = await own.json();
      check(
        ownBody.devices.every((d) => d.account_email === "dev@kikimimi.dev"),
        "GET /web/devices on a personal org only lists this account's own devices",
      );

      await fetch(`${BASE}/web/active-org`, {
        method: "POST",
        ...authed,
        headers: { ...authed.headers, "Content-Type": "application/json" },
        body: JSON.stringify({ slug: "acme" }),
      });
      const asAdmin = await fetch(`${BASE}/web/devices`, authed);
      const asAdminBody = await asAdmin.json();
      check(
        asAdminBody.devices.some((d) => d.account_email === "taylor@example.com"),
        "GET /web/devices as acme admin includes a teammate's device",
      );
      const target = asAdminBody.devices.find((d) => d.account_email === "taylor@example.com");
      const revoke = await fetch(`${BASE}/web/devices/${target.id}/revoke`, { method: "POST", ...authed });
      check(revoke.status === 200, "POST /web/devices/:id/revoke (as admin, org device) -> 200");

      await fetch(`${BASE}/web/active-org`, {
        method: "POST",
        ...authed,
        headers: { ...authed.headers, "Content-Type": "application/json" },
        body: JSON.stringify({ slug: personalSlug }),
      });
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

    // /web/q/unused-mcp
    {
      const res = await fetch(`${BASE}/web/q/unused-mcp?days=14`, authed);
      check(res.status === 200, "GET /web/q/unused-mcp -> 200");
      const body = await res.json();
      checkQueryResult(
        body,
        [
          "mcp_server",
          "configured",
          "calls",
          "distinct_sessions",
          "last_called_dt",
          "sessions_configured",
          "configured_from_snapshot",
        ],
        "unused-mcp",
      );
      check(
        body.rows.some((r) => r[1] === true && r[2] === 0),
        "unused-mcp: at least one configured server with 0 calls (the whole point of the query)",
      );
      check(
        body.rows.some((r) => r[1] === false),
        "unused-mcp: includes a server that was called historically but isn't in the current config",
      );
      check(
        typeof body.rows[0]?.[6] === "boolean",
        "unused-mcp: configured_from_snapshot is a boolean",
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
