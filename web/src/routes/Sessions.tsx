import { t } from "../i18n";
import { useState } from "react";
import { getSessions } from "../api/client";
import { fmtClient, fmtCost, fmtDateTime, fmtNum, fmtStr } from "../api/format";
import { useAsync } from "../hooks/useAsync";
import { Link } from "../router/Router";
import { QueryBoundary } from "../components/QueryBoundary";
import { SortableTable, type ColumnDef } from "../components/SortableTable";
import type { SessionRow } from "../api/types";

const LIMIT = 500;
const FILTER_KEY = "sessions-filters-v1";
const CLIENTS = [
  "claude_desktop",
  "claude_code_cli",
  "claude_desktop_code",
  "claude_desktop_cowork",
  "codex_cli",
  "codex_desktop",
  "chatgpt_desktop_cowork",
] as const;
function savedFilters() {
  try {
    const saved = JSON.parse(sessionStorage.getItem(FILTER_KEY) || "{}");
    return {
      days: [14, 30, 90, 365].includes(saved.days)
        ? (saved.days as number)
        : 14,
      application: ["all", "unknown", ...CLIENTS].includes(saved.application)
        ? (saved.application as string)
        : "all",
    };
  } catch {
    return { days: 14, application: "all" };
  }
}

const columns: ColumnDef<SessionRow>[] = [
  {
    key: "session_id",
    get label() {
      return t("Session");
    },
    sortValue: (r) => r[0],
    render: (r) => (
      <Link
        to={`/sessions/${encodeURIComponent(r[0])}`}
        className="mono session-id"
        title={r[0]}
      >
        {r[0].slice(0, 8)}
      </Link>
    ),
  },
  {
    key: "client",
    get label() {
      return t("Application");
    },
    sortValue: (r) => fmtClient(r[11]),
    render: (r) => (r[11] ? fmtClient(r[11]) : t("{0} · Unknown app", [r[1]])),
  },
  {
    key: "host_id",
    get label() {
      return t("Host");
    },
    sortValue: (r) => r[2],
    render: (r) => (
      <span className="mono" title={r[2]}>
        {r[2].slice(0, 8)}
      </span>
    ),
  },
  {
    key: "started_at",
    get label() {
      return t("Started");
    },
    sortValue: (r) => new Date(r[3]).getTime(),
    render: (r) => fmtDateTime(r[3]),
  },
  {
    key: "events",
    get label() {
      return t("Events");
    },
    align: "right",
    sortValue: (r) => r[4],
    render: (r) => fmtNum(r[4]),
  },
  {
    key: "tool_calls",
    get label() {
      return t("Tool calls");
    },
    align: "right",
    sortValue: (r) => r[5],
    render: (r) => fmtNum(r[5]),
  },
  {
    key: "failures",
    get label() {
      return t("Failures");
    },
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
    get label() {
      return t("Models");
    },
    sortValue: (r) => r[7],
    render: (r) => <span className="mono">{fmtStr(r[7])}</span>,
  },
  {
    key: "input_tokens",
    get label() {
      return t("Input tokens");
    },
    align: "right",
    sortValue: (r) => r[8],
    render: (r) => fmtNum(r[8]),
  },
  {
    key: "output_tokens",
    get label() {
      return t("Output tokens");
    },
    align: "right",
    sortValue: (r) => r[9],
    render: (r) => fmtNum(r[9]),
  },
  {
    key: "estimated_tokens",
    get label() { return t("Estimated text tokens in / out"); },
    align: "right",
    sortValue: (r) => r[12] == null && r[13] == null ? null : (r[12] ?? 0) + (r[13] ?? 0),
    render: (r) => r[12] == null && r[13] == null ? "–" : `≈ ${fmtNum(r[12] ?? null)} / ${fmtNum(r[13] ?? null)}`,
  },
  {
    key: "cost_usd",
    get label() {
      return t("Cost");
    },
    align: "right",
    sortValue: (r) => r[10],
    render: (r) => fmtCost(r[10]),
  },
];

export function Sessions() {
  const [filters, setFilters] = useState(savedFilters);
  const { days, application } = filters;
  const changeFilters = (next: typeof filters) => {
    setFilters(next);
    try {
      sessionStorage.setItem(FILTER_KEY, JSON.stringify(next));
    } catch {
      /* Browsing also works without storage. */
    }
  };
  const sessions = useAsync(() => getSessions(days, LIMIT), [days]);
  const matches = (row: SessionRow) =>
    application === "all" ||
    (application === "unknown"
      ? !row[11]
      : row[11]?.split(",").includes(application));

  return (
    <div className="page">
      <div className="page__header">
        <h1>{t("Sessions")}</h1>
        <p className="page__subtitle">
          {t("Most recent")} {LIMIT}{" "}
          {t(
            "sessions in the selected period. Sessions with failures are highlighted.",
          )}
        </p>
      </div>

      <div className="sessions-filters">
        <label className="field">
          <span>{t("Period")}</span>
          <select
            aria-label={t("Period")}
            value={days}
            onChange={(event) =>
              changeFilters({ ...filters, days: Number(event.target.value) })
            }
          >
            <option value={14}>{t("Last 14 days")}</option>
            <option value={30}>{t("Last 30 days")}</option>
            <option value={90}>{t("Last 90 days")}</option>
            <option value={365}>{t("Last year")}</option>
          </select>
        </label>
        <label className="field">
          <span>{t("Application")}</span>
          <select
            aria-label={t("Application")}
            value={application}
            onChange={(event) =>
              changeFilters({ ...filters, application: event.target.value })
            }
          >
            <option value="all">{t("All applications")}</option>
            {CLIENTS.map((client) => (
              <option key={client} value={client}>
                {fmtClient(client)}
              </option>
            ))}
            <option value="unknown">{t("Unknown app")}</option>
          </select>
        </label>
      </div>
      <section className="panel">
        <QueryBoundary
          state={sessions}
          isEmpty={(d) => !d.rows.some(matches)}
          emptyLabel={t("No sessions match this period and application")}
          onRetry={sessions.reload}
        >
          {(data) => (
            <SortableTable
              columns={columns}
              rows={data.rows.filter(matches)}
              rowKey={(r) => r[0]}
              defaultSortKey="started_at"
              rowClassName={(r) =>
                r[6] !== null && r[6] > 0 ? "row-danger" : undefined
              }
              caption={t(
                "List of sessions: application, host, start time, event count, tool calls, failures, models, token counts, and cost.",
              )}
            />
          )}
        </QueryBoundary>
      </section>
    </div>
  );
}
