import { useLocale } from "../../web/src/i18n";
import { t } from "../../web/src/i18n";
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
  const locale = useLocale();
  useEffect(() => {
    void window.__TAURI__.core
      .invoke("set_ui_language", { locale })
      .catch(console.error);
  }, [locale]);
  const [c] = useState(() => new DesktopController());
  useSyncExternalStore(c.subscribe, c.snapshot);
  useEffect(() => c.mount(), [c]);
  const main = useRef<HTMLElement>(null);
  const scroll = useRef<Record<string, number>>({});
  const scrollKey =
    c.view === "settings" ? `settings:${c.settingsPage}` : c.view;
  useLayoutEffect(() => {
    if (main.current) main.current.scrollTop = scroll.current[scrollKey] || 0;
  }, [scrollKey]);
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
          aria-label={t("kikimimi home")}
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

        <nav id="navigation" aria-label={t("App")}>
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
            {t("Your activity")}
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
            {t("Settings")}
          </Button>
        </nav>
        <Button
          id="nav-dashboard"
          className="cloud-link"
          variant="ghost"
          aria-label={t("Open Cloud in browser")}
          onClick={() => {
            void c.openCloud();
          }}
        >
          Cloud ↗
        </Button>
      </header>
      <div
        id="collection-bar"
        className="collection-bar"
        role="status"
        aria-live="polite"
        hidden={c.view === "workspace" || c.view === "connection"}
      >
        <span
          id="status-dot"
          className={
            "status-dot " + (c.collectorHealthy ? "running" : "stopped")
          }
        ></span>
        <strong id="status">{c.collectionLabel}</strong>
        <span id="status-detail" className="muted">
          {c.pending
            ? t("Waiting for collector confirmation")
            : c.status?.running
              ? activityText(c.status.last_event_ts)
              : t("Saved history stays available")}
        </span>
        <Button
          id="status-dismiss"
          className="text-button small"
          hidden
          variant="ghost"
          size="sm"
        >
          {t("Dismiss")}
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
        hidden={!["workspace", "connection"].includes(c.view)}
      >
        <div className="area-copy">
          <strong id="area-title">{t("Data sources")}</strong>
          <span id="area-detail">{t("Viewing connections on this Mac.")}</span>
        </div>
        <label id="workspace-control" className="workspace-control">
          {t("Workspace")}{" "}
          <NativeSelect
            id="workspace"
            aria-label={t("Workspace")}
            value={c.workspace}
            disabled={c.sourceBusy || c.collectionSwitch}
            onChange={(e) => void c.changeWorkspace(e.target.value)}
          >
            <option value="personal">{t("Personal")}</option>
            <option value="s3-team">{t("Team · S3")}</option>
          </NativeSelect>
        </label>
      </div>
      <main
        ref={main}
        className="shell-main"
        onScroll={(e) => {
          scroll.current[scrollKey] = e.currentTarget.scrollTop;
        }}
      >
        <section
          id="loading-view"
          className="centered"
          aria-label={t("Loading")}
          hidden={c.view !== "loading"}
        >
          <span className="spinner" aria-hidden="true"></span>
          <p className="muted">{t("Opening kikimimi…")}</p>
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
          hidden={!(c.view === "dashboard" && c.viewerError)}
        >
          <span
            id="viewer-spinner"
            className="spinner"
            aria-hidden="true"
            hidden={c.viewerError}
          ></span>
          <h1 id="dashboard-title" className="compact-heading">
            {t(c.viewerTitle)}
          </h1>
          <p id="viewer-detail" className="muted">
            {t(c.viewerDetail)}
          </p>
          <Button
            id="retry-dashboard"
            variant="default"
            hidden={!c.viewerError}
            onClick={() => {
              void c.navigate("dashboard");
            }}
          >
            {t("Try again")}
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
          <span id="message-text">{c.message ? t(c.message.text) : ""}</span>
          <Button
            id="dismiss-message"
            className="text-button"
            aria-label={t("Dismiss message")}
            variant="ghost"
            onClick={() => {
              c.notice(null);
            }}
          >
            {t("Dismiss")}
          </Button>
        </div>
      </main>
      <Confirmations c={c} />
    </div>
  );
}
