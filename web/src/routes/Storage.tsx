import type { ReactNode } from "react";
import { useSession } from "../hooks/useSession";
import { useAsync } from "../hooks/useAsync";
import { getStorageSettings } from "../api/client";
import { Link } from "../router/Router";
import { S3ReadSource } from "../components/S3ReadSource";

function SetupCommands({ children }: { children: ReactNode }) {
  return <details className="storage-setup">
    <summary>Set up from a terminal</summary>
    <p>Run these on the machine collecting your agent activity.</p>
    {children}
    <p>Installed only the Mac app? Replace <code>kikimimi</code> with <code>/Applications/kikimimi.app/Contents/MacOS/kikimimi</code>.</p>
  </details>;
}

function CloudSetup() {
  return <SetupCommands>
    <p>For your own machines, sign in to the same personal workspace on each one. Approve the code in your browser.</p>
    <pre>kikimimi login</pre>
    <p>For a team, first create or join it in Cloud. Set the repositories you want to share before approving the team connection.</p>
    <pre>{"kikimimi login --org your-team --repo 'github.com/your-team/*'"}</pre>
    <p>Repeat <code>--repo</code> to share more repositories. It replaces the saved filter before connecting. Each machine sends to one Cloud workspace at a time. Switching the workspace you view in Cloud does not change where a machine sends data. Without a repository filter, all repositories are shared.</p>
    <p>To stop Cloud sharing on this machine:</p>
    <pre>kikimimi logout</pre>
  </SetupCommands>;
}

function S3Setup() {
  return <SetupCommands>
    <p>Configure your AWS CLI access first, then add your bucket. S3-compatible services can use an endpoint URL.</p>
    <pre>{"kikimimi sink add s3 s3://your-bucket/prefix\n# Optional profile and S3-compatible endpoint:\nkikimimi sink add s3 s3://your-bucket/prefix --profile your-profile --endpoint-url https://your-storage-host"}</pre>
    <p>One S3 destination per machine. Adding another replaces the destination. Export starts with newly collected events; existing local history is not automatically uploaded.</p>
    <pre>{"kikimimi status\nkikimimi flush\n# Stop future uploads; keep existing objects:\nkikimimi sink remove s3"}</pre>
  </SetupCommands>;
}

function LocalStorage() {
  const settings = useAsync(getStorageSettings, []);
  if (settings.status === "loading") return <p role="status">Reading storage settings…</p>;
  if (settings.status === "error") return <div className="state-panel state-panel--error" role="alert">
    <p>Could not read storage settings. Sharing status is unknown.</p>
    <button className="btn" onClick={settings.reload}>Try again</button>
  </div>;
  const { cloud, s3, local_path } = settings.data;
  return <>
    <S3ReadSource />
    <section className="panel storage-card">
      <h2>This machine</h2>
      <p>This machine keeps its own history independently of the dashboard source selected above. No account is needed, and saved history remains available when collection is off.</p>
      <p>{cloud || s3 ? "Additional destinations are configured below. They do not change which data this dashboard shows." : "No Cloud or S3 destination is configured."}</p>
      <details><summary>Local files</summary><p className="mono storage-path">{local_path}</p><p>History is stored as Parquet files on disk, not in browser storage.</p></details>
    </section>
    <section className="panel storage-card">
      <h2>{cloud && !cloud.hosted ? "Your Cloud instance" : "Kikimimi Cloud"}</h2>
      <span className="badge badge--neutral">{cloud ? "Configured" : "Not connected"}</span>
      <p>Bring your own machines together, or share agent activity with a team.</p>
      {cloud && <>
        <p>Sending to: <strong>{cloud.org_slug || "Workspace not recorded — sign in again to identify it"}</strong>{cloud.org_kind ? ` (${cloud.org_kind})` : ""}.</p>
        {cloud.org_kind === "team" && <p>{cloud.repo_patterns.length ? `Shared repositories: ${cloud.repo_patterns.join(", ")}` : "No repository filter is set. All repositories are shared with this team."}</p>}
      </>}
      {(!cloud || cloud.hosted) && <a className="btn" href="https://kikimimi.dev/" referrerPolicy="no-referrer">Open Kikimimi Cloud</a>}
      {cloud && !cloud.hosted && <p>Open your configured Cloud instance in your browser.</p>}
      <CloudSetup />
    </section>
    <section className="panel storage-card">
      <h2>Your S3 bucket</h2>
      <span className="badge badge--neutral">{s3 ? "Configured" : "Not configured"}</span>
      <p>Keep an additional copy in storage you control. Works independently of Cloud.</p>
      {s3 && <p className="mono storage-path">{s3.url}</p>}
      <p>S3 receives all locally recorded fields and repositories. Cloud repository filters do not apply. Team members can connect this export as a dashboard source above using their own AWS read access.</p>
      <S3Setup />
    </section>
    <p className="panel__note">Configured destinations are not proof of successful uploads. Check <code>kikimimi status</code> for pending uploads and errors. Opening this page does not enable sharing.</p>
    <button className="btn btn--ghost" onClick={settings.reload}>Refresh settings</button>
  </>;
}

function CloudStorage() {
  const { session } = useSession();
  const org = session?.orgs.find((item) => item.slug === session.active_org);
  return <>
    <section className="panel storage-card">
      <h2>{org?.kind === "personal" ? "Personal" : org?.name || "Current workspace"}</h2>
      <p>This dashboard shows activity uploaded to this workspace, within your role’s permissions. Each machine still keeps its own local history.</p>
      <div className="storage-actions"><Link className="btn" to="/devices">Manage devices</Link><Link className="btn btn--ghost" to="/team">Manage teams</Link></div>
      <CloudSetup />
    </section>
    <section className="panel storage-card">
      <h2>Local history and S3 export</h2>
      <p>Storage settings belong to each collecting machine. Open its local dashboard to see its configured destinations. This Cloud session cannot read or change them.</p>
      <p>S3 export is optional and does not require Cloud. It copies all locally recorded fields and repositories, regardless of the Cloud repository filter. Team members can read the shared export from Storage &amp; sharing in their Mac app or local dashboard.</p>
      <S3Setup />
    </section>
  </>;
}

export function Storage() {
  const { session } = useSession();
  return <div className="page storage-page">
    <div className="page__header"><h1>Storage &amp; sharing</h1><p className="page__subtitle">Keep history on your machine. Choose whether to share or export it.</p></div>
    {session?.local ? <LocalStorage /> : <CloudStorage />}
  </div>;
}
