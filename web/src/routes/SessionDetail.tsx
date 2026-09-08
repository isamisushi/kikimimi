import { useMemo, useState } from "react";
import { getSessionDetail } from "../api/client";
import { fmtCost, fmtDateTime, fmtDuration, fmtMs, fmtNum, fmtStr, fmtTime } from "../api/format";
import { useAsync } from "../hooks/useAsync";
import { QueryBoundary } from "../components/QueryBoundary";
import { SortableTable, type ColumnDef } from "../components/SortableTable";
import { StatTile } from "../components/StatTile";
import { Link } from "../router/Router";
import type {
  SessionDetail as SessionDetailData,
  SessionEventRow,
  SessionModelRow,
  SessionSubagentRow,
  SessionSummaryRow,
  SessionTimelineRow,
  SessionToolRow,
} from "../api/types";

const EVENTS_LIMIT = 500;

/** Parses a `["github","playwright"]` snapshot string; [] when absent/unreadable. */
function parseNames(json: string | null): string[] {
  if (!json) return [];
  try {
    const v: unknown = JSON.parse(json);
    return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];
  } catch {
    return [];
  }
}

const toolColumns: ColumnDef<SessionToolRow>[] = [
  { key: "tool_name", label: "Tool", sortValue: (r) => r[0], render: (r) => <span className="mono">{r[0]}</span> },
  { key: "tool_kind", label: "Kind", sortValue: (r) => r[1], render: (r) => fmtStr(r[1]) },
  { key: "mcp_server", label: "MCP server", sortValue: (r) => r[2], render: (r) => <span className="mono">{fmtStr(r[2])}</span> },
  { key: "calls", label: "Calls", align: "right", sortValue: (r) => r[3], render: (r) => fmtNum(r[3]) },
  {
    key: "subagent_calls",
    label: "From subagents",
    align: "right",
    sortValue: (r) => r[4],
    render: (r) => (r[4] > 0 ? fmtNum(r[4]) : <span className="text-muted">0</span>),
  },
  {
    key: "failures",
    label: "Failures",
    align: "right",
    sortValue: (r) => r[5],
    render: (r) => <span className={r[5] > 0 ? "text-danger" : undefined}>{fmtNum(r[5])}</span>,
  },
  { key: "denied", label: "Denied", align: "right", sortValue: (r) => r[6], render: (r) => fmtNum(r[6]) },
  { key: "p50", label: "p50", align: "right", sortValue: (r) => r[7], render: (r) => fmtMs(r[7]) },
  { key: "p95", label: "p95", align: "right", sortValue: (r) => r[8], render: (r) => fmtMs(r[8]) },
  { key: "total", label: "Total time", align: "right", sortValue: (r) => r[9], render: (r) => fmtDuration(r[9]) },
];

/** A subagent's model / effort. `otel_window` means it was attributed from
 * the session's OTel api.request rows by agent type and time window (live
 * sessions: hooks carry no model, OTel carries no agent_id) -- marked so the
 * reader knows parallel subagents of one type share the value. */
function ModelCell({ value, source }: { value: string | null; source: "agent" | "otel_window" | null }) {
  if (!value) return <span className="text-muted">–</span>;
  const attributed = source === "otel_window";
  return (
    <span className="mono" title={attributed ? "attributed from OTel by agent type and time window" : undefined}>
      {value}
      {attributed && <span className="text-muted">≈</span>}
    </span>
  );
}

