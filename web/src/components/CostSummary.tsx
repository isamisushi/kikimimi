import { useLocale } from "../i18n";
import { useAsync } from "../hooks/useAsync";
import { getModels } from "../api/client";
import { QueryBoundary } from "./QueryBoundary";
import { Link } from "../router/Router";
import type { ModelRow } from "../api/types";
import { summarizeCosts, isStale } from "../cost/estimate";
import catalog from "../cost/pricing.json";
import "../styles/cost.css";

export function CostSummary({
  rows,
  days,
  detailed = false,
}: {
  rows: ModelRow[];
  days: number;
  detailed?: boolean;
}) {
  const locale = useLocale();
  const l = (ja: string, en: string) => (locale === "ja" ? ja : en);
  const summary = summarizeCosts(rows, catalog.models);
  const stale = isStale(catalog.fetchedAt);
  const usd = (v: number) =>
    new Intl.NumberFormat(locale, {
      style: "currency",
      currency: "USD",
    }).format(v);
  const equivalent = summary.equivalent;
  const price =
    stale || !equivalent
      ? "—"
      : Math.abs(equivalent.min - equivalent.max) < 0.005
        ? usd(equivalent.min)
        : `${usd(equivalent.min)} – ${usd(equivalent.max)}`;
  const priced = stale ? 0 : summary.pricedRequests;
  const reportedMissing =
    summary.reportedRequests === null
      ? null
      : summary.totalRequests - summary.reportedRequests;
  const reasons: Record<string, string> = {
    unsupported: l("料金表にないモデル", "Model not in price catalog"),
    "coverage-unknown": l(
      "内訳の取得件数が不明・サーバー更新が必要",
      "Coverage unavailable; server update required",
    ),
    incomplete: l(
      "トークン内訳が未取得、または不整合",
      "Missing or inconsistent token breakdown",
    ),
    partial: l("一部のリクエストのみ換算", "Only some requests are calculable"),
    complete: l(
      "取得済みリクエストを換算可能",
      "Captured requests are calculable",
    ),
  };
  return (
    <section
      className="cost-accounting"
      aria-label={l("費用とデータの充足率", "Cost and data coverage")}
    >
      <div className="cost-accounting-heading">
        <h2>{l("費用の内訳", "Cost breakdown")}</h2>
        <span>{l(`過去${days}日 / USD`, `Last ${days} days / USD`)}</span>
      </div>
      <div className="cost-accounting-grid">
        <div>
          <span>{l("API換算額（推定）", "API equivalent (estimate)")}</span>
          <strong data-cost="equivalent">{price}</strong>
          <small>
            {l(
              `${summary.totalRequests}件中${priced}件を換算`,
              `${priced} of ${summary.totalRequests} requests priced`,
            )}
          </small>
          <p>
            {l(
              "公式の標準単価による換算。実際の請求・支払いではありません。",
              "Official standard rates. Not an invoice or actual payment.",
            )}
          </p>
        </div>
        <div>
          <span>{l("記録された費用", "Reported cost")}</span>
          <strong data-cost="reported">
            {summary.reportedCost === null ? "—" : usd(summary.reportedCost)}
          </strong>
          <small>
            {summary.reportedRequests === null
              ? l("記録率を取得できません", "Coverage unavailable")
              : l(
                  `${summary.totalRequests}件中${summary.reportedRequests}件に金額あり`,
                  `${summary.reportedRequests} of ${summary.totalRequests} requests have a cost`,
                )}
          </small>
          <p>
            {l(
              "エージェントが報告した金額。請求書との照合はしていません。",
              "Amounts reported by agents. Not reconciled with invoices.",
            )}
          </p>
        </div>
        <div>
          <span>{l("実際の支払額", "Actual payment")}</span>
          <strong data-cost="payment">{l("未連携", "Not connected")}</strong>
          <small>
            {l(
              "請求書・決済情報が必要です",
              "Requires invoices or payment records",
            )}
          </small>
          <p>
            {l(
              "Subscription、従量課金、税・割引を含む支払額は利用ログだけでは分かりません。未取得は0円ではありません。",
              "Usage logs cannot establish payments including subscriptions, metered usage, tax or discounts. Unknown does not mean zero.",
            )}
          </p>
        </div>
      </div>
      <div className="cost-coverage-note" role="status">
        <strong>{l("費用データの欠損", "Missing cost data")}</strong>
        <span>
          {l(
            `API換算できないリクエスト：${summary.totalRequests - priced} / ${summary.totalRequests}件`,
            `Requests not priced: ${summary.totalRequests - priced} / ${summary.totalRequests}`,
          )}
        </span>
        <span>
          {reportedMissing === null
            ? l("金額未記録の件数：不明", "Missing reported costs: unknown")
            : l(
                `金額が記録されていないリクエスト：${reportedMissing} / ${summary.totalRequests}件`,
                `Requests without reported cost: ${reportedMissing} / ${summary.totalRequests}`,
              )}
        </span>
      </div>
      <p className="panel__note">
        {l(
          "対象は収集できたAPIリクエストのみ。収集されていない利用や、トークン数のないチャット利用はこの件数に含まれません。",
          "Counts cover captured API requests only. Uncollected activity and chat usage without measured tokens are outside this denominator.",
        )}
      </p>
      {stale && (
        <p role="alert">
          {l(
            "料金が古いため換算を停止しています。公式料金を更新してください。",
            "Estimates are paused because prices are stale. Refresh official prices.",
          )}
        </p>
      )}
      {detailed ? (
        <details>
          <summary>
            {l("モデルごとの欠損を見る", "Missing data by model")}
          </summary>
          <div className="cost-coverage-table">
            <table>
              <thead>
                <tr>
                  <th>{l("モデル / 思考レベル", "Model / effort")}</th>
                  <th>{l("APIリクエスト", "API requests")}</th>
                  <th>{l("換算可能", "Priced")}</th>
                  <th>{l("金額あり", "Cost reported")}</th>
                  <th>{l("換算の状態", "Estimation status")}</th>
                </tr>
              </thead>
              <tbody>
                {summary.details.map((d) => (
                  <tr key={`${d.row[0]}:${d.row[1]}`}>
                    <td>
                      {d.row[0]} / {d.row[1] ?? "—"}
                    </td>
                    <td>{d.row[2]}</td>
                    <td>{stale ? 0 : d.pricedRequests}</td>
                    <td>{d.row[13] ?? "—"}</td>
                    <td>
                      {stale
                        ? l("料金更新が必要", "Price refresh required")
                        : reasons[d.reason]}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </details>
      ) : (
        <Link to="/cost">
          {l(
            "欠損の内訳と削減候補を確認 →",
            "Review missing data and savings candidates →",
          )}
        </Link>
      )}
      <p className="panel__note">
        {l("料金取得日", "Prices retrieved")}:{" "}
        {new Date(catalog.fetchedAt).toLocaleDateString(locale)} ·{" "}
        <a href={catalog.sources.anthropic} target="_blank" rel="noreferrer">
          Anthropic
        </a>{" "}
        /{" "}
        <a href={catalog.sources.openai} target="_blank" rel="noreferrer">
          OpenAI
        </a>{" "}
        ·{" "}
        {l(
          "標準・グローバルのテキストトークン。長文・キャッシュ保存期間の差は幅で表示。税・割引・ツール料金は対象外。",
          "Standard global text tokens; ranges cover context tiers and cache durations. Excludes tax, discounts and tool fees.",
        )}
      </p>
    </section>
  );
}

export function CostSummaryLoader({ days }: { days: number }) {
  const data = useAsync(() => getModels(days), [days]);
  return (
    <QueryBoundary state={data} onRetry={data.reload}>
      {(d) => <CostSummary rows={d.models.rows} days={days} />}
    </QueryBoundary>
  );
}
