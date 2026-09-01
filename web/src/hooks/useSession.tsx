import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import * as api from "../api/client";
import type { SessionInfo } from "../api/types";
import { useRouter } from "../router/Router";

type SessionStatus = "loading" | "authed" | "anon";

/** Where `Join.tsx` stashes the invite token before sending an anonymous
 * visitor off to log in (full-page GitHub OAuth redirect, or the in-SPA
 * legacy form) — read back once here so they land back on the invite
 * instead of the default "/" after authenticating. Session-scoped (not
 * localStorage): a stale entry should never survive past this tab. */
const PENDING_INVITE_KEY = "kikimimi:pending_invite_token";

export function stashPendingInvite(token: string): void {
  try {
    sessionStorage.setItem(PENDING_INVITE_KEY, token);
  } catch {
    // Storage unavailable (private mode, etc.) -- the sign-in link still
    // works, it just lands on "/" afterwards instead of back on the invite.
  }
}

function takePendingInvite(): string | null {
  try {
    const token = sessionStorage.getItem(PENDING_INVITE_KEY);
    if (token) sessionStorage.removeItem(PENDING_INVITE_KEY);
    return token;
  } catch {
    return null;
  }
}

interface SessionState {
  status: SessionStatus;
  session: SessionInfo | null;
  login: (email: string, inviteCode: string) => Promise<void>;
  logout: () => Promise<void>;
}

const SessionContext = createContext<SessionState | null>(null);

/** A path reachable while anonymous, in addition to "/login" -- the
 * `/join/:token` confirmation view handles its own anon state (a sign-in
 * prompt) rather than being bounced away before it can even show what the
 * invite is for. */
function isPublicWhileAnon(path: string): boolean {
  return path === "/login" || path.startsWith("/join/");
}

export function SessionProvider({ children }: { children: ReactNode }) {
  const { navigate, path } = useRouter();
  const [status, setStatus] = useState<SessionStatus>("loading");
  const [session, setSession] = useState<SessionInfo | null>(null);

  // Any data-endpoint 401 (session expired mid-use) bounces to /login.
  useEffect(() => {
    api.onUnauthorized(() => {
      setStatus("anon");
      setSession(null);
      navigate("/login", { replace: true });
    });
  }, [navigate]);

  // Initial session check on load.
  useEffect(() => {
    let cancelled = false;
    api
      .me()
      .then((info) => {
        if (cancelled) return;
        setSession(info);
        setStatus("authed");
      })
      .catch(() => {
        if (cancelled) return;
        setSession(null);
        setStatus("anon");
      });
    return () => {
      cancelled = true;
    };
    // Only run once on mount; route changes are handled separately below.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Once authenticated: resume a pending invite (see stashPendingInvite)
  // ahead of anything else, otherwise leave /login for "/".  Covers both
  // the legacy in-SPA login and a full-page GitHub OAuth round trip landing
  // back on "/" with a fresh session.
  useEffect(() => {
    if (status !== "authed") return;
    const pending = takePendingInvite();
    if (pending) {
      navigate(`/join/${pending}`, { replace: true });
      return;
    }
    if (path === "/login") {
      navigate("/", { replace: true });
    }
    // Only the authed transition (and pending-invite consumption) should
    // trigger this; re-checking on every path change would fight normal
    // in-app navigation.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [status]);

  // Redirect unauthenticated users away from protected routes.
  useEffect(() => {
    if (status === "anon" && !isPublicWhileAnon(path)) {
      navigate("/login", { replace: true });
    }
  }, [status, path, navigate]);

  const login = useCallback(async (email: string, inviteCode: string) => {
    await api.login({ email, invite_code: inviteCode });
    // POST /web/login's own response is just {email, org_id} (legacy
    // shape) -- fetch the full {orgs, active_org, github_login} session
    // separately, same as a GitHub OAuth round trip effectively does.
    const info = await api.me();
    setSession(info);
    setStatus("authed");
  }, []);

  const logout = useCallback(async () => {
    try {
      await api.logout();
    } finally {
      setSession(null);
      setStatus("anon");
      navigate("/login", { replace: true });
    }
  }, [navigate]);

  const value = useMemo(
    () => ({ status, session, login, logout }),
    [status, session, login, logout],
  );

  return (
    <SessionContext.Provider value={value}>{children}</SessionContext.Provider>
  );
}

export function useSession(): SessionState {
  const ctx = useContext(SessionContext);
  if (!ctx) throw new Error("useSession must be used within SessionProvider");
  return ctx;
}