const subagentColumns: ColumnDef<SessionSubagentRow>[] = [
  {
    key: "agent_id",
    label: "Agent",
    sortValue: (r) => r[0],
    render: (r) => (
      <span className="mono" title={r[0]}>
        {r[0].slice(0, 8)}
      </span>
    ),
  },
  { key: "agent_type", label: "Type", sortValue: (r) => r[1], render: (r) => fmtStr(r[1]) },
  { key: "models", label: "Model", sortValue: (r) => r[11] ?? "", render: (r) => <ModelCell value={r[11]} source={r[13]} /> },
  { key: "efforts", label: "Effort", sortValue: (r) => r[12] ?? "", render: (r) => <ModelCell value={r[12]} source={r[13]} /> },
  { key: "started_at", label: "Started", sortValue: (r) => new Date(r[3]).getTime(), render: (r) => fmtDateTime(r[3]) },
  { key: "duration_ms", label: "Duration", align: "right", sortValue: (r) => r[4], render: (r) => fmtDuration(r[4]) },
  { key: "events", label: "Events", align: "right", sortValue: (r) => r[5], render: (r) => fmtNum(r[5]) },
  { key: "tool_calls", label: "Tool calls", align: "right", sortValue: (r) => r[6], render: (r) => fmtNum(r[6]) },
  {
    key: "failures",
    label: "Failures",
    align: "right",
    sortValue: (r) => r[7],
    render: (r) => <span className={r[7] > 0 ? "text-danger" : undefined}>{fmtNum(r[7])}</span>,
  },
  { key: "api_requests", label: "API requests", align: "right", sortValue: (r) => r[8], render: (r) => fmtNum(r[8]) },
  {
    key: "tokens_est",
    label: "Tokens (est.)",
    align: "right",
    sortValue: (r) => r[9],
    render: (r) => (r[9] === null ? <span className="text-muted">unknown</span> : fmtNum(r[9])),
  },
  { key: "tools", label: "Tools used", render: (r) => <span className="mono">{fmtStr(r[10])}</span> },
];

const nullableNum = (v: number | null) => (v === null ? <span className="text-muted">–</span> : fmtNum(v));

const modelColumns: ColumnDef<SessionModelRow>[] = [
  { key: "model", label: "Model", sortValue: (r) => r[0], render: (r) => <span className="mono">{r[0]}</span> },
  {
    key: "effort",
    label: "Effort",
    sortValue: (r) => r[1] ?? "",
    render: (r) =>
      r[1] === null ? (
        <span className="text-muted" title="not reported by Claude Code (internal helper calls)">
          –
        </span>
      ) : (
        <span className="mono">{r[1]}</span>
      ),
  },
  { key: "api_requests", label: "API requests", align: "right", sortValue: (r) => r[2], render: (r) => fmtNum(r[2]) },
  {
    key: "api_errors",
    label: "Errors",
    align: "right",
    sortValue: (r) => r[3],
    render: (r) => <span className={r[3] > 0 ? "text-danger" : undefined}>{fmtNum(r[3])}</span>,
  },
  {
    key: "subagent_api_requests",
    label: "From subagents",
    align: "right",
    sortValue: (r) => r[4],
    render: (r) => (r[4] > 0 ? fmtNum(r[4]) : <span className="text-muted">0</span>),
  },
  { key: "input_tokens", label: "In", align: "right", sortValue: (r) => r[5], render: (r) => nullableNum(r[5]) },
  { key: "output_tokens", label: "Out", align: "right", sortValue: (r) => r[6], render: (r) => nullableNum(r[6]) },
  { key: "cache_read_tokens", label: "Cache read", align: "right", sortValue: (r) => r[7], render: (r) => nullableNum(r[7]) },
  { key: "cache_write_tokens", label: "Cache write", align: "right", sortValue: (r) => r[8], render: (r) => nullableNum(r[8]) },
  { key: "reasoning_tokens", label: "Reasoning", align: "right", sortValue: (r) => r[9], render: (r) => nullableNum(r[9]) },
  { key: "cost_usd", label: "Cost", align: "right", sortValue: (r) => r[10], render: (r) => fmtCost(r[10]) },
];

type EventFilter = "all" | "tools" | "api" | "subagents" | "failures";
const EVENT_FILTERS: { key: EventFilter; label: string }[] = [
  { key: "all", label: "All" },
  { key: "tools", label: "Tools" },
  { key: "api", label: "API" },
  { key: "subagents", label: "Subagents" },
  { key: "failures", label: "Failures" },
];

