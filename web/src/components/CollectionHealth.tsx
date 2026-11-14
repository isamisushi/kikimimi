import { createContext, useContext, type ReactNode } from "react";
import { getCollectionHealth } from "../api/client";
import type { CollectionHealth, SyncHealth } from "../api/types";
import { fmtDateTime, fmtNum } from "../api/format";
import { useAsync } from "../hooks/useAsync";
import { t } from "../i18n";
import { Link } from "../router/Router";

const Context = createContext<CollectionHealth | null>(null);
export function useCollectionHealth() {
  return useContext(Context);
}
export function collectionReady(health: CollectionHealth | null): boolean {
  return (
    !!health &&
    health.collector === "running" &&
    health.snapshot_fresh &&
    health.issues.length === 0 &&
    Date.now() - health.checked_at_ms <= 30_000
  );
}
function collectionHealthy(health: CollectionHealth | null): boolean {
  return (
    collectionReady(health) &&
    health?.cloud.status !== "error" &&
    health?.cloud.status !== "unknown" &&
    health?.s3.status !== "error" &&
    health?.s3.status !== "unknown"
  );
}
const ISSUES: Record<string, string> = {
  destination_unconfirmed:
    "The collector has not confirmed the saved sync destination. Previous sync times may belong to an older destination.",
  collector_unconfirmed:
    "The service and collector could not both be confirmed. Check collection in Settings.",
  collector_stopped: "Collection is stopped. Saved history is still available.",
  heartbeat_missing_or_stale:
    "The collector heartbeat is missing or stale. Old counters do not confirm current collection.",
  telemetry_unavailable:
    "Real-time telemetry is unavailable. Some activity may be missing.",
  storage_error:
    "Recent events could not be saved. Check available disk space and collector logs.",
  codex_malformed_records_since_start:
    "Some Codex records could not be parsed since the collector started. Recorded totals may be incomplete.",
  claude_read_error: "Claude Code history could not be read completely.",
  claude_desktop_read_error:
    "Claude Desktop history could not be read completely.",
  telemetry_rejected_since_start:
    "Telemetry requests were rejected since the collector started. Check agent setup if this continues.",
  spool_backlog:
    "Events are waiting to be collected. Check collector logs if the backlog grows.",
};
function timestamp(value: number | null) {
  return value === null
    ? t("Not observed")
    : fmtDateTime(new Date(value).toISOString());
}
function SyncStatus({ label, sync }: { label: string; sync: SyncHealth }) {
  const labels: Record<SyncHealth["status"], string> = {
    off: "Off",
    paused: "Paused",
    unknown: "Unable to confirm",
    error: "Sync error",
    waiting: "Waiting for first sync",
    pending: "Syncing",
    synced: "Synced",
  };
  return (
    <div>
      <dt>{label}</dt>
      <dd>
        {t(labels[sync.status])}
        {sync.pending !== null &&
          sync.pending > 0 &&
          ` · ${fmtNum(sync.pending)} ${t("pending")}`}
        {sync.last_push_at !== null && (
          <small>
            {t("Last successful sync")}: {timestamp(sync.last_push_at)}
          </small>
        )}
      </dd>
    </div>
  );
}
export function CollectionHealthProvider({
  children,
}: {
  children: ReactNode;
}) {
  const health = useAsync(getCollectionHealth, [], 10_000);
  const data = health.status === "ok" ? health.data : null;
  const healthy = collectionHealthy(data);
  const attention = health.status !== "loading" && !healthy;
  return (
    <Context.Provider value={data}>
      <div
        className={`collection-status ${attention ? "collection-status--attention" : ""}`}
        role="status"
      >
        <span className="collection-status__dot" aria-hidden="true" />
        <span>
          {t(
            health.status === "loading"
              ? "Checking collection…"
              : healthy
                ? "Collector running"
                : !data
                  ? "Collection status is unavailable. Usage totals do not prove that collection is working."
                  : data.collector === "stopped"
                    ? "Collection stopped"
                    : "Collection needs attention",
          )}
        </span>
        <Link to="/settings">{t("Open collection settings")}</Link>
      </div>
      {children}
    </Context.Provider>
  );
}

/** Full diagnostics belong in Settings, in both browser and desktop hosts. */
export function CollectionHealthDetails({
  snapshot = false,
}: {
  snapshot?: boolean;
}) {
  const health = useAsync(getCollectionHealth, [], 10_000);
  const data = health.status === "ok" ? health.data : null;
  const healthy = collectionHealthy(data);
  return (
    <section
      className={`panel collection-health ${healthy ? "" : "collection-health--attention"}`}
      aria-labelledby="collection-health-title"
    >
      <div className="workflow-heading">
        <div>
          <h2 className="panel__title" id="collection-health-title">
            {t("Collection on this machine")}
          </h2>
          <p className="panel__note">
            {t("Collection and sync status for this machine.")}
          </p>
        </div>
        <button
          type="button"
          className="btn btn--small"
          onClick={health.reload}
        >
          {t("Check again")}
        </button>
      </div>
      {data ? (
        <>
          <p className="health-summary" role="status">
            <span
              className={`badge ${healthy ? "badge--success" : "badge--neutral"}`}
            >
              {t(
                data.collector === "running"
                  ? !healthy
                    ? "Collection needs attention"
                    : "Collector running"
                  : data.collector === "stopped"
                    ? "Collection stopped"
                    : "Unable to confirm collection",
              )}
            </span>
            <span>
              {t("Checked")}: {timestamp(data.checked_at_ms)}
            </span>
          </p>
          <dl className="health-grid">
            <div>
              <dt>{t("Collector heartbeat")}</dt>
              <dd>{timestamp(data.heartbeat_at_ms)}</dd>
            </div>
            <div>
              <dt>{t("Latest recorded event")}</dt>
              <dd>
                {timestamp(data.last_event_ts)}
                <small>
                  {t("No recent event can also mean no agent activity.")}
                </small>
              </dd>
            </div>
            <div>
              <dt>{t("Last saved batch")}</dt>
              <dd>
                {timestamp(data.last_flush_at_ms)}
                <small>
                  {t("Waiting in collector")}: {fmtNum(data.spool_pending)}
                </small>
              </dd>
            </div>
            <SyncStatus label={t("Cloud sync")} sync={data.cloud} />
            {data.s3.status !== "off" && (
              <SyncStatus label={t("S3 export")} sync={data.s3} />
            )}
          </dl>
          {data.events_by_source && (
            <p className="panel__note">
              {t(
                "Recorded since collector start: hooks {0} · telemetry {1} · history {2}",
                [
                  fmtNum(data.events_by_source.hook),
                  fmtNum(data.events_by_source.otel),
                  fmtNum(data.events_by_source.log),
                ],
              )}
            </p>
          )}
          {data.issues.length > 0 && (
            <ul className="health-issues">
              {data.issues.map((code) => (
                <li key={code}>
                  {t(ISSUES[code] ?? "Collection needs attention")}
                </li>
              ))}
            </ul>
          )}
          {!data.snapshot_fresh && (
            <p className="panel__note">
              {t(
                "The event and batch times above are historical observations.",
              )}
            </p>
          )}
        </>
      ) : (
        <p role={health.status === "error" ? "alert" : "status"}>
          {t(
            health.status === "loading"
              ? "Checking collection…"
              : "Collection status is unavailable. Usage totals do not prove that collection is working.",
          )}
        </p>
      )}
      {snapshot && (
        <p className="panel__note">
          {t(
            "You are viewing an S3 snapshot. This machine's health cannot verify collection on the exporting devices.",
          )}
        </p>
      )}
    </section>
  );
}
