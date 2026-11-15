import assert from "node:assert/strict";
import { test } from "node:test";
import { readFile } from "node:fs/promises";
import {
  summarizeCosts,
  completeUsage,
  estimate,
  matchModel,
  isStale,
} from "../src/cost/estimate.ts";
import { parsePrices } from "./refresh-pricing.mjs";

import { listPrice, monthlyPrice, subscriptionComparison } from "../src/cost/subscriptions.ts";

const history = JSON.parse(
  await readFile(new URL("../src/cost/price-history.json", import.meta.url), "utf8"),
);
const catalog = history.versions.at(-1);
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
test("Unsupported models and aliases do not inherit a guessed price", () => {
  assert.equal(matchModel("claude-example-1-20260101", [claude]), claude);
  assert.equal(matchModel("claude-example-1-fast", [claude]), undefined);
  assert.equal(matchModel("bedrock/claude-example-1", [claude]), undefined);
});
test("Stale and malformed prices are flagged", () => {
  const now = Date.parse("2026-11-04T00:00:00Z");
  assert.equal(isStale("2026-11-03T00:00:00Z", now), false);
  assert.equal(isStale("2026-10-01T00:00:00Z", now), true);
  assert.equal(isStale("garbage", now), true);
  assert.throws(() =>
    parsePrices("unrecognized document", "unrecognized document"),
  );
});
test("Downloaded official catalog contains valid complete rate variants", () => {
  assert.ok(catalog.models.length > 0);
  assert.equal(new Set(catalog.models.map((m) => m.id)).size, catalog.models.length);
  for (const m of catalog.models)
    for (const v of m.variants) {
      for (const value of [v.input, v.output, v.read ?? 0])
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

test("subscription prices resolve per service with explicit contract amounts taking precedence", () => {
  assert.equal(listPrice("ChatGPT", " ChatGPT plus "), 20);
  assert.equal(listPrice("Claude", "Pro"), 20);
  assert.equal(listPrice("ChatGPT", "Pro"), null);
  assert.equal(listPrice("Claude", "Max"), null);
  assert.equal(listPrice("Claude", "Max 20x"), 200);
  assert.equal(monthlyPrice({service:"ChatGPT", plan:"Plus", monthlyUsd:0}), 0);
});

test("prorate all registered fees once and price eligible model usage once", () => {
  const standard = {service:"Claude", plan:"Pro", monthlyUsd:null};
  const discounted = {...standard, monthlyUsd:10};
  const other = {...claude, id:"other", provider:"openai"};
  const comparison = subscriptionComparison([standard, discounted], [row(), row("other")], [claude, other], 14);
  assert.equal(comparison.periodCost, 14);
  assert.deepEqual(comparison.usage.equivalent, {min:22.05, max:24.3});
  assert.equal(comparison.usage.totalRequests, 10);
  assert.equal(subscriptionComparison([standard,{...standard,plan:"custom"}], [], [], 14).periodCost, null);
  assert.equal(subscriptionComparison([], [], [], 14).periodCost, null);
});

test("historical rates split a row across revisions and leave unknown days unpriced", () => {
  const rates = input => ({...claude, variants: [{input, output: 0, read: 0, write: 0}]});
  const history = {fetchedAt: "2026-08-20T00:00:00Z", versions: [
    {effectiveFrom: "2026-08-10", models: [rates(2)]},
    {effectiveFrom: "2026-08-15", models: [rates(5)]},
  ]};
  const r = row();
  r[19] = JSON.stringify(["2026-08-09", "2026-08-14", "2026-08-15"].map(
    day => [day, 1, 1000000, 0, 0, 0]));
  const summary = summarizeCosts([r], [], history);
  assert.equal(summary.pricedRequests, 2);
  assert.deepEqual(summary.equivalent, {min: 7, max: 7});
  assert.equal(summarizeCosts([row()], [rates(5)], history).pricedRequests, 0);
});

test("refresh adds new models and preserves earlier effective versions", async () => {
  const { recordPrices } = await import("./refresh-pricing.mjs");
  const first = { fetchedAt: "2026-01-01T01:00:00Z",
    versions: [{effectiveFrom: "2026-01-01", models: [claude]}] };
  const models = [{...claude, id: "new-model"}, claude];
  const next = recordPrices(first, models, "2026-01-04T01:00:00Z");
  assert.deepEqual(next.versions[0], first.versions[0]);
  assert.equal(next.versions[1].effectiveFrom, "2026-01-04");
  assert.equal(first.versions.length, 1);
  assert.deepEqual(recordPrices(next, models, "2026-01-05T01:00:00Z").versions, next.versions);
  assert.throws(() => recordPrices(next, [claude], "2026-01-04T02:00:00Z"));
});