function matchesFilter(r: SessionEventRow, f: EventFilter): boolean {
  switch (f) {
    case "all":
      return true;
    case "tools":
      return r[1].startsWith("tool.");
    case "api":
      return r[1].startsWith("api.");
    case "subagents":
      return r[7] !== null || r[8] !== null || r[1].startsWith("subagent.");
    case "failures":
      return r[10] === false || r[1] === "api.error" || r[1] === "tool.denied";
  }
}

const eventColumns: ColumnDef<SessionEventRow>[] = [
  { key: "ts", label: "Time", sortValue: (r) => r[0], render: (r) => <span className="mono">{fmtTime(r[0])}</span> },
  { key: "event_type", label: "Event", sortValue: (r) => r[1], render: (r) => <span className="mono">{r[1]}</span> },
  {
    key: "what",
    label: "Tool / model",
    sortValue: (r) => r[3] ?? r[13] ?? "",
    render: (r) => {
      if (r[3]) {
        return (
          <span className="mono">
            {r[3]}
            {r[6] && <span className="text-muted"> ({r[6]})</span>}
            {r[5] && !r[6] && <span className="text-muted"> ({r[5]})</span>}
          </span>
        );
      }
      if (r[13]) {
        return (
          <span className="mono">
            {r[13]}
            {r[18] && <span className="text-muted"> ({r[18]})</span>}
          </span>
        );
      }
      return <span className="text-muted">–</span>;
    },
  },
  {
    key: "agent",
    label: "Agent",
    sortValue: (r) => r[7] ?? "",
    render: (r) =>
      r[7] ? (
        <span className="mono" title={r[7]}>{r[8] ?? r[7].slice(0, 8)}</span>
      ) : r[8] ? (
        <span className="mono" title="OTel row: agent type only, no agent id">{r[8]}</span>
      ) : (
        <span className="text-muted">main</span>
      ),
  },
  { key: "duration_ms", label: "Duration", align: "right", sortValue: (r) => r[9], render: (r) => fmtMs(r[9]) },
  {
    key: "outcome",
    label: "Outcome",
    sortValue: (r) => (r[10] === null ? null : r[10] ? 1 : 0),
    render: (r) => {
      if (r[12]) return <span className={r[12] === "deny" || r[12] === "reject" ? "text-danger" : undefined}>{r[12]}</span>;
      if (r[10] === false) return <span className="text-danger">failed{r[11] ? ` (${r[11]})` : ""}</span>;
      if (r[10] === true) return "ok";
      return <span className="text-muted">–</span>;
    },
  },
  {
    key: "tokens",
    label: "Tokens in / out",
    align: "right",
    sortValue: (r) => (r[14] === null && r[15] === null ? null : (r[14] ?? 0) + (r[15] ?? 0)),
    render: (r) => (r[14] === null && r[15] === null ? <span className="text-muted">–</span> : `${fmtNum(r[14])} / ${fmtNum(r[15])}`),
  },
  { key: "source", label: "Source", sortValue: (r) => r[2], render: (r) => <span className="text-muted">{r[2]}</span> },
];

const TL_W = 760;
const TL_H = 140;
const TL_PAD_TOP = 8;
const TL_PAD_BOTTOM = 22;

/** Activity strip: one bar per bucket (main-conversation events + subagent
 * events stacked), failures as a red marker at the foot of the bar. */
