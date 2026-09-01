import type { ReactNode } from "react";
import { Link, useRouter } from "../router/Router";
import { useSession } from "../hooks/useSession";
import { OrgSwitcher } from "./OrgSwitcher";

const NAV_ITEMS: { to: string; label: string }[] = [
  { to: "/", label: "Overview" },
  { to: "/tools", label: "Tools" },
  { to: "/mcp", label: "MCP" },
  { to: "/skills", label: "Skills" },
  { to: "/sessions", label: "Sessions" },
  { to: "/team", label: "Team" },
  { to: "/members", label: "Members" },
  { to: "/devices", label: "Devices" },
];

export function Layout({ children }: { children: ReactNode }) {
  const { path } = useRouter();
  const { session, logout } = useSession();

  return (
    <div className="app-shell">
      <header className="topbar">
        <div className="topbar__brand">
          <span className="brand-mark" aria-hidden="true">
            K
          </span>
          <span className="brand-name">kikimimi</span>
        </div>
        {session && session.orgs.length > 1 && <OrgSwitcher />}
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
          {session && (
            <span className="topbar__email">
              {session.github_login ? `@${session.github_login}` : session.email}
            </span>
          )}
          <button type="button" className="btn btn--ghost" onClick={() => void logout()}>
            Log out
          </button>
        </div>
      </header>
      <main className="app-main">{children}</main>
    </div>
  );
}
