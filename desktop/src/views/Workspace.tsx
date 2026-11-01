import { t } from "../../../web/src/i18n";
import { Button } from "@kikimimi/ui";
import type { DesktopController } from "../controller";
export function Workspace({ c }: { c: DesktopController }) {
  return (
    <section
      id="workspace-view"
      className="page settings workspace-page"
      aria-labelledby="workspace-heading"
      hidden={c.view !== "workspace"}
    >
      <header className="workspace-page-header">
        <Button
          variant="ghost"
          className="text-button"
          onClick={() => void c.navigate("settings")}
        >
          {t("← Settings")}
        </Button>
        <h1 id="workspace-heading">{t("Advanced data sources")}</h1>
        <p>{t("Connect an S3 export or manage saved viewing connections.")}</p>
      </header>
      <section
        className="workspace-panel"
        aria-labelledby="workspace-details-title"
      >
        <h2 id="workspace-details-title">{t("Workspace")}</h2>
        <div className="workspace-setting">
          <div>
            <strong id="workspace-name">{c.workspaceName()}</strong>
            <p id="workspace-scope">
              {c.workspace === "personal"
                ? t("Your personal viewing connection.")
                : t("A shared viewing connection.")}
            </p>
          </div>
          <Button
            id="workspace-signin"
            className="secondary"
            variant="outline"
            onClick={() => {
              void c.openCloud();
            }}
          >
            {t("Open Cloud in browser ↗")}
          </Button>
        </div>
        <p id="workspace-description" className="workspace-help">
          {t(
            "Viewing a workspace does not change where this Mac collects activity.",
          )}
        </p>
        <p className="workspace-help">{t("Settings are saved on this Mac.")}</p>
      </section>
      <section
        className="workspace-panel"
        aria-labelledby="workspace-source-title"
      >
        <h2 id="workspace-source-title">{t("Data source")}</h2>
        <div className="workspace-setting">
          <div>
            <strong id="source-label">
              {{ local: t("This Mac"), s3: "S3" }[c.preference().kind]}
            </strong>
            <p id="workspace-connection">
              {c.preference().url || t("Choose the history you want to view.")}
            </p>
          </div>
          <Button
            id="workspace-connection-settings"
            className="secondary"
            variant="outline"
            onClick={() => {
              void c.navigate("connection");
            }}
          >
            {t("Edit data source")}
          </Button>
        </div>
        <p className="workspace-help">
          {t(
            "Used by this Mac to read workspace activity. Changing it does not change collection or uploads.",
          )}
        </p>
      </section>
      <section
        className="workspace-panel"
        aria-labelledby="workspace-collection-title"
      >
        <h2 id="workspace-collection-title">{t("Collection & uploads")}</h2>
        <div className="workspace-setting">
          <p>
            {t("Manage this Mac’s agents and upload destination in Settings.")}
          </p>
          <Button
            id="workspace-app"
            className="secondary"
            variant="outline"
            onClick={() => {
              void c.navigate("settings");
            }}
          >
            {t("Open Settings")}
          </Button>
        </div>
      </section>
    </section>
  );
}
