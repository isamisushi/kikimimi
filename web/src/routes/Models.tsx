import { t } from "../i18n";
import { getModels } from "../api/client";
import { fmtCost, fmtNum, fmtStr } from "../api/format";
import { useAsync } from "../hooks/useAsync";
import { QueryBoundary } from "../components/QueryBoundary";
import { StatTile } from "../components/StatTile";
import { SortableTable, type ColumnDef } from "../components/SortableTable";
import {
  ModelBarChart,
  assignModelSeries,
  seriesClass,
} from "../components/ModelBarChart";
import type { ModelRow } from "../api/types";

const DAYS = 14;

function sumOrNull(values: (number | null)[]): number | null {
  const known = values.filter((v): v is number => v !== null);
  if (known.length === 0) return null;
  return known.reduce((a, b) => a + b, 0);
}

function tokensOf(r: ModelRow): number | null {
  return r[7] === null && r[8] === null ? null : (r[7] ?? 0) + (r[8] ?? 0);
}

function fmtShare(num: number | null, den: number | null): string {
  if (num === null || den === null || den === 0) return t("unknown");
  return `${Math.round((num / den) * 100)}%`;
}

const nullable = (v: number | null) =>
  v === null ? <span className="text-muted">–</span> : fmtNum(v);

function makeColumns(
  slotOf: Map<string, number | "other">,
): ColumnDef<ModelRow>[] {
  return [
    {
      key: "model",
      label: t("Model"),
      sortValue: (r) => r[0],
      render: (r) => {
        const slot = slotOf.get(r[0]);
        return (
          <span className="mono">
            {slot !== undefined && (
              <span
                className={`legend-swatch ${seriesClass(slot)} series-dot`}
                aria-hidden="true"
              />
            )}
            {r[0]}
          </span>
        );
      },
    },
    {
      key: "effort",
      label: t("Effort"),
      sortValue: (r) => r[1] ?? "",
      render: (r) =>
        r[1] === null ? (
          <span
            className="text-muted"
            title={t("not reported by Claude Code (internal helper calls)")}
          >
            –
          </span>
        ) : (
          <span className="mono">{r[1]}</span>
        ),
    },
    {
      key: "api_requests",
      label: t("API requests"),
      align: "right",
      sortValue: (r) => r[2],
      render: (r) => fmtNum(r[2]),
    },
    {
      key: "api_errors",
      label: t("Errors"),
      align: "right",
      sortValue: (r) => r[3],
      render: (r) => (
        <span className={r[3] > 0 ? "text-danger" : undefined}>
          {fmtNum(r[3])}
        </span>
      ),
    },
    {
      key: "sessions",
      label: t("Sessions"),
      align: "right",
      sortValue: (r) => r[4],
      render: (r) => fmtNum(r[4]),
    },
    {
      key: "subagent_share",
      label: t("In subagents"),
      align: "right",
      sortValue: (r) => (r[2] > 0 ? r[5] / r[2] : null),
      render: (r) => (
        <span
          title={t("{0} of {1} requests · {2} tokens", [
            fmtNum(r[5]),
            fmtNum(r[2]),
            r[6] === null ? t("unknown") : fmtNum(r[6]),
          ])}
        >
          {r[2] > 0 ? `${Math.round((r[5] / r[2]) * 100)}%` : "–"}
          {r[6] !== null && (
            <span className="text-muted"> · {fmtNum(r[6])}</span>
          )}
        </span>
      ),
    },
    {
      key: "tokens",
      label: t("Tokens"),
      align: "right",
      sortValue: tokensOf,
      render: (r) => nullable(tokensOf(r)),
    },
    {
      key: "input_tokens",
      label: t("Input"),
      align: "right",
      sortValue: (r) => r[7],
      render: (r) => nullable(r[7]),
    },
    {
      key: "output_tokens",
      label: t("Output"),
      align: "right",
      sortValue: (r) => r[8],
      render: (r) => nullable(r[8]),
    },
    {
      key: "cache_read_tokens",
      label: t("Cache read"),
      align: "right",
      sortValue: (r) => r[9],
      render: (r) => nullable(r[9]),
    },
    {
      key: "cache_write_tokens",
      label: t("Cache write"),
      align: "right",
      sortValue: (r) => r[10],
      render: (r) => nullable(r[10]),
    },
    {
      key: "reasoning_tokens",
      label: t("Reasoning"),
      align: "right",
      sortValue: (r) => r[11],
      render: (r) => nullable(r[11]),
    },
    {
      key: "cost_usd",
      label: t("Cost"),
      align: "right",
      sortValue: (r) => r[12],
      render: (r) => fmtCost(r[12]),
    },
  ];
}

