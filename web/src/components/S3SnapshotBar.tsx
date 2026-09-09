import { useEffect, useState } from "react";
import { useSession } from "../hooks/useSession";
import { apiErrorMessage, getReadSource, refreshReadSource } from "../api/client";
import { Link } from "../router/Router";

export function S3SnapshotBar() {
  const { session } = useSession();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [info, setInfo] = useState(session?.source_info);
  useEffect(() => {
    if (session?.data_source !== "s3" || info?.refreshed_at || info?.error) return;
    let cancelled = false;
    const timer = setInterval(() => { void getReadSource().then(value => { if (!cancelled) setInfo(value); }).catch(() => {}); }, 2000);
    return () => { cancelled = true; clearInterval(timer); };
  }, [session?.data_source, info?.refreshed_at, info?.error]);
  if (session?.data_source !== "s3") return null;
  async function refresh() {
    setBusy(true); setError(null);
    try { await refreshReadSource(); window.location.reload(); }
    catch (err) { setError(apiErrorMessage(err)); setBusy(false); }
  }
  return <aside className="s3-snapshot-bar" aria-label="S3 snapshot">
    <span><strong>S3 · {info?.connection?.url}</strong><br />{info?.refreshed_at ? `Snapshot from ${new Date(info.refreshed_at).toLocaleString()}` : "Loading the S3 snapshot"} · Read-only</span>
    <button className="btn btn--small" disabled={busy} onClick={() => void refresh()}>{busy ? "Refreshing…" : "Refresh from S3"}</button>
    <Link to="/storage">Change source</Link>
    {(error || info?.error) && <span role="alert">{error || info?.error} The previous snapshot is still shown if available.</span>}
  </aside>;
}
