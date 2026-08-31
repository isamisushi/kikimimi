import type { ReactNode } from "react";
import { Link, useRouter } from "../router/Router";
import { useSession } from "../hooks/useSession";

const NAV_ITEMS: { to: string; label: string }[] = [
  { to: "/", label: "Overview" },
  { to: "/tools", label: "Tools" },
  { to: "/mcp", label: "MCP" },
  { to: "/sessions", label: "Sessions" },
];

export function Layout({ children }: { children: ReactNode }) {
  const { path } = useRouter();
  const { session, logout } = useSession();

  return (
    <div className="app-shell">
      <header className="topbar">
        <div className="topbar__brand">
          <span className="brand-mark" aria-hidden="true">
            G
          </span>
          <span className="brand-name">guru</span>
        </div>
        <nav className="topbar__nav">
          {NAV_ITEMS.map((item) => (
            <Link
              key={item.to}
              to={item.to}
              className={
                "topbar__link" +
                (path === item.to ? " topbar__link--active" : "")
              }
            >
              {item.label}
            </Link>
          ))}
        </nav>
        <div className="topbar__user">
          {session && <span className="topbar__email">{session.email}</span>}
          <button type="button" className="btn btn--ghost" onClick={() => void logout()}>
            Log out
          </button>
        </div>
      </header>
      <main className="app-main">{children}</main>
    </div>
  );
}
