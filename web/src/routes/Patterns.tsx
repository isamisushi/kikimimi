import { useState } from "react";
import { getPatternHits, getPatterns } from "../api/client";
import { fmtDateShort, fmtDateTime, fmtNum } from "../api/format";
import { useAsync } from "../hooks/useAsync";
import { QueryBoundary } from "../components/QueryBoundary";
import { SortableTable, type ColumnDef } from "../components/SortableTable";
import type { PatternHitRow, PatternRow } from "../api/types";

const DAYS = 30;
const HIT_LIMIT = 50;

/** Plain-language label per pattern_id (architecture.md §7.2 table). */
const PATTERN_LABEL: Record<string, string> = {
  mcp_bypass: "MCP bypass",
  deny_detour: "Denied, then detoured",
  retry_spiral: "Retry spiral",
  permission_denied_loop: "Permission denied loop",
  context_bloat: "Context bloat",
  long_tool_tail: "Slow MCP call",
  unused_mcp_server: "Configured, never called",
};

/** What the subject column means for each pattern -- the thing to fix. */
const SUBJECT_HINT: Record<string, string> = {
  mcp_bypass: "MCP server that failed before the detour",
  deny_detour: "tool that was denied",
  retry_spiral: "tool that kept failing",
  permission_denied_loop: "tool that kept being denied",
  context_bloat: "last tool result before the jump",
  long_tool_tail: "MCP server with the slow call",
  unused_mcp_server: "MCP server carried for nothing",
};

function patternLabel(id: string): string {
  return PATTERN_LABEL[id] ?? id;
}

function rowId(r: PatternRow): string {
  return `${r[0]}:${r[1]}`;
}

const columns: ColumnDef<PatternRow>[] = [
  {
    key: "pattern_id",
    label: "Pattern",
    sortValue: (r) => r[0],
    render: (r) => <span title={r[0]}>{patternLabel(r[0])}</span>,
  },
  {
    key: "subject",
    label: "Subject",
    sortValue: (r) => r[1],
    render: (r) => (
      <span className="mono" title={SUBJECT_HINT[r[0]] ?? ""}>
        {r[1]}
      </span>
    ),
  },
  {
    key: "sessions",
    label: "Sessions",
    align: "right",
    sortValue: (r) => r[2],
    render: (r) => fmtNum(r[2]),
  },
  {
    key: "incidents",
    label: "Incidents",
    align: "right",
    sortValue: (r) => r[3],
    render: (r) => fmtNum(r[3]),
  },
  {
    key: "wasted_tokens_est",
    label: "Wasted tokens (est.)",
    align: "right",
    sortValue: (r) => r[4],
    render: (r) =>
      r[4] === null ? (
        <span
          className="text-muted"
          title="No OTel usage fell inside these incidents, so the cost is unknown -- not zero."
        >
          unknown
        </span>
      ) : (
        <span title={`${r[5]} of ${r[6]} incidents priced`}>{fmtNum(r[4])}</span>
      ),
  },
  {
    key: "priority",
    label: "Priority",
    align: "right",
    sortValue: (r) => r[7],
    render: (r) => (r[7] === null ? <span className="text-muted">–</span> : fmtNum(r[7])),
  },
  {
    key: "last_seen_dt",
    label: "Last seen",
    sortValue: (r) => r[9],
    render: (r) => fmtDateShort(r[9]),
  },
];

const hitColumns: ColumnDef<PatternHitRow>[] = [
  {
    key: "session_id",
    label: "Session",
    sortValue: (r) => r[1],
    render: (r) => (
      <span className="mono session-id" title={r[1]}>
        {r[1].slice(0, 8)}
      </span>
    ),
  },
  {
    key: "first_ts",
    label: "When",
    sortValue: (r) => r[2],
    render: (r) => fmtDateTime(new Date(r[2]).toISOString()),
  },
  {
    key: "incidents",
    label: "Incidents",
    align: "right",
    sortValue: (r) => r[4],
    render: (r) => fmtNum(r[4]),
  },
  {
    key: "wasted_tokens_est",
    label: "Wasted tokens (est.)",
    align: "right",
    sortValue: (r) => r[5],
    render: (r) => (r[5] === null ? <span className="text-muted">unknown</span> : fmtNum(r[5])),
  },
  {
    key: "detail",
    label: "Detail",
    render: (r) => <span className="mono detail-json">{r[6] ?? ""}</span>,
  },
];

function Drilldown({ row }: { row: PatternRow }) {
  const hits = useAsync(() => getPatternHits(row[0], row[1], DAYS, HIT_LIMIT), [row[0], row[1]]);
  return (
    <div className="drilldown">
      <p className="page__subtitle">
        {patternLabel(row[0])} · <span className="mono">{row[1]}</span> — most recent {HIT_LIMIT} incidents
        (metadata only).
      </p>
      <QueryBoundary
        state={hits}
        isEmpty={(d) => d.rows.length === 0}
        emptyLabel="No incidents visible to you in this window"
        onRetry={hits.reload}
      >
        {(data) => (
          <SortableTable
            columns={hitColumns}
            rows={data.rows}
            rowKey={(r, i) => `${r[1]}-${r[2]}-${i}`}
            defaultSortKey="first_ts"
            caption="Incidents behind this ranking row: session, time, incident count, estimated wasted tokens, and detail."
          />
        )}
      </QueryBoundary>
    </div>
  );
}

export function Patterns() {
  const patterns = useAsync(() => getPatterns(DAYS), [DAYS]);
  const [selected, setSelected] = useState<string | null>(null);

  return (
    <div className="page">
      <div className="page__header">
        <h1>Struggles</h1>
        <p className="page__subtitle">
          Where agents get stuck, ranked by estimated wasted tokens × sessions over the last {DAYS} days. Each row
          names the MCP server or tool to fix, never a person. Click a row for the sessions behind it.
        </p>
      </div>

      <section className="panel">
        <QueryBoundary
          state={patterns}
          isEmpty={(d) => d.rows.length === 0}
          emptyLabel="No struggle patterns detected in this window"
          onRetry={patterns.reload}
        >
          {(data) => {
            const selectedRow = selected ? data.rows.find((r) => rowId(r) === selected) : undefined;
            return (
              <>
                <SortableTable
                  columns={columns}
                  rows={data.rows}
                  rowKey={rowId}
                  defaultSortKey="priority"
                  onRowClick={(r) => setSelected(selected === rowId(r) ? null : rowId(r))}
                  rowClassName={(r) => (selected === rowId(r) ? "row-selected" : undefined)}
                  caption="Struggle patterns ranked by priority: pattern, subject, sessions, incidents, estimated wasted tokens, priority, last seen."
                />
                {selectedRow && <Drilldown row={selectedRow} />}
              </>
            );
          }}
        </QueryBoundary>
      </section>
    </div>
  );
}
