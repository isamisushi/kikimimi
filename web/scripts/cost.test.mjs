import assert from "node:assert/strict";
import { test } from "node:test";
import { readFile } from "node:fs/promises";
import {
  summarizeCosts,
  completeUsage,
  estimate,
  opportunities,
  subscriptionSaving,
  matchModel,
  isStale,
} from "../src/cost/estimate.ts";
import { parsePrices } from "./refresh-pricing.mjs";

const catalog = JSON.parse(
  await readFile(new URL("../src/cost/pricing.json", import.meta.url), "utf8"),
);
const claude = {
  id: "claude-example-1",
  name: "Example",
  provider: "anthropic",
  variants: [
    { input: 3, output: 15, read: 0.3, write: 3.75 },
    { input: 3, output: 15, read: 0.3, write: 6 },
  ],
};
const row = (id = claude.id) => [
  id,
  "high",
  10,
  0,
  1,
  0,
  0,
  1e6,
  1e6,
  1e6,
  1e6,
  0,
  null,
  0,
  10,
  1e6,
  1e6,
  1e6,
  1e6,
];
test("Claude cache is additive; TTL uncertainty remains a range", () => {
  assert.deepEqual(estimate(row(), claude, claude), { min: 22.05, max: 24.3 });
});
test("OpenAI cached input is not double billed; invalid subsets are excluded", () => {
  const model = { ...claude, provider: "openai" };
  const r = row();
  r[7] = 3e6;
  assert.deepEqual(estimate(r, model, model), { min: 22.05, max: 24.3 });
  assert.equal(estimate(row(), model, model), null);
});
test("Unknown, negative and nonfinite tokens never become zero-cost usage", () => {
  for (const index of [7, 8, 9, 10])
    for (const bad of [null, -1, Infinity, NaN]) {
      const r = row();
      r[index] = bad;
      assert.equal(estimate(r, claude, claude), null);
    }
});
test("Reasoning tokens already included in output are not added twice", () => {
  const r = row();
  r[11] = 500000;
  assert.deepEqual(
    estimate(r, claude, claude),
    estimate(row(), claude, claude),
  );
});
test("Recommendations are exclusive and conservatively positive", () => {
  const cheaper = {
    ...claude,
    id: "cheaper",
    variants: [{ input: 1, output: 1, read: 0.1, write: 1 }],
  };
  const cheapest = {
    ...cheaper,
    id: "cheapest",
    variants: [{ input: 0.5, output: 0.5, read: 0.05, write: 0.5 }],
  };
  const result = opportunities(
    [row(), row("unknown")],
    [claude, cheaper, cheapest],
  );
  assert.equal(result.recommendations.length, 1);
  assert.equal(result.recommendations[0].target.id, "cheapest");
  assert.equal(result.excluded.length, 1);
  assert.ok(result.recommendations[0].saving.min > 0);
});
test("Unsupported models and aliases do not inherit a guessed price", () => {
  assert.equal(matchModel("claude-example-1-20260101", [claude]), claude);
  assert.equal(matchModel("claude-example-1-fast", [claude]), undefined);
  assert.equal(matchModel("bedrock/claude-example-1", [claude]), undefined);
});
test("Subscription comparison requires explicit nonnegative contract totals", () => {
  assert.equal(subscriptionSaving("", "20"), null);
  assert.equal(subscriptionSaving("-1", "20"), null);
  assert.equal(subscriptionSaving("100", "20"), 80);
  assert.equal(subscriptionSaving("20", "100"), -80);
  assert.equal(subscriptionSaving("20", "0"), 20);
});
test("Stale and malformed prices disable recommendations", () => {
  const now = Date.parse("2026-11-04T00:00:00Z");
  assert.equal(isStale("2026-11-03T00:00:00Z", now), false);
  assert.equal(isStale("2026-10-01T00:00:00Z", now), true);
  assert.equal(isStale("garbage", now), true);
  assert.throws(() =>
    parsePrices("unrecognized document", "unrecognized document"),
  );
});
test("Downloaded official catalog contains valid complete rate variants", () => {
  assert.equal(catalog.models.length, 15);
  assert.equal(new Set(catalog.models.map((m) => m.id)).size, 15);
  for (const m of catalog.models)
    for (const v of m.variants) {
      for (const value of [v.input, v.output, v.read])
        assert.ok(Number.isFinite(value) && value >= 0);
    }
});

