import { getSessions } from "../api/client";
import { fmtCost, fmtDateTime, fmtNum, fmtStr } from "../api/format";
import { useAsync } from "../hooks/useAsync";
import { QueryBoundary } from "../components/QueryBoundary";
import { SortableTable, type ColumnDef } from "../components/SortableTable";
import type { SessionRow } from "../api/types";

const DAYS = 14;
const LIMIT = 50;

const columns: ColumnDef<SessionRow>[] = [
  {
    key: "session_id",
    label: "Session",
    sortValue: (r) => r[0],
    render: (r) => (
      <span className="mono session-id" title={r[0]}>
        {r[0].slice(0, 8)}
      </span>
    ),
  },
  {
    key: "agent",
    label: "Agent",
    sortValue: (r) => r[1],
    render: (r) => r[1],
  },
  {
    key: "host_id",
    label: "Host",
    sortValue: (r) => r[2],
    render: (r) => <span className="mono">{r[2]}</span>,
  },
  {
    key: "started_at",
    label: "Started",
    sortValue: (r) => new Date(r[3]).getTime(),
    render: (r) => fmtDateTime(r[3]),
  },
  {
    key: "events",
    label: "Events",
    align: "right",
    sortValue: (r) => r[4],
    render: (r) => fmtNum(r[4]),
  },
  {
    key: "tool_calls",
    label: "Tool calls",
    align: "right",
    sortValue: (r) => r[5],
    render: (r) => fmtNum(r[5]),
  },
  {
    key: "failures",
    label: "Failures",
    align: "right",
    sortValue: (r) => r[6],
    render: (r) => (
      <span className={r[6] !== null && r[6] > 0 ? "text-danger" : undefined}>
        {fmtNum(r[6])}
      </span>
    ),
  },
  {
    key: "models",
    label: "Models",
    sortValue: (r) => r[7],
    render: (r) => <span className="mono">{fmtStr(r[7])}</span>,
  },
  {
    key: "input_tokens",
    label: "Input tokens",
    align: "right",
    sortValue: (r) => r[8],
    render: (r) => fmtNum(r[8]),
  },
  {
    key: "output_tokens",
    label: "Output tokens",
    align: "right",
    sortValue: (r) => r[9],
    render: (r) => fmtNum(r[9]),
  },
  {
    key: "cost_usd",
    label: "Cost",
    align: "right",
    sortValue: (r) => r[10],
    render: (r) => fmtCost(r[10]),
  },
];

export function Sessions() {
  const sessions = useAsync(() => getSessions(DAYS, LIMIT), [DAYS, LIMIT]);

  return (
    <div className="page">
      <div className="page__header">
        <h1>Sessions</h1>
        <p className="page__subtitle">
          Last {DAYS} days, most recent {LIMIT}. Sessions with failures are highlighted.
        </p>
      </div>

      <section className="panel">
        <QueryBoundary
          state={sessions}
          isEmpty={(d) => d.rows.length === 0}
          emptyLabel="No sessions in this period"
          onRetry={sessions.reload}
        >
          {(data) => (
            <SortableTable
              columns={columns}
              rows={data.rows}
              rowKey={(r) => r[0]}
              defaultSortKey="started_at"
              rowClassName={(r) => (r[6] !== null && r[6] > 0 ? "row-danger" : undefined)}
              caption="List of sessions: agent, host, start time, event count, tool calls, failures, models, token counts, and cost."
            />
          )}
        </QueryBoundary>
      </section>
    </div>
  );
}
