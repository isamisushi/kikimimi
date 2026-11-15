import { useState } from "react";
import { getSubscriptionUsage } from "../api/client";
import { changeContract, useContracts, type Contract } from "../cost/contracts";
import { planPrices, listPrice, monthlyPrice } from "../cost/subscriptions";
import { useAsync } from "../hooks/useAsync";
import { useLocale } from "../i18n";

export function SubscriptionSettings() {
  const ja = useLocale() === "ja";
  const saved = useContracts();
  const observed = useAsync(getSubscriptionUsage, []);
  const [service, setService] = useState<Contract["service"]>("ChatGPT");
  const [account, setAccount] = useState("");
  const [plan, setPlan] = useState("");
  const [monthly, setMonthly] = useState<string | null>(null);
  const standard = listPrice(service, plan);
  const [message, setMessage] = useState("");
  function persist(
    service: Contract["service"],
    account: string,
    row: Contract | null,
  ) {
    try {
      changeContract(service, account, row);
      return true;
    } catch {
      setMessage(
        ja
          ? "端末の設定を更新できませんでした"
          : "Could not update settings on this device",
      );
      return false;
    }
  }
  function select(row: Contract) {
    setService(row.service);
    setAccount(row.account);
    setPlan(row.plan);
    setMonthly(row.monthlyUsd?.toString() ?? null);
    setMessage("");
  }
  return (
    <section className="panel subscription-settings">
      <h2 className="panel__title">
        {ja ? "サブスクリプション" : "Subscriptions"}
      </h2>
      {saved.map((row) => (
        <div
          className="storage-actions"
          key={JSON.stringify([row.service, row.account])}
        >
          <button className="btn btn--ghost" onClick={() => select(row)}>
            {row.service} · {row.plan} · {row.account}
            {monthlyPrice(row) !== null &&
              ` · $${monthlyPrice(row)}/${ja ? "月" : "mo"}`}
          </button>
          <button
            className="btn btn--ghost"
            onClick={() => {
              if (!persist(row.service, row.account, null)) return;
              setMessage(ja ? "削除しました" : "Removed");
            }}
          >
            {ja ? "削除" : "Remove"}
          </button>
        </div>
      ))}
      {observed.status === "ok" &&
        observed.data.map((s) => {
          const name = s.agent === "codex" ? "ChatGPT" : "Claude";
          if (
            saved.some(
              (row) => row.service === name && row.account === s.account,
            )
          )
            return null;
          return (
            <button
              className="btn btn--ghost"
              key={JSON.stringify([name, s.account])}
              onClick={() =>
                select({
                  service: name,
                  account: s.account,
                  plan: s.plan ?? "",
                  monthlyUsd: null,
                })
              }
            >
              {ja ? "検出済み" : "Detected"} · {name} · {s.account} {s.plan} ·{" "}
              {new Date(s.observed_at).toLocaleDateString(ja ? "ja" : "en")}
            </button>
          );
        })}
      {observed.status === "error" && (
        <p>
          {ja
            ? "ログイン情報を取得できません。手動で登録できます。"
            : "Account detection unavailable. Enter your plan below."}
        </p>
      )}
      <form
        onSubmit={(e) => {
          e.preventDefault();
          if (!account.trim() || !plan.trim()) return;
          if (
            !persist(service, account.trim(), {
              service,
              account: account.trim(),
              plan: plan.trim(),
              monthlyUsd:
                monthly === null || monthly === "" ? null : Number(monthly),
            })
          )
            return;
          setAccount("");
          setPlan("");
          setMonthly(null);
          setMessage(ja ? "保存しました" : "Saved");
        }}
      >
        <label className="field">
          {ja ? "サービス" : "Service"}
          <select
            value={service}
            onChange={(e) => {
              setService(e.target.value as Contract["service"]);
              setPlan("");
              setMonthly(null);
            }}
          >
            <option>ChatGPT</option>
            <option>Claude</option>
          </select>
        </label>
        <label className="field">
          {ja ? "アカウント名" : "Account label"}
          <input
            required
            value={account}
            onChange={(e) => setAccount(e.target.value)}
          />
        </label>
        <label className="field">
          {ja ? "プラン名" : "Plan"}
          <input
            required
            list="subscription-plan-names"
            value={plan}
            onChange={(e) => {
              setPlan(e.target.value);
              setMonthly(null);
            }}
          />
        </label>
        <datalist id="subscription-plan-names">
          {Object.keys(planPrices[service]).map((name) => (
            <option key={name} value={name} />
          ))}
        </datalist>
        <label className="field">
          {ja ? "月額（USD）" : "Monthly price (USD)"}
          <input
            type="number"
            min="0"
            step="any"
            value={monthly ?? standard ?? ""}
            onChange={(e) => setMonthly(e.target.value)}
          />
          <small>
            {standard === null
              ? ja
                ? "プランの種類を選ぶか、契約月額を入力"
                : "Choose a tier or enter your monthly price"
              : ja
                ? `米国定価 $${standard}。実額が異なる場合は変更`
                : `US list price $${standard}. Edit if yours differs.`}
          </small>
        </label>
        <button
          className="btn btn--primary"
          disabled={!account.trim() || !plan.trim()}
        >
          {ja ? "この端末に保存" : "Save on this device"}
        </button>{" "}
        <span role="status">{message}</span>
      </form>
    </section>
  );
}