function Timeline({ rows, bucketMs, startedAt }: { rows: SessionTimelineRow[]; bucketMs: number; startedAt: string }) {
  if (rows.length === 0) return null;
  const first = rows[0][0];
  const last = rows[rows.length - 1][0];
  const n = Math.max(1, Math.round((last - first) / bucketMs) + 1);
  const slot = TL_W / n;
  const barW = Math.max(1, slot * 0.7);
  const plotH = TL_H - TL_PAD_TOP - TL_PAD_BOTTOM;
  const max = Math.max(1, ...rows.map((r) => r[1]));
  const label = (ms: number) => fmtTime(ms).slice(0, 5);
  const startMs = new Date(startedAt).getTime();
  const bucketLabel = bucketMs >= 3_600_000 ? `${bucketMs / 3_600_000}h` : `${bucketMs / 60_000}min`;

  return (
    <div className="bar-chart">
      <div className="bar-chart__legend">
        <span className="legend-item">
          <span className="legend-swatch legend-swatch--input" /> main conversation
        </span>
        <span className="legend-item">
          <span className="legend-swatch legend-swatch--output" /> subagents
        </span>
        <span className="legend-item">
          <span className="legend-swatch legend-swatch--danger" /> failures
        </span>
        <span className="legend-item legend-item--muted">{bucketLabel} buckets, events per bucket</span>
      </div>
      <svg
        className="bar-chart__svg"
        viewBox={`0 0 ${TL_W} ${TL_H}`}
        role="img"
        aria-label="Events per time bucket across the session; failures marked in red."
      >
        {rows.map((r) => {
          const i = Math.round((r[0] - first) / bucketMs);
          const x = i * slot + (slot - barW) / 2;
          const total = r[1];
          const sub = Math.min(r[6], total);
          const main = total - sub;
          const hTotal = (total / max) * plotH;
          const hSub = (sub / max) * plotH;
          const yTop = TL_PAD_TOP + plotH - hTotal;
          const rel = r[0] - startMs;
          const title = `${label(r[0])} (+${fmtDuration(Math.max(0, rel))})\n${fmtNum(total)} events · ${fmtNum(r[2])} tool calls · ${fmtNum(r[4])} API requests\n${fmtNum(r[3])} failures · ${fmtNum(r[5])} tokens · ${fmtNum(sub)} subagent events`;
          return (
            <g key={r[0]}>
              <title>{title}</title>
              <rect className="bar-chart__bar--input" x={x} y={yTop} width={barW} height={Math.max(0, hTotal - hSub)} />
              <rect className="bar-chart__bar--output" x={x} y={yTop + (hTotal - hSub)} width={barW} height={hSub} />
              {r[3] > 0 && <rect className="bar-chart__bar--danger" x={x} y={TL_PAD_TOP + plotH + 2} width={barW} height={3} />}
              {main === 0 && sub === 0 && <rect className="bar-chart__bar--zero" x={x} y={TL_PAD_TOP + plotH - 1} width={barW} height={1} />}
            </g>
          );
        })}
        <text className="bar-chart__label" x={0} y={TL_H - 6}>
          {label(first)}
        </text>
        {n > 2 && (
          <text className="bar-chart__label" x={TL_W / 2} y={TL_H - 6} textAnchor="middle">
            {label(first + Math.floor(n / 2) * bucketMs)}
          </text>
        )}
        <text className="bar-chart__label" x={TL_W} y={TL_H - 6} textAnchor="end">
          {label(last + bucketMs)}
        </text>
      </svg>
    </div>
  );
}

