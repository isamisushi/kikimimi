import type { Contract } from "./contracts";
import type { ModelRow } from "../api/types";
import { matchModel, summarizeCosts, type PriceHistory, type PriceModel } from "./estimate.ts";

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

export function listPrice(service: Contract["service"], plan: string) {
  const name = plan
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

export function subscriptionComparison(
  contracts: Contract[],
  rows: ModelRow[],
  catalog: PriceModel[],
  days: number,
  history?: PriceHistory,
) {
  const prices = contracts.map(monthlyPrice);
  const periodCost =
    prices.length && prices.every((p): p is number => p !== null)
      ? (prices.reduce((total, p) => total + p, 0) * days) / 30
      : null;
  const providers = new Set<string>(
    contracts.map((c) => (c.service === "Claude" ? "anthropic" : "openai")),
  );
  const eligible = rows.filter((row) => {
    const model = matchModel(row[0], catalog) ?? history?.versions.map(v => matchModel(row[0], v.models)).find(Boolean);
    return model && providers.has(model.provider);
  });
  const usage = summarizeCosts(eligible, catalog, history);
  return { periodCost, usage };
}
