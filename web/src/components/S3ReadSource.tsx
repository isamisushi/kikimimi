import { useState } from "react";
import { useAsync } from "../hooks/useAsync";
import { apiErrorMessage, getReadSource, selectReadSource } from "../api/client";
import type { ReadSourceInfo } from "../api/types";

export function S3ReadSource() {
  const source = useAsync(getReadSource, []);
  if (source.status === "loading") return <p role="status">Reading dashboard connection…</p>;
  if (source.status === "error") return <p role="alert">Could not read the dashboard connection. <button className="btn" onClick={source.reload}>Retry</button></p>;
  return <SourceForm source={source.data} />;
}

function SourceForm({ source }: { source: ReadSourceInfo }) {
  const [url, setUrl] = useState(source.connection?.url || "");
  const [profile, setProfile] = useState(source.connection?.profile || "");
  const [endpoint, setEndpoint] = useState(source.connection?.endpoint_url || "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  async function select(kind: "local" | "s3") {
    setBusy(true); setError(null);
    try {
      await selectReadSource(kind === "local" ? { kind } : { kind, url: url.trim(), profile: profile.trim() || null, endpoint_url: endpoint.trim() || null });
      window.location.assign("/");
    } catch (err) { setError(apiErrorMessage(err)); setBusy(false); }
  }
  return <section className="panel storage-card">
    <h2>What to show in your dashboard</h2>
    <p>Viewing: <strong>{source.kind === "s3" ? source.connection?.url || "S3" : "This machine"}</strong>.</p>
    <p>Read your team’s shared S3 export in this app. Overview, machines, sessions and analysis use the bucket’s data. No Cloud account or PostgreSQL is needed.</p>
    {source.kind === "s3" && <button className="btn btn--ghost" disabled={busy} onClick={() => void select("local")}>View this machine instead</button>}
    <form className="source-form" onSubmit={(event) => { event.preventDefault(); void select("s3"); }}>
      <label htmlFor="read-s3-url">S3 export destination</label>
      <input id="read-s3-url" required value={url} placeholder="s3://team-bucket/prefix" onChange={event => setUrl(event.target.value)} disabled={busy} />
      <p className="panel__note">Use the same bucket and prefix as your team’s S3 output. kikimimi reads the kikimimi.v1/events folder beneath it.</p>
      <details><summary>AWS profile and compatible storage</summary>
        <label htmlFor="read-s3-profile">AWS profile (optional)</label>
        <input id="read-s3-profile" value={profile} onChange={event => setProfile(event.target.value)} disabled={busy} />
        <label htmlFor="read-s3-endpoint">S3-compatible endpoint (optional)</label>
        <input id="read-s3-endpoint" type="url" value={endpoint} placeholder="https://storage.example.com" onChange={event => setEndpoint(event.target.value)} disabled={busy} />
      </details>
      <p>Your AWS CLI sign-in controls access. Everyone with read access to this prefix can view all its included activity. This connection only reads; it does not enable uploads.</p>
      <button className="btn" disabled={busy}>{busy ? "Reading S3 and preparing your dashboard…" : "Connect and view S3"}</button>
      {busy && <p role="status">The first connection may take up to three minutes. Existing data remains available until the new snapshot is ready.</p>}
      {error && <p role="alert">{error}</p>}
    </form>
    <details><summary>Refresh and local cache</summary><p>Data is downloaded to a private temporary cache. Refresh checks S3 for changes; unchanged objects are reused while this viewer is running. Closing the viewer removes its temporary snapshots. Up to 512 MiB and 10,000 Parquet objects per export prefix are supported. Failed refreshes keep the previous snapshot and its timestamp.</p></details>
  </section>;
}
