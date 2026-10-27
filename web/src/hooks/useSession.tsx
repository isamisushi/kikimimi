import { createContext, useContext, useEffect, useState, type ReactNode } from "react";
import { me, onUnauthorized } from "../api/client";
import type { SessionInfo } from "../api/types";
type SessionState = { status: "loading" | "authed" | "error"; session: SessionInfo | null };
const SessionContext = createContext<SessionState | null>(null);
export function SessionProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<SessionState>({ status: "loading", session: null });
  useEffect(() => {
    let cancelled = false;
    const fail = () => { if (!cancelled) setState({ status: "error", session: null }); };
    onUnauthorized(fail);
    me().then(session => {
      if (!session.local) throw new Error("Expected a local dashboard server");
      if (!cancelled) setState({ status: "authed", session });
    }).catch(fail);
    return () => { cancelled = true; };
  }, []);
  return <SessionContext.Provider value={state}>{children}</SessionContext.Provider>;
}
export function useSession(): SessionState {
  const state = useContext(SessionContext);
  if (!state) throw new Error("useSession must be used within SessionProvider");
  return state;
}
