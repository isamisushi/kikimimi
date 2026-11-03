import { useEffect, useRef, useState } from "react";
import { getModels } from "../api/client";
import { useAsync } from "../hooks/useAsync";
import { QueryBoundary } from "../components/QueryBoundary";
import { CostSummary } from "../components/CostSummary";
import { useSession } from "../hooks/useSession";
import { SubscriptionUsage } from "../components/SubscriptionUsage";
import { Link } from "../router/Router";
import { useLocale } from "../i18n";
import catalog from "../cost/pricing.json";
import {
  isStale,
  opportunities,
  subscriptionSaving,
  type Opportunity,
  type Range,
} from "../cost/estimate";
import "../styles/cost.css";

export function CostIntelligence() {
  const locale = useLocale();
  const { session } = useSession();
  const l = (ja: string, en: string) => (locale === "ja" ? ja : en);
  const [mode, setMode] = useState("unknown");
  const [days, setDays] = useState(30);
  const [currentBill, setCurrentBill] = useState("");
  const [proposedBill, setProposedBill] = useState("");
  const planRef = useRef<HTMLElement>(null);
  const [plan, setPlan] = useState<Opportunity | null>(null);
  const [checked, setChecked] = useState<boolean[]>([false, false, false]);
  useEffect(() => {
    if (plan) {
      planRef.current?.focus();
      planRef.current?.scrollIntoView({ block: "nearest" });
    }
  }, [plan]);
  const data = useAsync(() => getModels(days), [days]);
  const stale = isStale(catalog.fetchedAt);
  const money = (n: number) =>
    new Intl.NumberFormat(locale, {
      style: "currency",
      currency: "USD",
    }).format(n);
  const range = (r: Range) =>
    Math.abs(r.max - r.min) < 0.005
      ? money(r.min)
      : `${money(r.min)} – ${money(r.max)}`;
  const saving = subscriptionSaving(currentBill, proposedBill);
  const steps = [
    l(
      "代表的な作業を10件選ぶ（難しい例を含める）",
      "Choose 10 representative tasks, including difficult cases.",
    ),
    l(
      "両モデルで実行し、正確さ・速度・再試行回数を記録する",
      "Run both models; record accuracy, latency and retries.",
    ),
    l(
      "実際の請求額と利用制限を比較し、採用を判断する",
      "Compare actual billing and limits before deciding to switch.",
    ),
  ];
  function downloadPlan() {
    if (!plan) return;
    const text = [
      `${plan.current.name} → ${plan.target.name}`,
      `${days} days / USD / standard global API`,
      `Before: ${range(plan.before)}`,
      `After: ${range(plan.after)}`,
      `Potential reduction: ${range(plan.saving)}`,
      "Same token counts assumed; quality and compatibility NOT verified.",
      `Prices retrieved: ${catalog.fetchedAt}`,
      catalog.sources[plan.current.provider as keyof typeof catalog.sources],
      ...steps.map((s, i) => `${checked[i] ? "[x]" : "[ ]"} ${s}`),
    ].join("\n");
    const url = URL.createObjectURL(
      new Blob([text], { type: "text/plain;charset=utf-8" }),
    );
    const a = document.createElement("a");
    a.href = url;
    a.download = "kikimimi-model-evaluation.txt";
    a.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  }
  return (
    <div className="page cost-page">
      <header className="cost-heading">
        <div>
          <span className="cost-eyebrow">COST INTELLIGENCE / MVP</span>
          <h1>{l("次に、どこを変えるか。", "Your next cost decision.")}</h1>
          <p>
            {l(
              "利用実績から、検証する価値のあるモデル変更を見つける。",
              "Turn observed usage into model changes worth testing.",
            )}
          </p>
        </div>
        <label>
          {l("対象期間", "Period")}
          <select
            value={days}
            onChange={(e) => {
              setDays(Number(e.target.value));
              setPlan(null);
            }}
          >
            <option value={7}>{l("過去7日", "Last 7 days")}</option>
            <option value={30}>{l("過去30日", "Last 30 days")}</option>
            <option value={90}>{l("過去90日", "Last 90 days")}</option>
          </select>
        </label>
      </header>
      <section className="cost-controls">
        <label>
          {l("この表示範囲の支払い方式", "Billing for this view")}
          <select
            value={mode}
            onChange={(e) => {
              setMode(e.target.value);
              setPlan(null);
            }}
          >
            <option value="unknown">
              {l("未確認 / 混在", "Unknown / mixed")}
            </option>
            <option value="api">
              {l(
                "API従量課金のみ（標準・グローバル）",
                "API only (standard, global)",
              )}
            </option>
            <option value="subscription">
              {l("Subscription（定額契約）", "Subscription")}
            </option>
          </select>
        </label>
        <p>
          {l(
            "契約情報はログから判定できません。混在する場合は上部のフィルターで対象を絞ってください。",
            "Logs do not identify your billing contract. Filter to a single billing scope when usage is mixed.",
          )}
        </p>
      </section>
      <QueryBoundary
        state={data}
        onRetry={() => {
          setPlan(null);
          data.reload();
        }}
      >
        {(d) => <CostSummary rows={d.models.rows} days={days} detailed />}
      </QueryBoundary>
      {mode === "subscription" ? (
        <>
          <section className="cost-hero">
            <span className="cost-eyebrow">SUBSCRIPTION</span>
            <h2>
              {l(
                "モデルを変えても、定額料金は下がりません。",
                "Changing models does not reduce a fixed fee.",
              )}
            </h2>
            <p>
              {l(
                "見直すのはプラン、席数、追加課金。API換算額を節約額として扱いません。",
                "Review plans, seats and extra charges. API-equivalent usage is not cash savings.",
              )}
            </p>
          </section>
          <section className="panel cost-subscription">
            <h2>
              {l(
                "次のアクション：契約を見直す",
                "Next action: review your contract",
              )}
            </h2>
            <p>
              {l(
                "同じ期間・通貨で、契約全体の月額を入力してください（年払いは月割り）。追加利用料は含めません。",
                "Enter total monthly contract costs in USD (annual bills divided by 12). Extra usage is excluded.",
              )}
            </p>
            <div className="cost-bills">
              <label>
                {l("現在の月額合計（USD）", "Current monthly total (USD)")}
                <input
                  type="number"
                  min="0"
                  step="0.01"
                  value={currentBill}
                  onChange={(e) => setCurrentBill(e.target.value)}
                />
              </label>
              <label>
                {l("変更候補の月額合計（USD）", "Proposed monthly total (USD)")}
                <input
                  type="number"
                  min="0"
                  step="0.01"
                  value={proposedBill}
                  onChange={(e) => setProposedBill(e.target.value)}
                />
              </label>
            </div>
            <p aria-live="polite" className="cost-result">
              {saving === null
                ? l(
                    "契約額を入力すると差額がわかります。",
                    "Enter both costs to compare.",
                  )
                : `${l("契約変更後の月額差", "Monthly difference after contract change")}: ${money(Math.abs(saving))} ${saving < 0 ? l("増加", "increase") : l("削減候補", "potential reduction")}`}
            </p>
            <p>
              {l(
                "下位プランで足りるかは未検証です。利用枠・更新日・最低席数・追加課金を契約画面で確認してください。",
                "Downgrade feasibility is unverified. Check limits, renewal dates, minimum seats and extra charges in your billing settings.",
              )}
            </p>
          </section>
          {session?.subscription_usage ? (
            <SubscriptionUsage />
          ) : (
            <p className="panel__note">
              {l(
                "利用枠は各メンバーのデスクトップアプリで確認してください。",
                "Check subscription limits in each member’s desktop app.",
              )}
            </p>
          )}
        </>
      ) : (
        <>
          {mode === "unknown" && (
            <p className="cost-notice">
              {l(
                "表示額はAPI料金換算です。実際の支出や削減額ではありません。API契約を確認すると検証計画を作れます。",
                "These are API-equivalent estimates, not actual spend or cash savings. Confirm API billing to create an evaluation plan.",
              )}
            </p>
          )}
          {stale && (
            <p className="cost-notice" role="alert">
              {l(
                "料金データが30日以上前、または取得日が無効です。更新まで変更提案を停止しています。",
                "Prices are older than 30 days or have an invalid retrieval date. Recommendations are paused until updated.",
              )}
            </p>
          )}
          <QueryBoundary
            state={data}
            onRetry={data.reload}
            isEmpty={(d) => d.models.rows.length === 0}
            emptyLabel={l(
              "この期間のモデル利用記録がありません。エージェントを利用してから再読み込みしてください。",
              "No model usage in this period. Use your agent, then reload.",
            )}
          >
            {(d) => {
              const result = opportunities(d.models.rows, catalog.models);
              const before = result.eligible.reduce(
                (s, x) => ({
                  min: s.min + x.cost.min,
                  max: s.max + x.cost.max,
                }),
                { min: 0, max: 0 },
              );
              const total = result.recommendations.reduce(
                (s, x) => ({
                  min: s.min + x.saving.min,
                  max: s.max + x.saving.max,
                }),
                { min: 0, max: 0 },
              );
              return (
                <>
                  <section className="cost-summary">
                    <div>
                      <span>
                        {l(
                          "計算可能な範囲のAPI換算額",
                          "Calculated subset · API equivalent",
                        )}
                      </span>
                      <strong>
                        {result.eligible.length ? range(before) : "—"}
                      </strong>
                      <small>
                        {l(
                          `過去${days}日・計算可能な記録のみ`,
                          `Last ${days} days · calculable records only`,
                        )}
                      </small>
                    </div>
                    <div className="cost-summary-saving">
                      <span>
                        {l(
                          "モデル変更による差額候補",
                          "Potential model-switch difference",
                        )}
                      </span>
                      <strong>
                        {stale || !result.recommendations.length
                          ? "—"
                          : range(total)}
                      </strong>
                      <small>
                        {l(
                          "同じ利用量を仮定・品質は未検証",
                          "Same usage assumed · quality unverified",
                        )}
                      </small>
                    </div>
                  </section>
                  <div className="cost-section-heading">
                    <h2>{l("まず検証する変更", "Changes worth testing")}</h2>
                    <span>
                      {l(
                        "削減幅の下限が大きい順",
                        "Largest lower-bound reduction first",
                      )}
                    </span>
                  </div>
                  {!stale &&
                    result.recommendations.map((r, i) => (
                      <article
                        className="cost-opportunity"
                        key={`${r.row[0]}:${r.row[1]}`}
                      >
                        <div className="cost-rank">
                          {String(i + 1).padStart(2, "0")}
                        </div>
                        <div className="cost-opportunity-body">
                          <span className="cost-eyebrow">
                            {r.row[1] || l("思考レベル不明", "Unknown effort")}{" "}
                            · {r.row[2].toLocaleString()}{" "}
                            {l("リクエスト", "requests")}
                          </span>
                          <h3>
                            {r.current.name}{" "}
                            <span aria-label={l("から", "to")}>→</span>{" "}
                            {r.target.name}
                          </h3>
                          <p>
                            {l(
                              "同一プロバイダーの候補で、記録された利用量を換算した差額が最大です。業務への適合性は未検証です。",
                              "Largest estimated difference among supported models from the same provider. Task suitability is unverified.",
                            )}
                          </p>
                          <p className="cost-before-after">
                            {range(r.before)} → {range(r.after)}
                          </p>
                          <button
                            className="btn"
                            disabled={mode !== "api"}
                            onClick={() => {
                              setPlan(r);
                              setChecked([false, false, false]);
                            }}
                          >
                            {l("検証計画を作る", "Create evaluation plan")}
                          </button>
                        </div>
                        <div className="cost-opportunity-saving">
                          <strong>{range(r.saving)}</strong>
                          <span>
                            {l(
                              `${days}日分の差額候補`,
                              `Potential difference over ${days} days`,
                            )}
                          </span>
                          <small>
                            {l(
                              "品質・互換性の検証が必要",
                              "Quality and compatibility need testing",
                            )}
                          </small>
                        </div>
                      </article>
                    ))}
                  {!stale && !result.recommendations.length && (
                    <p className="panel">
                      {l(
                        "現在の料金・記録では、条件差を考慮しても安くなる候補を特定できません。",
                        "No lower-cost candidate can be established across the supported pricing conditions.",
                      )}
                    </p>
                  )}
                  {result.excluded.length > 0 && (
                    <details className="panel">
                      <summary>
                        {l(
                          `計算できない記録：${result.excluded.length}件`,
                          `${result.excluded.length} records excluded from estimates`,
                        )}
                      </summary>
                      <p>
                        {l(
                          "未対応モデル、トークン内訳の欠落、または不整合があります。不明値をゼロとして計算しません。",
                          "Unsupported model, missing token breakdown or inconsistent usage. Unknown values are never treated as zero.",
                        )}
                      </p>
                      <ul>
                        {result.excluded.map((r, i) => (
                          <li key={i}>
                            {r[0]} · {r[1] || "—"}
                          </li>
                        ))}
                      </ul>
                      <Link to="/models">
                        {l("利用記録を確認", "Inspect model usage")}
                      </Link>
                    </details>
                  )}
                </>
              );
            }}
          </QueryBoundary>
        </>
      )}
      {plan && mode === "api" && (
        <section
          ref={planRef}
          tabIndex={-1}
          className="panel cost-plan"
          aria-label={l("検証計画", "Evaluation plan")}
        >
          <h2>
            {l(
              "小さく試して、変更を判断する",
              "Test a small sample before switching",
            )}
          </h2>
          <p>
            {plan.current.name} → {plan.target.name}
          </p>
          <p>
            {l(
              "会話本文を収集していないため、自動再実行は行いません。以下の計画を手元の環境で実施してください。チェックはこの画面を開いている間のみ保持されます。",
              "Conversation content is not collected, so replay is not available. Run this plan in your environment. Checks last only while this page stays open.",
            )}
          </p>
          {steps.map((s, i) => (
            <label key={s}>
              <input
                type="checkbox"
                checked={checked[i]}
                onChange={(e) =>
                  setChecked((old) =>
                    old.map((v, j) => (j === i ? e.target.checked : v)),
                  )
                }
              />
              {s}
            </label>
          ))}
          <button className="btn" onClick={downloadPlan}>
            {l("検証計画をダウンロード", "Download evaluation plan")}
          </button>
          <button className="btn" onClick={() => setPlan(null)}>
            {l("閉じる", "Close")}
          </button>
        </section>
      )}
      <details className="panel cost-method">
        <summary>
          {l("料金の出典と計算条件", "Pricing sources and assumptions")}
        </summary>
        <p>
          {l("公式料金取得日", "Official prices retrieved")}:{" "}
          {new Date(catalog.fetchedAt).toLocaleString(locale)} · USD / 1M tokens
        </p>
        <p>
          <a href={catalog.sources.anthropic} target="_blank" rel="noreferrer">
            Anthropic
          </a>{" "}
          ·{" "}
          <a href={catalog.sources.openai} target="_blank" rel="noreferrer">
            OpenAI
          </a>
        </p>
        <p>
          {l(
            "標準・グローバルAPIのテキストトークン料金。長文入力とキャッシュ保存期間の差は幅で表示。税、為替、割引、Batch、Priority、Fast、地域指定、ツール料金は含みません。請求実績ではありません。",
            "Standard global API text-token rates. Ranges cover context tiers and cache-write durations. Excludes tax, FX, discounts, Batch, Priority, Fast, regional and tool charges. Not an invoice.",
          )}
        </p>
        <p>
          {l(
            "同じトークン数・キャッシュ利用を仮定します。モデル変更でトークン数や再試行が変わる可能性があり、実際の削減を保証しません。ログ内の取得済み数量のみが対象です。",
            "Assumes identical token counts and caching. Tokenization and retries can change after switching; savings are not guaranteed. Covers only captured quantities.",
          )}
        </p>
        <p>
          {l(
            "料金はアプリ同梱のスナップショットです。開発者向け更新手順：",
            "Prices are a bundled snapshot. Maintainer refresh command:",
          )}{" "}
          <code>npm run pricing:refresh --workspace web</code>
        </p>
      </details>
    </div>
  );
}
