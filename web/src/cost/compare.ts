import type { Range } from "./estimate";

export interface ContractInput {
  price: string;
  seats: string;
  period: "monthly" | "annual";
  extras: string;
  transition: string;
}
export interface ComparisonInput {
  current: ContractInput;
  proposed: ContractInput;
  currency: "USD" | "JPY";
  yenPerDollar: string;
  months: string;
  usagePercent: string;
  apiExtras: string;
  apiTransition: string;
  scopeConfirmed: boolean;
}
export interface UsageBasis {
  equivalent: Range | null;
  totalRequests: number;
  pricedRequests: number;
  days: number;
  stale: boolean;
}
export const initialComparison = (): ComparisonInput => ({
  current: {
    price: "",
    seats: "1",
    period: "monthly",
    extras: "0",
    transition: "0",
  },
  proposed: {
    price: "",
    seats: "1",
    period: "monthly",
    extras: "0",
    transition: "0",
  },
  currency: "USD",
  yenPerDollar: "",
  months: "12",
  usagePercent: "100",
  apiExtras: "0",
  apiTransition: "0",
  scopeConfirmed: false,
});

export function amount(value: string): number | null {
  if (!/^\d+(\.\d+)?$/.test(value.trim())) return null;
  const n = Number(value);
  return Number.isFinite(n) && n >= 0 && n <= 1e12 ? n : null;
}
function positive(value: string, integer = false, max = 1e12) {
  const n = amount(value);
  return n !== null && n > 0 && n <= max && (!integer || Number.isInteger(n))
    ? n
    : null;
}
export function monthlyContract(input: ContractInput): number | null {
  const price = amount(input.price),
    seats = positive(input.seats, true),
    extras = amount(input.extras);
  if (price === null || seats === null || extras === null) return null;
  const total = (price * seats) / (input.period === "annual" ? 12 : 1) + extras;
  return Number.isFinite(total) ? total : null;
}
function totalCost(
  monthly: Range | null,
  transition: number | null,
  months: number | null,
): Range | null {
  if (!monthly || transition === null || months === null) return null;
  return {
    min: monthly.min * months + transition,
    max: monthly.max * months + transition,
  };
}
export type Choice = "current" | "proposed" | "api";

/** A range midpoint is never evidence that an option is cheaper. */
export function cheapest(totals: Record<Choice, Range>): Choice[] {
  const ids: Choice[] = ["current", "proposed", "api"];
  const epsilon = 1e-7;
  return ids.filter((id) =>
    ids.every((other) => {
      if (id === other) return true;
      const a = totals[id],
        b = totals[other];
      const exactTie =
        Math.abs(a.max - a.min) < epsilon &&
        Math.abs(b.max - b.min) < epsilon &&
        Math.abs(a.min - b.min) < epsilon;
      return a.max < b.min - epsilon || exactTie;
    }),
  );
}

export function compareOptions(input: ComparisonInput, basis: UsageBasis) {
  const months = positive(input.months, true, 120);
  const fx = input.currency === "USD" ? 1 : positive(input.yenPerDollar);
  const percent = positive(input.usagePercent, false, 10000);
  const overhead = amount(input.apiExtras);
  const variable =
    basis.equivalent && fx !== null && percent !== null && basis.days > 0
      ? {
          min:
            (((basis.equivalent.min * 30) / basis.days) * fx * percent) / 100,
          max:
            (((basis.equivalent.max * 30) / basis.days) * fx * percent) / 100,
        }
      : null;
  const api =
    variable && overhead !== null
      ? { min: variable.min + overhead, max: variable.max + overhead }
      : null;
  const current = monthlyContract(input.current),
    proposed = monthlyContract(input.proposed);
  const monthly = {
    current: current === null ? null : { min: current, max: current },
    proposed: proposed === null ? null : { min: proposed, max: proposed },
    api,
  };
  const totals = {
    current: totalCost(
      monthly.current,
      amount(input.current.transition),
      months,
    ),
    proposed: totalCost(
      monthly.proposed,
      amount(input.proposed.transition),
      months,
    ),
    api: totalCost(api, amount(input.apiTransition), months),
  };
  const complete =
    basis.totalRequests > 0 && basis.pricedRequests === basis.totalRequests;
  const comparable = !!(
    totals.current &&
    totals.proposed &&
    totals.api &&
    basis.pricedRequests > 0 &&
    !basis.stale &&
    input.scopeConfirmed
  );
  // Missing requests have nonnegative cost but no known upper bound. A known
  // subscription can beat this lower bound; the incomplete API can never win.
  const winners = comparable
    ? cheapest({
        ...(totals as Record<Choice, Range>),
        api: {
          min: totals.api!.min,
          max: complete ? totals.api!.max : Infinity,
        },
      })
    : [];
  const ready = comparable && (complete || winners.length > 0);
  // Usage multiplier where recurring API charges equal a subscription. One-time costs are excluded.
  function breakEven(subscription: number | null): Range | null {
    if (
      !complete ||
      basis.stale ||
      !input.scopeConfirmed ||
      subscription === null ||
      overhead === null ||
      !variable ||
      variable.min <= 0
    )
      return null;
    const budget = Math.max(0, subscription - overhead);
    return { min: budget / variable.max, max: budget / variable.min };
  }
  const savings =
    ready && totals.current && totals.api && totals.proposed
      ? {
          proposed: {
            min: totals.current.min - totals.proposed.max,
            max: totals.current.max - totals.proposed.min,
          },
          api: complete
            ? {
                min: totals.current.min - totals.api.max,
                max: totals.current.max - totals.api.min,
              }
            : null,
        }
      : null;
  return {
    months,
    fx,
    percent,
    monthly,
    totals,
    complete,
    ready,
    winners,
    savings,
    breakEven: { current: breakEven(current), proposed: breakEven(proposed) },
  };
}
