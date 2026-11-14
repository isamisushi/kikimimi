import { useState } from "react";
import { LanguageSetting } from "../../../web/src/i18n/LanguageSetting";
import { t } from "../../../web/src/i18n";
import { Button, Card, NativeSelect, Modal } from "@kikimimi/ui";
import type { DesktopController } from "../controller";
import { OpenCodeConnection } from "./OpenCodeConnection";
export function Settings({ c }: { c: DesktopController }) {
  const [editingConnection, setEditingConnection] = useState(false);
  const cloud = c.status?.collection_target?.cloud;
  const editConnection = async () => {
    setEditingConnection(false);
    if (!(await c.beginCloudConnection())) setEditingConnection(true);
  };
  return (
    <section
      id="settings-view"
      className="page settings"
      aria-labelledby="settings-title"
      hidden={c.view !== "settings"}
    >
      <aside className="settings-sidebar" aria-label={t("Settings navigation")}>
        <p className="sidebar-label">{t("Device settings")}</p>
        <nav aria-label={t("Mac settings")}>
          <Button
            id="settings-collection"
            data-settings-page="collection"
            aria-controls="settings-panel-collection"
            variant="default"
            onClick={() => {
              c.settingsPage = "collection";
              c.emit();
            }}
            aria-current={c.settingsPage === "collection" ? "page" : undefined}
            disabled={c.settingsPage === "collection"}
          >
            <svg
              className="sidebar-icon"
              viewBox="0 0 24 24"
              aria-hidden="true"
            >
              <path d="M4 7h16M4 12h16M4 17h16" />
              <circle cx="8" cy="7" r="2" />
              <circle cx="16" cy="12" r="2" />
              <circle cx="10" cy="17" r="2" />
            </svg>
            {t("Collection & sync")}
          </Button>
          <Button
            id="settings-opencode"
            data-settings-page="opencode"
            aria-controls="settings-panel-opencode"
            variant="default"
            onClick={() => {
              c.settingsPage = "opencode";
              c.emit();
            }}
            aria-current={c.settingsPage === "opencode" ? "page" : undefined}
            disabled={c.settingsPage === "opencode"}
          >
            {t("OpenCode connection")}
          </Button>
          <Button
            id="settings-general"
            data-settings-page="general"
            aria-controls="settings-panel-general"
            variant="default"
            onClick={() => {
              c.settingsPage = "general";
              c.emit();
            }}
            aria-current={c.settingsPage === "general" ? "page" : undefined}
            disabled={c.settingsPage === "general"}
          >
            <svg
              className="sidebar-icon"
              viewBox="0 0 24 24"
              aria-hidden="true"
            >
              <circle cx="12" cy="12" r="8" />
              <path d="M12 7v5l3 2" />
            </svg>
            {t("General")}
          </Button>
        </nav>
        <p className="sidebar-label">{t("Advanced")}</p>
        <nav aria-label={t("Advanced settings")}>
          <Button
            id="settings-workspace"
            type="button"
            variant="default"
            onClick={() => {
              void c.navigate("workspace");
            }}
          >
            {t("Data sources & S3")} <span aria-hidden="true">›</span>
          </Button>
        </nav>
        <p className="sidebar-help">
          {t("These settings apply to this Mac. Your activity can include all your devices. Team administration is in Cloud.")}
        </p>
      </aside>
      <div className="settings-content">
        <div className="page-heading">
          <h1 id="settings-title">
            {
              {
                collection: t("Collection & sync"),
                opencode: t("OpenCode connection"),
                general: t("General"),
              }[c.settingsPage]
            }
          </h1>
          <span id="version" className="version">
            {c.updates.current_version ? "v" + c.updates.current_version : ""}
          </span>
        </div>
        <p id="settings-description" className="settings-description">
          {
            {
              collection: t(
                "Manage local collection and Cloud sync.",
              ),
              opencode: t(
                "Choose the default model provider for OpenCode on this Mac.",
              ),
              general: t("Startup and app updates."),
            }[c.settingsPage]
          }
        </p>
        <div
          id="settings-panel-collection"
          data-settings-panel="collection"
          hidden={c.settingsPage !== "collection"}
        >
          <Card className="settings-card collection-settings-card" aria-labelledby="connection-title">
            <div className="card-heading">
              <div>
                <h2 id="connection-title">{t("Collection on this Mac")}</h2>
                <p>{t("Save activity from connected agents to local history.")}</p>
              </div>
              <span className="badge" role="status">{c.collectorStatusLabel}</span>
            </div>
            <div className="setting-row">
              <div>
                <strong>{t("Agent connection")}</strong>
                <p>{t("Claude, Claude Code, Claude Cowork, Codex, and ChatGPT Cowork.")}</p>
              </div>
              {!c.status?.service_installed ? (
                <Button id="setup" variant="outline" disabled={c.collectionBlocked}
                  onClick={() => void c.navigate("onboarding")}>{t("Set up collection")}</Button>
              ) : c.status?.running ? (
                <Button id="disconnect" variant="outline" disabled={c.collectionBlocked}
                  onClick={() => { c.disconnectDialog = "disconnect"; c.emit(); }}>{t("Disconnect…")}</Button>
              ) : (
                <Button disabled={c.collectionBlocked} onClick={() => void c.collection("resume")}>{t("Resume collection")}</Button>
              )}
            </div>
            {c.status?.codex_collection && c.status.codex_collection !== "absent" && (
              <div className="setting-row">
                <div>
                  <strong>{t("Codex usage")}</strong>
                  <p id="codex-collection-detail">
                    {t(
                      c.status.codex_collection === "otel"
                        ? "Sent by Codex as it happens. Session records fill in anything missed. Restart Codex after setup."
                        : c.status.codex_collection === "own_otel"
                          ? "Codex already sends OpenTelemetry elsewhere, so kikimimi reads session records only."
                          : "Read from session records. Set up real-time collection to receive it from Codex as it happens.",
                    )}
                  </p>
                </div>
                {c.status.codex_collection === "rollout" && c.status.service_installed ? (
                  // Resuming writes Codex's [otel] and is a no-op for a running collector.
                  <Button id="codex-realtime" variant="outline" disabled={c.collectionBlocked}
                    onClick={() => void c.collection("resume")}>{t("Set up real time")}</Button>
                ) : (
                  <span id="codex-collection-status" className="muted">
                    {t(c.status.codex_collection === "otel" ? "Real time" : "Session records")}
                  </span>
                )}
              </div>
            )}
            <details className="collection-storage-details">
              <summary>{t("How collection works")}</summary>
              <p>{t("Codex CLI, Codex Desktop, and ChatGPT Cowork on this Mac: Codex sends model names and measured tokens to kikimimi as it happens, and local session records fill in anything missed. No plugin is required.")}</p>
              <p>{t("Claude chat: requires the kikimimi plugin for measured usage. Claude Code and Claude Cowork use their existing collection connections.")}</p>
              <p>{t("Ordinary ChatGPT Chat and cloud-only Work are not supported by this local collector.")}</p>
            </details>
            <div className="setting-row">
              <div><strong>{t("Saved history")}</strong><p>{t("Available even when collection or Cloud sync is off.")}</p></div>
              <Button id="history" variant="outline" onClick={() => void c.openLocal()}>{t("View history")}</Button>
            </div>
          </Card>
          <Card className="settings-card collection-settings-card" aria-labelledby="cloud-sync-title">
            <div className="card-heading">
              <div>
                <h2 id="cloud-sync-title">{t("Cloud sync")}</h2>
                <p>{t("Keep your activity in sync across your own devices.")}</p>
              </div>
              {cloud ? (
                <Button id="cloud-sync-toggle" role="switch" aria-label={t("Cloud sync")}
                  aria-checked={c.cloudSyncEnabled} variant={c.cloudSyncEnabled ? "default" : "outline"}
                  disabled={c.collectionBlocked || c.statusError || c.pending}
                  onClick={() => void c.toggleCloudSync()}>{t(c.cloudSyncEnabled ? "On" : "Off")}</Button>
              ) : (
                <Button id="cloud-sign-in" disabled={c.collectionBlocked || c.workspacesBusy}
                  onClick={editConnection}>{t("Sign in to Cloud")}</Button>
              )}
            </div>
            {cloud && <>
              <div className="setting-row">
                <div><strong>{t("Workspace")}</strong><p>{cloud.org_slug}</p></div>
                <Button id="change-cloud-workspace" variant="outline" disabled={c.collectionBlocked || c.pending || c.workspacesBusy}
                  onClick={editConnection}>{t("Change workspace")}</Button>
              </div>
              <div className="setting-row">
                <strong>{t("Sync status")}</strong><span role="status">{c.cloudSyncStatus}</span>
              </div>
            </>}
            {(c.workspacesBusy || c.collectionSwitch) && <div className="card-body" role="status">
              {t(c.workspacesBusy ? "Signing in…" : "Connecting…")}
            </div>}
            {editingConnection && (
              <Modal open={editingConnection} onClose={() => { if (!c.collectionBlocked && !c.workspacesBusy) setEditingConnection(false); }} aria-labelledby="cloud-picker-title">
              <div className="cloud-connection-editor">
                <h2 id="cloud-picker-title">{t(c.workspacesBusy ? "Signing in…" : c.collectionSwitch ? "Connecting…" : "Choose a workspace")}</h2>
                <label htmlFor="collection-cloud-slug">{t("Choose a workspace from your account")}</label>
                <NativeSelect id="collection-cloud-slug" value={c.cloudSlug}
                  disabled={c.collectionBlocked || c.workspacesBusy}
                  onChange={(e) => {
                    const org = c.cloudWorkspaces.find((o) => o.slug === e.target.value);
                    c.cloudSlug = org?.slug ?? ""; c.cloudKind = org?.kind ?? "team"; c.emit();
                  }}>
                  <option value="">{t("Choose a workspace")}</option>
                  {c.cloudWorkspaces.map((o) => <option key={o.slug} value={o.slug}>
                    {o.name || o.slug} · {t(o.kind === "team" ? "Team" : "Personal")}
                  </option>)}
                </NativeSelect>
                {c.workspacesBusy && <p role="status">{t("Loading workspaces / waiting for sign-in…")}</p>}
                {c.workspacesError && <p role="alert">{t(c.workspacesError)}</p>}
                <div className="cloud-connection-actions">
                  <Button id="review-collection" disabled={c.collectionBlocked || c.workspacesBusy || !c.cloudSlug}
                    onClick={async () => { if (await c.connectCloudWorkspace()) setEditingConnection(false); }}>{t("Connect and sync")}</Button>
                  <Button variant="outline" disabled={c.workspacesBusy || c.collectionBlocked}
                    onClick={editConnection}>{t("Refresh workspaces")}</Button>
                  <Button variant="ghost" disabled={c.collectionBlocked || c.workspacesBusy} onClick={() => setEditingConnection(false)}>{t("Cancel")}</Button>
                </div>
                <p>{t("Choose where to sync your activity. Sync starts when you connect.")}</p>
              </div>
              </Modal>
            )}
            <div className="card-body sync-explanation">
              <p>{cloud
                ? t("Turning sync off keeps this connection and your local history.")
                : t("Signing in enables Cloud sync automatically. Choose a workspace only if you belong to more than one.")}</p>
              <p>{t("Only owners and administrators can view team activity in Cloud.")}</p>
            </div>
          </Card>
          <details className="collection-storage-details">
            <summary>{t("Local storage & advanced exports")}</summary>
            <p>{c.storage?.local_path || t("Loading…")}</p>
            <Button variant="outline" onClick={() => void c.navigate("workspace")}>{t("Manage data sources & S3")}</Button>
          </details>
        </div>
        <div
          id="settings-panel-opencode"
          data-settings-panel="opencode"
          hidden={c.settingsPage !== "opencode"}
        >
          <OpenCodeConnection c={c} />
        </div>
        <div
          id="settings-panel-general"
          data-settings-panel="general"
          hidden={c.settingsPage !== "general"}
        >
          <Card className="settings-card" aria-label={t("Language")}>
            <LanguageSetting />
          </Card>
          <Card className="settings-card" aria-labelledby="startup-title">
            <div className="card-heading">
              <h2 id="startup-title">{t("Startup")}</h2>
            </div>
            <div className="setting-row">
              <div>
                <strong>{t("Start at login")}</strong>
                <p id="login-detail">
                  {c.status?.service_installed
                    ? t("Collection starts automatically when you log in.")
                    : t("Enable collection to start it at login.")}
                </p>
              </div>
              <span id="login-status" className="muted">
                {c.status?.service_installed ? t("On") : t("Off")}
              </span>
            </div>
          </Card>
          <Card className="settings-card">
            <div className="setting-row">
              <div>
                <strong>{t("Collection setup")}</strong>
                <p>{t("Return to first-time setup and keep your local activity history.")}</p>
              </div>
              <Button variant="outline" disabled={c.collectionBlocked}
                onClick={() => { c.disconnectDialog = "reset_setup"; c.emit(); }}>
                {t("Reset collection settings")}
              </Button>
            </div>
          </Card>
          <Card className="settings-card" aria-labelledby="updates-title">
            <div className="card-heading">
              <div>
                <h2 id="updates-title">{t("App updates")}</h2>
                <p id="update-status" role="status">
                  {!c.updates.configured
                    ? t("Updates are unavailable in this build.")
                    : c.updates.phase === "error"
                      ? c.updates.error_kind === "install"
                        ? t("Could not install the update.")
                        : t("Could not check for updates.")
                      : c.updates.phase === "idle"
                        ? t("Check for updates")
                        : c.updates.phase === "current"
                          ? t("You’re up to date.")
                          : c.updates.phase === "available"
                            ? t("An update is available.")
                            : t(c.updates.phase) + "…"}
                </p>
              </div>
              <Button
                id="check-updates"
                className="secondary small"
                variant="outline"
                size="sm"
                disabled={
                  !c.updates.configured || c.updateBusy || c.collectionBlocked
                }
                onClick={() => {
                  void c.update("check_updates");
                }}
              >
                {t("Check for updates")}
              </Button>
            </div>
            <div className="setting-row">
              <label className="switch-label" htmlFor="auto-update">
                <strong>{t("Install updates automatically")}</strong>
                <span>
                  {t(
                    "Download, install, and restart kikimimi when an update is ready.",
                  )}
                </span>
              </label>
              <input
                id="auto-update"
                type="checkbox"
                role="switch"
                disabled={
                  !c.updates.configured || c.updateBusy || c.collectionBlocked
                }
                checked={Boolean(c.updates.auto_update)}
                onChange={(e) =>
                  void c.update("set_auto_update", {
                    enabled: e.target.checked,
                  })
                }
              />
            </div>
            <div
              id="available-update"
              className="setting-row"
              hidden={!c.updates.available_version}
            >
              <p id="update-description">
                {t("Version ") + c.updates.available_version + t(" is ready.")}
              </p>
              <Button
                id="install-update"
                className="small"
                variant="default"
                size="sm"
                onClick={() => {
                  void c.update("install_update");
                }}
                disabled={c.updateBusy || c.collectionBlocked}
              >
                {t("Update and restart")}
              </Button>
            </div>
            <details
              id="update-error-details"
              className="diagnostics"
              hidden={!c.updates.error}
            >
              <summary>{t("Technical details")}</summary>
              <p id="update-error">{c.updates.error}</p>
            </details>
          </Card>
        </div>
        <p className="footnote settings-shortcut">
          {t("Settings")} <kbd>⌘</kbd> <kbd>,</kbd>
        </p>
      </div>
    </section>
  );
}
