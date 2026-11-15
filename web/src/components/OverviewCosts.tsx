import { useState } from "react";
import { NativeSelect } from "@kikimimi/ui";
import {
  planPrices,
  monthlyPrice,
  subscriptionComparison,
} from "../cost/subscriptions";
import { changeContract, useContracts } from "../cost/contracts";
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
  const [saveError, setSaveError] = useState(false);
  const { rows: contracts, status } = useContracts(
    session?.data_source !== "s3",
  );
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
        const summary = summarizeCosts(
          result.models.rows,
          catalog.models,
          catalog.history,
        );
        const stale = isStale(catalog.fetchedAt) && !summary.pricedRequests;
        const estimate = stale ? null : summary.equivalent;
        const { periodCost, usage } = subscriptionComparison(
          contracts,
          result.subscription_models?.rows ?? [],
          catalog.models,
          days,
          catalog.history,
        );
        const covered = stale ? null : usage.equivalent;
        const partial = usage.pricedRequests < usage.totalRequests;
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
                        {row.account && row.account + " · "}
                        {monthlyPrice(row) === null
                          ? l("月額未登録", "Monthly price unknown")
                          : currency.format(monthlyPrice(row)!) +
                            l(" / 月", " / month")}
                      </p>
                    </div>
                  ))}
                  <small>
                    {l(
                      "月額を30日で日割り。プラン変更は履歴の変更日から適用し、初回確認より前は最初の月額で概算。",
                      "Monthly fees prorated over 30 days. Changes apply from the effective date in plan history. Earlier dates use the first known rate.",
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
                      partial ? "サブスクでの推定節約額（換算できた分）" : "サブスクでの推定節約額",
                      partial ? "Estimated subscription savings (priced usage only)" : "Estimated subscription savings",
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
                  {status !== "loading" &&
                    !contracts.some(
                      (c) => c.service === "Claude" && monthlyPrice(c) !== null,
                    ) && (
                      <label className="field">
                        {l(
                          "Claude のプランを選んで節約額を表示",
                          "Choose your Claude plan to estimate savings",
                        )}
                        <NativeSelect
                          aria-label={l(
                            "Claude の契約プラン",
                            "Claude subscription plan",
                          )}
                          value=""
                          onChange={(event) => {
                            const plan = event.target.value;
                            if (!plan) return;
                            const account =
                              contracts.find((c) => c.service === "Claude")
                                ?.account ?? "";
                            try {
                              changeContract("Claude", account, {
                                service: "Claude",
                                account,
                                plan,
                                monthlyUsd: null,
                              });
                              setSaveError(false);
                            } catch {
                              setSaveError(true);
                            }
                          }}
                        >
                          <option value="">
                            {l("プランを選択", "Select a plan")}
                          </option>
                          {Object.entries(planPrices.Claude).map(
                            ([plan, price]) => (
                              <option key={plan} value={plan}>
                                {plan} · ${price}
                                {l("/月", "/mo")}
                              </option>
                            ),
                          )}
                        </NativeSelect>
                        {saveError && (
                          <small role="alert">
                            {l(
                              "端末に保存できませんでした",
                              "Could not save on this device",
                            )}
                          </small>
                        )}
                      </label>
                    )}
                  {covered && (
                    <small>
                      {l(
                        "対象モデルのAPI換算額",
                        "API equivalent for eligible models",
                      )}{" "}
                      {range(covered.min, covered.max)} ·{" "}
                      {l(
                        `${usage.totalRequests}件中${usage.pricedRequests}件を換算`,
                        `${usage.pricedRequests} of ${usage.totalRequests} requests priced`,
                      )}
                    </small>
                  )}
                  <small>
                    {result.billing_routes ? l(
                      `節約額の対象外：APIキー ${result.billing_routes.api_key}件、Router ${result.billing_routes.router}件、経路不明 ${result.billing_routes.unknown}件。`,
                      `Excluded from savings: API key ${result.billing_routes.api_key}, Router ${result.billing_routes.router}, unknown route ${result.billing_routes.unknown}.`
                    ) : l(`経路・契約を照合できない ${summary.totalRequests - usage.totalRequests}件を除外。`, `${summary.totalRequests - usage.totalRequests} requests excluded.`)}
                    {result.billing_routes && result.billing_routes.subscription > usage.totalRequests && l(
                      ` 対象契約・モデルとの照合外 ${result.billing_routes.subscription - usage.totalRequests}件。`,
                      ` Outside matched contracts and models: ${result.billing_routes.subscription - usage.totalRequests}.`
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