export function Models() {
  const data = useAsync(() => getModels(DAYS), [DAYS]);

  return (
    <div className="page">
      <div className="page__header">
        <h1>{t("Models")}</h1>
        <p className="page__subtitle">
          {t("Which model and effort level burned the tokens over the last")}{" "}
          {DAYS} {t("days, across this view. Counted from the agents’ own")}{" "}
          <span className="mono">api.request</span>{" "}
          {t(
            "records; when a session was seen by both OTel and its transcript, OTel wins so nothing is double counted.",
          )}
        </p>
      </div>

      <QueryBoundary
        state={data}
        isEmpty={(d) => d.models.rows.length === 0}
        emptyLabel={t("No API usage captured in this period")}
        onRetry={data.reload}
      >
        {(d) => {
          const rows = d.models.rows;
          const requests = rows.reduce((a, r) => a + r[2], 0);
          const tokens = sumOrNull(rows.map(tokensOf));
          const cost = sumOrNull(rows.map((r) => r[12]));
          const subTokens = sumOrNull(rows.map((r) => r[6]));
          const models = new Set(rows.map((r) => r[0])).size;
          const series = assignModelSeries(d.daily.rows);
          const slotOf = new Map(series.map((s) => [s.model, s.slot]));
          const columns = makeColumns(slotOf);
          return (
            <>
              <div className="stat-grid stat-grid--models">
                <StatTile
                  label={t("API requests")}
                  value={fmtNum(requests)}
                  hint={t("{0} errors", [
                    fmtNum(rows.reduce((a, r) => a + r[3], 0)),
                  ])}
                />
                <StatTile
                  label={t("Tokens")}
                  value={tokens === null ? "–" : fmtNum(tokens)}
                  hint={
                    tokens === null
                      ? t("no usage captured")
                      : t("{0} in / {1} out", [
                          fmtNum(sumOrNull(rows.map((r) => r[7]))),
                          fmtNum(sumOrNull(rows.map((r) => r[8]))),
                        ])
                  }
                />
                <StatTile label={t("Cost")} value={fmtCost(cost)} />
                <StatTile
                  label={t("Model / effort pairs")}
                  value={fmtNum(rows.length)}
                  hint={t("{0} model{1}", [models, models === 1 ? "" : "s"])}
                />
                <StatTile
                  label={t("In subagents")}
                  value={fmtShare(subTokens, tokens)}
                  hint={
                    subTokens === null
                      ? t("no subagent usage captured")
                      : t("{0} tokens inside Agent-tool subagents", [
                          fmtNum(subTokens),
                        ])
                  }
                />
              </div>

              <section className="panel">
                <h2 className="panel__title">{t("Daily tokens by model")}</h2>
                {d.daily.rows.length === 0 ? (
                  <div className="state-panel state-panel--empty">
                    {t("No daily usage")}
                  </div>
                ) : (
                  <ModelBarChart rows={d.daily.rows} series={series} />
                )}
              </section>

              <section className="panel">
                <h2 className="panel__title">{t("By model and effort")}</h2>
                <SortableTable
                  columns={columns}
                  rows={rows}
                  rowKey={(r) => `${r[0]}|${r[1] ?? ""}`}
                  defaultSortKey="tokens"
                  rowClassName={(r) => (r[3] > 0 ? "row-danger" : undefined)}
                  caption={t(
                    "Per model and effort level: API requests, errors, sessions, share inside subagents, input / output / cache / reasoning tokens and cost.",
                  )}
                />
                <p className="panel__note">
                  {t("Effort is the agents’ own")}{" "}
                  <span className="mono">effort</span>{" "}
                  {t(
                    'field (hooks, transcript and OTel all carry it) and is missing for its internal helper calls, shown as "–". Reasoning tokens only exist on transcript-sourced rows; OTel does not report them. A request seen by both OTel and the transcript is counted once — OTel wins per session. "',
                  )}
                  {fmtStr(null)}
                  {t('" is unknown, never 0.')}
                </p>
              </section>
            </>
          );
        }}
      </QueryBoundary>
    </div>
  );
}
