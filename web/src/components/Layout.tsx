import { ActivityFilters } from "./ActivityFilters";
import { useActivityFilter } from "../activityFilter";
import { t } from "../i18n";
import brandMark from "../assets/kikimimi-mark.svg";
import type { ReactNode } from "react";
import { Link, useRouter } from "../router/Router";
import { S3SnapshotBar } from "./S3SnapshotBar";
import { CollectionHealthProvider } from "./CollectionHealth";

const NAV_ITEMS = [
  { to: "/overview", label: "Overview" },
  { to: "/sessions", label: "Sessions" },
  { to: "/models", label: "Models" },
  { to: "/tools", label: "Tools" },
  { to: "/mcp", label: "MCP" },
  { to: "/skills", label: "Skills" },
  { to: "/subagents", label: "Subagents" },
  { to: "/patterns", label: "Improvements" },
];

export function Layout({ children }: { children: ReactNode }) {
  const { path } = useRouter();
  const filter = useActivityFilter();
  const showFilters = path !== "/settings" && path !== "/storage";
  const isActive = (to: string) =>
    path === to ||
    path.startsWith(to + "/") ||
    (path === "/" && to === "/overview");

  return (
    <div className="app-shell">
      <header className="topbar">
        <div className="topbar__brand">
          <img
            className="brand-mark"
            src={brandMark}
            alt=""
            width="32"
            height="32"
          />
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
              {t(item.label)}
            </Link>
          ))}
        </nav>
        <Link
          to="/settings"
          className="topbar__link topbar__settings"
          aria-current={isActive("/settings") ? "page" : undefined}
        >
          {t("Settings")}
        </Link>
      </header>
      <S3SnapshotBar />
      <main className="app-main">
        {showFilters ? <CollectionHealthProvider>
          <ActivityFilters />
          <div key={JSON.stringify(filter)}>{children}</div>
        </CollectionHealthProvider> : children}
      </main>
    </div>
  );
}
