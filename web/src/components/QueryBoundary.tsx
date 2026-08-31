import type { ReactNode } from "react";
import type { AsyncState } from "../hooks/useAsync";

interface Props<T> {
  state: AsyncState<T>;
  loadingLabel?: string;
  emptyLabel?: string;
  isEmpty?: (data: T) => boolean;
  onRetry?: () => void;
  children: (data: T) => ReactNode;
}

/** Shared loading / error / empty / content states for any async view. */
export function QueryBoundary<T>({
  state,
  loadingLabel = "Loading…",
  emptyLabel = "No data available",
  isEmpty,
  onRetry,
  children,
}: Props<T>) {
  if (state.status === "loading") {
    return (
      <div className="state-panel state-panel--loading" role="status">
        <span className="spinner" aria-hidden="true" />
        {loadingLabel}
      </div>
    );
  }

  if (state.status === "error") {
    return (
      <div className="state-panel state-panel--error" role="alert">
        <p>Failed to load: {state.error}</p>
        {onRetry && (
          <button type="button" className="btn btn--ghost" onClick={onRetry}>
            Retry
          </button>
        )}
      </div>
    );
  }

  if (isEmpty?.(state.data)) {
    return <div className="state-panel state-panel--empty">{emptyLabel}</div>;
  }

  return <>{children(state.data)}</>;
}
