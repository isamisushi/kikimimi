import { t } from "../../../web/src/i18n";
import { Button, Card, Input } from "@kikimimi/ui";
import type { DesktopController } from "../controller";

export function OpenCodeConnection({ c }: { c: DesktopController }) {
  const router = c.openCodeRouter;
  return (
    <Card
      className="settings-card opencode-connection-card"
      aria-labelledby="opencode-router-title"
    >
      <div className="card-heading">
        <div>
          <p className="connection-eyebrow">{t("GLOBAL CONNECTION")}</p>
          <h2 id="opencode-router-title">
            {router?.applied ? "kikimimi router" : "OpenCode"}
          </h2>
          <p>{router?.current_model || t("Default model")}</p>
        </div>
        <span className="badge">
          {router?.applied ? t("Router on") : t("Router off")}
        </span>
      </div>
      <div className="card-body connection-team">
        <h3>
          {router?.logged_in ? t("Authorized team") : t("Connect a team")}
        </h3>
        <p id="opencode-router-team-status">
          {router?.org_slug || t("Authorize your team in your browser.")}
        </p>
        <details open={router?.logged_in ? undefined : true}>
          <summary>
            {router?.logged_in
              ? t("Manage team connection")
              : t("Sign in and authorize")}
          </summary>
          <p>
            {t(
              "Enter the team’s workspace slug shown in Cloud, then approve access in your browser. Keep this app open while approving.",
            )}
          </p>
          <Button variant="outline" onClick={() => void c.openCloud()}>
            {t("Open Cloud in browser ↗")}
          </Button>
          {!router?.enabled ? (
            <div className="connection-team-picker">
              <label htmlFor="opencode-router-team">{t("Team")}</label>
              <div className="connection-actions">
                <Input
                  id="opencode-router-team"
                  value={c.openCodeRouterTeam}
                  disabled={c.openCodeRouterBusy}
                  placeholder="your-team"
                  autoCapitalize="off"
                  autoCorrect="off"
                  spellCheck={false}
                  onChange={(e) => {
                    c.openCodeRouterTeam = e.target.value.trim();
                    c.emit();
                  }}
                />
                <Button
                  id="opencode-router-connect"
                  disabled={c.openCodeRouterBusy || !c.openCodeRouterTeam}
                  onClick={() => void c.connectOpenCodeRouter()}
                >
                  {c.openCodeRouterBusy
                    ? t("Waiting for browser approval…")
                    : t("Authorize in browser")}
                </Button>
              </div>
            </div>
          ) : (
            <p>{t("Turn off the router before changing teams.")}</p>
          )}
          <p>{t("Connecting a team does not enable activity uploads.")}</p>
        </details>
      </div>
      <div className="setting-row connection-toggle">
        <label className="switch-label" htmlFor="opencode-router">
          <strong>{t("Use team router")}</strong>
          <span>
            {!router
              ? t("Could not read OpenCode settings.")
              : router.enabled
                ? router.applied
                  ? t("Configured for {0}", [router.org_slug])
                  : t(
                      "Settings differ from OpenCode. Restore the previous connection.",
                    )
                : router.logged_in
                  ? t("Off · Your current provider stays in use.")
                  : t("Authorize a team to enable routing.")}
          </span>
        </label>
        <input
          id="opencode-router"
          type="checkbox"
          role="switch"
          checked={Boolean(router?.enabled)}
          disabled={
            c.openCodeRouterBusy ||
            (!router?.enabled &&
              (!router?.logged_in || router.org_kind !== "team"))
          }
          onChange={(e) => void c.setOpenCodeRouter(e.target.checked)}
        />
      </div>
      <div className="card-body connection-help">
        <p>
          {t(
            "Restart OpenCode after switching. Project settings, environment overrides, or an explicitly selected model can override this global default.",
          )}
        </p>
        {router?.enabled && (
          <Button
            id="opencode-router-restore"
            variant="outline"
            disabled={c.openCodeRouterBusy}
            onClick={() => void c.setOpenCodeRouter(false)}
          >
            {t("Restore previous provider")}
          </Button>
        )}
        <details>
          <summary>{t("How switching works")}</summary>
          <p>
            {t(
              "Updates OpenCode’s global settings and keeps your previous model for restoration.",
            )}
          </p>
          <p>
            {t(
              "The router key is stored in a file readable only by your user account. Saving settings does not verify a successful model response.",
            )}
          </p>
          {router?.config_path && <p>{router.config_path}</p>}
        </details>
        <Button
          variant="outline"
          size="sm"
          disabled={c.openCodeRouterBusy}
          onClick={() => void c.refreshOpenCodeRouter()}
        >
          {t("Refresh connection")}
        </Button>
        <p
          id="opencode-router-error"
          role="alert"
          hidden={!c.openCodeRouterError}
        >
          {t(c.openCodeRouterError)}
        </p>
      </div>
    </Card>
  );
}
