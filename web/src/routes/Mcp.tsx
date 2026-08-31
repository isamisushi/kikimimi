import { getMcp } from "../api/client";
import { fmtDateShort, fmtNum } from "../api/format";
import { useAsync } from "../hooks/useAsync";
import { QueryBoundary } from "../components/QueryBoundary";
import { SortableTable, type ColumnDef } from "../components/SortableTable";
import type { McpRow } from "../api/types";

const DAYS = 14;

function isUnused(r: McpRow): boolean {
  return r[1] === 0 || r[1] === null;
}

const columns: ColumnDef<McpRow>[] = [
  {
    key: "mcp_server",
    label: "MCP server",
    sortValue: (r) => r[0],
    render: (r) => (
      <span className="mcp-name">
        <span className="mono">{r[0]}</span>
        {isUnused(r) && <span className="badge badge--warn">Unused</span>}
      </span>
    ),
  },
  {
    key: "calls",
    label: "Calls",
    align: "right",
    sortValue: (r) => r[1],
    render: (r) => fmtNum(r[1]),
  },
  {
    key: "failures",
    label: "Failures",
    align: "right",
    sortValue: (r) => r[2],
    render: (r) => (
      <span className={r[2] !== null && r[2] > 0 ? "text-danger" : undefined}>
        {fmtNum(r[2])}
      </span>
    ),
  },
  {
    key: "distinct_sessions",
    label: "Sessions using it",
    align: "right",
    sortValue: (r) => r[3],
    render: (r) => fmtNum(r[3]),
  },
  {
    key: "last_called_dt",
    label: "Last called",
    sortValue: (r) => r[4],
    render: (r) => (r[4] ? fmtDateShort(r[4]) : "–"),
  },
];

export function Mcp() {
  const mcp = useAsync(() => getMcp(DAYS), [DAYS]);

  return (
    <div className="page">
      <div className="page__header">
        <h1>MCP servers</h1>
        <p className="page__subtitle">
          Usage over the last {DAYS} days. Unused servers are candidates for removal or consolidation.
        </p>
      </div>

      <section className="panel">
        <QueryBoundary
          state={mcp}
          isEmpty={(d) => d.rows.length === 0}
          emptyLabel="No MCP servers connected"
          onRetry={mcp.reload}
        >
          {(data) => {
            const unusedCount = data.rows.filter(isUnused).length;
            return (
              <>
                {unusedCount > 0 && (
                  <div className="callout callout--warn">
                    <strong>{unusedCount}</strong> MCP server{unusedCount === 1 ? "" : "s"} not called in the last {DAYS}{" "}
                    days. Consider whether it's still worth giving agents access.
                  </div>
                )}
                <SortableTable
                  columns={columns}
                  rows={data.rows}
                  rowKey={(r) => r[0]}
                  defaultSortKey="calls"
                  defaultSortDir="asc"
                  rowClassName={(r) => (isUnused(r) ? "row-warn" : undefined)}
                  caption="Per-MCP-server usage: call count, failure count, sessions using it, and last called date."
                />
              </>
            );
          }}
        </QueryBoundary>
      </section>
    </div>
  );
}