test("Partial groups price only complete requests, not cross-row SUMs", () => {
  const r = row();
  r[12] = 5;
  r[13] = 2;
  r[14] = 3;
  r[15] = 100;
  r[16] = 20;
  r[17] = 0;
  r[18] = 0;
  const s = summarizeCosts([r], [claude]);
  assert.equal(s.totalRequests, 10);
  assert.equal(s.reportedRequests, 2);
  assert.equal(s.pricedRequests, 3);
  assert.equal(s.reportedCost, 5);
  assert.equal(s.details[0].reason, "partial");
  assert.equal(s.equivalent.min, 0.0006);
});
test("Legacy API without coverage never pretends complete sums are billable", () => {
  const r = row().slice(0, 13);
  assert.equal(completeUsage(r), null);
  const s = summarizeCosts([r], [claude]);
  assert.equal(s.reportedRequests, null);
  assert.equal(s.pricedRequests, 0);
  assert.equal(s.equivalent, null);
});
test("Unsupported models and missing costs remain explicit while real zero survives", () => {
  const r = row("unknown");
  r[12] = 0;
  r[13] = 10;
  const s = summarizeCosts([r], [claude]);
  assert.equal(s.reportedCost, 0);
  assert.equal(s.pricedRequests, 0);
  assert.equal(s.details[0].reason, "unsupported");
  const empty = summarizeCosts([], [claude]);
  assert.equal(empty.reportedCost, null);
  assert.equal(empty.equivalent, null);
});

const { initialComparison, monthlyContract, compareOptions, cheapest, amount } =
  await import("../src/cost/compare.ts");
