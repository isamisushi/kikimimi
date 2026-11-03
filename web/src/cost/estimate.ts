import type { ModelRow } from "../api/types";

export interface PriceModel {
  id: string;
  name: string;
  provider: string;
  variants: {
    input: number;
    output: number;
    read: number;
    write: number | null;
  }[];
}
export interface Range {
  min: number;
  max: number;
}
export interface Opportunity {
  row: ModelRow;
  current: PriceModel;
  target: PriceModel;
  before: Range;
  after: Range;
  saving: Range;
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

export function summarizeCosts(rows: ModelRow[], models: PriceModel[]) {
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
      const cost = model && complete ? estimate(complete, model, model) : null;
      return {
        row,
        cost,
        pricedRequests: cost ? complete![2] : 0,
        reason: !model
          ? "unsupported"
          : row[14] == null
            ? "coverage-unknown"
            : !cost
              ? "incomplete"
              : row[14] < row[2]
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
  return models.find(
    (m) =>
      id === m.id ||
      (m.provider === "anthropic" && new RegExp(`^${m.id}-\\d{8}$`).test(id)),
  );
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
    if (write > 0 && p.write === null) return NaN;
    return (
      (uncached * p.input +
        output * p.output +
        read * p.read +
        write * (p.write ?? 0)) /
      1e6
    );
  });
  if (!costs.length || costs.some((n) => !Number.isFinite(n))) return null;
  return { min: Math.min(...costs), max: Math.max(...costs) };
}
export function opportunities(rows: ModelRow[], models: PriceModel[]) {
  const eligible: { row: ModelRow; model: PriceModel; cost: Range }[] = [];
  const excluded: ModelRow[] = [];
  const recommendations: Opportunity[] = [];
  for (const row of rows) {
    const model = matchModel(row[0], models);
    const complete = completeUsage(row);
    const cost = model && complete && estimate(complete, model, model);
    if (!model || !cost) {
      excluded.push(row);
      continue;
    }
    eligible.push({ row: complete!, model, cost });
    const candidates = models
      .filter((m) => m.provider === model.provider && m.id !== model.id)
      .flatMap((target) => {
        const after = estimate(complete!, model, target);
        if (!after || after.max >= cost.min) return [];
        return [
          {
            row: complete!,
            current: model,
            target,
            before: cost,
            after,
            saving: { min: cost.min - after.max, max: cost.max - after.min },
          },
        ];
      })
      .sort((a, b) => b.saving.min - a.saving.min);
    // One mutually exclusive alternative per model/effort row: never double-count savings.
    if (candidates[0]) recommendations.push(candidates[0]);
  }
  recommendations.sort((a, b) => b.saving.min - a.saving.min);
  return { eligible, excluded, recommendations };
}
export function subscriptionSaving(
  current: string,
  proposed: string,
): number | null {
  if (!current.trim() || !proposed.trim()) return null;
  const values = [Number(current), Number(proposed)];
  return values.every((n) => Number.isFinite(n) && n >= 0)
    ? values[0] - values[1]
    : null;
}
export function isStale(fetchedAt: string, now = Date.now()) {
  const age = now - Date.parse(fetchedAt);
  return !Number.isFinite(age) || age < -86400000 || age > 30 * 86400000;
}
