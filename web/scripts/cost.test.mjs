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
