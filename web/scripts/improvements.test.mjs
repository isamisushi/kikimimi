import test from "node:test";
import assert from "node:assert/strict";
import { compareChange } from "../src/improvements/compare.ts";
const row = (dt, sessions, hit) => [
  dt,
  sessions,
  hit,
  sessions ? (100 * hit) / sessions : null,
  hit,
  null,
];
const before = [row("2026-11-01", 5, 3), row("2026-11-02", 5, 2)];
const after = [row("2026-11-04", 5, 0), row("2026-11-05", 5, 1)];
const compare = (rows, ready = true) =>
  compareChange(rows, "2026-11-03", "2026-11-06", ready);
test("a recorded change without later activity is waiting, never resolved", () => {
  const result = compare(before);
  assert.equal(result.status, "waiting");
  assert.equal(result.after.rate, null);
});
test("exclude the transition day, today and unequal historical windows", () => {
  const result = compare([
    ...before,
    ...after,
    row("2026-11-03", 1000, 1000),
    row("2026-11-06", 1000, 1000),
    row("2026-10-30", 1000, 1000),
  ]);
  assert.equal(result.windowDays, 2);
  assert.equal(result.before.sessions, 10);
  assert.equal(result.after.sessions, 10);
  assert.equal(result.status, "lower");
  assert.equal(result.after.hitSessions, 1);
});
test("no observations, small samples and one active day do not establish a trend", () => {
  assert.equal(
    compare([...before, row("2026-11-04", 10, 0)]).status,
    "insufficient",
  );
  assert.equal(
    compare([...before, row("2026-11-04", 1, 0), row("2026-11-05", 1, 0)])
      .status,
    "insufficient",
  );
  assert.equal(compare(after).status, "insufficient");
});
test("unknown collection blocks positive interpretation even with a lower rate", () => {
  assert.equal(
    compare([...before, ...after], false).status,
    "collection_unverified",
  );
});
test("show recurrence, and distinguish observed zero from no activity", () => {
  assert.equal(
    compare([...before, row("2026-11-04", 5, 3), row("2026-11-05", 5, 3)])
      .status,
    "recurring",
  );
  assert.equal(
    compare([...before, row("2026-11-04", 5, 0), row("2026-11-05", 5, 0)])
      .status,
    "no_recurrence",
  );
});
test("same-day and future records cannot include a pre-change observation as an outcome", () => {
  assert.equal(
    compareChange(before, "2026-11-06", "2026-11-06", true).status,
    "waiting",
  );
  assert.equal(
    compareChange(before, "2026-11-07", "2026-11-06", true).after.sessions,
    0,
  );
});
