import { useEffect, useState } from "react";
import { getSubscriptionUsage } from "../api/client";
import { useAsync } from "../hooks/useAsync";
import { QueryBoundary } from "./QueryBoundary";

function duration(minutes: number | null): string {
  if (minutes === null) return "Window duration unknown";
  if (minutes % 1440 === 0) return `${minutes / 1440} day window`;
  if (minutes % 60 === 0) return `${minutes / 60} hour window`;
  return `${minutes} minute window`;
}

export function SubscriptionUsage() {
  const state = useAsync(getSubscriptionUsage, []);
  const [now, setNow] = useState(Date.now());
  const [account, setAccount] = useState("");
  useEffect(() => {
    const timer = window.setInterval(() => { setNow(Date.now()); state.reload(); }, 60_000);
    return () => window.clearInterval(timer);
  }, [state.reload]);

  return (
    <section className="panel" aria-labelledby="subscription-usage-title">
      <h2 className="panel__title" id="subscription-usage-title">Subscription usage by account</h2>
      <p className="panel__note">Latest observations on this machine. Codex accounts are detected automatically. Refresh reloads recorded observations.</p>
      <button className="btn btn--ghost" type="button" onClick={() => { setNow(Date.now()); state.reload(); }}>Refresh</button>
      <QueryBoundary state={state} onRetry={state.reload}>
        {(snapshots) => snapshots.length === 0 ? (
          <div className="state-panel">
            <p>No account usage recorded yet.</p>
            <p>Codex updates automatically while the agent is running. To fetch now: <code>kikimimi usage codex</code></p>
            <p>Claude: configure <code>kikimimi usage claude --account personal</code> as your status-line command.</p>
            <p>Use a separate Claude label for each subscription.</p>
          </div>
        ) : (
          <>
            <label>Account{" "}
              <select value={account} onChange={(event) => setAccount(event.target.value)}>
                <option value="">All accounts</option>
                {snapshots.map((s) => <option key={`${s.agent}/${s.account}`} value={JSON.stringify([s.agent, s.account])}>{s.agent === "claude" ? "Claude" : "Codex"} · {s.account}</option>)}
              </select>
            </label>
            <div className="stat-grid">
              {snapshots.filter((s) => !account || account === JSON.stringify([s.agent, s.account])).map((s) => {
                const age = Math.max(0, Math.floor((now - s.observed_at) / 60_000));
                return (
                  <article className="panel" key={`${s.agent}/${s.account}`}>
                    <h3>{s.agent === "claude" ? "Claude" : "Codex"} · {s.account}</h3>
                    <p className="panel__note"><time dateTime={new Date(s.observed_at).toISOString()} title={new Date(s.observed_at).toLocaleString()}>Observed {age < 1 ? "just now" : `${age} min ago`}</time>{age >= 15 ? " · May be outdated" : ""}</p>
                    {s.windows.length === 0 && <p>Usage unavailable</p>}
                    {s.windows.map((w) => {
                      const expired = w.resets_at !== null && w.resets_at * 1000 <= now;
                      return (
                        <div key={w.name}>
                          <p>{duration(w.window_minutes)} · {w.name.replace(/_/g, " ")}</p>
                          <p>{w.used_percent.toFixed(1)}% used{expired ? " at last observation" : ""}</p>
                          <progress max={100} value={w.used_percent} aria-label={`${s.agent} ${s.account} ${w.name} usage`} style={{ width: "100%", opacity: expired ? 0.45 : 1 }} />
                          <p className="panel__note">{w.resets_at === null ? "Reset time unknown" : expired ? "Reset time passed — awaiting a new observation" : `Resets ${new Date(w.resets_at * 1000).toLocaleString()}`}</p>
                        </div>
                      );
                    })}
                  </article>
                );
              })}
            </div>
          </>
        )}
      </QueryBoundary>
    </section>
  );
}
