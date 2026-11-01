import { LanguageSetting } from "../../../web/src/i18n/LanguageSetting";
import { t } from "../../../web/src/i18n";
import { Button, Card, Input, NativeSelect } from "@kikimimi/ui";
import type { DesktopController } from "../controller";
import { OpenCodeConnection } from "./OpenCodeConnection";
import { targetName } from "../controller";
export function Settings({ c }: { c: DesktopController }) {
  return (
    <section
      id="settings-view"
      className="page settings"
      aria-labelledby="settings-title"
      hidden={c.view !== "settings"}
    >
      <aside className="settings-sidebar" aria-label={t("Settings navigation")}>
        <p className="sidebar-label">{t("This Mac")}</p>
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
            {t("Collection & storage")}
          </Button>
          <Button
            id="settings-codex"
            data-settings-page="codex"
            aria-controls="settings-panel-codex"
            variant="default"
            onClick={() => {
              c.settingsPage = "codex";
              c.emit();
            }}
            aria-current={c.settingsPage === "codex" ? "page" : undefined}
            disabled={c.settingsPage === "codex"}
          >
            <svg
              className="sidebar-icon"
              viewBox="0 0 24 24"
              aria-hidden="true"
            >
              <path d="m8 6-6 6 6 6m8-12 6 6-6 6m-3-15-2 18" />
            </svg>
            {t("Codex connection")}
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
          {t("Settings for this Mac. Manage your team in Cloud.")}
        </p>
      </aside>
      <div className="settings-content">
        <div className="page-heading">
          <h1 id="settings-title">
            {
              {
                collection: t("Collection & storage"),
                codex: t("Codex connection"),
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
                "Manage the agents and collection destination on this Mac.",
              ),
              codex: t("Choose how Codex connects to your model provider."),
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
          <Card className="settings-card" aria-labelledby="connection-title">
            <div className="card-heading">
              <div>
                <h2 id="connection-title">{t("Collection")}</h2>
                <p>{t("Claude Code, Cowork, and Codex, on this Mac.")}</p>
              </div>
            </div>
            <div className="card-body source-settings">
              <strong id="collection-current">
                {t("Collecting into ") + targetName(c.target)}
              </strong>
              <div className="destination-row">
                <label htmlFor="collection-destination">
                  {t("Collect into")}
                </label>
                <div className="destination-controls">
                  <NativeSelect
                    id="collection-destination"
                    aria-label={t("Collection destination")}
                    aria-describedby="destination-help"
                    value={c.destination}
                    disabled={c.statusError || c.collectionBlocked}
                    onChange={(e) => {
                      c.destination = e.target.value;
                      c.emit();
                    }}
                  >
                    {c.choices().map((o) => (
                      <option value={o.id} key={o.id}>
                        {o.label}
                      </option>
                    ))}
                  </NativeSelect>
                  <Button
                    id="review-collection"
                    type="button"
                    className="small secondary"
                    variant="outline"
                    size="sm"
                    onClick={() => {
                      c.reviewCollection();
                    }}
                    disabled={
                      c.statusError ||
                      c.collectionBlocked ||
                      (c.destination === "cloud" && !c.cloudSlug.trim())
                    }
                  >
                    {t("Review change…")}
                  </Button>
                </div>
              </div>
              {c.destination === "cloud" && (
                <div className="source-settings">
                  <label htmlFor="collection-cloud-slug">
                    {t("Workspace slug")}
                  </label>
                  <Input
                    id="collection-cloud-slug"
                    value={c.cloudSlug}
                    placeholder="your-workspace"
                    disabled={c.collectionBlocked}
                    autoCapitalize="off"
                    autoCorrect="off"
                    spellCheck={false}
                    onChange={(e) => {
                      c.cloudSlug = e.target.value;
                      c.emit();
                    }}
                  />
                  <label htmlFor="collection-cloud-kind">
                    {t("Workspace type")}
                  </label>
                  <NativeSelect
                    id="collection-cloud-kind"
                    value={c.cloudKind}
                    disabled={c.collectionBlocked}
                    onChange={(e) => {
                      c.cloudKind = e.target.value as "personal" | "team";
                      c.emit();
                    }}
                  >
                    <option value="team">
                      {t("Team · selected repositories")}
                    </option>
                    <option value="personal">
                      {t("Personal · all repositories")}
                    </option>
                  </NativeSelect>
                  <p>
                    {t(
                      "Use the slug shown in Cloud. After reviewing the sharing scope, approve this workspace in your browser. Keep this app open while approving.",
                    )}
                  </p>
                </div>
              )}
              <p id="destination-help">
                {t(
                  "Choose whether this Mac keeps activity locally or sends it to your team.",
                )}
              </p>
              <details className="collection-explanation">
                <summary>{t("What happens when I change this?")}</summary>
                <p>
                  {t(
                    "Replaces this Mac’s Cloud and S3 upload destinations. Local history is kept. You’ll review the destination and sharing scope before applying the change.",
                  )}
                </p>
              </details>
            </div>
            <div className="setting-row">
              <div>
                <strong>{t("Saved history")}</strong>
                <p>
                  {t("Available in the dashboard even when collection is off.")}
                </p>
              </div>
              <Button
                id="history"
                className="secondary small"
                variant="outline"
                size="sm"
                onClick={() => {
                  void c.openLocal();
                }}
              >
                {t("View history")}
              </Button>
            </div>
            <div className="setting-row">
              <div>
                <strong id="connection-action-title">
                  {t("Agent connection")}
                </strong>
                <p id="connection-detail">
                  {c.status?.running
                    ? t("All supported agents use this collection setting.")
                    : t(
                        "Collection is off. Your saved history is still available.",
                      )}
                </p>
              </div>
              <Button
                id="setup"
                disabled={c.collectionBlocked}
                className="secondary small"
                variant="outline"
                size="sm"
                onClick={() => {
                  void c.navigate("onboarding");
                }}
                hidden={Boolean(c.status?.service_installed)}
              >
                {t("Set up collection")}
              </Button>
              <Button
                id="disconnect"
                className="danger-text small"
                variant="ghost"
                size="sm"
                hidden={!c.status?.service_installed}
                onClick={() => {
                  c.disconnectOpen = true;
                  c.emit();
                }}
                disabled={c.collectionBlocked}
              >
                {t("Disconnect…")}
              </Button>
            </div>
          </Card>
          <Card className="settings-card" aria-labelledby="sharing-title">
            <div className="card-heading">
              <div>
                <h2 id="sharing-title">{t("Storage & uploads")}</h2>
                <p id="upload-status" role="status">
                  {(c.storageError && t(c.storageError)) ||
                    (c.storageBusy
                      ? t("Reading this Mac’s destinations…")
                      : c.storage?.cloud || c.storage?.s3
                        ? t("Upload destinations configured")
                        : t("This Mac only. No uploads configured."))}
                </p>
              </div>
              <Button
                id="refresh-storage"
                type="button"
                className="secondary small"
                variant="outline"
                size="sm"
                onClick={() => {
                  void c.refreshStorage();
                }}
                disabled={c.storageBusy}
              >
                {t("Refresh destinations")}
              </Button>
            </div>
            <div className="card-body">
              <dl
                id="upload-destinations"
                className="scope-details"
                hidden={!c.storage || Boolean(c.storageError)}
              >
                <dt>{t("Local history")}</dt>
                <dd>{c.storage?.local_path}</dd>
                <dt>Cloud</dt>
                <dd>{c.storage?.cloud?.org_slug || t("Not configured")}</dd>
                <dt>S3</dt>
                <dd>{c.storage?.s3?.url || t("Not configured")}</dd>
              </dl>
              <p>
                {t(
                  "Configuration only. Successful uploads are not verified here.",
                )}
              </p>
              <details>
                <summary>{t("Advanced configuration")}</summary>
                <p>
                  {t(
                    "Use Collect into on this page to change this Mac’s destination. Cloud sign-in for viewing alone does not enable uploads. Advanced CLI configuration is also available.",
                  )}
                </p>
                <code>kikimimi login --org your-workspace</code>
                <p>{t("For S3:")}</p>
                <code>
                  kikimimi sink add s3 s3://your-bucket/prefix --profile
                  your-profile
                </code>
                <p>
                  {t(
                    "New uploads use the configured destination. Selecting a viewing connection does not move or upload existing history.",
                  )}
                </p>
              </details>
            </div>
          </Card>
        </div>
        <div
          id="settings-panel-opencode"
          data-settings-panel="opencode"
          hidden={c.settingsPage !== "opencode"}
        >
          <OpenCodeConnection c={c} />
        </div>
        <div
          id="settings-panel-codex"
          data-settings-panel="codex"
          hidden={c.settingsPage !== "codex"}
        >
          <Card
            className="settings-card codex-connection-card"
            aria-labelledby="codex-router-title"
          >
            <div className="card-heading">
              <div>
                <p className="connection-eyebrow">{t("CURRENT CONNECTION")}</p>
                <h2 id="codex-router-title">
                  {c.router?.applied
                    ? "kikimimi router"
                    : c.router?.current_provider === "openai"
                      ? "OpenAI"
                      : c.router?.current_provider_name ||
                        c.router?.current_provider ||
                        t("Connection unavailable")}
                </h2>
                <p>{c.router?.current_model || t("Model not specified")}</p>
              </div>
              <span id="codex-router-badge" className="badge">
                {c.router?.applied ? t("Router on") : t("Router off")}
              </span>
            </div>
            <div className="card-body connection-team">
              <div className="connection-team-summary">
                <div>
                  <h3>
                    {c.router?.logged_in
                      ? t("Authorized team")
                      : t("Connect a team")}
                  </h3>
                  <p id="codex-router-team-status">
                    {c.router?.logged_in
                      ? c.router.org_slug
                      : t("Authorize your team in your browser.")}
                  </p>
                </div>
                {c.router?.logged_in && (
                  <span className="badge">{t("Authorized")}</span>
                )}
              </div>
              <details
                className="connection-team-options"
                open={c.router?.logged_in ? undefined : true}
              >
                <summary>
                  {c.router?.logged_in
                    ? t("Manage team connection")
                    : t("Sign in and authorize")}
                </summary>
                <p>
                  {t(
                    "Enter the team’s workspace slug shown in Cloud, then approve access in your browser. Keep this app open while approving.",
                  )}
                </p>
                <Button
                  id="codex-router-signin"
                  variant="outline"
                  onClick={() => void c.openCloud()}
                >
                  {t("Open Cloud in browser ↗")}
                </Button>
                {!c.router?.enabled ? (
                  <div className="connection-team-picker">
                    <label htmlFor="codex-router-team">{t("Team")}</label>
                    <div className="connection-actions">
                      <Input
                        id="codex-router-team"
                        value={c.routerTeam}
                        disabled={c.routerBusy}
                        placeholder="your-team"
                        autoCapitalize="off"
                        autoCorrect="off"
                        spellCheck={false}
                        onChange={(e) => {
                          c.routerTeam = e.target.value.trim();
                          c.emit();
                        }}
                      />
                      <Button
                        id="codex-router-connect"
                        disabled={c.routerBusy || !c.routerTeam}
                        onClick={() => void c.connectRouter()}
                      >
                        {c.routerBusy
                          ? t("Waiting for browser approval…")
                          : t("Authorize in browser")}
                      </Button>
                    </div>
                  </div>
                ) : (
                  <p>{t("Turn off the router before changing teams.")}</p>
                )}
                <p>
                  {t("Connecting a team does not enable activity uploads.")}
                </p>
              </details>
            </div>
            <div className="setting-row connection-toggle">
              <label className="switch-label" htmlFor="codex-router">
                <strong>{t("Use team router")}</strong>
                <span id="codex-router-detail">
                  {!c.router
                    ? t("Could not read Codex settings.")
                    : !c.router.codex_found
                      ? t("Codex was not found on this Mac.")
                      : c.router.enabled
                        ? c.router.applied
                          ? t("Routing through ") + c.router.org_slug
                          : t(
                              "Settings differ from Codex. Restore the previous connection.",
                            )
                        : c.router.logged_in
                          ? t("Off · Your current provider stays in use.")
                          : t("Authorize a team to enable routing.")}
                </span>
              </label>
              <input
                id="codex-router"
                type="checkbox"
                role="switch"
                disabled={
                  c.routerBusy ||
                  (!c.router?.enabled &&
                    (!c.router?.codex_found ||
                      !c.router?.logged_in ||
                      c.router?.org_kind !== "team"))
                }
                checked={Boolean(c.router?.enabled)}
                onChange={(e) => void c.setRouter(e.target.checked)}
              />
            </div>
            <div className="card-body connection-help">
              <p>{t("Changes apply to new Codex chats. No restart needed.")}</p>
              {c.router?.enabled && (
                <Button
                  id="codex-router-restore"
                  variant="outline"
                  disabled={c.routerBusy}
                  onClick={() => void c.setRouter(false)}
                >
                  {t("Restore previous provider")}
                </Button>
              )}
              <details>
                <summary>{t("How switching works")}</summary>
                <p>
                  {t(
                    "Existing chats may keep their previous connection. Turning the router off restores your previous provider. Your ChatGPT sign-in stays unchanged.",
                  )}
                </p>
                <p>
                  {t(
                    "Saving settings does not verify that the router can respond. You can restore your previous provider even when signed out of Cloud.",
                  )}
                </p>
              </details>
              <p id="codex-router-error" role="alert" hidden={!c.routerError}>
                {t(c.routerError)}
              </p>
            </div>
          </Card>
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
          <Card className="settings-card" aria-labelledby="updates-title">
            <div className="card-heading">
              <div>
                <h2 id="updates-title">{t("App updates")}</h2>
                <p id="update-status" role="status">
                  {!c.updates.configured
                    ? t("Updates are unavailable in this build.")
                    : c.updates.phase === "error"
                      ? c.updates.error_kind === "install"
                        ? t("install the update.")
                        : t("check for updates.")
                      : c.updates.phase === "idle"
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
