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
        <Button variant="ghost" className="text-button" onClick={() => void c.navigate("settings")}>
          ← Settings
        </Button>
        <h1 id="workspace-heading">Advanced data sources</h1>
        <p>Connect an S3 export or manage saved viewing connections.</p>
      </header>
      <section
        className="workspace-panel"
        aria-labelledby="workspace-details-title"
      >
        <h2 id="workspace-details-title">Workspace</h2>
        <div className="workspace-setting">
          <div>
            <strong id="workspace-name">{c.workspaceName()}</strong>
            <p id="workspace-scope">
              {c.workspace === "personal"
                ? "Your personal viewing connection."
                : "A shared viewing connection."}
            </p>
          </div>
          <Button
            id="workspace-signin"
            className="secondary"
            variant="outline"
            onClick={() => {
              void c.changeWorkspace("signin");
            }}
          >
            Sign in / refresh teams
          </Button>
        </div>
        <p id="workspace-description" className="workspace-help">
          Viewing a workspace does not change where this Mac collects activity.
        </p>
        <div className="workspace-setting workspace-access-row">
          <p id="workspace-access">
            {c.workspace.startsWith("cloud:")
              ? "Manage membership and sharing in Cloud."
              : "Settings are saved on this Mac."}
          </p>
          <Button
            id="manage-team"
            className="secondary"
            variant="outline"
            hidden={!c.workspace.startsWith("cloud:")}
            onClick={() => {
              void c.manageTeam();
            }}
          >
            Members &amp; invitations
          </Button>
        </div>
      </section>
      <section
        className="workspace-panel"
        aria-labelledby="workspace-source-title"
      >
        <h2 id="workspace-source-title">Data source</h2>
        <div className="workspace-setting">
          <div>
            <strong id="source-label">
              {
                { local: "This Mac", s3: "S3", cloud: "Kikimimi Cloud" }[
                  c.preference().kind
                ]
              }
            </strong>
            <p id="workspace-connection">
              {c.preference().url || "Choose the history you want to view."}
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
            Edit data source
          </Button>
        </div>
        <p className="workspace-help">
          Used by this Mac to read workspace activity. Changing it does not
          change collection or uploads.
        </p>
      </section>
      <section
        className="workspace-panel"
        aria-labelledby="workspace-collection-title"
      >
        <h2 id="workspace-collection-title">Collection &amp; uploads</h2>
        <div className="workspace-setting">
          <p>
            Manage this Mac’s agents and upload destination in Settings.
          </p>
          <Button
            id="workspace-app"
            className="secondary"
            variant="outline"
            onClick={() => {
              void c.navigate("settings");
            }}
          >
            Open Settings
          </Button>
        </div>
      </section>
    </section>
  );
}
