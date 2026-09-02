import type {
  Device,
  Invite,
  InviteInfo,
  LoginRequest,
  McpRow,
  SkillRow,
  UnusedMcpRow,
  MachineRow,
  Member,
  MemberRow,
  OverviewRow,
  QueryResult,
  Role,
  SessionInfo,
  SessionRow,
  ToolRow,
  WebConfig,
} from "./types";

/**
 * Typed fetch client for the kikimimi web API contract.
 *
 * All /web/q/* endpoints require the `kikimimi_session` cookie (HttpOnly, set by
 * POST /web/login) and are org-scoped server-side. A 401 from any endpoint
 * other than /web/me during the initial check means "not logged in" and
 * triggers a redirect to /login via the handler registered with
 * `onUnauthorized`.
 */

export class ApiError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }
}

/** Server error bodies are `{"error": "..."}` JSON (`error.rs`'s
 * `AppError::into_response`); unwrap that for display instead of showing
 * the raw response body verbatim. Falls back to the raw message (or a
 * generic one) for anything that isn't in that shape. */
export function apiErrorMessage(err: unknown): string {
  if (err instanceof ApiError) {
    try {
      const parsed: unknown = JSON.parse(err.message);
      if (parsed && typeof parsed === "object" && typeof (parsed as { error?: unknown }).error === "string") {
        return (parsed as { error: string }).error;
      }
    } catch {
      // Not JSON -- fall through to the raw message.
    }
    return err.message || `request failed (${err.status})`;
  }
  return err instanceof Error ? err.message : String(err);
}

let unauthorizedHandler: (() => void) | null = null;

/** Called once from the app root so 401 responses can redirect to /login. */
export function onUnauthorized(handler: () => void): void {
  unauthorizedHandler = handler;
}

async function request<T>(
  path: string,
  init?: RequestInit,
  opts: { suppressUnauthorizedRedirect?: boolean } = {},
): Promise<T> {
  let res: Response;
  try {
    res = await fetch(path, {
      ...init,
      credentials: "same-origin",
      headers: {
        ...(init?.body ? { "Content-Type": "application/json" } : {}),
        ...init?.headers,
      },
    });
  } catch (err) {
    throw new ApiError(0, "network error: could not reach kikimimi server");
  }

  if (res.status === 401) {
    if (!opts.suppressUnauthorizedRedirect) {
      unauthorizedHandler?.();
    }
    throw new ApiError(401, "unauthorized");
  }
  if (res.status === 403) {
    throw new ApiError(403, "forbidden");
  }
  if (!res.ok) {
    let detail = "";
    try {
      detail = await res.text();
    } catch {
      // ignore
    }
    throw new ApiError(res.status, detail || `request failed: ${res.status}`);
  }
  if (res.status === 204) {
    return undefined as T;
  }
  return (await res.json()) as T;
}

// --- Session auth ---

/** GET /web/config — which login path(s) this deployment has live. Public
 * (no session needed): the login page calls it before any auth exists. */
export function getConfig(): Promise<WebConfig> {
  return request<WebConfig>("/web/config", undefined, { suppressUnauthorizedRedirect: true });
}

/** POST /web/login (legacy email+invite path — only offered when
 * GET /web/config says `legacy_login`). Returns just `{email, org_id}`;
 * callers still need a follow-up `me()` for the full session shape
 * (`orgs`/`active_org`/`github_login`), same as the GitHub OAuth path does
 * implicitly via its redirect. */
export function login(body: LoginRequest): Promise<{ email: string; org_id: string }> {
  return request(
    "/web/login",
    { method: "POST", body: JSON.stringify(body) },
    { suppressUnauthorizedRedirect: true },
  );
}

export function logout(): Promise<void> {
  return request<void>("/web/logout", { method: "POST" });
}

/** Used for the initial session check; never triggers the redirect itself. */
export function me(): Promise<SessionInfo> {
  return request<SessionInfo>(
    "/web/me",
    undefined,
    { suppressUnauthorizedRedirect: true },
  );
}

