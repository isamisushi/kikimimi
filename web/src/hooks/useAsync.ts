import { useCallback, useEffect, useRef, useState } from "react";
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
): AsyncState<T> & { reload: () => void } {
  const [state, setState] = useState<AsyncState<T>>({ status: "loading" });
  const [tick, setTick] = useState(0);
  const seq = useRef(0);

  useEffect(() => {
    const mySeq = ++seq.current;
    setState({ status: "loading" });
    fn()
      .then((data) => {
        if (seq.current !== mySeq) return;
        setState({ status: "ok", data });
      })
      .catch((err) => {
        if (seq.current !== mySeq) return;
        if (err instanceof ApiError && err.status === 401) {
          // Redirect is handled globally; stay in loading state until it fires.
          return;
        }
        const message = err instanceof Error ? err.message : String(err);
        setState({ status: "error", error: message });
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...deps, tick]);

  const reload = useCallback(() => setTick((t) => t + 1), []);

  return { ...state, reload } as AsyncState<T> & { reload: () => void };
}
