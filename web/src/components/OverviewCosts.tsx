import { monthlyPrice, subscriptionComparison } from "../cost/subscriptions";
import { useContracts } from "../cost/contracts";
import { useSession } from "../hooks/useSession";
import { Link } from "../router/Router";
import { getModels } from "../api/client";
import { useAsync } from "../hooks/useAsync";
import { isStale, summarizeCosts } from "../cost/estimate";
import { useCatalog } from "../cost/catalog";
import { useLocale } from "../i18n";
import { QueryBoundary } from "./QueryBoundary";

export function OverviewCosts({ days }: { days: number }) {
  const catalog = useCatalog();
  const locale = useLocale();
  const l = (ja: string, en: string) => (locale === "ja" ? ja : en);
  const data = useAsync(() => getModels(days), [days]);
  const { session } = useSession();
  const { rows: contracts } = useContracts(session?.data_source !== "s3");
  const currency = new Intl.NumberFormat(locale, {
    style: "currency",
    currency: "USD",
  });
  const range = (min: number, max: number) => {
    const low = currency.format(min);
    const high = currency.format(max);
    return low === high ? low : `${low} – ${high}`;
  };

  return (
    <QueryBoundary state={data} onRetry={data.reload}>
      {(result) => {
        const summary = summarizeCosts(result.models.rows, catalog.models, catalog.history);
        const stale = isStale(catalog.fetchedAt) && !summary.pricedRequests;
        const estimate = stale ? null : summary.equivalent;
        const { periodCost, usage } = subscriptionComparison(
          contracts,
          result.subscription_models?.rows ?? [],
          catalog.models,
          days,
          catalog.history,
        );
        const covered =
          !stale && usage.totalRequests === usage.pricedRequests
            ? usage.equivalent
            : null;
        return (
          <div className="cost-accounting-grid overview-costs">
            <div>
              <span>{l("API換算額（推定）", "Estimated API equivalent")}</span>
              <strong>
                {estimate ? range(estimate.min, estimate.max) : "—"}
              </strong>
              <small>
                {stale
                  ? l("料金表の更新が必要です", "Price refresh required")
                  : l(
                      `${summary.totalRequests}件中${summary.pricedRequests}件を利用日の単価で換算`,
                      `${summary.pricedRequests} of ${summary.totalRequests} captured requests priced at usage-date rates`,
                    )}
              </small>
            </div>
            {session?.data_source !== "s3" && (
              <>
                <div>
                  <span>
                    {l(
                      `サブスク料金（${days}日分）`,
                      `Subscription cost (${days} days)`,
                    )}
                  </span>
                  <strong>
                    {periodCost === null ? "—" : currency.format(periodCost)}
                  </strong>
                  {contracts.map((row) => (
                    <div key={JSON.stringify([row.service, row.account])}>
                      <b>
                        {row.service} · {row.plan}
                      </b>
                      <p>
                        {row.account} ·{" "}
                        {monthlyPrice(row) === null
                          ? l("月額未登録", "Monthly price unknown")
                          : currency.format(monthlyPrice(row)!) +
                            l(" / 月", " / month")}
                      </p>
                    </div>
                  ))}
                  <small>
                    {l(
                      "契約月額の合計を30日で日割り",
                      "Combined monthly fees prorated over 30 days",
                    )}
                  </small>
                  <Link to="/settings">
                    {l(
                      "設定の「一般」で契約を管理",
                      "Manage plans in General settings",
                    )}
                  </Link>
                </div>
                <div>
                  <span>
                    {l(
                      "サブスクでの推定節約額",
                      "Estimated subscription savings",
                    )}
                  </span>
                  <strong>
                    {covered && periodCost !== null
                      ? range(
                          covered.min - periodCost,
                          covered.max - periodCost,
                        )
                      : "—"}
                  </strong>
                  {covered && (
                    <small>
                      {l(
                        "対象モデルのAPI換算額",
                        "API equivalent for eligible models",
                      )}{" "}
                      {range(covered.min, covered.max)} ·{" "}
                      {l(
                        `${usage.pricedRequests}件`,
                        `${usage.pricedRequests} requests`,
                      )}
                    </small>
                  )}
                  <small>
                    {l(
                      `サブスク経由を確認できた対象モデルのみ。APIキー・Router・経路不明など計${summary.totalRequests - usage.totalRequests}件を除外。`,
                      `Only attributed subscription requests for registered providers. ${summary.totalRequests - usage.totalRequests} requests excluded (API keys, Router, unknown or other providers).`,
                    )}
                  </small>
                  {covered &&
                    periodCost !== null &&
                    covered.min < periodCost && (
                      <small>
                        {l(
                          "マイナス分はサブスクの方が割高です。",
                          "Negative amounts mean the subscriptions cost more.",
                        )}
                      </small>
                    )}
                </div>
              </>
            )}
          </div>
        );
      }}
    </QueryBoundary>
  );
}
