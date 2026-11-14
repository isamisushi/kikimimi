import type { PatternTimelineRow } from "../api/types";

export interface Observation {
  days: number;
  sessions: number;
  hitSessions: number;
  incidents: number;
  rate: number | null;
}
export interface Comparison {
  status:
    | "waiting"
    | "insufficient"
    | "collection_unverified"
    | "lower"
    | "recurring"
    | "no_recurrence";
  before: Observation;
  after: Observation;
  windowDays: number;
}
const DAY = 86_400_000;

/** UTC dates match the stored event partitions. Exclude both the change day
 * (unknown change time) and today (incomplete), and use equal calendar windows.
 * These are observed rates, never a causal claim or proof of cost savings. */
export function compareChange(
  rows: PatternTimelineRow[],
  markedDt: string,
  today: string,
  collectionReady: boolean,
): Comparison {
  const mark = Date.parse(`${markedDt}T00:00:00Z`);
  const end = Date.parse(`${today}T00:00:00Z`);
  const windowDays = Math.max(
    0,
    Math.min(14, Math.floor((end - mark) / DAY) - 1),
  );
  const beforeStart = new Date(mark - windowDays * DAY)
    .toISOString()
    .slice(0, 10);
  const afterEnd = new Date(mark + (windowDays + 1) * DAY)
    .toISOString()
    .slice(0, 10);
  const summarize = (part: PatternTimelineRow[]): Observation => {
    const sessions = part.reduce((n, r) => n + r[1], 0);
    const hitSessions = part.reduce((n, r) => n + r[2], 0);
    return {
      days: part.filter((r) => r[1] > 0).length,
      sessions,
      hitSessions,
      incidents: part.reduce((n, r) => n + r[4], 0),
      rate: sessions > 0 ? (100 * hitSessions) / sessions : null,
    };
  };
  const before = summarize(
    rows.filter((r) => r[0] >= beforeStart && r[0] < markedDt),
  );
  const after = summarize(
    rows.filter((r) => r[0] > markedDt && r[0] < afterEnd),
  );
  let status: Comparison["status"] = "waiting";
  if (!collectionReady) status = "collection_unverified";
  else if (after.sessions === 0) status = "waiting";
  else if (
    before.sessions < 5 ||
    after.sessions < 5 ||
    before.days < 2 ||
    after.days < 2
  )
    status = "insufficient";
  else if (after.hitSessions === 0) status = "no_recurrence";
  else if (after.rate! < before.rate!) status = "lower";
  else status = "recurring";
  return { status, before, after, windowDays };
}
