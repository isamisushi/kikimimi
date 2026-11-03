import { NativeSelect } from "@kikimimi/ui";
import { useEffect, useState } from "react";
import type { ModelRow } from "../api/types";
import { useLocale } from "../i18n";
import catalog from "../cost/pricing.json";
import { isStale, summarizeCosts, type Range } from "../cost/estimate";
import {
  amount,
  compareOptions,
  initialComparison,
  type Choice,
  type ContractInput,
} from "../cost/compare";

function NumberField({
  label,
  value,
  onChange,
  min = 0,
  max,
  integer = false,
  id,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  min?: number;
  max?: number;
  integer?: boolean;
  id: string;
}) {
  const n = amount(value);
  const invalid =
    value !== "" &&
    (n === null ||
      n < min ||
      (max !== undefined && n > max) ||
      (integer && !Number.isInteger(n)));
  return (
    <label htmlFor={id}>
      {label}
      <input
        id={id}
        type="number"
        min={min}
        max={max}
        step={integer ? "1" : "any"}
        value={value}
        aria-invalid={invalid || undefined}
        onChange={(e) => onChange(e.target.value)}
      />
    </label>
  );
}

export function BillingComparison({
  rows,
  days,
  status,
}: {
  rows: ModelRow[] | null;
  days: number;
  status: "ok" | "loading" | "error";
}) {
  const locale = useLocale();
  const l = (ja: string, en: string) => (locale === "ja" ? ja : en);
  const [input, setInput] = useState(initialComparison);
  // Confirmations are tied to the period. Parent remounts this view when account/team/filters change.
  const [confirmedDays, setConfirmedDays] = useState<number | null>(null);
  useEffect(() => setConfirmedDays(null), [days]);
  const scopeConfirmed = input.scopeConfirmed && confirmedDays === days;
  const summary = summarizeCosts(rows ?? [], catalog.models);
  const stale = isStale(catalog.fetchedAt);
  const result = compareOptions(
    { ...input, scopeConfirmed },
    { ...summary, days, stale },
  );
  const labels: Record<Choice, string> = {
    current: l("サブスク継続", "Keep subscription"),
    proposed: l("プラン変更", "Change plan"),
    api: l("API移行", "Move to API"),
  };
  const ids: Choice[] = ["current", "proposed", "api"];
  const money = (n: number) =>
    new Intl.NumberFormat(locale, {
      style: "currency",
      currency: input.currency,
      minimumFractionDigits: 2,
      maximumFractionDigits: 2,
    }).format(n);
  const range = (r: Range | null) =>
    !r
      ? "—"
      : Math.abs(r.min - r.max) < 0.005
        ? money(r.min)
        : `${money(r.min)} – ${money(r.max)}`;
  const optionCost = (id: Choice, value: Range | null) =>
    id === "api" && !result.complete && value
      ? `≥ ${money(value.min)}`
      : range(value);
  const updateContract = (
    choice: "current" | "proposed",
    field: keyof ContractInput,
    value: string,
  ) => {
    setInput((prev) => ({
      ...prev,
      [choice]: { ...prev[choice], [field]: value },
    }));
  };
  const reasons: string[] = [];
  if (!result.totals.current || !result.totals.proposed)
    reasons.push(
      l(
        "契約単価・席数・比較期間を正しく入力してください。",
        "Enter valid contract prices, seat counts and comparison months.",
      ),
    );
  if (status !== "ok")
    reasons.push(
      l(
        "使用量を取得できるまでAPIとの比較は保留です。",
        "API comparison is pending until usage is available.",
      ),
    );
  else if (!summary.totalRequests)
    reasons.push(
      l(
        "観測したリクエストがありません。API費用は0とは判断できません。",
        "No requests were captured. API cost cannot be assumed to be zero.",
      ),
    );
  else if (!result.complete)
    reasons.push(
      l(
        `API換算できるのは${summary.pricedRequests.toLocaleString()} / ${summary.totalRequests.toLocaleString()}件です。欠損分を含む総額は不明です。`,
        `Only ${summary.pricedRequests.toLocaleString()} of ${summary.totalRequests.toLocaleString()} requests are priced. The total including missing data is unknown.`,
      ),
    );
  if (stale)
    reasons.push(
      l(
        "料金表が古いため、APIの最安判定を停止しています。",
        "The price catalog is stale; API ranking is disabled.",
      ),
    );
  if (!result.totals.api && summary.equivalent)
    reasons.push(
      l(
        "為替・利用量・API追加費用・一時費用を正しく入力してください。",
        "Enter valid exchange rate, usage, API extras and transition costs.",
      ),
    );
  if (!scopeConfirmed)
    reasons.push(
      l(
        "契約と使用量が同じ対象か確認してください。",
        "Confirm that the contracts and usage cover the same scope.",
      ),
    );
  const title = result.ready
    ? result.winners.length
      ? `${l("入力条件で最安", "Lowest cost under these assumptions")}: ${result.winners.map((id) => labels[id]).join(" / ")}`
      : l(
          "料金の幅が重なるため、最安を断定できません。",
          "Cost ranges overlap; there is no certain cheapest option.",
        )
    : l("3案の最安判定は保留です。", "The three-way ranking is pending.");

  function download() {
    const report = [
      l("支払い方式の比較（試算）", "Billing comparison (scenario)"),
      new Date().toISOString(),
      `${days} ${l("日間の使用量 → 30日換算", "days of usage → 30-day estimate")}`,
      `${l("比較期間", "Comparison horizon")}: ${input.months} ${l("か月", "months")} / ${input.currency}`,
      title,
      ...ids.map(
        (id) =>
          `${labels[id]}: ${optionCost(id, result.monthly[id])} / ${l("月", "month")}; ${l("期間総額", "horizon total")}: ${optionCost(id, result.totals[id])}`,
      ),
      ...reasons,
      l(
        "契約変更・API実行は行っていません。機能・利用枠・税・解約条件を確認してください。",
        "No contract was changed and no API was called. Verify capabilities, limits, tax and cancellation terms.",
      ),
      `Prices: ${catalog.fetchedAt}`,
      ...Object.values(catalog.sources),
      JSON.stringify(
        {
          ...input,
          scopeConfirmed,
          pricedRequests: summary.pricedRequests,
          totalRequests: summary.totalRequests,
        },
        null,
        2,
      ),
    ].join("\n");
    const url = URL.createObjectURL(
      new Blob([report], { type: "text/plain;charset=utf-8" }),
    );
    const a = document.createElement("a");
    a.href = url;
    a.download = "kikimimi-billing-comparison.txt";
    a.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  }

  return (
    <section
      className="billing-comparison"
      aria-labelledby="billing-compare-title"
    >
      <header className="cost-section-heading">
        <div>
          <span className="cost-eyebrow">SUBSCRIPTION / PLAN / API</span>
          <h2 id="billing-compare-title">
            {l("どの支払い方が安いか。", "Which way of paying costs less?")}
          </h2>
        </div>
        <button className="btn" type="button" onClick={download}>
          {l("比較をダウンロード", "Download comparison")}
        </button>
      </header>
      <p>
        {l(
          "同じ人数・作業範囲の3案を比較します。契約額は請求書や契約画面から入力してください。入力値はこの画面内の試算で、画面を離れるとリセットされます。",
          "Compare three options for the same people and work. Enter contract amounts from your invoices or billing page. Inputs are a scenario for this view and reset when you leave.",
        )}
      </p>
      <div className="billing-settings">
        <label htmlFor="compare-currency">
          {l("比較通貨", "Comparison currency")}
          <NativeSelect
            id="compare-currency"
            value={input.currency}
            onChange={(e) =>
              setInput((prev) => ({
                ...initialComparison(),
                currency: e.target.value as "USD" | "JPY",
                months: prev.months,
                usagePercent: prev.usagePercent,
              }))
            }
          >
            <option value="USD">USD</option>
            <option value="JPY">JPY</option>
          </NativeSelect>
          <small>
            {l(
              "通貨変更で料金の入力をクリアします。",
              "Changing currency clears entered prices.",
            )}
          </small>
        </label>
        {input.currency === "JPY" && (
          <NumberField
            id="compare-fx"
            label={l("1 USDあたりの円（手入力）", "JPY per USD (manual)")}
            value={input.yenPerDollar}
            min={0.000001}
            onChange={(v) => setInput((prev) => ({ ...prev, yenPerDollar: v }))}
          />
        )}
        <NumberField
          id="compare-months"
          label={l("比較する月数（1〜120）", "Months to compare (1–120)")}
          value={input.months}
          min={1}
          max={120}
          integer
          onChange={(v) => setInput((prev) => ({ ...prev, months: v }))}
        />
        <NumberField
          id="compare-usage"
          label={l(
            "APIで想定する利用量（現在比 %）",
            "Expected API usage (% of observed)",
          )}
          value={input.usagePercent}
          min={0.01}
          max={10000}
          onChange={(v) => setInput((prev) => ({ ...prev, usagePercent: v }))}
        />
      </div>
      <p className="billing-note">
        {l(
          "全案で税・割引の扱いをそろえてください。年払いは月割りの費用比較です。支払時期や返金は推定しません。",
          "Use the same tax and discount basis for every option. Annual bills are amortized monthly; payment timing and refunds are not inferred.",
        )}
      </p>
      <div className="billing-options">
        {(["current", "proposed"] as const).map((choice) => (
          <article
            key={choice}
            className="billing-option"
            aria-label={labels[choice]}
          >
            <span className="cost-eyebrow">
              {choice === "current" ? "01 / KEEP" : "02 / CHANGE"}
            </span>
            <h3>{labels[choice]}</h3>
            <div className="billing-price" data-comparison-monthly={choice}>
              {range(result.monthly[choice])}
              <small>{l("月額換算", "per month")}</small>
            </div>
            <p data-comparison-total={choice}>
              {l("期間総額", "Horizon total")}:{" "}
              <strong>{range(result.totals[choice])}</strong>
            </p>
            <NumberField
              id={`compare-${choice}-price`}
              label={`${l("1席あたりの契約単価", "Contract price per seat")} (${input.currency})`}
              value={input[choice].price}
              onChange={(v) => updateContract(choice, "price", v)}
            />
            <div className="billing-pair">
              <label htmlFor={`compare-${choice}-period`}>
                {l("単価の期間", "Price period")}
                <NativeSelect
                  id={`compare-${choice}-period`}
                  value={input[choice].period}
                  onChange={(e) =>
                    updateContract(choice, "period", e.target.value)
                  }
                >
                  <option value="monthly">{l("月払い", "Monthly")}</option>
                  <option value="annual">
                    {l("年払い（年間総額）", "Annual total")}
                  </option>
                </NativeSelect>
              </label>
              <NumberField
                id={`compare-${choice}-seats`}
                label={l("席数", "Seats")}
                value={input[choice].seats}
                min={1}
                integer
                onChange={(v) => updateContract(choice, "seats", v)}
              />
            </div>
            <NumberField
              id={`compare-${choice}-extras`}
              label={`${l("全席の追加課金 / 月", "Extras for all seats / month")} (${input.currency})`}
              value={input[choice].extras}
              onChange={(v) => updateContract(choice, "extras", v)}
            />
            {choice === "proposed" && (
              <NumberField
                id="compare-proposed-transition"
                label={`${l("変更時の一時費用・残契約分", "Change fees and remaining commitment")} (${input.currency})`}
                value={input.proposed.transition}
                onChange={(v) => updateContract(choice, "transition", v)}
              />
            )}
            <small>
              {choice === "current"
                ? l(
                    "単価 × 席数 ＋ 全席の追加課金。",
                    "Price × seats + extras for all seats.",
                  )
                : l(
                    "同じ人数をカバーする席数を入力。必要な機能・利用上限を満たすか確認してください。",
                    "Enter enough seats for the same people. Verify required features and usage limits.",
                  )}
            </small>
          </article>
        ))}
        <article className="billing-option" aria-label={labels.api}>
          <span className="cost-eyebrow">03 / API</span>
          <h3>{labels.api}</h3>
          <div className="billing-price" data-comparison-monthly="api">
            {!result.complete && result.monthly.api
              ? `≥ ${money(result.monthly.api.min)}`
              : range(result.monthly.api)}
            <small>{l("30日あたりの推定", "estimated per 30 days")}</small>
          </div>
          <p data-comparison-total="api">
            {l("期間総額", "Horizon total")}:{" "}
            <strong>
              {!result.complete && result.totals.api
                ? `≥ ${money(result.totals.api.min)}`
                : range(result.totals.api)}
            </strong>
          </p>
          <p className="billing-api-basis">
            {l(
              `過去${days}日の計算可能な使用量 × 30/${days} × 利用量${input.usagePercent || "—"}%`,
              `Priced usage over ${days} days × 30/${days} × ${input.usagePercent || "—"}% usage`,
            )}
            <br />
            {l(
              "現在と同じモデル・トークン量を前提とします。席数は使用量に含まれるため再乗算しません。",
              "Assumes the same models and token mix. Seats are already represented in usage and are not multiplied again.",
            )}
          </p>
          <NumberField
            id="compare-api-extras"
            label={`${l("ツール・運用などの追加費用 / 月", "Tools and operating extras / month")} (${input.currency})`}
            value={input.apiExtras}
            onChange={(v) => setInput((prev) => ({ ...prev, apiExtras: v }))}
          />
          <NumberField
            id="compare-api-transition"
            label={`${l("移行費用・残契約分（一時費用）", "Migration and remaining commitment (one-time)")} (${input.currency})`}
            value={input.apiTransition}
            onChange={(v) =>
              setInput((prev) => ({ ...prev, apiTransition: v }))
            }
          />
          <small>
            {l(
              "継続するサブスク料金は月額追加費用に、解約できない残額は一時費用に含めてください。API利用には別のアプリや実装が必要な場合があります。",
              "Include retained subscriptions in monthly extras and uncancellable balances in one-time costs. API use may require another app or implementation.",
            )}
          </small>
          {!result.complete && (
            <p className="billing-incomplete">
              {l(
                "計算可能な部分の下限です。欠損を含む上限は不明です。",
                "Lower bound from priced usage; the upper bound including missing data is unknown.",
              )}
            </p>
          )}
        </article>
      </div>
      <label className="billing-confirm" htmlFor="compare-scope">
        <input
          id="compare-scope"
          type="checkbox"
          checked={scopeConfirmed}
          onChange={(e) => {
            setConfirmedDays(days);
            setInput((prev) => ({ ...prev, scopeConfirmed: e.target.checked }));
          }}
        />
        <span>
          {l(
            "表示期間が普段の利用を代表し、対象の全員・全デバイス・必要な作業の使用量が収集され、入力した2つの契約が同じ対象をカバーすることを確認しました。",
            "I confirm this period represents normal usage, captures all relevant people, devices and work, and both entered contracts cover that same scope.",
          )}
        </span>
      </label>
      <div
        className={`billing-verdict ${result.ready && result.winners.length ? "billing-verdict--ready" : ""}`}
        role="status"
        aria-live="polite"
        data-comparison-verdict
      >
        <h3>{title}</h3>
        <p>
          {l(
            "最安は一時費用を含む比較期間の総額で判定します。実際の請求額や、移行できることの保証ではありません。",
            "Ranking uses the horizon total, including one-time costs. It is not an invoice or a guarantee that migration is feasible.",
          )}
        </p>
        {reasons.length > 0 && (
          <ul>
            {reasons.map((reason) => (
              <li key={reason}>{reason}</li>
            ))}
          </ul>
        )}
        {!result.ready && result.totals.current && result.totals.proposed && (
          <p data-subscription-verdict>
            {l(
              "入力したサブスク2案の費用だけなら",
              "Among the two entered subscriptions only",
            )}
            :{" "}
            {Math.abs(result.totals.current.min - result.totals.proposed.min) <
            0.005
              ? l("同額", "Equal cost")
              : labels[
                  result.totals.current.min < result.totals.proposed.min
                    ? "current"
                    : "proposed"
                ]}
            。
          </p>
        )}
        {!result.complete && result.ready && (
          <p>
            {l(
              "計算できたAPI費用の下限だけで、最安のサブスク案を上回っています。欠損分が増えても、この費用順位は変わりません。",
              "The priced API lower bound already exceeds the cheapest subscription. Adding missing costs cannot change this cost ranking.",
            )}
          </p>
        )}
        {result.savings && (
          <ul>
            {(["proposed", "api"] as const).map((id) => {
              const s = result.savings![id];
              if (!s) return null;
              return (
                <li key={id}>
                  {labels[id]} —{" "}
                  {s.min >= 0
                    ? `${l("継続より削減見込み", "Estimated savings vs keeping")}: ${range(s)}`
                    : s.max <= 0
                      ? `${l("継続より増額見込み", "Estimated increase vs keeping")}: ${range({ min: -s.max, max: -s.min })}`
                      : l(
                          "継続より安くなるかは料金条件によります。",
                          "Whether this is cheaper than keeping depends on pricing conditions.",
                        )}
                </li>
              );
            })}
          </ul>
        )}
        {result.winners.includes("api") && (
          <p>
            {l(
              "次のアクション：代表的な作業をAPIで試し、機能と品質を確認してから契約を変更してください。",
              "Next: trial representative work through the API and check capabilities and quality before changing contracts.",
            )}
          </p>
        )}
        {result.winners.includes("proposed") && (
          <p>
            {l(
              "次のアクション：候補プランの利用枠・最低席数・更新日・解約条件を確認してください。",
              "Next: verify the candidate plan’s usage limits, minimum seats, renewal date and cancellation terms.",
            )}
          </p>
        )}
        {result.winners.length === 1 && result.winners[0] === "current" && (
          <p>
            {l(
              "入力条件では継続が最安です。利用量や更新料金が変わった時点で再比較してください。",
              "Keeping is cheapest under these assumptions. Compare again when usage or renewal pricing changes.",
            )}
          </p>
        )}
      </div>
      {result.breakEven.current && (
        <p className="billing-note" data-comparison-breakeven>
          {l(
            "APIと継続サブスクの損益分岐点",
            "API vs current subscription break-even",
          )}
          : {result.breakEven.current.min.toFixed(2)}–
          {result.breakEven.current.max.toFixed(2)}
          {l(
            "倍（上記の想定API利用量に対して）。範囲の下限未満ならAPIが安く、上限超ならサブスクが安くなります。月額追加費用込み・一時費用を除く比較です。",
            "× the projected API usage above. Below this range API is cheaper; above it subscription is cheaper. Includes monthly extras, excludes one-time costs.",
          )}
        </p>
      )}
      <p className="billing-note">
        {l(
          "API単価は上の料金表取得日時・公式出典を参照。未収集の利用、税、地域差、割引、再試行の変化は自動補完しません。",
          "API rates use the catalog date and official sources above. Uncaptured activity, tax, regional pricing, discounts and changed retries are not automatically filled in.",
        )}
      </p>
    </section>
  );
}
