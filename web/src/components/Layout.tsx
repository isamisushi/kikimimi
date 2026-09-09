import brandMark from "../assets/kikimimi-mark.svg";
import type { ReactNode } from "react";
import { Link, useRouter } from "../router/Router";
import { useSession } from "../hooks/useSession";
import { OrgSwitcher } from "./OrgSwitcher";
import { S3SnapshotBar } from "./S3SnapshotBar";

const NAV_ITEMS: { to: string; label: string; cloudOnly?: boolean }[] = [
  { to: "/overview", label: "Overview" },
  { to: "/models", label: "Models" },
  { to: "/tools", label: "Tools" },
  { to: "/mcp", label: "MCP" },
  { to: "/skills", label: "Skills" },
  { to: "/patterns", label: "Struggles" },
  { to: "/sessions", label: "Sessions" },
  { to: "/subagents", label: "Subagents" },
  { to: "/team", label: "Team", cloudOnly: true },
  { to: "/members", label: "Members", cloudOnly: true },
  { to: "/devices", label: "Devices", cloudOnly: true },
  { to: "/storage", label: "Storage & sharing" },
];

export function Layout({ children }: { children: ReactNode }) {
  const { path } = useRouter();
  const { session, logout } = useSession();

  return (
    <div className="app-shell">
      <header className="topbar">
        <div className="topbar__brand">
          <img className="brand-mark" src={brandMark} alt="" width="32" height="32" />
          <span className="brand-name">kikimimi</span>
        </div>
        {!session?.local && <OrgSwitcher />}
        <nav className="topbar__nav">
          {NAV_ITEMS.filter((item) => !session?.local || !item.cloudOnly).map((item) => (
            <Link
              key={item.to}
              to={item.to}
              className={
                "topbar__link" +
                ((path === item.to || (path === "/" && item.to === "/overview")) ? " topbar__link--active" : "")
              }
            >
              {item.label}
            </Link>
          ))}
        </nav>
        <div className="topbar__user">
          {session && !session.local && (
            <span className="topbar__email">
              {session.github_login ? `@${session.github_login}` : session.email}
            </span>
          )}
          {session && !session.local && (
            <button type="button" className="btn btn--ghost" onClick={() => void logout()}>
              Log out
            </button>
          )}
        </div>
      </header>
      <S3SnapshotBar />
      <main className="app-main">{children}</main>
    </div>
  );
}
