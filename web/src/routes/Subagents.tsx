import { getSubagents } from "../api/client";
import { fmtDateTime, fmtMs, fmtNum, fmtStr } from "../api/format";
import { useAsync } from "../hooks/useAsync";
import { QueryBoundary } from "../components/QueryBoundary";
import { SortableTable, type ColumnDef } from "../components/SortableTable";
import type { SubagentRow } from "../api/types";

const DAYS = 14;
const LIMIT = 50;

function fmtShare(value: number | null): string {
  if (value === null || value === undefined) return "unknown";
  return `${Math.round(value * 100)}%`;
}

const columns: ColumnDef<SubagentRow>[] = [
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
    key: "started_at",
    label: "Started",
    sortValue: (r) => new Date(r[1]).getTime(),
    render: (r) => fmtDateTime(r[1]),
  },
  {
    key: "subagents",
    label: "Subagents",
    align: "right",
    sortValue: (r) => r[2],
    render: (r) => fmtNum(r[2]),
  },
  {
    key: "agent_types",
    label: "Types",
    sortValue: (r) => r[3] ?? "",
    render: (r) => <span className="mono">{fmtStr(r[3])}</span>,
  },
  {
    key: "subagent_tool_calls",
    label: "Tool calls (sub / all)",
    align: "right",
    sortValue: (r) => r[4],
    render: (r) => `${fmtNum(r[4])} / ${fmtNum(r[5])}`,
  },
  {
    key: "subagent_duration_ms",
    label: "Subagent time",
    align: "right",
    sortValue: (r) => r[6] ?? -1,
    render: (r) => fmtMs(r[6]),
  },
  {
    key: "duration_share",
    label: "Time share",
    align: "right",
    sortValue: (r) => r[8] ?? -1,
    render: (r) => fmtShare(r[8]),
  },
  {
    key: "subagent_tokens_est",
    label: "Subagent tokens",
    align: "right",
    sortValue: (r) => r[10] ?? -1,
    render: (r) => (r[10] === null ? <span className="text-muted">unknown</span> : fmtNum(r[10])),
  },
  {
    key: "token_share",
    label: "Token share",
    align: "right",
    sortValue: (r) => r[12] ?? -1,
    render: (r) => fmtShare(r[12]),
  },
  {
    key: "subagents_with_usage",
    label: "Priced",
    align: "right",
    sortValue: (r) => r[13] / Math.max(r[2], 1),
    render: (r) => `${fmtNum(r[13])} / ${fmtNum(r[2])}`,
  },
];

export function Subagents() {
  const data = useAsync(() => getSubagents(DAYS, LIMIT), [DAYS, LIMIT]);

  return (
    <div className="page">
      <div className="page__header">
        <h1>Subagents</h1>
        <p className="page__subtitle">
          Sessions that fanned work out to Agent-tool subagents in the last {DAYS} days, most recent{" "}
          {LIMIT}. Subagents stay inside their parent session; this shows how much of the session they
          were. Token figures come from transcripts and only exist where Claude Code recorded them —
          &ldquo;Priced&rdquo; is how many of the subagents that was true for.
        </p>
      </div>

      <section className="panel">
        <QueryBoundary
          state={data}
          isEmpty={(d) => d.rows.length === 0}
          emptyLabel="No subagent runs in this period"
          onRetry={data.reload}
        >
          {(d) => (
            <SortableTable
              columns={columns}
              rows={d.rows}
              rowKey={(r) => r[0]}
              defaultSortKey="started_at"
              caption="Per-session subagent fan-out: count, types, tool calls, time and token share, and how many subagents had usage recorded."
            />
          )}
        </QueryBoundary>
      </section>
    </div>
  );
}
