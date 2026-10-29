import { Button, Card, NativeSelect } from "@kikimimi/ui";
import type { DesktopController } from "../controller";
import { targetName } from "../controller";
export function Settings({ c }: { c: DesktopController }) {
  return (
    <section
      id="settings-view"
      className="page settings"
      aria-labelledby="settings-title"
      hidden={c.view !== "settings"}
    >
      <aside className="settings-sidebar" aria-label="Settings navigation">
        <p className="sidebar-label">This Mac</p>
        <nav aria-label="Mac settings">
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
            Collection &amp; storage
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
            Codex connection
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
            General
          </Button>
        </nav>
        <p className="sidebar-label">Advanced</p>
        <nav aria-label="Advanced settings">
          <Button
            id="settings-workspace"
            type="button"
            variant="default"
            onClick={() => {
              void c.navigate("workspace");
            }}
          >
            Data sources &amp; S3 <span aria-hidden="true">›</span>
          </Button>
        </nav>
        <span id="settings-workspace-name" hidden></span>
        <p className="sidebar-help">
          Settings for this Mac. Manage your team in Cloud.
        </p>
      </aside>
      <div className="settings-content">
        <div className="page-heading">
          <h1 id="settings-title">
            {
              {
                collection: "Collection & storage",
                codex: "Codex connection",
                general: "General",
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
              collection:
                "Manage the agents and collection destination on this Mac.",
              codex: "Choose how Codex connects to your model provider.",
              general: "Startup and app updates.",
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
                <h2 id="connection-title">Collection</h2>
                <p>Claude Code and Codex, on this Mac.</p>
              </div>
              <span id="connection-badge" className="badge">
                {c.statusError
                  ? "Unavailable"
                  : c.status?.running
                    ? "Collecting"
                    : "Stopped"}
              </span>
            </div>
            <div className="card-body source-settings">
              <strong id="collection-current">
                {"Collecting into " + targetName(c.target)}
              </strong>
              <div className="destination-row">
                <label htmlFor="collection-destination">Collect into</label>
                <div className="destination-controls">
                  <NativeSelect
                    id="collection-destination"
                    aria-label="Collection destination"
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
                    disabled={c.statusError || c.collectionBlocked}
                  >
                    Review change…
                  </Button>
                </div>
              </div>
              <p id="destination-help">
                Choose whether this Mac keeps activity locally or sends it to
                your team.
              </p>
              <details className="collection-explanation">
                <summary>What happens when I change this?</summary>
                <p>
                  Replaces this Mac’s Cloud and S3 upload destinations. Local
                  history is kept. You’ll review the destination and sharing
                  scope before applying the change.
                </p>
              </details>
              <p id="collection-error" role="alert" hidden></p>
            </div>
            <div className="setting-row">
              <div>
                <strong>Saved history</strong>
                <p>Available in the dashboard even when collection is off.</p>
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
                View history
              </Button>
            </div>
            <div className="setting-row">
              <div>
                <strong id="connection-action-title">Agent connection</strong>
                <p id="connection-detail">
                  {c.status?.running
                    ? "Both agents use this collection setting."
                    : "Collection is off. Your saved history is still available."}
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
                Set up collection
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
                Disconnect…
              </Button>
            </div>
          </Card>
          <Card className="settings-card" aria-labelledby="sharing-title">
            <div className="card-heading">
              <div>
                <h2 id="sharing-title">Storage &amp; uploads</h2>
                <p id="upload-status" role="status">
                  {c.storageError ||
                    (c.storageBusy
                      ? "Reading this Mac’s destinations…"
                      : c.storage?.cloud || c.storage?.s3
                        ? "Upload destinations configured"
                        : "This Mac only. No uploads configured.")}
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
                Refresh destinations
              </Button>
            </div>
            <div className="card-body">
              <dl
                id="upload-destinations"
                className="scope-details"
                hidden={!c.storage || Boolean(c.storageError)}
              >
                <dt>Local history</dt>
                <dd>{c.storage?.local_path}</dd>
                <dt>Cloud</dt>
                <dd>{c.storage?.cloud?.org_slug || "Not configured"}</dd>
                <dt>S3</dt>
                <dd>{c.storage?.s3?.url || "Not configured"}</dd>
              </dl>
              <p>
                Configuration only. Successful uploads are not verified here.
              </p>
              <details>
                <summary>Advanced configuration</summary>
                <p>
                  Use Collect into on this page to change this Mac’s
                  destination. Cloud sign-in for viewing alone does not enable
                  uploads. Advanced CLI configuration is also available.
                </p>
                <code>kikimimi login --org your-workspace</code>
                <p>For S3:</p>
                <code>
                  kikimimi sink add s3 s3://your-bucket/prefix --profile
                  your-profile
                </code>
                <p>
                  New uploads use the configured destination. Selecting a
                  viewing connection does not move or upload existing history.
                </p>
              </details>
            </div>
          </Card>
        </div>
        <div
          id="settings-panel-codex"
          data-settings-panel="codex"
          hidden={c.settingsPage !== "codex"}
        >
          <Card className="settings-card" aria-labelledby="codex-router-title">
            <div className="card-heading">
              <div>
                <h2 id="codex-router-title">Codex model provider</h2>
                <p>Where Codex on this Mac sends its requests.</p>
              </div>
              <span id="codex-router-badge" className="badge">
                {c.router?.enabled ? "Team router" : "Current provider"}
              </span>
            </div>
            <div className="setting-row">
              <label className="switch-label" htmlFor="codex-router">
                <strong>Use your team’s kikimimi router</strong>
                <span id="codex-router-detail">
                  {!c.router
                    ? "Checking Codex…"
                    : !c.router.codex_found
                      ? "Codex was not found on this Mac."
                      : !c.router.logged_in
                        ? "Sign in to a team in Cloud first."
                        : c.router.org_kind !== "team"
                          ? "Select a team workspace to use its router."
                          : c.router.enabled
                            ? c.router.applied
                              ? "Connected to " + c.router.org_slug
                              : "Restart Codex to apply the connection."
                            : "Use the router for " + c.router.org_slug}
                </span>
              </label>
              <input
                id="codex-router"
                type="checkbox"
                role="switch"
                disabled={
                  c.routerBusy ||
                  !c.router?.codex_found ||
                  (!c.router.enabled &&
                    (!c.router.logged_in || c.router.org_kind !== "team"))
                }
                checked={Boolean(c.router?.enabled)}
                onChange={(e) => void c.setRouter(e.target.checked)}
              />
            </div>
            <div className="card-body">
              <p className="restart-note">
                Restart Codex after changing its connection.
              </p>
              <details>
                <summary>How this changes Codex</summary>
                <p>
                  Turning this on creates a router key for this Mac and sets{" "}
                  <code>model_provider</code> in{" "}
                  <code>~/.codex/config.toml</code>. Your ChatGPT sign-in is not
                  changed. Turning it off puts back your previous provider and
                  revokes the key. Restart Codex after you change this.
                </p>
              </details>
              <p id="codex-router-error" role="alert" hidden={!c.routerError}>
                {c.routerError}
              </p>
            </div>
          </Card>
        </div>
        <div
          id="settings-panel-general"
          data-settings-panel="general"
          hidden={c.settingsPage !== "general"}
        >
          <Card className="settings-card" aria-labelledby="startup-title">
            <div className="card-heading">
              <h2 id="startup-title">Startup</h2>
            </div>
            <div className="setting-row">
              <div>
                <strong>Start at login</strong>
                <p id="login-detail">
                  {c.status?.service_installed
                    ? "Collection starts automatically when you log in."
                    : "Enable collection to start it at login."}
                </p>
              </div>
              <span id="login-status" className="muted">
                {c.status?.service_installed ? "On" : "Off"}
              </span>
            </div>
          </Card>
          <Card className="settings-card" aria-labelledby="updates-title">
            <div className="card-heading">
              <div>
                <h2 id="updates-title">App updates</h2>
                <p id="update-status" role="status">
                  {!c.updates.configured
                    ? "Updates are unavailable in this build."
                    : c.updates.phase === "error"
                      ? "Could not " +
                        (c.updates.error_kind === "install"
                          ? "install the update."
                          : "check for updates.")
                      : c.updates.phase === "idle"
                        ? "You’re up to date."
                        : c.updates.phase === "available"
                          ? "An update is available."
                          : c.updates.phase + "…"}
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
                Check for updates
              </Button>
            </div>
            <div className="setting-row">
              <label className="switch-label" htmlFor="auto-update">
                <strong>Install updates automatically</strong>
                <span>
                  Download, install, and restart kikimimi when an update is
                  ready.
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
                {"Version " + c.updates.available_version + " is ready."}
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
                Update and restart
              </Button>
            </div>
            <details
              id="update-error-details"
              className="diagnostics"
              hidden={!c.updates.error}
            >
              <summary>Technical details</summary>
              <p id="update-error">{c.updates.error}</p>
            </details>
          </Card>
        </div>
        <p className="footnote settings-shortcut">
          Settings <kbd>⌘</kbd> <kbd>,</kbd>
        </p>
      </div>
    </section>
  );
}
