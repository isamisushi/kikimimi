import { CostSummaryLoader } from "../components/CostSummary";
import { t } from "../i18n";
import { getCoverage, getMachines, getOverview } from "../api/client";
import { COVERAGE_DAYS, fmtPct, summarize } from "../api/coverage";
import { fmtNum } from "../api/format";
import { useAsync } from "../hooks/useAsync";
import { QueryBoundary } from "../components/QueryBoundary";
import { StatTile } from "../components/StatTile";
import { TokenBarChart, type TokenBarDatum } from "../components/TokenBarChart";
import { FreshnessBadge } from "../components/FreshnessBadge";
import { SortableTable, type ColumnDef } from "../components/SortableTable";
import type { MachineRow } from "../api/types";
import { SubscriptionUsage } from "../components/SubscriptionUsage";
import { useSession } from "../hooks/useSession";

import { OverviewEmptyState } from "../components/OverviewEmptyState";
import { Link } from "../router/Router";

const DAYS = 14;

/** Sum of known (non-null) values, or null if every value in the window is null. */
function sumOrNull(values: (number | null)[]): number | null {
  const known = values.filter((v): v is number => v !== null);
  if (known.length === 0) return null;
  return known.reduce((a, b) => a + b, 0);
}

/** Count of null (unknown usage_source) entries in a column. */
function countUnknown(values: (number | null)[]): number {
  return values.filter((v) => v === null).length;
}

/** A sum silently drops unknown days rather than rendering "–" outright, so
 * flag when it's partial (some but not all days unknown) — otherwise the
 * total looks complete when it's actually an undercount. */
function partialHint(
  unknownDays: number,
  total: number | null,
): string | undefined {
  if (total === null || unknownDays === 0) return undefined;
  return t("{0} day{1} unknown (excluded from total)", [
    unknownDays,
    unknownDays === 1 ? "" : "s",
  ]);
}

const machineColumns: ColumnDef<MachineRow>[] = [
  {
    key: "host_id",
    get label() {
      return t("Host");
    },
    sortValue: (r) => r[0],
    render: (r) => <span className="mono">{r[0]}</span>,
  },
  {
    key: "env_kind",
    get label() {
      return t("Environment");
    },
    sortValue: (r) => r[1],
    render: (r) => r[1],
  },
  {
    key: "os",
    label: "OS",
    sortValue: (r) => r[2],
    render: (r) => r[2],
  },
  {
    key: "last_event_ts",
    get label() {
      return t("Last event");
    },
    sortValue: (r) => (r[3] ? new Date(r[3]).getTime() : null),
    render: (r) => <FreshnessBadge lastEventTs={r[3]} />,
  },
  {
    key: "events_30d",
    get label() {
      return t("Events (30d)");
    },
    align: "right",
    sortValue: (r) => r[4],
    render: (r) => fmtNum(r[4]),
  },
];

