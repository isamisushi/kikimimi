import { useState } from "react";
import { apiErrorMessage, createMark, deleteMark, getMarks, getPatternHits, getPatternTimeline, getPatterns } from "../api/client";
import { fmtDateShort, fmtDateTime, fmtNum } from "../api/format";
import { useAsync } from "../hooks/useAsync";
import { useSession } from "../hooks/useSession";
import { QueryBoundary } from "../components/QueryBoundary";
import { SortableTable, type ColumnDef } from "../components/SortableTable";
import type { ImprovementMark, PatternHitRow, PatternRow, PatternTimelineRow } from "../api/types";

const DAYS = 30;
const HIT_LIMIT = 50;
const TREND_DAYS = 60;

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

function todayIso(): string {
  return new Date().toISOString().slice(0, 10);
}

interface Split {
  before: { days: number; rate: number | null; wasted: number | null };
  after: { days: number; rate: number | null; wasted: number | null };
}

/** Average hit rate and total cost on each side of the latest mark. `rate`
 * is a session-weighted average (hits / sessions), null when that side has
 * no sessions at all; `wasted` sums priced days and is null when none were. */
function splitAtMark(rows: PatternTimelineRow[], markedDt: string): Split {
  const side = (part: PatternTimelineRow[]) => {
    const sessions = part.reduce((a, r) => a + r[1], 0);
    const hits = part.reduce((a, r) => a + r[2], 0);
    const priced = part.filter((r) => r[5] !== null);
    return {
      days: part.length,
      rate: sessions === 0 ? null : (100 * hits) / sessions,
      wasted: priced.length === 0 ? null : priced.reduce((a, r) => a + (r[5] ?? 0), 0),
    };
  };
  return {
    before: side(rows.filter((r) => r[0] < markedDt)),
    after: side(rows.filter((r) => r[0] >= markedDt)),
  };
}

function fmtRate(v: number | null): string {
  return v === null ? "–" : `${v.toFixed(1)}%`;
}

function Trend({ row }: { row: PatternRow }) {
  const { session } = useSession();
  const timeline = useAsync(() => getPatternTimeline(row[0], row[1], TREND_DAYS), [row[0], row[1]]);
  const marks = useAsync(() => getMarks(row[0], row[1]), [row[0], row[1]]);
  const [markedDt, setMarkedDt] = useState(todayIso());
  const [note, setNote] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function onSubmit(e: React.FormEvent) {
    e.preventDefault();
    setSaving(true);
    setError(null);
    try {
      await createMark(row[0], row[1], markedDt, note);
      setNote("");
      marks.reload();
    } catch (err) {
      setError(apiErrorMessage(err));
    } finally {
      setSaving(false);
    }
  }

  async function onDelete(m: ImprovementMark) {
    setError(null);
    try {
      await deleteMark(m.id);
      marks.reload();
    } catch (err) {
      setError(apiErrorMessage(err));
    }
  }

  const markList: ImprovementMark[] = marks.status === "ok" ? marks.data.marks : [];
  const latest = markList.length > 0 ? markList[markList.length - 1] : null;

  return (
    <div className="trend">
      <h3>Trend — last {TREND_DAYS} days</h3>
      <QueryBoundary
        state={timeline}
        isEmpty={(d) => d.rows.length === 0}
        emptyLabel="No sessions in this window"
        onRetry={timeline.reload}
      >
        {(data) => {
          const split = latest ? splitAtMark(data.rows, latest.marked_dt) : null;
          return (
            <>
              {split && latest && (
                <div className="before-after">
                  <div className="stat-tile">
                    <div className="stat-tile__label">Before {fmtDateShort(latest.marked_dt)} ({split.before.days} days)</div>
                    <div className="stat-tile__value">{fmtRate(split.before.rate)}</div>
                    <div className="stat-tile__sub">sessions hit · wasted {fmtNum(split.before.wasted)}</div>
                  </div>
                  <div className="stat-tile">
                    <div className="stat-tile__label">After ({split.after.days} days)</div>
                    <div className="stat-tile__value">{fmtRate(split.after.rate)}</div>
                    <div className="stat-tile__sub">sessions hit · wasted {fmtNum(split.after.wasted)}</div>
                  </div>
                </div>
              )}
              <div className="table-scroll">
                <table className="data-table">
                  <caption className="sr-only">Per-day sessions, sessions hit, hit rate and estimated wasted tokens, with improvement marks.</caption>
                  <thead>
                    <tr>
                      <th scope="col">Day</th>
                      <th scope="col" className="align-right">Sessions</th>
                      <th scope="col" className="align-right">Hit</th>
                      <th scope="col" className="align-right">Rate</th>
                      <th scope="col" className="align-right">Incidents</th>
                      <th scope="col" className="align-right">Wasted tokens (est.)</th>
                    </tr>
                  </thead>
                  <tbody>
                    {data.rows.map((r) => (
                      <tr key={r[0]} className={markList.some((m) => m.marked_dt === r[0]) ? "row-mark" : undefined}>
                        <td>
                          {fmtDateShort(r[0])}
                          {markList
                            .filter((m) => m.marked_dt === r[0])
                            .map((m) => (
                              <span key={m.id} className="badge badge--neutral" title={m.note}>
                                improved{m.note ? `: ${m.note}` : ""}
                              </span>
                            ))}
                        </td>
                        <td className="align-right">{fmtNum(r[1])}</td>
                        <td className="align-right">{fmtNum(r[2])}</td>
                        <td className="align-right">{fmtRate(r[3])}</td>
                        <td className="align-right">{fmtNum(r[4])}</td>
                        <td className="align-right">{r[5] === null ? <span className="text-muted">unknown</span> : fmtNum(r[5])}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </>
          );
        }}
      </QueryBoundary>

      {session?.data_source !== "s3" && <form className="mark-form" onSubmit={onSubmit}>
        <div className="field field--inline">
          <label className="field__label" htmlFor="mark-dt">Mark an improvement</label>
          <input id="mark-dt" type="date" value={markedDt} onChange={(e) => setMarkedDt(e.target.value)} required />
          <input
            type="text"
            placeholder="what changed (e.g. added search filters to the gh MCP)"
            value={note}
            maxLength={500}
            onChange={(e) => setNote(e.target.value)}
          />
          <button type="submit" className="btn btn--primary btn--small" disabled={saving}>
            {saving ? "Saving…" : "Save"}
          </button>
        </div>
        {error && <div className="callout callout--warn">{error}</div>}
      </form>}
      {markList.length > 0 && (
        <ul className="mark-list">
          {markList.map((m) => (
            <li key={m.id}>
              <span className="mono">{m.marked_dt}</span> {m.note || <span className="text-muted">(no note)</span>}{" "}
              <button type="button" className="btn btn--ghost btn--small" onClick={() => onDelete(m)}>
                remove
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

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
      <Trend row={row} />
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
