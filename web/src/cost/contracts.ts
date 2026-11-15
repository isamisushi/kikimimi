import { useSyncExternalStore } from "react";

import { request } from "../api/client";
import { useAsync } from "../hooks/useAsync";
import {
  planName,
  localDay,
  reconcileLogin,
  withPeriod,
  type PlanPeriod,
} from "./subscriptions";

export interface Contract {
  service: "Claude" | "ChatGPT";
  account: string;
  plan: string;
  monthlyUsd: number | null;
  periods?: PlanPeriod[];
  detectedOnly?: boolean;
  reportedPlan?: string;
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

function write(rows: Contract[]) {
  const json = JSON.stringify(rows);
  if (stored() !== json) {
    localStorage.setItem(storageKey, json);
    window.dispatchEvent(new Event(changed));
  }
}

async function detect() {
  const observed = await request<Contract[]>("/web/subscriptions");
  write(
    observed.reduce(
      (saved, row) => reconcileLogin(saved, row),
      parse(stored()),
    ),
  );
  return observed;
}

export function useContracts(local = true) {
  const detected = useAsync(
    () => (local ? detect() : Promise.resolve([])),
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
    ...saved.filter(
      (row) =>
        !row.detectedOnly ||
        observed.some(
          (s) => s.service === row.service && s.account === row.account,
        ),
    ),
  ].map((row) => ({ ...row, plan: planName(row.service, row.plan) }));
  return { rows, saved, observed, status: detected.status };
}

export function changeContract(
  service: Contract["service"],
  account: string,
  next: Contract | null,
) {
  const saved = parse(stored());
  const previous = saved.find(
    (row) => row.service === service && row.account === account,
  );
  const others = saved.filter((row) => row !== previous);
  const updated = next
    ? withPeriod({ reportedPlan: previous?.reportedPlan, ...next }, previous)
    : null;
  write(updated ? [...others, updated] : others);
}

export function changePeriodDate(
  contract: Contract,
  index: number,
  date: string,
) {
  const saved = parse(stored());
  const row = saved.find(
    (r) => r.service === contract.service && r.account === contract.account,
  );
  const periods = row?.periods;
  if (
    !row ||
    !periods ||
    JSON.stringify(periods) !== JSON.stringify(contract.periods)
  )
    throw new Error("History changed");
  const timestamp = Date.parse(date);
  if (
    index < 1 ||
    index >= periods.length ||
    !/^\d{4}-\d{2}-\d{2}$/.test(date) ||
    !Number.isFinite(timestamp) ||
    new Date(timestamp).toISOString().slice(0, 10) !== date ||
    date > localDay() ||
    date <= periods[index - 1].from ||
    (periods[index + 1] && date >= periods[index + 1].from)
  )
    throw new Error("Invalid effective date");
  row.periods = periods.map((p, i) => (i === index ? { ...p, from: date } : p));
  write(saved);
}
