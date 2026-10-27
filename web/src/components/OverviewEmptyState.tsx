import { useSession } from "../hooks/useSession";
import { Link } from "../router/Router";

declare global {
  interface Window { __kikimimiDesktop?: boolean }
}

export function OverviewEmptyState({ onRefresh }: { onRefresh: () => void }) {
  const { session } = useSession();
  const local = session?.local && session.data_source !== "s3";
  const desktop = local && window.__kikimimiDesktop;
  return (
    <section className="panel overview-empty" aria-labelledby="overview-empty-title">
      <p className="overview-empty__eyebrow">GET STARTED</p>
      <h2 id="overview-empty-title">No activity in the last 14 days</h2>
      <p>{local
        ? "Bring your Claude Code and Codex activity into one place. Already collecting? Run a new agent session, then refresh this page."
        : "This workspace has no activity for this period. Check the data source and where your machines send activity."}</p>
      {local && <ol className="overview-empty__steps">
        <li><strong>Review collection</strong><span>Choose where this Mac saves activity and review the setup changes.</span></li>
        <li><strong>Use your agent</strong><span>After setup, start a new Claude Code or Codex session.</span></li>
        <li><strong>See your activity</strong><span>Refresh to see recorded sessions, tools, and usage.</span></li>
      </ol>}
      <div className="overview-empty__actions">
        {desktop ? <a className="btn btn--primary" href="/__desktop_setup">Review collection setup…</a>
          : <Link className="btn btn--primary" to="/storage">{local ? "View setup options" : "Check data source"}</Link>}
        <button type="button" className="btn btn--ghost" onClick={onRefresh}>Refresh activity</button>
      </div>
      {desktop && <p className="overview-empty__note">Opening setup does not start collection or change sharing.</p>}
    </section>
  );
}
