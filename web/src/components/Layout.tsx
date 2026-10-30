import brandMark from "../assets/kikimimi-mark.svg";
import type { ReactNode } from "react";
import { Link, useRouter } from "../router/Router";
import { S3SnapshotBar } from "./S3SnapshotBar";

const NAV_ITEMS: { to: string; label: string }[] = [
  { to: "/overview", label: "Overview" },
  { to: "/models", label: "Models" },
  { to: "/tools", label: "Tools" },
  { to: "/mcp", label: "MCP" },
  { to: "/skills", label: "Skills" },
  { to: "/patterns", label: "Struggles" },
  { to: "/sessions", label: "Sessions" },
  { to: "/subagents", label: "Subagents" },
  { to: "/storage", label: "Storage & sharing" },
];

export function Layout({ children }: { children: ReactNode }) {
  const { path } = useRouter();
  const isActive = (to: string) => path === to || path.startsWith(to + "/") || (path === "/" && to === "/overview");

  return (
    <div className="app-shell">
      <header className="topbar">
        <div className="topbar__brand">
          <img className="brand-mark" src={brandMark} alt="" width="32" height="32" />
          <span className="brand-name">kikimimi</span>
        </div>
        <nav className="topbar__nav">
          {NAV_ITEMS.map((item) => (
            <Link
              key={item.to}
              to={item.to}
              aria-current={isActive(item.to) ? "page" : undefined}
              className={
                "topbar__link" +
                (isActive(item.to) ? " topbar__link--active" : "")
              }
            >
              {item.label}
            </Link>
          ))}
        </nav>
      </header>
      <S3SnapshotBar />
      <main className="app-main">{children}</main>
    </div>
  );
}
