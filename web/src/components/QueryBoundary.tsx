import { Button } from "@kikimimi/ui";
import type { ReactNode } from "react";
import type { AsyncState } from "../hooks/useAsync";

interface Props<T> {
  state: AsyncState<T>;
  loadingLabel?: string;
  emptyLabel?: string;
  emptyContent?: ReactNode;
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
  emptyContent,
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
        <p>Couldn’t load this section. Try again.</p>
        <details className="query-error-details">
          <summary>Technical details</summary>
          <pre>{state.error}</pre>
        </details>
        {onRetry && (
          <Button variant="outline" type="button"  onClick={onRetry}>
            Retry
          </Button>
        )}
      </div>
    );
  }

  if (isEmpty?.(state.data)) {
    return <>{emptyContent ?? <div className="state-panel state-panel--empty">{emptyLabel}</div>}</>;
  }

  return <>{children(state.data)}</>;
}
