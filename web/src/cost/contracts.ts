import { useSyncExternalStore } from "react";

import { request } from "../api/client";
import { useAsync } from "../hooks/useAsync";
import { planName } from "./subscriptions";

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

export function useContracts(local = true) {
  const detected = useAsync(
    () =>
      local ? request<Contract[]>("/web/subscriptions") : Promise.resolve([]),
    [local],
    60000,
  );
  const raw = useSyncExternalStore(subscribe, stored);
  let saved: Contract[] = [];
  try {
    saved = parse(raw);
  } catch {
    /* Invalid local settings */
  }
  const observed = detected.status === "ok" ? detected.data : [];
  const rows = [
    ...observed.filter(
      (row) =>
        !saved.some(
          (s) => s.service === row.service && s.account === row.account,
        ),
    ),
    ...saved,
  ].map((row) => ({ ...row, plan: planName(row.service, row.plan) }));
  return { rows, saved, observed, status: detected.status };
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
