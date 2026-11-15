import { useState } from "react";
import { request } from "../api/client";
import { useAsync } from "../hooks/useAsync";
import { useLocale } from "../i18n";
import { routerApiMicros, type RouterPriceDay } from "../cost/estimate";
import { useCatalog } from "../cost/catalog";
import { QueryBoundary } from "./QueryBoundary";

interface Totals {
  month: string;
  requests: number;
  spend_micros: number;
  comparison: {
    requests: number;
    input: number;
    output: number;
    reads: number;
    writes: number;
    actual_micros: number;
    days?: RouterPriceDay[];
  };
}

const availability = {
  cloud_update_required: ["接続先のCloudは個人のRouter利用額の取得にまだ対応していません。Cloudの更新後に利用額と節約額を表示します。", "Your Cloud server needs an update to show personal Router spend and savings."],
  reconnect: ["チームの認証が必要です。設定でRouterのチームに再接続してください。", "Reconnect your Router team in Settings to view personal spend."],
  access_denied: ["このアカウントにはRouterの個人利用額を取得する権限がありません。", "This account does not have access to personal Router spend."],
};
type Spend = Totals | { status: keyof typeof availability } | null;

export function PersonalRouterSpend() {
  const catalog = useCatalog();
  const ja = useLocale() === "ja";
  const state = useAsync(() => request<Spend>("/web/router-spend"), []);
  const models = catalog.models.filter((m) =>
    ["claude-sonnet-5-5", "claude-opus-5-5", "gpt-5.4-mini"].includes(m.id),
  );
  const [baseline, setBaseline] = useState("claude-sonnet-5-5");
  const usd = (micros: number) =>
    new Intl.NumberFormat(ja ? "ja" : "en", {
      style: "currency",
      currency: "USD",
      maximumFractionDigits: 6,
    }).format(micros / 1e6);
  return (
    <section className="overview-router-note">
      <h3>
        {ja
          ? "自分の Router 利用額と節約額（月初来）"
          : "My Router spend and savings this month"}
      </h3>
      <QueryBoundary state={state} onRetry={state.reload}>
        {(data) => {
          if (data && "status" in data) return <p>{availability[data.status][ja ? 0 : 1]}</p>;
          if (!data?.comparison)
            return (
              <p>
                {ja
                  ? "チームの Router 利用台帳は未連携です。"
                  : "Team Router ledger not connected."}
              </p>
            );
          const c = data.comparison;
          const estimate = routerApiMicros(catalog.history, baseline, c.days);
          const delta = estimate === null ? null : estimate - c.actual_micros;
          return (
            <>
              <p>
                {ja ? "利用額" : "Provider spend"}{" "}
                <strong>{usd(data.spend_micros)}</strong> · {data.requests}{" "}
                {ja ? "件" : "requests"} ({data.month.slice(0, 7)} JST)
              </p>
              <label>
                {ja ? "直接APIの比較モデル " : "Direct API baseline "}
                <select
                  value={baseline}
                  onChange={(e) => setBaseline(e.target.value)}
                >
                  {models.map((m) => (
                    <option key={m.id} value={m.id}>
                      {m.name}
                    </option>
                  ))}
                </select>
              </label>
              <p>
                {ja ? "推定節約額" : "Estimated savings"}{" "}
                <strong>{delta === null ? "—" : usd(delta)}</strong> ·{" "}
                {ja
                  ? `比較対象 ${c.requests}件の利用額 ${usd(c.actual_micros)}`
                  : `Provider spend for ${c.requests} comparable requests: ${usd(c.actual_micros)}`}
              </p>
              <small>
                {ja
                  ? "同じトークン数を標準単価・短時間キャッシュで比較。kikimimi 手数料は別途。マイナスは Router の方が割高です。"
                  : "Standard direct API rates with short cache writes, excluding the kikimimi fee. Negative savings mean Router cost more."}
              </small>
            </>
          );
        }}
      </QueryBoundary>
    </section>
  );
}
