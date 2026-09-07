import type { CoverageRow } from "./types";

/** A rate as a whole-number percent, or null when the denominator is 0 —
 * "no data" must never render as 0% or 100%. */
export function pct(num: number, den: number): number | null {
  if (!den) return null;
  return Math.round((num / den) * 100);
}

export function fmtPct(value: number | null): string {
  return value === null ? "–" : `${value}%`;
}

export interface CoverageSummary {
  /** Sessions whose usage (tokens) is known. */
  usageKnownPct: number | null;
  /** Hook tool results that OTel also reported (the §5.1 correlation). */
  hookOtelMatchPct: number | null;
  /** Tool results the hook/OTel dedup folded away. */
  dedupDroppedPct: number | null;
  /** Subagents whose usage is known. */
  subagentUsageKnownPct: number | null;
  /** Events attributed to an account (cloud only; local is always 0). */
  attributedPct: number | null;
  hostsSilent: number;
  hosts: number;
  /** Something is missing enough to warn about. */
  warn: boolean;
}

export const COVERAGE_DAYS = 30;

export function summarize(r: CoverageRow): CoverageSummary {
  const usageKnownPct = pct(r[2] - r[3], r[2]);
  const hookOtelMatchPct = pct(r[6], r[4]);
  const dedupDroppedPct = pct(r[7] - r[8], r[7]);
  const subagentUsageKnownPct = pct(r[10], r[9]);
  const attributedPct = pct(r[0] - r[1], r[0]);
  const warn =
    (usageKnownPct !== null && usageKnownPct < 80) ||
    (hookOtelMatchPct !== null && hookOtelMatchPct < 80) ||
    r[12] > 0;
  return {
    usageKnownPct,
    hookOtelMatchPct,
    dedupDroppedPct,
    subagentUsageKnownPct,
    attributedPct,
    hostsSilent: r[12],
    hosts: r[11],
    warn,
  };
}
