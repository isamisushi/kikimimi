import { t } from "../i18n";
import { useState } from "react";
import {
  apiErrorMessage,
  createMark,
  deleteMark,
  getMarks,
  getPatternHits,
  getPatternTimeline,
  getPatterns,
} from "../api/client";
import { fmtDateShort, fmtDateTime, fmtNum } from "../api/format";
import { useAsync } from "../hooks/useAsync";
import { useSession } from "../hooks/useSession";
import { QueryBoundary } from "../components/QueryBoundary";
import { SortableTable, type ColumnDef } from "../components/SortableTable";
import type {
  ImprovementMark,
  PatternHitRow,
  PatternRow,
  PatternTimelineRow,
} from "../api/types";

const DAYS = 30;
const HIT_LIMIT = 50;
const TREND_DAYS = 60;

/** Plain-language label per pattern_id (architecture.md §7.2 table). */
const PATTERN_LABEL: Record<string, string> = {
  get mcp_bypass() {
    return t("MCP bypass");
  },
  get deny_detour() {
    return t("Denied, then detoured");
  },
  get retry_spiral() {
    return t("Retry spiral");
  },
  get permission_denied_loop() {
    return t("Permission denied loop");
  },
  get context_bloat() {
    return t("Context bloat");
  },
  get long_tool_tail() {
    return t("Slow MCP call");
  },
  get unused_mcp_server() {
    return t("Configured, never called");
  },
};

/** What the subject column means for each pattern -- the thing to fix. */
const SUBJECT_HINT: Record<string, string> = {
  get mcp_bypass() {
    return t("MCP server that failed before the detour");
  },
  get deny_detour() {
    return t("tool that was denied");
  },
  get retry_spiral() {
    return t("tool that kept failing");
  },
  get permission_denied_loop() {
    return t("tool that kept being denied");
  },
  get context_bloat() {
    return t("last tool result before the jump");
  },
  get long_tool_tail() {
    return t("MCP server with the slow call");
  },
  get unused_mcp_server() {
    return t("MCP server carried for nothing");
  },
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
    get label() {
      return t("Pattern");
    },
    sortValue: (r) => r[0],
    render: (r) => <span title={r[0]}>{patternLabel(r[0])}</span>,
  },
  {
    key: "subject",
    get label() {
      return t("Subject");
    },
    sortValue: (r) => r[1],
    render: (r) => (
      <span className="mono" title={SUBJECT_HINT[r[0]] ?? ""}>
        {r[1]}
      </span>
    ),
  },
  {
    key: "sessions",
    get label() {
      return t("Sessions");
    },
    align: "right",
    sortValue: (r) => r[2],
    render: (r) => fmtNum(r[2]),
  },
  {
    key: "incidents",
    get label() {
      return t("Incidents");
    },
    align: "right",
    sortValue: (r) => r[3],
    render: (r) => fmtNum(r[3]),
  },
  {
    key: "wasted_tokens_est",
    get label() {
      return t("Wasted tokens (est.)");
    },
    align: "right",
    sortValue: (r) => r[4],
    render: (r) =>
      r[4] === null ? (
        <span
          className="text-muted"
          title={t(
            "No OTel usage fell inside these incidents, so the cost is unknown -- not zero.",
          )}
        >
          {t("unknown")}
        </span>
      ) : (
        <span title={t("{0} of {1} incidents priced", [r[5], r[6]])}>
          {fmtNum(r[4])}
        </span>
      ),
  },
  {
    key: "priority",
    get label() {
      return t("Priority");
    },
    align: "right",
    sortValue: (r) => r[7],
    render: (r) =>
      r[7] === null ? <span className="text-muted">–</span> : fmtNum(r[7]),
  },
  {
    key: "last_seen_dt",
    get label() {
      return t("Last seen");
    },
    sortValue: (r) => r[9],
    render: (r) => fmtDateShort(r[9]),
  },
];

