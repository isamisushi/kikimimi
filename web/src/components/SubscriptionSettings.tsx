import { useState } from "react";
import { NativeSelect } from "@kikimimi/ui";
import "../styles/cost.css";
import { changeContract, useContracts, type Contract } from "../cost/contracts";
import { planPrices, listPrice, monthlyPrice } from "../cost/subscriptions";
import { useLocale } from "../i18n";

export function SubscriptionSettings() {
  const ja = useLocale() === "ja";
  const { rows, saved, observed, status } = useContracts();
  const [editing, setEditing] = useState<"new" | "existing" | null>(null);
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
      {rows.map((row) => {
        const matches = (other: Contract) =>
          other.service === row.service && other.account === row.account;
        const automatic = observed.some(matches);
        const price = monthlyPrice(row);
        return (
          <div
            className="subscription-row"
            key={JSON.stringify([row.service, row.account])}
          >
            <div className="subscription-identity">
              <strong>
                {row.service} ·{" "}
                {row.plan || (ja ? "プラン未取得" : "Unknown plan")}
              </strong>
              <small>
                {automatic && (ja ? "ログインを検出 · " : "Signed in · ")}
                {row.account}
              </small>
            </div>
            <span>
              {price === null
                ? ja
                  ? "月額未登録"
                  : "Price unknown"
                : `$${price} / ${ja ? "月" : "mo"}`}
            </span>
            <button
              className="btn btn--ghost"
              onClick={() => {
                select(row);
                setEditing("existing");
              }}
            >
              {ja ? "編集" : "Edit"}
            </button>
            {saved.some(matches) && (
              <button
                className="btn btn--ghost"
                onClick={() => {
                  if (persist(row.service, row.account, null)) setEditing(null);
                }}
              >
                {automatic
                  ? ja
                    ? "自動に戻す"
                    : "Use detected plan"
                  : ja
                    ? "削除"
                    : "Remove"}
              </button>
            )}
          </div>
        );
      })}
      <p className="panel__note">
        {status === "error"
          ? ja
            ? "ログイン情報を取得できませんでした。"
            : "Could not read local logins."
          : ja
            ? "このMacの Codex・Claude Code のログインからプランを検出し、米国定価で概算します。"
            : "Plans from Codex and Claude Code logins on this Mac use estimated US list prices."}
      </p>
      {!editing && (
        <button
          className="btn btn--ghost"
          onClick={() => {
            select({
              service: "ChatGPT",
              account: "",
              plan: "",
              monthlyUsd: null,
            });
            setEditing("new");
          }}
        >
          {ja ? "契約を手動で追加" : "Add a plan manually"}
        </button>
      )}
      {editing && (
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
            setEditing(null);
            setMessage(ja ? "保存しました" : "Saved");
          }}
        >
          <label className="field">
            {ja ? "サービス" : "Service"}
            <NativeSelect
              disabled={editing === "existing"}
              value={service}
              onChange={(e) => {
                setService(e.target.value as Contract["service"]);
                setPlan("");
                setMonthly(null);
              }}
            >
              <option>ChatGPT</option>
              <option>Claude</option>
            </NativeSelect>
          </label>
          <label className="field">
            {ja ? "アカウント名" : "Account label"}
            <input
              required
              disabled={editing === "existing"}
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
          <div className="subscription-controls">
            <button
              className="btn btn--primary"
              disabled={!account.trim() || !plan.trim()}
            >
              {ja ? "この端末に保存" : "Save on this device"}
            </button>
            <button
              type="button"
              className="btn btn--ghost"
              onClick={() => setEditing(null)}
            >
              {ja ? "キャンセル" : "Cancel"}
            </button>
          </div>
        </form>
      )}
      <span role="status">{message}</span>
    </section>
  );
}
