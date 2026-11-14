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
import { Link } from "../router/Router";
import {
  collectionReady,
  useCollectionHealth,
} from "../components/CollectionHealth";
import { compareChange } from "../improvements/compare";
import { fmtDateShort, fmtDateTime, fmtNum } from "../api/format";
import { useAsync } from "../hooks/useAsync";
import { useSession } from "../hooks/useSession";
import { QueryBoundary } from "../components/QueryBoundary";
import { SortableTable, type ColumnDef } from "../components/SortableTable";
import type { ImprovementMark, PatternHitRow, PatternRow } from "../api/types";

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
    return t("Configured MCP server with no observed calls");
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
      return t("Associated tokens (est.)");
    },
    align: "right",
    sortValue: (r) => r[4],
    render: (r) =>
      r[4] === null ? (
        <span
          className="text-muted"
          title={t(
            "No measured token usage fell inside these incidents. The amount is unknown, not zero.",
          )}
        >
          {t("unknown")}
        </span>
      ) : (
        <span
          title={t("{0} of {1} incidents have token estimates", [r[5], r[6]])}
        >
          {fmtNum(r[4])}
        </span>
      ),
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
      <Link
        to={`/sessions/${encodeURIComponent(r[1])}`}
        className="mono session-id"
        title={r[1]}
      >
        {r[1].slice(0, 8)}
      </Link>
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
      return t("Associated tokens (est.)");
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

type Subject = readonly [string, string];
function subjectKey(subject: Subject) {
  return JSON.stringify(subject);
}
function todayIso() {
  return new Date().toISOString().slice(0, 10);
}
function fmtRate(v: number | null) {
  return v === null ? t("Not observed") : `${v.toFixed(1)}%`;
}

const GUIDANCE: Record<
  string,
  { evidence: string; action: string; caveat: string }
> = {
  retry_spiral: {
    evidence: "The same tool failed at least three times in a detected run.",
    action:
      "Open an affected session. Check the tool's authentication, inputs and availability. Fix the cause, then retry the same kind of task.",
    caveat:
      "Repeated failures are an investigation lead. The recorded metadata does not establish their cause or the cost you could avoid.",
  },
  permission_denied_loop: {
    evidence: "The same tool was repeatedly denied in a session.",
    action:
      "Check the intended permissions and the requested operation in an affected session. Correct the agent instructions or narrowly scoped permissions, then repeat the task.",
    caveat:
      "A denial can be intentional. Do not broaden permissions just to remove this signal.",
  },
  mcp_bypass: {
    evidence:
      "An MCP failure was followed by a shell or browser call nearby in the session.",
    action:
      "Check the affected sessions for a broken connection or authentication. Keep a deliberate, working fallback when it helps the task.",
    caveat:
      "A fallback can be reasonable. This sequence alone does not demonstrate wasted work.",
  },
  deny_detour: {
    evidence:
      "A denied operation was followed by a shell or browser call nearby in the session.",
    action:
      "Review whether the follow-up respected the intended permission boundary. Correct the workflow or instructions before trying again.",
    caveat: "Nearby events do not prove the second call bypassed the denial.",
  },
  context_bloat: {
    evidence: "A large context jump or repeated compaction was observed.",
    action:
      "Inspect the surrounding session. If an oversized tool response is unnecessary, narrow its query or output and compare similar tasks.",
    caveat:
      "The preceding tool is a clue, not proof of the cause. Larger context may be necessary for the task.",
  },
  long_tool_tail: {
    evidence: "An MCP call was substantially slower than the observed median.",
    action:
      "Check response size, connection reliability and server latency in affected sessions. Retry comparable operations after the change.",
    caveat: "A slow tool call is not itself a measured LLM charge.",
  },
  unused_mcp_server: {
    evidence:
      "A server was listed as configured but had no observed calls in a session.",
    action:
      "Check whether the server is needed for this workflow. If it is optional, test a narrower configuration on comparable tasks.",
    caveat:
      "Unused does not prove billed overhead. The token allocation is a proxy and is not a savings estimate.",
  },
};
const COMPARISON_LABELS = {
  waiting: "Waiting for activity after the change",
  insufficient: "More observations needed",
  collection_unverified:
    "Collection must be checked before interpreting results",
  lower: "Lower observed recurrence rate",
  recurring: "The issue is still recurring",
  no_recurrence: "No recurrence in the observed sessions",
};

