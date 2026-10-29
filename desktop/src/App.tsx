import { Confirmations } from "./views/Confirmations";
import { Workspace } from "./views/Workspace";
import { Connection } from "./views/Connection";
import { Settings } from "./views/Settings";
import { Onboarding } from "./views/Onboarding";
import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { Button, NativeSelect } from "@kikimimi/ui";
import { App as History } from "../../web/src/App";
import { DesktopController, activityText } from "./controller";
export function App() {
  const [c] = useState(() => new DesktopController());
  useSyncExternalStore(c.subscribe, c.snapshot);
  useEffect(() => c.mount(), [c]);
  const main = useRef<HTMLElement>(null);
  const scroll = useRef<Record<string, number>>({});
  useLayoutEffect(() => {
    if (main.current) main.current.scrollTop = scroll.current[c.view] || 0;
  }, [c.view]);
  return (
    <div className="desktop-shell">
      <header className="toolbar">
        <a
          href="#dashboard"
          onClick={(e) => {
            e.preventDefault();
            void c.openLocal();
          }}
          className="brand"
          aria-label="kikimimi home"
        >
          <img
            className="brand-mark"
            src="brand.svg"
            alt=""
            width="32"
            height="32"
          />
          kikimimi
        </a>

        <nav id="navigation" aria-label="App">
          <Button
            id="nav-local"
            className="nav-button"
            variant="ghost"
            disabled={
              (c.view === "dashboard" && c.reader === "local") ||
              c.sourceBusy ||
              c.collectionSwitch
            }
            aria-current={
              c.view === "dashboard" && c.reader === "local"
                ? "page"
                : undefined
            }
            onClick={() => {
              void c.openLocal();
            }}
            title={undefined}
          >
            This Mac
          </Button>
          <Button
            id="nav-dashboard"
            className="nav-button"
            variant="ghost"
            disabled={c.view === "cloud" || c.sourceBusy || c.collectionSwitch}
            aria-current={c.view === "cloud" ? "page" : undefined}
            onClick={() => {
              void c.openCloud();
            }}
          >
            Cloud
          </Button>
          <Button
            id="nav-settings"
            className="nav-button"
            variant="ghost"
            disabled={
              c.view === "settings" || c.sourceBusy || c.collectionSwitch
            }
            aria-current={
              ["settings", "workspace", "connection"].includes(c.view)
                ? "page"
                : undefined
            }
            onClick={() => {
              void c.navigate("settings");
            }}
          >
            Settings
          </Button>
        </nav>
      </header>
      <div
        id="collection-bar"
        className="collection-bar"
        role="status"
        aria-live="polite"
        hidden={
          c.view === "cloud" ||
          c.view === "workspace" ||
          c.view === "connection"
        }
      >
        <span
          id="status-dot"
          className={
            "status-dot " + (c.status?.running ? "running" : "stopped")
          }
        ></span>
        <strong id="status">{c.collectionLabel}</strong>
        <span id="status-detail" className="muted">
          {c.pending
            ? "Waiting for collector confirmation"
            : c.status?.running
              ? activityText(c.status.last_event_ts)
              : "Saved history stays available"}
        </span>
        <Button
          id="status-dismiss"
          className="text-button small"
          hidden
          variant="ghost"
          size="sm"
        >
          Dismiss
        </Button>
        <Button
          id="collection-change"
          className="small secondary"
          variant="outline"
          size="sm"
          hidden={!c.status?.running}
          onClick={() => {
            c.settingsPage = "collection";
            void c.navigate("settings");
          }}
        >
          Change collection…
        </Button>
        <Button
          id="status-action"
          className="small secondary"
          variant="outline"
          size="sm"
          hidden={!c.statusAction}
          onClick={() => {
            void c.statusActionClick();
          }}
          disabled={c.collectionBlocked}
        >
          {c.statusAction}
        </Button>
      </div>
      <div
        id="area-context"
        className="collection-bar area-context"
        hidden={!["cloud", "workspace", "connection"].includes(c.view)}
      >
        <div className="area-copy">
          <strong id="area-title">
            {c.view === "cloud" ? "Cloud" : "Data sources"}
          </strong>
          <span id="area-detail">
            {c.view === "cloud"
              ? "Your team’s activity and model routing."
              : "Viewing connections on this Mac."}
          </span>
        </div>
        <label id="workspace-control" className="workspace-control">
          Workspace{" "}
          <span
            id="workspace-collecting"
            className="collecting-mark"
            aria-label="Collecting on this Mac"
            hidden
          >
            ●
          </span>
          <NativeSelect
            id="workspace"
            aria-label="Workspace"
            value={c.workspace}
            disabled={c.sourceBusy || c.collectionSwitch}
            onChange={(e) => void c.changeWorkspace(e.target.value)}
          >
            <option value="personal">Personal</option>
            {c.orgs
              .filter((o) => o.kind === "team")
              .map((o) => (
                <option key={o.slug} value={"cloud:" + o.slug}>
                  {o.name}
                </option>
              ))}
            <option value="s3-team">Team · S3</option>
            <option value="signin">Sign in / refresh teams</option>
          </NativeSelect>
        </label>
      </div>
      <main
        ref={main}
        className="shell-main"
        onScroll={(e) => {
          scroll.current[c.view] = e.currentTarget.scrollTop;
        }}
      >
        <header id="workspace-context" className="workspace-context" hidden>
          <nav id="workspace-pages" aria-label="Workspace pages"></nav>
        </header>
        <section
          id="loading-view"
          className="centered"
          aria-label="Loading"
          hidden={c.view !== "loading"}
        >
          <span className="spinner" aria-hidden="true"></span>
          <p className="muted">Opening kikimimi…</p>
        </section>
        <Onboarding c={c} />
        <div
          className="history-host"
          hidden={c.view !== "dashboard" || c.viewerError}
        >
          {c.historyReady && (
            <History
              onNavigate={() => {
                if (main.current) main.current.scrollTop = 0;
              }}
              key={c.historyKey}
              embedded
              onStorage={() => void c.navigate("settings")}
            />
          )}
        </div>
        <section
          id="dashboard-view"
          className="centered"
          aria-labelledby="dashboard-title"
          hidden={
            c.view !== "cloud" && !(c.view === "dashboard" && c.viewerError)
          }
        >
          <span
            id="viewer-spinner"
            className="spinner"
            aria-hidden="true"
            hidden={c.viewerError}
          ></span>
          <h1 id="dashboard-title" className="compact-heading">
            {c.viewerTitle}
          </h1>
          <p id="viewer-detail" className="muted">
            {c.viewerDetail}
          </p>
          <Button
            id="retry-dashboard"
            variant="default"
            hidden={!c.viewerError}
            onClick={() => {
              void c.navigate(c.lastDashboard);
            }}
          >
            Try again
          </Button>
        </section>
        <Settings c={c} />
        <Connection c={c} />
        <Workspace c={c} />
        <div
          id="message"
          className="notice"
          role="status"
          aria-live="polite"
          hidden={!c.message}
        >
          <span id="message-text">{c.message?.text}</span>
          <Button
            id="dismiss-message"
            className="text-button"
            aria-label="Dismiss message"
            variant="ghost"
            onClick={() => {
              c.notice(null);
            }}
          >
            Dismiss
          </Button>
        </div>
      </main>
      <Confirmations c={c} />
    </div>
  );
}
