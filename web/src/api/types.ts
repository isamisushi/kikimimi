// Types mirroring the WEB API CONTRACT implemented by the Rust `kikimimi cloud` server
// (and, for local dev, by web/mock/server.mjs). Keep in sync with the contract.

export interface LoginRequest {
  email: string;
  invite_code: string;
}

export type Role = "owner" | "admin" | "member" | "viewer";
export type OrgKind = "personal" | "team";

export interface OrgMembership {
  slug: string;
  name: string;
  kind: OrgKind;
  role: Role;
}

/** GET /web/me */
export interface SessionInfo {
  email: string;
  github_login: string | null;
  orgs: OrgMembership[];
  active_org: string;
}

/** GET /web/config — tells the login page which login paths are live. */
export interface WebConfig {
  github_oauth: boolean;
  legacy_login: boolean;
}

/** GET /web/orgs/:slug/members */
export interface Member {
  account_id: string;
  email: string;
  github_login: string | null;
  role: Role;
  created_at: string;
}

/** GET /web/orgs/:slug/invites */
export interface Invite {
  id: string;
  role: Role;
  expires_at: string;
  max_uses: number | null;
  uses: number;
  revoked: boolean;
  created_at: string;
}

/** GET /web/invites/:token */
export interface InviteInfo {
  org_name: string;
  role: Role;
  usable: boolean;
  revoked: boolean;
  expired: boolean;
  exhausted: boolean;
}

/** GET /web/devices — admin of the active org sees every device in that
 * org (any member); everyone else sees only their own, across all of their
 * orgs (not just the active one) — `org_slug`/`org_kind` say which. */
export interface Device {
  id: string;
  host_id: string;
  hostname: string | null;
  created_at: string;
  last_seen_at: string | null;
  revoked: boolean;
  account_email: string;
  org_slug: string;
  org_kind: OrgKind;
}

/** Generic shape every /web/q/* endpoint returns: a column list plus row tuples. */
export interface QueryResult<Row extends unknown[]> {
  columns: string[];
  rows: Row[];
}

// --- /web/q/overview?days=14 ---
// [dt, events, tool_calls, failures, input_tokens, output_tokens, cost_usd]
export type OverviewRow = [
  dt: string,
  events: number | null,
  tool_calls: number | null,
  failures: number | null,
  input_tokens: number | null,
  output_tokens: number | null,
  cost_usd: number | null,
];

// --- /web/q/machines ---
// [host_id, env_kind, os, last_event_ts, events_30d]
export type MachineRow = [
  host_id: string,
  env_kind: string,
  os: string,
  last_event_ts: string | null,
  events_30d: number | null,
];

// --- /web/q/tools?days=14 ---
// [tool_name, tool_kind, calls, failures, p50_duration_ms, p95_duration_ms]
export type ToolRow = [
  tool_name: string,
  tool_kind: string,
  calls: number | null,
  failures: number | null,
  p50_duration_ms: number | null,
  p95_duration_ms: number | null,
];

// --- /web/q/mcp?days=14 ---
// [mcp_server, calls, failures, distinct_sessions, last_called_dt]
export type McpRow = [
  mcp_server: string,
  calls: number | null,
  failures: number | null,
  distinct_sessions: number | null,
  last_called_dt: string | null,
];

// --- /web/q/sessions?days=14&limit=50 ---
// [session_id, agent, host_id, started_at, events, tool_calls, failures, models, input_tokens, output_tokens, cost_usd]
export type SessionRow = [
  session_id: string,
  agent: string,
  host_id: string,
  started_at: string,
  events: number | null,
  tool_calls: number | null,
  failures: number | null,
  models: string,
  input_tokens: number | null,
  output_tokens: number | null,
  cost_usd: number | null,
];