// --- Orgs ---

export function createOrg(name: string, slug: string): Promise<{ slug: string; name: string; kind: "team"; role: Role }> {
  return request("/web/orgs", { method: "POST", body: JSON.stringify({ name, slug }) });
}

export function setActiveOrg(slug: string): Promise<{ active_org: string }> {
  return request("/web/active-org", { method: "POST", body: JSON.stringify({ slug }) });
}

export function getMembers(orgSlug: string): Promise<{ members: Member[] }> {
  return request(`/web/orgs/${encodeURIComponent(orgSlug)}/members`);
}

// --- Invites ---

export function getInvites(orgSlug: string): Promise<{ invites: Invite[] }> {
  return request(`/web/orgs/${encodeURIComponent(orgSlug)}/invites`);
}

export function createInvite(
  orgSlug: string,
  body: { role: Role; expires_hours?: number; max_uses?: number | null },
): Promise<{ url: string }> {
  return request(`/web/orgs/${encodeURIComponent(orgSlug)}/invites`, {
    method: "POST",
    body: JSON.stringify(body),
  });
}

export function revokeInvite(orgSlug: string, id: string): Promise<{ ok: true }> {
  return request(`/web/orgs/${encodeURIComponent(orgSlug)}/invites/${encodeURIComponent(id)}`, {
    method: "DELETE",
  });
}

/** GET /web/invites/:token — preview an invite before accepting it (the
 * `/join/:token` confirmation view). */
export function getInviteInfo(token: string): Promise<InviteInfo> {
  return request(`/web/invites/${encodeURIComponent(token)}`);
}

/** POST /join/:token — actually accept the invite. */
export function joinInvite(token: string): Promise<{ joined: true; org_slug: string; role: Role }> {
  return request(`/join/${encodeURIComponent(token)}`, { method: "POST" });
}

// --- Devices ---

export function getDevices(): Promise<{ devices: Device[] }> {
  return request("/web/devices");
}

export function revokeDevice(id: string): Promise<{ ok: true }> {
  return request(`/web/devices/${encodeURIComponent(id)}/revoke`, { method: "POST" });
}

// --- Data endpoints ---

export function getOverview(days = 14): Promise<QueryResult<OverviewRow>> {
  return request(`/web/q/overview?days=${days}`);
}

export function getMachines(): Promise<QueryResult<MachineRow>> {
  return request("/web/q/machines");
}

export function getTools(days = 14): Promise<QueryResult<ToolRow>> {
  return request(`/web/q/tools?days=${days}`);
}

export function getMcp(days = 14): Promise<QueryResult<McpRow>> {
  return request(`/web/q/mcp?days=${days}`);
}

/** GET /web/q/unused-mcp?days=N — configured MCP servers unioned with
 * observed (ever-called) ones, so a configured-but-never-called server is
 * present with calls=0 rather than just missing from `getMcp`. See
 * `UnusedMcpRow`'s doc comment for the column shape. */
export function getUnusedMcp(days = 14): Promise<QueryResult<UnusedMcpRow>> {
  return request(`/web/q/unused-mcp?days=${days}`);
}

export function getSkills(days = 14): Promise<QueryResult<SkillRow>> {
  return request(`/web/q/skills?days=${days}`);
}

export function getSessions(
  days = 14,
  limit = 50,
): Promise<QueryResult<SessionRow>> {
  return request(`/web/q/sessions?days=${days}&limit=${limit}`);
}

/** GET /web/q/members?days=N — "Member usage" (admin/owner-only in a team
 * org; a 403 for anyone below admin, so callers should gate on role before
 * even requesting this, same as Team.tsx does for the members/invites
 * panels). Named `getMemberUsage`, not `getMembers`, to not collide with
 * the existing GET /web/orgs/:slug/members roster fetch above. */
export function getMemberUsage(days = 30): Promise<QueryResult<MemberRow>> {
  return request(`/web/q/members?days=${days}`);
}