function Summary({ s, data }: { s: SessionSummaryRow; data: SessionDetailData }) {
  const mcp = parseNames(s[25]);
  const skills = parseNames(s[26]);
  const tokens = s[20] === null && s[21] === null ? null : (s[20] ?? 0) + (s[21] ?? 0);
  return (
    <>
      <div className="stat-grid">
        <StatTile label="Duration" value={fmtDuration(s[7])} hint={s[8] ? `${fmtDateTime(s[5])} → ${fmtDateTime(s[6])}` : `since ${fmtDateTime(s[5])} · no session.end yet`} />
        <StatTile label="Events" value={fmtNum(s[9])} hint={s[10] > 0 ? `${fmtNum(s[10])} turns` : undefined} />
        <StatTile label="Tool calls" value={fmtNum(s[11])} hint={s[13] > 0 ? `${fmtNum(s[13])} denied` : undefined} />
        <StatTile label="Failures" value={fmtNum(s[12])} tone={s[12] > 0 ? "danger" : "default"} hint={s[15] > 0 ? `${fmtNum(s[15])} API errors` : undefined} />
        <StatTile label="Subagents" value={fmtNum(s[17])} hint={data.subagents.rows.length > 0 ? fmtDuration(data.subagents.rows.reduce((a, r) => a + (r[4] ?? 0), 0)) + " of agent time" : undefined} />
        <StatTile label="API requests" value={fmtNum(s[14])} hint={s[16] > 0 ? `${fmtNum(s[16])} compactions` : undefined} />
        <StatTile
          label="Tokens"
          value={tokens === null ? "–" : fmtNum(tokens)}
          hint={tokens === null ? "no usage captured" : `${fmtNum(s[20])} in / ${fmtNum(s[21])} out${s[22] ? ` · ${fmtNum(s[22])} cache read` : ""}`}
        />
        <StatTile
          label="Cost"
          value={fmtCost(s[24])}
          hint={s[18] ? `${s[18]}${s[27] ? ` · effort ${s[27]}` : ""}` : undefined}
        />
      </div>
      {(mcp.length > 0 || skills.length > 0) && (
        <p className="panel__note">
          {mcp.length > 0 && (
            <>
              Configured MCP servers: <span className="mono">{mcp.join(", ")}</span>
            </>
          )}
          {mcp.length > 0 && skills.length > 0 && " · "}
          {skills.length > 0 && (
            <>
              Skills loaded: <span className="mono">{skills.join(", ")}</span>
            </>
          )}
        </p>
      )}
    </>
  );
}

export function SessionDetail({ sessionId }: { sessionId: string }) {
  const detail = useAsync(() => getSessionDetail(sessionId, EVENTS_LIMIT), [sessionId]);
  const [filter, setFilter] = useState<EventFilter>("all");

  return (
    <div className="page">
      <div className="page__header">
        <p className="page__subtitle">
          <Link to="/sessions">← Sessions</Link>
        </p>
        <h1>
          Session <span className="mono">{sessionId.slice(0, 8)}</span>
        </h1>
        <p className="page__subtitle mono session-detail__id">{sessionId}</p>
      </div>

      <QueryBoundary
        state={detail}
        isEmpty={(d) => d.summary.rows.length === 0}
        emptyLabel="Session not found (or not visible to you)"
        onRetry={detail.reload}
      >
        {(data) => {
          const s = data.summary.rows[0];
          return <SessionBody s={s} data={data} filter={filter} setFilter={setFilter} />;
        }}
      </QueryBoundary>
    </div>
  );
}

