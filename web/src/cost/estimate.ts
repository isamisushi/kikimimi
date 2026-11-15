import type { ModelRow } from "../api/types";

export interface PriceModel {
  id: string;
  name: string;
  provider: string;
  variants: {
    input: number;
    output: number;
    read: number | null;
    write: number | null;
  }[];
}
export interface Range {
  min: number;
  max: number;
}
/** Never infer request completeness from SUM: SQL ignores NULL within groups. */
export function completeUsage(row: ModelRow): ModelRow | null {
  if (
    row[14] == null ||
    !Number.isInteger(row[14]) ||
    row[14] <= 0 ||
    row[14] > row[2]
  )
    return null;
  const quantities = row.slice(15, 19);
  if (
    quantities.length !== 4 ||
    quantities.some(
      (n) => typeof n !== "number" || !Number.isFinite(n) || n < 0,
    )
  )
    return null;
  const subset: ModelRow = [...row];
  subset[2] = row[14];
  [subset[7], subset[8], subset[9], subset[10]] = quantities as number[];
  return subset;
}

export interface PriceHistory {
  fetchedAt: string;
  versions: {effectiveFrom: string; models: PriceModel[]}[];
}
export function modelOn(history: PriceHistory, day: string, id: string) {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(day) || Date.parse(day) > Date.parse(history.fetchedAt) + 30 * 86400000) return;
  for (let i = history.versions.length - 1; i >= 0; i--) {
    const version = history.versions[i];
    if (version.effectiveFrom <= day) return matchModel(id, version.models);
  }
}
function historicalCost(row: ModelRow, history: PriceHistory) {
  let pricedRequests = 0, min = 0, max = 0;
  const days = JSON.parse(row[19] ?? "[]") ?? [];
  for (const [day, count, input, output, reads, writes] of days) {
    const model = modelOn(history, day, row[0]);
    if (!model) continue;
    const subset: ModelRow = [...row];
    [subset[7], subset[8], subset[9], subset[10]] = [input, output, reads, writes];
    const amount = estimate(subset, model, model);
    if (amount) {
      pricedRequests += count;
      min += amount.min;
      max += amount.max;
    }
  }
  return {cost: pricedRequests ? {min, max} : null, pricedRequests};
}

export function summarizeCosts(rows: ModelRow[], models: PriceModel[], history?: PriceHistory) {
  const totalRequests = rows.reduce((sum, row) => sum + row[2], 0);
  const coverageKnown = rows.every(
    (row) => row[2] === 0 || (row[13] != null && row[14] != null),
  );
  const reportedRequests = coverageKnown
    ? rows.reduce((sum, row) => sum + (row[13] ?? 0), 0)
    : null;
  const amounts = rows
    .map((row) => row[12])
    .filter((v): v is number => v != null && Number.isFinite(v) && v >= 0);
  const reportedCost = amounts.length
    ? amounts.reduce((a, b) => a + b, 0)
    : null;
  const details = rows
    .filter((row) => row[2] > 0)
    .map((row) => {
      const model = matchModel(row[0], models);
      const complete = completeUsage(row);
      const aggregateCost = !history && model && complete ? estimate(complete, model, model) : null;
      const {cost, pricedRequests} = history ? historicalCost(row, history)
        : {cost: aggregateCost, pricedRequests: aggregateCost ? complete![2] : 0};
      return {
        row,
        cost,
        pricedRequests,
        reason: history && complete && !pricedRequests ? "no-historical-rate" : !model && !cost
          ? "unsupported"
          : row[14] == null
            ? "coverage-unknown"
            : !cost
              ? "incomplete"
              : pricedRequests < row[2]
                ? "partial"
                : "complete",
      };
    });
  const pricedRequests = details.reduce((sum, d) => sum + d.pricedRequests, 0);
  const equivalent = pricedRequests
    ? details.reduce(
        (sum, d) => ({
          min: sum.min + (d.cost?.min ?? 0),
          max: sum.max + (d.cost?.max ?? 0),
        }),
        { min: 0, max: 0 },
      )
    : null;
  return {
    totalRequests,
    reportedRequests,
    reportedCost,
    pricedRequests,
    equivalent,
    details,
  };
}
export function matchModel(id: string, models: PriceModel[]) {
  // Only exact IDs or an explicit dated snapshot; no guessed provider prefixes.
  const exact = models.find(m => m.id === id);
  if (exact) return exact;
  return models.find(m => id.startsWith(m.id + "-") &&
    (m.provider === "anthropic" ? /^\d{8}$/ : /^\d{4}-\d{2}-\d{2}$/)
      .test(id.slice(m.id.length + 1)));
}
export function estimate(
  row: ModelRow,
  source: PriceModel,
  target: PriceModel,
): Range | null {
  const amounts = [row[7], row[8], row[9], row[10]];
  if (amounts.some((n) => n === null || !Number.isFinite(n) || n < 0))
    return null;
  const [input, output, read, write] = amounts as number[];
  // Codex reports cached input as a subset of total input; Claude reports it separately.
  const uncached = source.provider === "openai" ? input - read - write : input;
  if (uncached < 0 || source.provider !== target.provider) return null;
  const costs = target.variants.map((p) => {
    if ((write > 0 && p.write === null) || (read > 0 && p.read === null)) return NaN;
    return (
      (uncached * p.input +
        output * p.output +
        read * (p.read ?? 0) +
        write * (p.write ?? 0)) /
      1e6
    );
  });
  if (!costs.length || costs.some((n) => !Number.isFinite(n))) return null;
  return { min: Math.min(...costs), max: Math.max(...costs) };
}
export function isStale(fetchedAt: string, now = Date.now()) {
  const age = now - Date.parse(fetchedAt);
  return !Number.isFinite(age) || age < -86400000 || age > 30 * 86400000;
}

export type RouterPriceDay = [string, number, number, number, number, number, number];
export function routerApiMicros(history: PriceHistory, id: string, days?: RouterPriceDay[]) {
  if (!days?.length) return null;
  let total = 0;
  for (const [day, , input, output, reads, writes] of days) {
    const rate = modelOn(history, day, id)?.variants[0];
    if (!rate || (writes && rate.write === null) || (reads && rate.read === null)) return null;
    total += (input - reads - writes) * rate.input + output * rate.output +
      reads * (rate.read ?? 0) + writes * (rate.write ?? 0);
  }
  return Math.round(total);
}