function Trend({
  subject,
  marks,
  onChange,
}: {
  subject: Subject;
  marks: ImprovementMark[];
  onChange: () => void;
}) {
  const { session } = useSession();
  const health = useCollectionHealth();
  const timeline = useAsync(
    () => getPatternTimeline(subject[0], subject[1], TREND_DAYS),
    [subject[0], subject[1]],
    30_000,
  );
  const [markedDt, setMarkedDt] = useState(todayIso);
  const [note, setNote] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const sortedMarks = [...marks].sort(
    (a, b) =>
      a.marked_dt.localeCompare(b.marked_dt) ||
      a.created_at.localeCompare(b.created_at),
  );
  const latest = sortedMarks[sortedMarks.length - 1];
  const writable = session?.data_source !== "s3";

  async function onSubmit(e: React.FormEvent) {
    e.preventDefault();
    setSaving(true);
    setError(null);
    try {
      await createMark(subject[0], subject[1], markedDt, note.trim());
      setNote("");
      onChange();
      timeline.reload();
    } catch (err) {
      setError(apiErrorMessage(err));
    } finally {
      setSaving(false);
    }
  }
  async function onDelete(mark: ImprovementMark) {
    setSaving(true);
    setError(null);
    try {
      await deleteMark(mark.id);
      onChange();
    } catch (err) {
      setError(apiErrorMessage(err));
    } finally {
      setSaving(false);
    }
  }

  return (
    <section className="trend" aria-labelledby="verify-heading">
      <h3 id="verify-heading">{t("3. Check after the change")}</h3>
      <p className="panel__note">
        {t(
          "Recording a change does not mark an issue as resolved. Compare observed recurrence after similar work resumes.",
        )}
      </p>
      <QueryBoundary state={timeline} onRetry={timeline.reload}>
        {(data) => {
          const comparison = latest
            ? compareChange(
                data.rows,
                latest.marked_dt,
                todayIso(),
                writable && collectionReady(health),
              )
            : null;
          return (
            <>
              {comparison && latest ? (
                <div className="verification" role="status">
                  <h4>{t(COMPARISON_LABELS[comparison.status])}</h4>
                  <p>
                    {t("Latest change")}: {fmtDateShort(latest.marked_dt)} ·{" "}
                    {latest.note || t("(no note)")}
                  </p>
                  <div className="before-after">
                    {(["before", "after"] as const).map((side) => (
                      <div className="stat-tile" key={side}>
                        <div className="stat-tile__label">
                          {t(side === "before" ? "Before" : "After")}
                        </div>
                        <div className="stat-tile__value">
                          {fmtRate(comparison[side].rate)}
                        </div>
                        <div className="stat-tile__sub">
                          {t("{0} of {1} observed sessions affected", [
                            fmtNum(comparison[side].hitSessions),
                            fmtNum(comparison[side].sessions),
                          ])}
                        </div>
                        <div className="stat-tile__sub">
                          {t("{0} active days · {1} incidents", [
                            comparison[side].days,
                            comparison[side].incidents,
                          ])}
                        </div>
                      </div>
                    ))}
                  </div>
                  <p className="panel__note">
                    {t(
                      "Equal windows of up to 14 complete UTC days. The change day and today are excluded. At least 5 eligible sessions across 2 active days on each side are needed to show a trend.",
                    )}
                  </p>
                  <p className="panel__note">
                    {t(
                      "Observed change is not proof of causation, task quality or invoice savings. Missing activity is not a successful outcome.",
                    )}
                  </p>
                </div>
              ) : (
                <p className="state-panel">
                  {t(
                    "Record what you changed below to start a before-and-after comparison.",
                  )}
                </p>
              )}
              <details className="observation-details">
                <summary>{t("Daily evidence (UTC)")}</summary>
                <p className="panel__note">
                  {t(
                    "Only sessions with observable opportunities for this pattern enter the rate. A day without eligible activity is absent, not a zero-failure day.",
                  )}
                </p>
                {data.rows.length ? (
                  <div className="table-scroll">
                    <table className="data-table">
                      <caption className="sr-only">
                        {t("Daily recurrence evidence")}
                      </caption>
                      <thead>
                        <tr>
                          {[
                            "Day",
                            "Eligible sessions",
                            "Affected sessions",
                            "Rate",
                            "Incidents",
                          ].map((label) => (
                            <th scope="col" key={label}>
                              {t(label)}
                            </th>
                          ))}
                        </tr>
                      </thead>
                      <tbody>
                        {data.rows.map((r) => (
                          <tr key={r[0]}>
                            <td>{fmtDateShort(r[0])}</td>
                            <td>{fmtNum(r[1])}</td>
                            <td>{fmtNum(r[2])}</td>
                            <td>{fmtRate(r[3])}</td>
                            <td>{fmtNum(r[4])}</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                ) : (
                  <p>{t("No eligible activity observed in this window.")}</p>
                )}
              </details>
            </>
          );
        }}
      </QueryBoundary>
      {writable ? (
        <form className="mark-form" onSubmit={onSubmit}>
          <h3>{t("Record a change")}</h3>
          <p className="panel__note">
            {t(
              "Stored on this machine for the selected device and repository filters. Change records are not synced to your team.",
            )}
          </p>
          <div className="field field--inline">
            <label className="field__label" htmlFor="mark-dt">
              {t("Change date (UTC)")}
            </label>
            <input
              id="mark-dt"
              type="date"
              required
              max={todayIso()}
              value={markedDt}
              onChange={(e) => setMarkedDt(e.target.value)}
            />
            <label className="sr-only" htmlFor="mark-note">
              {t("What changed?")}
            </label>
            <input
              id="mark-note"
              type="text"
              required
              maxLength={500}
              value={note}
              onChange={(e) => setNote(e.target.value)}
              placeholder={t("What changed? e.g. renewed the tool credentials")}
            />
            <button
              type="submit"
              className="btn btn--primary"
              disabled={saving || !note.trim()}
            >
              {saving ? t("Saving…") : t("Save change")}
            </button>
          </div>
        </form>
      ) : (
        <p className="callout">
          {t(
            "S3 exports are read-only. Record and verify changes on the collecting machine.",
          )}
        </p>
      )}
      {error && (
        <p className="callout callout--warn" role="alert">
          {error}
        </p>
      )}
      {marks.length > 0 && (
        <ul className="mark-list">
          {marks.map((mark) => (
            <li key={mark.id}>
              <span className="badge badge--neutral">
                {t("Change recorded")}
              </span>{" "}
              {mark.marked_dt} · {mark.note || t("(no note)")}
              {writable && (
                <button
                  type="button"
                  className="btn btn--ghost btn--small"
                  disabled={saving}
                  onClick={() => void onDelete(mark)}
                >
                  {t("Remove record")}
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function Drilldown({
  subject,
  marks,
  onChange,
}: {
  subject: Subject;
  marks: ImprovementMark[];
  onChange: () => void;
}) {
  const hits = useAsync(
    () => getPatternHits(subject[0], subject[1], DAYS, HIT_LIMIT),
    [subject[0], subject[1]],
    30_000,
  );
  const guidance = GUIDANCE[subject[0]];
  return (
    <section className="drilldown" aria-labelledby="issue-heading">
      <h2 id="issue-heading">
        {patternLabel(subject[0])} · <span className="mono">{subject[1]}</span>
      </h2>
      <div className="fix-guide">
        <div>
          <h3>{t("1. Review the evidence")}</h3>
          <p>
            {guidance
              ? t(guidance.evidence)
              : t("Review the recorded sessions before making a change.")}
          </p>
        </div>
        <div>
          <h3>{t("2. Fix the cause")}</h3>
          <p>
            {guidance
              ? t(guidance.action)
              : t("Inspect the affected workflow and record your change.")}
          </p>
        </div>
      </div>
      {guidance && <p className="callout">{t(guidance.caveat)}</p>}
      <QueryBoundary
        state={hits}
        isEmpty={(d) => d.rows.length === 0}
        emptyLabel={t(
          "No incidents in the last 30 days. This alone does not confirm a fix.",
        )}
        onRetry={hits.reload}
      >
        {(data) => (
          <SortableTable
            columns={hitColumns}
            rows={data.rows}
            rowKey={(r, i) => `${r[1]}-${r[2]}-${i}`}
            defaultSortKey="first_ts"
            caption={t("Open an affected session to inspect the evidence.")}
          />
        )}
      </QueryBoundary>
      <Trend subject={subject} marks={marks} onChange={onChange} />
    </section>
  );
}

export function Patterns() {
  const patterns = useAsync(() => getPatterns(DAYS), [DAYS], 30_000);
  const marks = useAsync(() => getMarks(), [], 30_000);
  const [selected, setSelected] = useState<Subject | null>(null);
  const tracked = new Map<string, ImprovementMark>();
  if (marks.status === "ok")
    for (const mark of marks.data.marks) {
      const key = subjectKey([mark.pattern_id, mark.subject]);
      const previous = tracked.get(key);
      if (
        !previous ||
        `${mark.marked_dt}:${mark.created_at}` >
          `${previous.marked_dt}:${previous.created_at}`
      )
        tracked.set(key, mark);
    }
  return (
    <div className="page">
      <div className="page__header">
        <h1>{t("Improvements")}</h1>
        <p className="page__subtitle">
          {t(
            "Find a recurring problem, record a concrete change, then check what happened next.",
          )}
        </p>
      </div>
      <p className="callout">
        {t(
          "These are investigation leads, not proven waste or promised savings. Start with repeated failures and permission loops.",
        )}
      </p>
      <section className="panel" aria-labelledby="tracked-title">
        <h2 className="panel__title" id="tracked-title">
          {t("Changes to follow up")}
        </h2>
        <QueryBoundary
          state={marks}
          onRetry={marks.reload}
          isEmpty={(d) => d.marks.length === 0}
          emptyLabel={t(
            "No changes recorded for these filters yet. Choose a candidate below.",
          )}
        >
          {() => (
            <div className="tracked-changes">
              {[...tracked.values()].map((mark) => (
                <button
                  type="button"
                  className={`tracked-change ${selected && subjectKey(selected) === subjectKey([mark.pattern_id, mark.subject]) ? "row-selected" : ""}`}
                  key={mark.id}
                  onClick={() => setSelected([mark.pattern_id, mark.subject])}
                >
                  <span className="badge badge--neutral">{t("Follow up")}</span>
                  <strong>
                    {patternLabel(mark.pattern_id)} · {mark.subject}
                  </strong>
                  <span>
                    {mark.marked_dt} · {mark.note || t("(no note)")}
                  </span>
                  <span>{t("Check recurrence →")}</span>
                </button>
              ))}
            </div>
          )}
        </QueryBoundary>
        <p className="panel__note">
          {t(
            "Recorded changes stay here even when the problem disappears from the candidate list.",
          )}
        </p>
      </section>
      <section className="panel">
        <h2 className="panel__title">
          {t("Improvement candidates · last 30 days")}
        </h2>
        <QueryBoundary
          state={patterns}
          isEmpty={(d) => d.rows.length === 0}
          emptyLabel={t(
            "No patterns found in captured activity. Check collection before treating this as an absence of problems.",
          )}
          onRetry={patterns.reload}
        >
          {(data) => (
            <SortableTable
              columns={columns}
              rows={[...data.rows].sort((a, b) => {
                const focus = (id: string) =>
                  ["retry_spiral", "permission_denied_loop"].includes(id)
                    ? 1
                    : 0;
                return focus(b[0]) - focus(a[0]) || b[2] - a[2];
              })}
              rowKey={rowId}
              onRowClick={(r) => setSelected([r[0], r[1]])}
              rowClassName={(r) =>
                selected && subjectKey(selected) === subjectKey([r[0], r[1]])
                  ? "row-selected"
                  : undefined
              }
              caption={t(
                "Candidates from captured activity. Select a row to review evidence and record a change.",
              )}
            />
          )}
        </QueryBoundary>
      </section>
      {selected && (
        <section className="panel" key={subjectKey(selected)}>
          <button
            type="button"
            className="btn btn--ghost btn--small"
            onClick={() => setSelected(null)}
          >
            {t("Close details")}
          </button>
          {marks.status === "ok" ? (
            <Drilldown
              subject={selected}
              marks={marks.data.marks.filter(
                (m) =>
                  m.pattern_id === selected[0] && m.subject === selected[1],
              )}
              onChange={marks.reload}
            />
          ) : (
            <QueryBoundary state={marks} onRetry={marks.reload}>
              {() => null}
            </QueryBoundary>
          )}
        </section>
      )}
    </div>
  );
}
