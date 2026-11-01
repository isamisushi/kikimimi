import { t } from "../i18n";
import type { ReactNode } from "react";
import { useAsync } from "../hooks/useAsync";
import { getStorageSettings } from "../api/client";
import { S3ReadSource } from "../components/S3ReadSource";

function SetupCommands({ children }: { children: ReactNode }) {
  return (
    <details className="storage-setup">
      <summary>{t("Set up from a terminal")}</summary>
      <p>{t("Run these on the machine collecting your agent activity.")}</p>
      {children}
      <p>
        {t("Installed only the Mac app? Replace")} <code>kikimimi</code>{" "}
        {t("with")}{" "}
        <code>/Applications/kikimimi.app/Contents/MacOS/kikimimi</code>.
      </p>
    </details>
  );
}

function CloudSetup() {
  return (
    <SetupCommands>
      <p>
        {t(
          "For your own machines, sign in to the same personal workspace on each one. Approve the code in your browser.",
        )}
      </p>
      <pre>kikimimi login</pre>
      <p>
        {t(
          "For a team, first create or join it in Cloud. Set the repositories you want to share before approving the team connection.",
        )}
      </p>
      <pre>
        {"kikimimi login --org your-team --repo 'github.com/your-team/*'"}
      </pre>
      <p>
        {t("Repeat")} <code>--repo</code>{" "}
        {t(
          "to share more repositories. It replaces the saved filter before connecting. Each machine sends to one Cloud workspace at a time. Switching the workspace you view in Cloud does not change where a machine sends data. Without a repository filter, all repositories are shared.",
        )}
      </p>
      <p>{t("To stop Cloud sharing on this machine:")}</p>
      <pre>kikimimi logout</pre>
    </SetupCommands>
  );
}

function S3Setup() {
  return (
    <SetupCommands>
      <p>
        {t(
          "Configure your AWS CLI access first, then add your bucket. S3-compatible services can use an endpoint URL.",
        )}
      </p>
      <pre>
        {
          "kikimimi sink add s3 s3://your-bucket/prefix\n# Optional profile and S3-compatible endpoint:\nkikimimi sink add s3 s3://your-bucket/prefix --profile your-profile --endpoint-url https://your-storage-host"
        }
      </pre>
      <p>
        {t(
          "One S3 destination per machine. Adding another replaces the destination. Export starts with newly collected events; existing local history is not automatically uploaded.",
        )}
      </p>
      <pre>
        {
          "kikimimi status\nkikimimi flush\n# Stop future uploads; keep existing objects:\nkikimimi sink remove s3"
        }
      </pre>
    </SetupCommands>
  );
}

function LocalStorage() {
  const settings = useAsync(getStorageSettings, []);
  if (settings.status === "loading")
    return <p role="status">{t("Reading storage settings…")}</p>;
  if (settings.status === "error")
    return (
      <div className="state-panel state-panel--error" role="alert">
        <p>
          {t("Could not read storage settings. Sharing status is unknown.")}
        </p>
        <button className="btn" onClick={settings.reload}>
          {t("Try again")}
        </button>
      </div>
    );
  const { cloud, s3, local_path } = settings.data;
  return (
    <>
      <S3ReadSource />
      <section className="panel storage-card">
        <h2>{t("This machine")}</h2>
        <p>
          {t(
            "This machine keeps its own history independently of the dashboard source selected above. No account is needed, and saved history remains available when collection is off.",
          )}
        </p>
        <p>
          {cloud || s3
            ? t(
                "Additional destinations are configured below. They do not change which data this dashboard shows.",
              )
            : t("No Cloud or S3 destination is configured.")}
        </p>
        <details>
          <summary>{t("Local files")}</summary>
          <p className="mono storage-path">{local_path}</p>
          <p>
            {t(
              "History is stored as Parquet files on disk, not in browser storage.",
            )}
          </p>
        </details>
      </section>
      <section className="panel storage-card">
        <h2>
          {cloud && !cloud.hosted ? t("Your Cloud instance") : "Kikimimi Cloud"}
        </h2>
        <span className="badge badge--neutral">
          {cloud ? t("Configured") : t("Not connected")}
        </span>
        <p>
          {t(
            "Bring your own machines together, or share agent activity with a team.",
          )}
        </p>
        {cloud && (
          <>
            <p>
              {t("Sending to:")}{" "}
              <strong>
                {cloud.org_slug ||
                  t("Workspace not recorded — sign in again to identify it")}
              </strong>
              {cloud.org_kind ? ` (${cloud.org_kind})` : ""}.
            </p>
            {cloud.org_kind === "team" && (
              <p>
                {cloud.repo_patterns.length
                  ? t("Shared repositories: {0}", [
                      cloud.repo_patterns.join(", "),
                    ])
                  : t(
                      "No repository filter is set. All repositories are shared with this team.",
                    )}
              </p>
            )}
          </>
        )}
        {(!cloud || cloud.hosted) && (
          <a
            className="btn"
            href="https://kikimimi.dev/"
            referrerPolicy="no-referrer"
          >
            {t("Open Kikimimi Cloud")}
          </a>
        )}
        {cloud && !cloud.hosted && (
          <p>{t("Open your configured Cloud instance in your browser.")}</p>
        )}
        <CloudSetup />
      </section>
      <section className="panel storage-card">
        <h2>{t("Your S3 bucket")}</h2>
        <span className="badge badge--neutral">
          {s3 ? t("Configured") : t("Not configured")}
        </span>
        <p>
          {t(
            "Keep an additional copy in storage you control. Works independently of Cloud.",
          )}
        </p>
        {s3 && <p className="mono storage-path">{s3.url}</p>}
        <p>
          {t(
            "S3 receives all locally recorded fields and repositories. Cloud repository filters do not apply. Team members can connect this export as a dashboard source above using their own AWS read access.",
          )}
        </p>
        <S3Setup />
      </section>
      <p className="panel__note">
        {t(
          "Configured destinations are not proof of successful uploads. Check",
        )}{" "}
        <code>kikimimi status</code>{" "}
        {t(
          "for pending uploads and errors. Opening this page does not enable sharing.",
        )}
      </p>
      <button className="btn btn--ghost" onClick={settings.reload}>
        {t("Refresh settings")}
      </button>
    </>
  );
}

export function Storage() {
  return (
    <div className="page storage-page">
      <div className="page__header">
        <h1>{t("Storage & sharing")}</h1>
        <p className="page__subtitle">
          {t(
            "Keep history on your machine. Choose whether to share or export it.",
          )}
        </p>
      </div>
      <LocalStorage />
    </div>
  );
}
