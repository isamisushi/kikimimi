import { getTools } from "../api/client";
import { fmtMs, fmtNum } from "../api/format";
import { useAsync } from "../hooks/useAsync";
import { QueryBoundary } from "../components/QueryBoundary";
import { SortableTable, type ColumnDef } from "../components/SortableTable";
import type { ToolRow } from "../api/types";

const DAYS = 14;

const columns: ColumnDef<ToolRow>[] = [
  {
    key: "tool_name",
    label: "Tool",
    sortValue: (r) => r[0],
    render: (r) => <span className="mono">{r[0]}</span>,
  },
  {
    key: "tool_kind",
    label: "Kind",
    sortValue: (r) => r[1],
    render: (r) => <span className="badge badge--neutral">{r[1]}</span>,
  },
  {
    key: "calls",
    label: "Calls",
    align: "right",
    sortValue: (r) => r[2],
    render: (r) => fmtNum(r[2]),
  },
  {
    key: "failures",
    label: "Failures",
    align: "right",
    sortValue: (r) => r[3],
    render: (r) => (
      <span className={r[3] !== null && r[3] > 0 ? "text-danger" : undefined}>
        {fmtNum(r[3])}
      </span>
    ),
  },
  {
    key: "p50_duration_ms",
    label: "p50",
    align: "right",
    sortValue: (r) => r[4],
    render: (r) => fmtMs(r[4]),
  },
  {
    key: "p95_duration_ms",
    label: "p95",
    align: "right",
    sortValue: (r) => r[5],
    render: (r) => fmtMs(r[5]),
  },
];

export function Tools() {
  const tools = useAsync(() => getTools(DAYS), [DAYS]);

  return (
    <div className="page">
      <div className="page__header">
        <h1>Tools</h1>
        <p className="page__subtitle">Last {DAYS} days, sorted by call count</p>
      </div>

      <section className="panel">
        <QueryBoundary
          state={tools}
          isEmpty={(d) => d.rows.length === 0}
          emptyLabel="No tool calls in this period"
          onRetry={tools.reload}
        >
          {(data) => (
            <SortableTable
              columns={columns}
              rows={data.rows}
              rowKey={(r) => r[0]}
              defaultSortKey="calls"
              rowClassName={(r) => (r[3] !== null && r[3] > 0 ? "row-danger" : undefined)}
              caption="Per-tool call statistics: tool name, kind, call count, failure count, and p50/p95 duration."
            />
          )}
        </QueryBoundary>
      </section>
    </div>
  );
}
