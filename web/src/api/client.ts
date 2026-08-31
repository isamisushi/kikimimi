import type {
  LoginRequest,
  McpRow,
  MachineRow,
  OverviewRow,
  QueryResult,
  SessionInfo,
  SessionRow,
  ToolRow,
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

export function login(body: LoginRequest): Promise<SessionInfo> {
  return request<SessionInfo>(
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

export function getSessions(
  days = 14,
  limit = 50,
): Promise<QueryResult<SessionRow>> {
  return request(`/web/q/sessions?days=${days}&limit=${limit}`);
}
