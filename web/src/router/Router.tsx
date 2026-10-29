import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";

interface RouterState {
  path: string;
  navigate: (to: string, opts?: { replace?: boolean }) => void;
}

const RouterContext = createContext<RouterState | null>(null);

export function RouterProvider({
  children,
  memory = false,
  onStorage,
  onNavigate,
}: {
  children: ReactNode;
  memory?: boolean;
  onStorage?: () => void;
  onNavigate?: () => void;
}) {
  const [path, setPath] = useState(() =>
    memory ? "/overview" : window.location.pathname,
  );

  useEffect(() => {
    if (memory) return;
    const onPopState = () => setPath(window.location.pathname);
    window.addEventListener("popstate", onPopState);
    return () => window.removeEventListener("popstate", onPopState);
  }, [memory]);

  const navigate = useCallback(
    (to: string, opts?: { replace?: boolean }) => {
      if (to === "/storage" && onStorage) {
        onStorage();
        return;
      }
      onNavigate?.();
      if (memory) {
        setPath(to.split("?")[0]);
        return;
      }
      if (opts?.replace) {
        window.history.replaceState(null, "", to);
      } else {
        window.history.pushState(null, "", to);
      }
      setPath(to.split("?")[0]);
    },
    [memory, onStorage, onNavigate],
  );

  const value = useMemo(() => ({ path, navigate }), [path, navigate]);

  return (
    <RouterContext.Provider value={value}>{children}</RouterContext.Provider>
  );
}

export function useRouter(): RouterState {
  const ctx = useContext(RouterContext);
  if (!ctx) throw new Error("useRouter must be used within RouterProvider");
  return ctx;
}

export function Link({
  to,
  className,
  title,
  children,
  onClick,
}: {
  to: string;
  className?: string;
  title?: string;
  children: ReactNode;
  onClick?: () => void;
}) {
  const { navigate } = useRouter();
  return (
    <a
      href={to}
      className={className}
      title={title}
      onClick={(e) => {
        // Allow modified clicks (new tab, etc.) to behave natively.
        if (
          e.defaultPrevented ||
          e.button !== 0 ||
          e.metaKey ||
          e.ctrlKey ||
          e.shiftKey ||
          e.altKey
        ) {
          return;
        }
        e.preventDefault();
        onClick?.();
        navigate(to);
      }}
    >
      {children}
    </a>
  );
}