const hitColumns: ColumnDef<PatternHitRow>[] = [
  {
    key: "session_id",
    get label() {
      return t("Session");
    },
    sortValue: (r) => r[1],
    render: (r) => (
      <span className="mono session-id" title={r[1]}>
        {r[1].slice(0, 8)}
      </span>
    ),
  },
  {
    key: "first_ts",
    get label() {
      return t("When");
    },
    sortValue: (r) => r[2],
    render: (r) => fmtDateTime(new Date(r[2]).toISOString()),
  },
  {
    key: "incidents",
    get label() {
      return t("Incidents");
    },
    align: "right",
    sortValue: (r) => r[4],
    render: (r) => fmtNum(r[4]),
  },
  {
    key: "wasted_tokens_est",
    get label() {
      return t("Wasted tokens (est.)");
    },
    align: "right",
    sortValue: (r) => r[5],
    render: (r) =>
      r[5] === null ? (
        <span className="text-muted">{t("unknown")}</span>
      ) : (
        fmtNum(r[5])
      ),
  },
  {
    key: "detail",
    get label() {
      return t("Detail");
    },
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
      wasted:
        priced.length === 0
          ? null
          : priced.reduce((a, r) => a + (r[5] ?? 0), 0),
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
  const timeline = useAsync(
    () => getPatternTimeline(row[0], row[1], TREND_DAYS),
    [row[0], row[1]],
  );
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

  const markList: ImprovementMark[] =
    marks.status === "ok" ? marks.data.marks : [];
  const latest = markList.length > 0 ? markList[markList.length - 1] : null;

  return (
    <div className="trend">
      <h3>
        {t("Trend — last")} {TREND_DAYS} {t("days")}
      </h3>
      <QueryBoundary
        state={timeline}
        isEmpty={(d) => d.rows.length === 0}
        emptyLabel={t("No sessions in this window")}
        onRetry={timeline.reload}
      >
        {(data) => {
          const split = latest
            ? splitAtMark(data.rows, latest.marked_dt)
            : null;
          return (
            <>
              {split && latest && (
                <div className="before-after">
                  <div className="stat-tile">
                    <div className="stat-tile__label">
                      {t("Before")} {fmtDateShort(latest.marked_dt)} (
                      {split.before.days} {t("days)")}
                    </div>
                    <div className="stat-tile__value">
                      {fmtRate(split.before.rate)}
                    </div>
                    <div className="stat-tile__sub">
                      {t("sessions hit · wasted")} {fmtNum(split.before.wasted)}
                    </div>
                  </div>
                  <div className="stat-tile">
                    <div className="stat-tile__label">
                      {t("After (")}
                      {split.after.days} {t("days)")}
                    </div>
                    <div className="stat-tile__value">
                      {fmtRate(split.after.rate)}
                    </div>
                    <div className="stat-tile__sub">
                      {t("sessions hit · wasted")} {fmtNum(split.after.wasted)}
                    </div>
                  </div>
                </div>
              )}
              <div className="table-scroll">
                <table className="data-table">
                  <caption className="sr-only">
                    {t(
                      "Per-day sessions, sessions hit, hit rate and estimated wasted tokens, with improvement marks.",
                    )}
                  </caption>
                  <thead>
                    <tr>
                      <th scope="col">{t("Day")}</th>
                      <th scope="col" className="align-right">
                        {t("Sessions")}
                      </th>
                      <th scope="col" className="align-right">
                        {t("Hit")}
                      </th>
                      <th scope="col" className="align-right">
                        {t("Rate")}
                      </th>
                      <th scope="col" className="align-right">
                        {t("Incidents")}
                      </th>
                      <th scope="col" className="align-right">
                        {t("Wasted tokens (est.)")}
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    {data.rows.map((r) => (
                      <tr
                        key={r[0]}
                        className={
                          markList.some((m) => m.marked_dt === r[0])
                            ? "row-mark"
                            : undefined
                        }
                      >
                        <td>
                          {fmtDateShort(r[0])}
                          {markList
                            .filter((m) => m.marked_dt === r[0])
                            .map((m) => (
                              <span
                                key={m.id}
                                className="badge badge--neutral"
                                title={m.note}
                              >
                                {t("improved")}
                                {m.note ? `: ${m.note}` : ""}
                              </span>
                            ))}
                        </td>
                        <td className="align-right">{fmtNum(r[1])}</td>
                        <td className="align-right">{fmtNum(r[2])}</td>
                        <td className="align-right">{fmtRate(r[3])}</td>
                        <td className="align-right">{fmtNum(r[4])}</td>
                        <td className="align-right">
                          {r[5] === null ? (
                            <span className="text-muted">{t("unknown")}</span>
                          ) : (
                            fmtNum(r[5])
                          )}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </>
          );
        }}
      </QueryBoundary>

      {session?.data_source !== "s3" && (
        <form className="mark-form" onSubmit={onSubmit}>
          <div className="field field--inline">
            <label className="field__label" htmlFor="mark-dt">
              {t("Mark an improvement")}
            </label>
            <input
              id="mark-dt"
              type="date"
              value={markedDt}
              onChange={(e) => setMarkedDt(e.target.value)}
              required
            />
            <input
              type="text"
              placeholder={t(
                "what changed (e.g. added search filters to the gh MCP)",
              )}
              value={note}
              maxLength={500}
              onChange={(e) => setNote(e.target.value)}
            />
            <button
              type="submit"
              className="btn btn--primary btn--small"
              disabled={saving}
            >
              {saving ? t("Saving…") : t("Save")}
            </button>
          </div>
          {error && <div className="callout callout--warn">{error}</div>}
        </form>
      )}
      {markList.length > 0 && (
        <ul className="mark-list">
          {markList.map((m) => (
            <li key={m.id}>
              <span className="mono">{m.marked_dt}</span>{" "}
              {m.note || <span className="text-muted">{t("(no note)")}</span>}{" "}
              <button
                type="button"
                className="btn btn--ghost btn--small"
                onClick={() => onDelete(m)}
              >
                {t("remove")}
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

function Drilldown({ row }: { row: PatternRow }) {
  const hits = useAsync(
    () => getPatternHits(row[0], row[1], DAYS, HIT_LIMIT),
    [row[0], row[1]],
  );
  return (
    <div className="drilldown">
      <p className="page__subtitle">
        {patternLabel(row[0])} · <span className="mono">{row[1]}</span>{" "}
        {t("— most recent")} {HIT_LIMIT} {t("incidents (metadata only).")}
      </p>
      <QueryBoundary
        state={hits}
        isEmpty={(d) => d.rows.length === 0}
        emptyLabel={t("No incidents visible to you in this window")}
        onRetry={hits.reload}
      >
        {(data) => (
          <SortableTable
            columns={hitColumns}
            rows={data.rows}
            rowKey={(r, i) => `${r[1]}-${r[2]}-${i}`}
            defaultSortKey="first_ts"
            caption={t(
              "Incidents behind this ranking row: session, time, incident count, estimated wasted tokens, and detail.",
            )}
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
        <h1>{t("Struggles")}</h1>
        <p className="page__subtitle">
          {t(
            "Where agents get stuck, ranked by estimated wasted tokens × sessions over the last",
          )}{" "}
          {DAYS}{" "}
          {t(
            "days. Each row names the MCP server or tool to fix, never a person. Click a row for the sessions behind it.",
          )}
        </p>
      </div>

      <section className="panel">
        <QueryBoundary
          state={patterns}
          isEmpty={(d) => d.rows.length === 0}
          emptyLabel={t("No struggle patterns detected in this window")}
          onRetry={patterns.reload}
        >
          {(data) => {
            const selectedRow = selected
              ? data.rows.find((r) => rowId(r) === selected)
              : undefined;
            return (
              <>
                <SortableTable
                  columns={columns}
                  rows={data.rows}
                  rowKey={rowId}
                  defaultSortKey="priority"
                  onRowClick={(r) =>
                    setSelected(selected === rowId(r) ? null : rowId(r))
                  }
                  rowClassName={(r) =>
                    selected === rowId(r) ? "row-selected" : undefined
                  }
                  caption={t(
                    "Struggle patterns ranked by priority: pattern, subject, sessions, incidents, estimated wasted tokens, priority, last seen.",
                  )}
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
