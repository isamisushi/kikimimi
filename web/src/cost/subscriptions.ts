import type { Contract } from "./contracts";
import type { ModelRow } from "../api/types";
import {
  matchModel,
  summarizeCosts,
  type PriceHistory,
  type PriceModel,
} from "./estimate.ts";

// US monthly web prices checked 2026-11-15. Tier-less Pro and Max names are ambiguous.
// https://help.openai.com/en/articles/9793128-about-chatgpt-pro-tiers
// https://help.openai.com/en/articles/6950777-what-is-chatgpt-plus
// https://openai.com/index/introducing-chatgpt-go/
// https://support.claude.com/en/articles/11049762-choose-a-claude-plan
export const planPrices = {
  ChatGPT: {
    Free: 0,
    Go: 8,
    Plus: 20,
    "Pro 100": 100,
    "Pro 200": 200,
    "Pro 500": 500,
  },
  Claude: { Free: 0, Pro: 20, "Max 5x": 100, "Max 20x": 200 },
};

export function planName(service: Contract["service"], plan: string) {
  if (service === "ChatGPT" && plan.toLowerCase() === "prolite")
    return "Pro 100";
  return (
    Object.keys(planPrices[service]).find(
      (key) => key.toLowerCase() === plan.toLowerCase(),
    ) ?? plan
  );
}

export function listPrice(service: Contract["service"], plan: string) {
  const name = planName(service, plan)
    .trim()
    .toLowerCase()
    .replace(new RegExp("^" + service.toLowerCase() + "\\s+"), "");
  return (
    Object.entries(planPrices[service]).find(
      ([key]) => key.toLowerCase() === name,
    )?.[1] ?? null
  );
}

export function monthlyPrice(contract: Contract) {
  return contract.monthlyUsd ?? listPrice(contract.service, contract.plan);
}

export function localDay(date = new Date()) {
  return [
    date.getFullYear(),
    String(date.getMonth() + 1).padStart(2, "0"),
    String(date.getDate()).padStart(2, "0"),
  ].join("-");
}

export type PlanPeriod = { from: string; plan: string; price: number | null };

// The first known rate is the baseline estimate for dates before observation.
export function withPeriod(
  next: Contract,
  previous?: Contract,
  day = localDay(),
): Contract {
  const periods =
    previous?.periods ??
    (previous
      ? [
          {
            from: "",
            plan: previous.plan,
            price: monthlyPrice(previous),
          },
        ]
      : []);
  const plan = next.plan;
  const price = monthlyPrice(next);
  const last = periods[periods.length - 1];
  if (last && last.plan === plan && last.price === price)
    return { ...next, periods };
  const earlier = periods.filter((period) => period.from < day);
  return { ...next, periods: [...earlier, { from: day, plan, price }] };
}

// Only the explicit overview fallback (empty account) can bind to a new login.
// Separately registered contracts keep their own identity.
export function reconcileLogin(
  saved: Contract[],
  login: Contract,
  day = localDay(),
) {
  const previous = saved.find(
    (row) => row.service === login.service && row.account === login.account,
  );
  const fallback =
    (login.service === "Claude" &&
      saved.find((row) => row.service === "Claude" && row.account === "")) ||
    undefined;
  const base =
    fallback && (!previous || previous.detectedOnly)
      ? fallback
      : (previous ?? fallback);
  const known = listPrice(login.service, login.plan) !== null;
  const reported = planName(login.service, login.plan);
  const familyMatches =
    !reported ||
    base?.reportedPlan === reported ||
    base?.plan.toLowerCase().startsWith(reported.toLowerCase());
  const plan = !known && familyMatches && base ? base.plan : reported;
  const samePlan = base && planName(login.service, base.plan) === plan;
  const next = withPeriod(
    {
      ...login,
      reportedPlan: reported,
      plan,
      monthlyUsd: samePlan ? base.monthlyUsd : null,
      detectedOnly: base?.detectedOnly ?? !base,
    },
    base,
    day,
  );
  return [...saved.filter((row) => row !== previous && row !== fallback), next];
}

export function periodFee(row: Contract, days: number, end = localDay()) {
  const periods = row.periods;
  if (!periods?.length) {
    const price = monthlyPrice(row);
    return price === null ? null : (price * days) / 30;
  }
  const endDay = Date.parse(end) / 86400000;
  let start = endDay - days + 1;
  let total = 0;
  for (let i = 0; i < periods.length && start <= endDay; i++) {
    const next = periods[i + 1];
    const until = next
      ? Math.min(endDay + 1, Date.parse(next.from) / 86400000)
      : endDay + 1;
    if (until <= start) continue;
    const rate = periods[i].price;
    if (rate === null) return null;
    total += rate * (until - start);
    start = until;
  }
  return total / 30;
}

export function subscriptionComparison(
  contracts: Contract[],
  rows: ModelRow[],
  catalog: PriceModel[],
  days: number,
  history?: PriceHistory,
) {
  const prices = contracts.map((c) => periodFee(c, days));
  const periodCost =
    prices.length && prices.every((p): p is number => p !== null)
      ? prices.reduce((total, p) => total + p, 0)
      : null;
  const providers = new Set<string>(
    contracts.map((c) => (c.service === "Claude" ? "anthropic" : "openai")),
  );
  const eligible = rows.filter((row) => {
    const model =
      matchModel(row[0], catalog) ??
      history?.versions.map((v) => matchModel(row[0], v.models)).find(Boolean);
    return model && providers.has(model.provider);
  });
  const usage = summarizeCosts(eligible, catalog, history);
  return { periodCost, usage };
}
