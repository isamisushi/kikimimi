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

interface SessionState {
  status: SessionStatus;
  session: SessionInfo | null;
  login: (email: string, inviteCode: string) => Promise<void>;
  logout: () => Promise<void>;
}

const SessionContext = createContext<SessionState | null>(null);

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

  // Redirect unauthenticated users away from protected routes, and
  // authenticated users away from /login.
  useEffect(() => {
    if (status === "authed" && path === "/login") {
      navigate("/", { replace: true });
    }
    if (status === "anon" && path !== "/login") {
      navigate("/login", { replace: true });
    }
  }, [status, path, navigate]);

  const login = useCallback(async (email: string, inviteCode: string) => {
    const info = await api.login({ email, invite_code: inviteCode });
    setSession(info);
    setStatus("authed");
    navigate("/", { replace: true });
  }, [navigate]);

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
