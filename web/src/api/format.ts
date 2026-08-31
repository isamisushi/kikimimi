/** Shared render helpers. The contract says numbers may be null when
 * usage_source is unknown — those must render as "–", never "0". */

const NULL_GLYPH = "–";

export function fmtNum(value: number | null | undefined): string {
  if (value === null || value === undefined) return NULL_GLYPH;
  return new Intl.NumberFormat("en-US").format(value);
}

export function fmtCost(value: number | null | undefined): string {
  if (value === null || value === undefined) return NULL_GLYPH;
  return `$${value.toFixed(2)}`;
}

export function fmtMs(value: number | null | undefined): string {
  if (value === null || value === undefined) return NULL_GLYPH;
  if (value >= 1000) return `${(value / 1000).toFixed(1)}s`;
  return `${Math.round(value)}ms`;
}

export function fmtStr(value: string | null | undefined): string {
  if (value === null || value === undefined || value === "") return NULL_GLYPH;
  return value;
}

/** e.g. "2026-08-31T10:03:00Z" -> "08/31 10:03" */
export function fmtDateTime(value: string | null | undefined): string {
  if (!value) return NULL_GLYPH;
  const d = new Date(value);
  if (Number.isNaN(d.getTime())) return value;
  const mm = String(d.getMonth() + 1).padStart(2, "0");
  const dd = String(d.getDate()).padStart(2, "0");
  const hh = String(d.getHours()).padStart(2, "0");
  const mi = String(d.getMinutes()).padStart(2, "0");
  return `${mm}/${dd} ${hh}:${mi}`;
}

/** e.g. "2026-08-31" -> "08/31" for chart axis labels. */
export function fmtDateShort(value: string): string {
  const parts = value.split("-");
  if (parts.length === 3) return `${parts[1]}/${parts[2]}`;
  return value;
}

/** Minutes since `value` (ISO timestamp), or null if unknown/unparseable. */
export function minutesSince(value: string | null | undefined): number | null {
  if (!value) return null;
  const d = new Date(value);
  if (Number.isNaN(d.getTime())) return null;
  return Math.max(0, Math.floor((Date.now() - d.getTime()) / 60000));
}
