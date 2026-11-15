import { useSyncExternalStore } from "react";

export interface Contract {
  service: "Claude" | "ChatGPT";
  account: string;
  plan: string;
  monthlyUsd: number | null;
}

const storageKey = "kikimimi.subscription-contracts.v1";
const changed = "kikimimi-subscription-contracts-changed";

function subscribe(refresh: () => void) {
  window.addEventListener(changed, refresh);
  window.addEventListener("storage", refresh);
  return () => {
    window.removeEventListener(changed, refresh);
    window.removeEventListener("storage", refresh);
  };
}

function stored() {
  try {
    return localStorage.getItem(storageKey) || "[]";
  } catch {
    return "[]";
  }
}

function parse(raw: string): Contract[] {
  const records: unknown = JSON.parse(raw);
  if (
    !Array.isArray(records) ||
    !records.every(
      (row) =>
        row &&
        ["Claude", "ChatGPT"].includes(row.service) &&
        typeof row.account === "string" &&
        typeof row.plan === "string" &&
        (row.monthlyUsd === null ||
          (Number.isFinite(row.monthlyUsd) && row.monthlyUsd >= 0)),
    )
  )
    throw new Error("Invalid subscription settings");
  return records;
}

export function useContracts() {
  const raw = useSyncExternalStore(subscribe, stored);
  try {
    return parse(raw);
  } catch {
    return [];
  }
}

export function changeContract(
  service: Contract["service"],
  account: string,
  next: Contract | null,
) {
  const others = parse(stored()).filter(
    (row) => row.service !== service || row.account !== account,
  );
  localStorage.setItem(
    storageKey,
    JSON.stringify(next ? [...others, next] : others),
  );
  window.dispatchEvent(new Event(changed));
}
