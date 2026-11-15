import { useState } from "react";
import { NativeSelect } from "@kikimimi/ui";
import "../styles/cost.css";
import {
  changePeriodDate,
  changeContract,
  useContracts,
  type Contract,
} from "../cost/contracts";
import {
  localDay,
  planName,
  planPrices,
  listPrice,
  monthlyPrice,
} from "../cost/subscriptions";
import { useLocale } from "../i18n";

function adjacentDay(day: string, offset: number) {
  return new Date(Date.parse(day) + offset * 86400000)
    .toISOString()
    .slice(0, 10);
}

function PlanHistory({ row, ja }: { row: Contract; ja: boolean }) {
  const [message, setMessage] = useState("");
  const periods = row.periods ?? [];
  if (periods.length < 2) return null;
  return (
    <details className="plan-history">
      <summary>{ja ? "プラン変更履歴" : "Plan history"}</summary>
      {periods.map((period, index) => (
        <div key={period.from}>
          <span>
            {period.plan} · {period.price === null ? "—" : `$${period.price}`}
          </span>
          {index === 0 ? (
            <small>
              {ja ? "初期プラン" : "Initial plan"} {period.from}
            </small>
          ) : (
            <form
              onSubmit={(event) => {
                event.preventDefault();
                const date = String(
                  new FormData(event.currentTarget).get("effective"),
                );
                try {
                  changePeriodDate(row, index, date);
                  setMessage(ja ? "変更日を保存しました" : "Change date saved");
                } catch {
                  setMessage(
                    ja
                      ? "変更日を保存できませんでした。履歴と日付を確認してください。"
                      : "Could not save. Check the history and date.",
                  );
                }
              }}
            >
              <label className="field">
                {ja ? "プラン変更日" : "Effective date"}
                <input
                  type="date"
                  name="effective"
                  required
                  defaultValue={period.from}
                  min={
                    periods[index - 1].from
                      ? adjacentDay(periods[index - 1].from, 1)
                      : undefined
                  }
                  max={
                    periods[index + 1]
                      ? adjacentDay(periods[index + 1].from, -1)
                      : localDay()
                  }
                />
              </label>
              <button className="btn btn--ghost">
                {ja ? "日付を保存" : "Save date"}
              </button>
            </form>
          )}
        </div>
      ))}
      <span role="status">{message}</span>
    </details>
  );
}

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
                {row.account ||
                  (ja ? "このMacの Claude" : "Claude on this Mac")}
              </small>
            </div>
            <PlanHistory row={row} ja={ja} />
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
            {saved.some((s) => matches(s) && !s.detectedOnly) && (
              <button
                className="btn btn--ghost"
                onClick={() => {
                  const login = observed.find(matches);
                  if (
                    persist(
                      row.service,
                      row.account,
                      login
                        ? {
                            ...login,
                            plan: planName(login.service, login.plan),
                            detectedOnly: true,
                          }
                        : null,
                    )
                  )
                    setEditing(null);
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
            if ((editing === "new" && !account.trim()) || !plan.trim()) return;
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
              disabled={(editing === "new" && !account.trim()) || !plan.trim()}
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
