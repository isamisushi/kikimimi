import { Button, Card, Input, NativeSelect } from "@kikimimi/ui";
import type { DesktopController } from "../controller";
import type { Source } from "../types";
export function Connection({ c }: { c: DesktopController }) {
  return (
    <section
      id="connection-view"
      className="page settings"
      aria-labelledby="connection-heading"
      hidden={c.view !== "connection"}
    >
      <div className="page-heading">
        <div>
          <h1 id="connection-heading">Data source on this Mac</h1>
        </div>
      </div>
      <p>
        Choose the data this Mac reads for the selected workspace. This does not
        configure storage or uploads.
      </p>
      <Card className="settings-card" aria-labelledby="source-title">
        <div className="card-heading">
          <div>
            <h2 id="source-title">Data source</h2>
            <p>
              <span id="source-workspace">{c.workspaceName()}</span> · Saved on
              this Mac only. Other members and upload destinations are
              unaffected.
            </p>
          </div>
        </div>
        <form
          id="source-form"
          className="card-body source-settings"
          onSubmit={(e) => {
            e.preventDefault();
            void c.applySource();
          }}
        >
          <label htmlFor="data-source">Show data from</label>
          <NativeSelect
            id="data-source"
            value={c.sourceForm.kind}
            disabled={c.sourceBusy}
            onChange={(e) => {
              c.sourceForm.kind = e.target.value as Source;
              c.emit();
            }}
          >
            <option value="local">This Mac</option>
            <option value="s3">S3</option>
            <option value="cloud">Kikimimi Cloud</option>
          </NativeSelect>
          <div id="s3-fields" hidden={c.sourceForm.kind !== "s3"}>
            <label htmlFor="source-url">S3 bucket / prefix</label>
            <Input
              autoCapitalize="off"
              autoCorrect="off"
              spellCheck="false"
              id="source-url"
              placeholder="s3://your-bucket/shared"
              value={c.sourceForm.url || ""}
              disabled={c.sourceBusy}
              onChange={(e) => {
                c.sourceForm.url = e.target.value;
                c.emit();
              }}
            />
            <label htmlFor="source-profile">
              AWS profile on this Mac (optional)
            </label>
            <Input
              autoCapitalize="off"
              autoCorrect="off"
              spellCheck="false"
              id="source-profile"
              placeholder="default"
              value={c.sourceForm.profile || ""}
              disabled={c.sourceBusy}
              onChange={(e) => {
                c.sourceForm.profile = e.target.value;
                c.emit();
              }}
            />
            <label htmlFor="source-endpoint">
              S3-compatible endpoint (optional)
            </label>
            <Input
              autoCapitalize="off"
              autoCorrect="off"
              spellCheck="false"
              id="source-endpoint"
              type="url"
              placeholder="Leave empty for AWS S3"
              value={c.sourceForm.endpoint_url || ""}
              disabled={c.sourceBusy}
              onChange={(e) => {
                c.sourceForm.endpoint_url = e.target.value;
                c.emit();
              }}
            />
            <p>
              Uses your AWS CLI authentication. Connect reads the shared export;
              it does not upload your history.
            </p>
          </div>
          <p id="cloud-source-help" hidden={c.sourceForm.kind !== "cloud"}>
            Sign in to Cloud to view your personal or team workspace. Use the
            Workspace selector above to switch workspaces.
          </p>
          <p id="source-error" role="alert" hidden={!c.sourceError}>
            {c.sourceError}
          </p>
          <Button
            id="apply-source"
            type="submit"
            className="small"
            variant="default"
            size="sm"
            disabled={c.sourceBusy}
          >
            Connect and view
          </Button>
        </form>
      </Card>
      <div className="connection-actions">
        <Button
          id="connection-workspace"
          className="text-button"
          variant="ghost"
          onClick={() => {
            void c.navigate("workspace");
          }}
        >
          ← Data sources
        </Button>
        <Button
          id="connection-app"
          className="text-button"
          variant="ghost"
          onClick={() => {
            void c.navigate("settings");
          }}
        >
          Settings
        </Button>
      </div>
    </section>
  );
}