const basis = {
  equivalent: { min: 20, max: 30 },
  totalRequests: 10,
  pricedRequests: 10,
  days: 30,
  stale: false,
};
function comparison() {
  const x = initialComparison();
  x.current.price = "100";
  x.proposed.price = "80";
  x.scopeConfirmed = true;
  return x;
}
test("annual per-seat plans include monthly team extras without multiplying extras by seats", () => {
  assert.equal(
    monthlyContract({
      price: "120",
      seats: "3",
      period: "annual",
      extras: "7",
      transition: "0",
    }),
    37,
  );
  assert.equal(
    monthlyContract({
      price: "0",
      seats: "1",
      period: "monthly",
      extras: "0",
      transition: "0",
    }),
    0,
  );
  for (const value of ["", "-1", "Infinity", "0x10", "NaN"])
    assert.equal(amount(value), null);
  const x = comparison();
  x.current.seats = "1.5";
  assert.equal(monthlyContract(x.current), null);
});
test("all three payment methods can be the cheapest with conservative range comparisons", () => {
  const x = comparison();
  assert.deepEqual(compareOptions(x, basis).winners, ["api"]);
  x.current.price = "10";
  assert.deepEqual(compareOptions(x, basis).winners, ["current"]);
  x.proposed.price = "5";
  assert.deepEqual(compareOptions(x, basis).winners, ["proposed"]);
});
test("migration fees reverse the short-term winner and are not repeated monthly", () => {
  const x = comparison();
  x.months = "1";
  x.apiTransition = "200";
  let r = compareOptions(x, basis);
  assert.deepEqual(r.winners, ["proposed"]);
  assert.deepEqual(r.totals.api, { min: 220, max: 230 });
  x.months = "12";
  r = compareOptions(x, basis);
  assert.deepEqual(r.winners, ["api"]);
  assert.deepEqual(r.totals.api, { min: 440, max: 560 });
  x.proposed.transition = "5000";
  x.apiTransition = "5000";
  assert.deepEqual(compareOptions(x, basis).winners, ["current"]);
});
test("API normalization applies period, usage and FX once, and never multiplies by seats", () => {
  const x = comparison();
  x.currency = "JPY";
  x.yenPerDollar = "150";
  x.usagePercent = "200";
  x.apiExtras = "500";
  x.current.seats = "10";
  const r = compareOptions(x, {
    ...basis,
    days: 7,
    equivalent: { min: 7, max: 14 },
  });
  assert.deepEqual(r.monthly.api, { min: 9500, max: 18500 });
  x.yenPerDollar = "";
  assert.equal(compareOptions(x, basis).monthly.api, null);
  x.yenPerDollar = "0";
  assert.equal(compareOptions(x, basis).monthly.api, null);
});
test("partial, empty, stale and unconfirmed usage never win a three-way comparison", () => {
  const x = comparison();
  for (const b of [
    { ...basis, pricedRequests: 9 },
    { ...basis, totalRequests: 0, pricedRequests: 0, equivalent: null },
    { ...basis, stale: true },
  ]) {
    const r = compareOptions(x, b);
    assert.equal(r.ready, false);
    assert.deepEqual(r.winners, []);
    assert.equal(r.breakEven.current, null);
  }
  x.scopeConfirmed = false;
  assert.equal(compareOptions(x, basis).ready, false);
  x.scopeConfirmed = true;
  x.proposed.price = "";
  assert.equal(compareOptions(x, basis).ready, false);
});
test("overlapping ranges abstain; exact ties preserve all equally cheap options", () => {
  const x = comparison();
  x.current.price = "25";
  assert.deepEqual(compareOptions(x, basis).winners, []);
  x.current.price = "10";
  x.proposed.price = "10";
  assert.deepEqual(compareOptions(x, basis).winners, ["current", "proposed"]);
  assert.deepEqual(
    cheapest({
      current: { min: 0, max: 0 },
      proposed: { min: 0, max: 0 },
      api: { min: 0, max: 0 },
    }),
    ["current", "proposed", "api"],
  );
});
test("break-even uses recurring extras and excludes one-time fees", () => {
  const x = comparison();
  x.apiExtras = "10";
  x.apiTransition = "900";
  assert.deepEqual(compareOptions(x, basis).breakEven.current, {
    min: 3,
    max: 4.5,
  });
  x.apiExtras = "110";
  assert.deepEqual(compareOptions(x, basis).breakEven.current, {
    min: 0,
    max: 0,
  });
  assert.equal(
    compareOptions(x, { ...basis, equivalent: { min: 0, max: 0 } }).breakEven
      .current,
    null,
  );
});
test("invalid horizon and unknown extras cannot become a free option", () => {
  for (const bad of ["0", "-1", "", "1.5", "121"]) {
    const x = comparison();
    x.months = bad;
    assert.equal(compareOptions(x, basis).ready, false);
  }
  const x = comparison();
  x.apiExtras = "";
  assert.equal(compareOptions(x, basis).ready, false);
  x.apiExtras = "0";
  x.usagePercent = "0";
  assert.equal(compareOptions(x, basis).ready, false);
});

test("a known subscription can beat incomplete API lower bounds, but API can never win", () => {
  const x = comparison();
  x.current.price = "10";
  const r = compareOptions(x, { ...basis, pricedRequests: 9 });
  assert.equal(r.ready, true);
  assert.deepEqual(r.winners, ["current"]);
  assert.equal(r.savings.api, null);
  assert.equal(r.breakEven.current, null);
  x.current.price = "30";
  x.proposed.price = "20";
  assert.deepEqual(
    compareOptions(x, { ...basis, pricedRequests: 9 }).winners,
    [],
  );
});
