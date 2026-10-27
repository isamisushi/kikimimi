import type {
  McpRow,
  SkillRow,
  UnusedMcpRow,
  MachineRow,
  OverviewRow,
  ImprovementMark,
  PatternHitRow,
  PatternRow,
  PatternTimelineRow,
  QueryResult,
  SessionInfo,
  SessionRow,
  ToolRow,
  SubagentRow,
  CoverageRow,
  UnusedSkillRow,
  SessionDetail,
  ModelsResponse,
  StorageSettings,
  ReadSourceInfo,
  S3ReadConnection,
} from "./types";

export function getStorageSettings(): Promise<StorageSettings> {
  return request("/web/storage");
}

export function getReadSource(): Promise<ReadSourceInfo> { return request("/web/source"); }
export function selectReadSource(source: { kind: "local" } | ({ kind: "s3" } & S3ReadConnection)): Promise<void> {
  return request("/web/source", { method: "POST", body: JSON.stringify(source) });
}
export function refreshReadSource(): Promise<void> { return request("/web/source/refresh", { method: "POST" }); }

/** Client for the local dashboard HTTP API. */

export class ApiError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }
}

export interface SubscriptionSnapshot {
  agent: "claude" | "codex";
  account: string;
  observed_at: number;
  windows: {
    name: string;
    used_percent: number;
    window_minutes: number | null;
    resets_at: number | null;
  }[];
}

export function getSubscriptionUsage(): Promise<SubscriptionSnapshot[]> {
  return request("/web/usage");
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

/** Notify the local app when its loopback session is unavailable. */
export function onUnauthorized(handler: () => void): void {
  unauthorizedHandler = handler;
}

export async function request<T>(
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

export function me(): Promise<SessionInfo> {
  return request<SessionInfo>(
    "/web/me",
    undefined,
    { suppressUnauthorizedRedirect: true },
  );
}

export function getOverview(days = 14): Promise<QueryResult<OverviewRow>> {
  return request(`/web/q/overview?days=${days}`);
}

/** GET /web/q/models?days=N — model × effort usage (KKM-34). Org-wide
 * aggregate, never per person, so every role may read it. */
export function getModels(days = 14): Promise<ModelsResponse> {
  return request(`/web/q/models?days=${days}`);
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

/** GET /web/q/session?session_id=X&events_limit=N — one session drilled
 * down (summary, per-tool, per-subagent, timeline, event list). 404 for an
 * unknown id or, in a team org, another member's session. */
export function getSessionDetail(sessionId: string, eventsLimit = 500): Promise<SessionDetail> {
  const qs = new URLSearchParams({ session_id: sessionId, events_limit: String(eventsLimit) });
  return request(`/web/q/session?${qs.toString()}`);
}

/** GET /web/q/unused-skills?days=N — configured-vs-invoked skills (KKM-18). */
export function getUnusedSkills(days = 14): Promise<QueryResult<UnusedSkillRow>> {
  return request(`/web/q/unused-skills?days=${days}`);
}

/** GET /web/q/coverage?days=N — the missing-data counts (KKM-17); one row,
 * org-wide aggregates, visible to every role. */
export function getCoverage(days = 30): Promise<QueryResult<CoverageRow>> {
  return request(`/web/q/coverage?days=${days}`);
}

/** GET /web/q/funnel?days=N&scope=org|all — onboarding funnel (KKM-21).
 * `org` needs admin/owner of the active org; `all` needs the account's
 * operator flag and is 404 otherwise. */
export function getSubagents(
  days = 14,
  limit = 50,
): Promise<QueryResult<SubagentRow>> {
  return request(`/web/q/subagents?days=${days}&limit=${limit}`);
}

/** GET /web/q/patterns?days=N — the org-wide struggle ranking (visible to
 * every role: it aggregates over patterns and subjects, never people). */
export function getPatterns(days = 30): Promise<QueryResult<PatternRow>> {
  return request(`/web/q/patterns?days=${days}`);
}

/** GET /web/q/pattern-hits — the incidents behind one ranking row. In a
 * team org a member below admin only gets their own sessions back. */
export function getPatternHits(
  patternId: string,
  subject: string,
  days = 30,
  limit = 50,
): Promise<QueryResult<PatternHitRow>> {
  const qs = new URLSearchParams({
    pattern_id: patternId,
    subject,
    days: String(days),
    limit: String(limit),
  });
  return request(`/web/q/pattern-hits?${qs.toString()}`);
}

/** GET /web/q/pattern-timeline — per-day hit rate and cost for one row. */
export function getPatternTimeline(
  patternId: string,
  subject: string,
  days = 60,
): Promise<QueryResult<PatternTimelineRow>> {
  const qs = new URLSearchParams({ pattern_id: patternId, subject, days: String(days) });
  return request(`/web/q/pattern-timeline?${qs.toString()}`);
}

export function getMarks(patternId: string, subject: string): Promise<{ marks: ImprovementMark[] }> {
  const qs = new URLSearchParams({ pattern_id: patternId, subject });
  return request(`/web/marks?${qs.toString()}`);
}

/** POST /web/marks — admin/owner in a team org (403 otherwise). */
export function createMark(
  patternId: string,
  subject: string,
  markedDt: string,
  note: string,
): Promise<ImprovementMark> {
  return request("/web/marks", {
    method: "POST",
    body: JSON.stringify({ pattern_id: patternId, subject, marked_dt: markedDt, note }),
  });
}

export function deleteMark(id: string): Promise<{ deleted: string }> {
  return request(`/web/marks/${encodeURIComponent(id)}`, { method: "DELETE" });
}