function SessionBody({
  s,
  data,
  filter,
  setFilter,
}: {
  s: SessionSummaryRow;
  data: SessionDetailData;
  filter: EventFilter;
  setFilter: (f: EventFilter) => void;
}) {
  const filtered = useMemo(() => data.events.rows.filter((r) => matchesFilter(r, filter)), [data.events.rows, filter]);
  const truncated = data.events.rows.length < s[9];
  return (
    <>
      <p className="page__subtitle">
        <span className="badge badge--neutral">{s[1]}{s[2] ? ` ${s[2]}` : ""}</span>{" "}
        host <span className="mono">{s[3]}</span>
        {s[4] && (
          <>
            {" "}· repo <span className="mono">{s[4]}</span>
          </>
        )}
        {" "}· sources <span className="mono">{fmtStr(s[19])}</span>
      </p>

      <section className="panel">
        <Summary s={s} data={data} />
      </section>

      <section className="panel">
        <h2 className="panel__title">Models</h2>
        {data.models.rows.length === 0 ? (
          <div className="state-panel state-panel--empty">No API usage captured for this session</div>
        ) : (
          <>
            <SortableTable
              columns={modelColumns}
              rows={data.models.rows}
              rowKey={(r) => `${r[0]}|${r[1] ?? ""}`}
              defaultSortKey="input_tokens"
              rowClassName={(r) => (r[3] > 0 ? "row-danger" : undefined)}
              caption="This session's API usage per model and effort level: requests, errors, requests made from subagents, tokens and cost."
            />
            <p className="panel__note">
              Effort is Claude Code's own field and is missing ("–") for its internal helper calls. A request seen by
              both OTel and the transcript is counted once (OTel wins). Reasoning tokens exist only on transcript rows.
            </p>
          </>
        )}
      </section>

      <section className="panel">
        <h2 className="panel__title">Activity over time</h2>
        <Timeline rows={data.timeline.rows} bucketMs={data.bucket_ms} startedAt={s[5]} />
      </section>

      <section className="panel">
        <h2 className="panel__title">Tools</h2>
        {data.tools.rows.length === 0 ? (
          <div className="state-panel state-panel--empty">No tool calls in this session</div>
        ) : (
          <SortableTable
            columns={toolColumns}
            rows={data.tools.rows}
            rowKey={(r) => r[0]}
            defaultSortKey="calls"
            rowClassName={(r) => (r[5] > 0 ? "row-danger" : undefined)}
            caption="Per-tool calls, failures, latency percentiles and total time in this session."
          />
        )}
      </section>

      <section className="panel">
        <h2 className="panel__title">Subagents</h2>
        {data.subagents.rows.length === 0 ? (
          <div className="state-panel state-panel--empty">No subagents in this session</div>
        ) : (
          <>
            <SortableTable
              columns={subagentColumns}
              rows={data.subagents.rows}
              rowKey={(r) => r[0]}
              defaultSortKey="started_at"
              defaultSortDir="asc"
              rowClassName={(r) => (r[7] > 0 ? "row-danger" : undefined)}
              caption="Each subagent spawned in this session: type, start, duration, tool calls, failures and estimated tokens."
            />
            <p className="panel__note">
              Duration is the SubagentStop hook's, else first-to-last event. Tokens are the agent's own api.request usage
              (transcript) or the SubagentStop usage block — "unknown" means Claude Code reported neither, never 0.
              Model / effort marked ≈ were attributed from the session's OTel api.request rows by agent type and
              time window (hooks carry no model, OTel no agent id); parallel subagents of one type share the value.
            </p>
          </>
        )}
      </section>

      <section className="panel">
        <h2 className="panel__title">Events</h2>
        <div className="segmented" role="tablist" aria-label="Event filter">
          {EVENT_FILTERS.map((f) => (
            <button
              key={f.key}
              type="button"
              role="tab"
              aria-selected={filter === f.key}
              className={`btn btn--small ${filter === f.key ? "btn--active" : "btn--ghost"}`}
              onClick={() => setFilter(f.key)}
            >
              {f.label}
            </button>
          ))}
          <span className="text-muted session-detail__count">
            {fmtNum(filtered.length)} of {fmtNum(data.events.rows.length)}
            {truncated ? ` (first ${fmtNum(data.events_limit)} of ${fmtNum(s[9])} events)` : ""}
          </span>
        </div>
        {filtered.length === 0 ? (
          <div className="state-panel state-panel--empty">No events match this filter</div>
        ) : (
          <SortableTable
            columns={eventColumns}
            rows={filtered}
            rowKey={(r, i) => `${r[0]}-${r[1]}-${i}`}
            defaultSortKey="ts"
            defaultSortDir="asc"
            rowClassName={(r) => (matchesFilter(r, "failures") ? "row-danger" : undefined)}
            caption="Chronological event list for this session: time, event type, tool or model, agent, duration, outcome, tokens and source. Metadata only."
          />
        )}
      </section>
    </>
  );
}
