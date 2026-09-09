import { getCoverage, getMachines, getOverview } from "../api/client";
import { COVERAGE_DAYS, fmtPct, summarize } from "../api/coverage";
import { fmtCost, fmtNum } from "../api/format";
import { useAsync } from "../hooks/useAsync";
import { QueryBoundary } from "../components/QueryBoundary";
import { StatTile } from "../components/StatTile";
import { TokenBarChart, type TokenBarDatum } from "../components/TokenBarChart";
import { FreshnessBadge } from "../components/FreshnessBadge";
import { SortableTable, type ColumnDef } from "../components/SortableTable";
import type { MachineRow } from "../api/types";
import { SubscriptionUsage } from "../components/SubscriptionUsage";
import { useSession } from "../hooks/useSession";

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
function partialHint(unknownDays: number, total: number | null): string | undefined {
  if (total === null || unknownDays === 0) return undefined;
  return `${unknownDays} day${unknownDays === 1 ? "" : "s"} unknown (excluded from total)`;
}

const machineColumns: ColumnDef<MachineRow>[] = [
  {
    key: "host_id",
    label: "Host",
    sortValue: (r) => r[0],
    render: (r) => <span className="mono">{r[0]}</span>,
  },
  {
    key: "env_kind",
    label: "Environment",
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
    label: "Last event",
    sortValue: (r) => (r[3] ? new Date(r[3]).getTime() : null),
    render: (r) => <FreshnessBadge lastEventTs={r[3]} />,
  },
  {
    key: "events_30d",
    label: "Events (30d)",
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
        <h1>Overview</h1>
        <p className="page__subtitle">Usage over the last {DAYS} days</p>
      </div>

      {session?.subscription_usage && <SubscriptionUsage />}

      <QueryBoundary state={overview} isEmpty={(d) => d.rows.length === 0}>
        {(data) => {
          const rows = data.rows;
          const totalEvents = sumOrNull(rows.map((r) => r[1]));
          const totalToolCalls = sumOrNull(rows.map((r) => r[2]));
          const totalFailures = sumOrNull(rows.map((r) => r[3]));
          const totalCost = sumOrNull(rows.map((r) => r[6]));

          const eventsUnknownDays = countUnknown(rows.map((r) => r[1]));
          const toolCallsUnknownDays = countUnknown(rows.map((r) => r[2]));
          const failuresUnknownDays = countUnknown(rows.map((r) => r[3]));
          const costUnknownDays = countUnknown(rows.map((r) => r[6]));

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
                  label="Events"
                  value={fmtNum(totalEvents)}
                  hint={partialHint(eventsUnknownDays, totalEvents)}
                />
                <StatTile
                  label="Tool calls"
                  value={fmtNum(totalToolCalls)}
                  hint={partialHint(toolCallsUnknownDays, totalToolCalls)}
                />
                <StatTile
                  label="Failures"
                  value={fmtNum(totalFailures)}
                  tone={totalFailures && totalFailures > 0 ? "danger" : "default"}
                  hint={partialHint(failuresUnknownDays, totalFailures)}
                />
                <StatTile
                  label="Cost"
                  value={fmtCost(totalCost)}
                  hint={partialHint(costUnknownDays, totalCost)}
                />
              </div>

              <section className="panel">
                <h2 className="panel__title">Daily token usage</h2>
                <TokenBarChart data={chartData} />
              </section>
            </>
          );
        }}
      </QueryBoundary>

      <section className="panel" id="coverage">
        <h2 className="panel__title">Data coverage (last {COVERAGE_DAYS} days)</h2>
        <p className="panel__note">
          What the numbers on every page cannot see. A rate is "–" when there is nothing to
          divide, never 0%.
        </p>
        <QueryBoundary
          state={coverage}
          isEmpty={(d) => d.rows.length === 0}
          emptyLabel="No events yet"
        >
          {(data) => {
            const r = data.rows[0];
            const s = summarize(r);
            const tone = (v: number | null, min: number) =>
              v !== null && v < min ? "danger" : "default";
            return (
              <div className="stat-grid">
                <StatTile
                  label="Sessions with usage"
                  value={fmtPct(s.usageKnownPct)}
                  tone={tone(s.usageKnownPct, 80)}
                  hint={`${fmtNum(r[3])} of ${fmtNum(r[2])} sessions had no token usage`}
                />
                <StatTile
                  label="Hook ↔ OTel match"
                  value={fmtPct(s.hookOtelMatchPct)}
                  tone={tone(s.hookOtelMatchPct, 80)}
                  hint={`${fmtNum(r[6])} of ${fmtNum(r[4])} hook tool results also seen by OTel`}
                />
                <StatTile
                  label="Dedup dropped"
                  value={fmtPct(s.dedupDroppedPct)}
                  hint={`${fmtNum(r[7] - r[8])} of ${fmtNum(r[7])} tool results were the same call twice`}
                />
                <StatTile
                  label="Subagents priced"
                  value={fmtPct(s.subagentUsageKnownPct)}
                  hint={`${fmtNum(r[10])} of ${fmtNum(r[9])} subagents had usage recorded`}
                />
                <StatTile
                  label="Attributed to a person"
                  value={fmtPct(s.attributedPct)}
                  hint={
                    r[1] === r[0] && r[0] > 0
                      ? "local data has no accounts"
                      : `${fmtNum(r[1])} of ${fmtNum(r[0])} events have no account`
                  }
                />
                <StatTile
                  label="Hosts silent > 24h"
                  value={`${fmtNum(r[12])} / ${fmtNum(r[11])}`}
                  tone={r[12] > 0 ? "danger" : "default"}
                  hint={r[13] ? `last event ${r[13]}` : undefined}
                />
              </div>
            );
          }}
        </QueryBoundary>
      </section>

      <section className="panel">
        <h2 className="panel__title">Machines</h2>
        <QueryBoundary
          state={machines}
          isEmpty={(d) => d.rows.length === 0}
          emptyLabel="No machines connected yet"
        >
          {(data) => (
            <SortableTable
              columns={machineColumns}
              rows={data.rows}
              rowKey={(r) => r[0]}
              defaultSortKey="last_event_ts"
              caption="List of connected machines: host, environment, OS, last event, and events in the last 30 days."
            />
          )}
        </QueryBoundary>
      </section>
    </div>
  );
}
