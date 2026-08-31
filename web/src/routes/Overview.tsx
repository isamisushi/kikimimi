import { getMachines, getOverview } from "../api/client";
import { fmtCost, fmtNum } from "../api/format";
import { useAsync } from "../hooks/useAsync";
import { QueryBoundary } from "../components/QueryBoundary";
import { StatTile } from "../components/StatTile";
import { TokenBarChart, type TokenBarDatum } from "../components/TokenBarChart";
import { FreshnessBadge } from "../components/FreshnessBadge";
import { SortableTable, type ColumnDef } from "../components/SortableTable";
import type { MachineRow } from "../api/types";

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
  const overview = useAsync(() => getOverview(DAYS), [DAYS]);
  const machines = useAsync(() => getMachines(), []);

  return (
    <div className="page">
      <div className="page__header">
        <h1>Overview</h1>
        <p className="page__subtitle">Team usage over the last {DAYS} days</p>
      </div>

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