export function Overview() {
  const { session } = useSession();
  const overview = useAsync(() => getOverview(DAYS), [DAYS]);
  const machines = useAsync(() => getMachines(), []);
  const coverage = useAsync(() => getCoverage(COVERAGE_DAYS), []);

  return (
    <div className="page">
      <div className="page__header">
        <h1>{t("Overview")}</h1>
        <p className="page__subtitle">
          {t("Usage over the last")} {DAYS} {t("days")}
        </p>
      </div>

      <section className="panel improvement-entry">
        <div><h2 className="panel__title">{t("Turn observations into improvements")}</h2>
          <p>{t("Review evidence, fix one cause, then check whether it happens again.")}</p></div>
        <Link to="/patterns" className="btn btn--primary">{t("Find an improvement")}</Link>
      </section>

      <QueryBoundary
        state={overview}
        isEmpty={(d) => d.rows.length === 0}
        onRetry={overview.reload}
        emptyContent={
          <OverviewEmptyState
            onRefresh={() => {
              overview.reload();
              machines.reload();
              coverage.reload();
            }}
          />
        }
      >
        {(data) => {
          const rows = data.rows;
          const totalEvents = sumOrNull(rows.map((r) => r[1]));
          const totalToolCalls = sumOrNull(rows.map((r) => r[2]));
          const totalFailures = sumOrNull(rows.map((r) => r[3]));

          const eventsUnknownDays = countUnknown(rows.map((r) => r[1]));
          const toolCallsUnknownDays = countUnknown(rows.map((r) => r[2]));
          const failuresUnknownDays = countUnknown(rows.map((r) => r[3]));

          const chartData: TokenBarDatum[] = rows.map((r) => ({
            dt: r[0],
            input: r[4],
            output: r[5],
            cost: r[6],
          }));

          return (
            <>
              <div className="stat-grid">
                <StatTile
                  label={t("Events")}
                  value={fmtNum(totalEvents)}
                  hint={partialHint(eventsUnknownDays, totalEvents)}
                />
                <StatTile
                  label={t("Tool calls")}
                  value={fmtNum(totalToolCalls)}
                  hint={partialHint(toolCallsUnknownDays, totalToolCalls)}
                />
                <StatTile
                  label={t("Failures")}
                  value={fmtNum(totalFailures)}
                  tone={
                    totalFailures && totalFailures > 0 ? "danger" : "default"
                  }
                  hint={partialHint(failuresUnknownDays, totalFailures)}
                />
              </div>

              <section className="panel">
                <h2 className="panel__title">{t("Daily token usage")}</h2>
                <TokenBarChart data={chartData} />
              </section>
            </>
          );
        }}
      </QueryBoundary>

      <CostSummaryLoader days={DAYS} />

      {session?.subscription_usage && <SubscriptionUsage compactEmpty />}

      {!(
        coverage.status === "ok" &&
        (coverage.data.rows.length === 0 || coverage.data.rows[0][0] === 0)
      ) && (
        <section className="panel" id="coverage">
          <h2 className="panel__title">
            {t("Data coverage (last")} {COVERAGE_DAYS} {t("days)")}
          </h2>
          <p className="panel__note">
            {t(
              'What the numbers on every page cannot see. A rate is "–" when there is nothing to divide, never 0%.',
            )}
          </p>
          <QueryBoundary
            state={coverage}
            onRetry={coverage.reload}
            isEmpty={(d) => d.rows.length === 0}
            emptyLabel={t("No events yet")}
          >
            {(data) => {
              const r = data.rows[0];
              const s = summarize(r);
              const tone = (v: number | null, min: number) =>
                v !== null && v < min ? "danger" : "default";
              return (
                <div className="stat-grid">
                  <StatTile
                    label={t("Sessions with usage")}
                    value={fmtPct(s.usageKnownPct)}
                    tone={tone(s.usageKnownPct, 80)}
                    hint={t("{0} of {1} sessions had no token usage", [
                      fmtNum(r[3]),
                      fmtNum(r[2]),
                    ])}
                  />
                  <StatTile
                    label={t("Hook ↔ OTel match")}
                    value={fmtPct(s.hookOtelMatchPct)}
                    tone={tone(s.hookOtelMatchPct, 80)}
                    hint={t("{0} of {1} hook tool results also seen by OTel", [
                      fmtNum(r[6]),
                      fmtNum(r[4]),
                    ])}
                  />
                  <StatTile
                    label={t("Dedup dropped")}
                    value={fmtPct(s.dedupDroppedPct)}
                    hint={t(
                      "{0} of {1} tool results were the same call twice",
                      [fmtNum(r[7] - r[8]), fmtNum(r[7])],
                    )}
                  />
                  <StatTile
                    label={t("Subagents priced")}
                    value={fmtPct(s.subagentUsageKnownPct)}
                    hint={t("{0} of {1} subagents had usage recorded", [
                      fmtNum(r[10]),
                      fmtNum(r[9]),
                    ])}
                  />
                  <StatTile
                    label={t("Attributed to a person")}
                    value={fmtPct(s.attributedPct)}
                    hint={
                      r[1] === r[0] && r[0] > 0
                        ? t("local data has no accounts")
                        : t("{0} of {1} events have no account", [
                            fmtNum(r[1]),
                            fmtNum(r[0]),
                          ])
                    }
                  />
                  <StatTile
                    label={t("Hosts silent > 24h")}
                    value={`${fmtNum(r[12])} / ${fmtNum(r[11])}`}
                    tone={r[12] > 0 ? "danger" : "default"}
                    hint={r[13] ? t("last event {0}", [r[13]]) : undefined}
                  />
                </div>
              );
            }}
          </QueryBoundary>
        </section>
      )}

      {!(machines.status === "ok" && machines.data.rows.length === 0) && (
        <section className="panel">
          <h2 className="panel__title">{t("Machines")}</h2>
          <QueryBoundary
            state={machines}
            onRetry={machines.reload}
            isEmpty={(d) => d.rows.length === 0}
            emptyLabel={t("No machines connected yet")}
          >
            {(data) => (
              <SortableTable
                columns={machineColumns}
                rows={data.rows}
                rowKey={(r) => r[0]}
                defaultSortKey="last_event_ts"
                caption={t(
                  "List of connected machines: host, environment, OS, last event, and events in the last 30 days.",
                )}
              />
            )}
          </QueryBoundary>
        </section>
      )}
    </div>
  );
}
