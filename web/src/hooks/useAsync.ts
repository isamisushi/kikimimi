import { useCallback, useEffect, useState } from "react";
import { ApiError } from "../api/client";

export type AsyncState<T> =
  | { status: "loading" }
  | { status: "error"; error: string }
  | { status: "ok"; data: T };

/**
 * Runs `fn` whenever `deps` change and exposes a loading/error/ok state.
 * Ignores results from stale (superseded) calls, and swallows 401s: the
 * global unauthorized handler (see useSession) owns the redirect for those.
 */
export function useAsync<T>(
  fn: () => Promise<T>,
  deps: React.DependencyList,
  refreshMs = 0,
): AsyncState<T> & { reload: () => void } {
  const [state, setState] = useState<AsyncState<T>>({ status: "loading" });
  const [tick, setTick] = useState(0);

  useEffect(() => {
    let cancelled = false;
    let timer: number | undefined;
    async function load(background: boolean) {
      try {
        if (background && document.hidden) return;
        const data = await fn();
        if (!cancelled) setState({ status: "ok", data });
      } catch (err) {
        if (cancelled || (err instanceof ApiError && err.status === 401)) return;
        const error = err instanceof Error ? err.message : String(err);
        setState({ status: "error", error });
      } finally {
        if (!cancelled && refreshMs > 0) {
          timer = window.setTimeout(() => void load(true), refreshMs);
        }
      }
    }
    setState({ status: "loading" });
    void load(false);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...deps, tick, refreshMs]);

  const reload = useCallback(() => setTick((t) => t + 1), []);

  return { ...state, reload } as AsyncState<T> & { reload: () => void };
}
