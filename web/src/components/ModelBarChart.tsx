import { fmtCost, fmtDateShort, fmtNum } from "../api/format";
import type { ModelDailyRow } from "../api/types";

/** How many models get their own color; the rest fold into "Other". Fixed
 * slot order (assigned by total tokens, once, never re-assigned on hover). */
export const MODEL_SERIES_SLOTS = 6;

const CHART_W = 700;
const CHART_H = 220;
const PAD_TOP = 12;
const PAD_BOTTOM = 28;
const BAR_GAP_RATIO = 0.35;
/** Surface gap between stacked segments (dataviz mark spec). */
const SEG_GAP = 2;

export interface ModelSeries {
  model: string;
  /** 1..MODEL_SERIES_SLOTS, or "other". */
  slot: number | "other";
}

/** Assigns each model a fixed slot by descending total tokens; every model
 * past `MODEL_SERIES_SLOTS` shares the "Other" slot. */
export function assignModelSeries(rows: ModelDailyRow[]): ModelSeries[] {
  const totals = new Map<string, number>();
  for (const r of rows) {
    totals.set(r[1], (totals.get(r[1]) ?? 0) + (r[2] ?? 0) + (r[3] ?? 0));
  }
  const ordered = [...totals.entries()].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]));
  return ordered.map(([model], i) => ({ model, slot: i < MODEL_SERIES_SLOTS ? i + 1 : "other" }));
}

export function seriesClass(slot: number | "other"): string {
  return slot === "other" ? "series--other" : `series--${slot}`;
}

interface DayStack {
  dt: string;
  /** Per-slot totals in slot order (1..N, then other); null day = no usage row at all. */
  segments: { label: string; slot: number | "other"; tokens: number; cost: number | null }[];
  total: number | null;
}

/** Stacked SVG bar chart: input+output tokens per day, one segment per model
 * (top-N by tokens, rest as "Other"). Cost rides along in the tooltip. */
export function ModelBarChart({ rows, series }: { rows: ModelDailyRow[]; series: ModelSeries[] }) {
  const slotOf = new Map(series.map((s) => [s.model, s.slot]));
  const days = new Map<string, DayStack>();
  for (const r of rows) {
    const day = days.get(r[0]) ?? { dt: r[0], segments: [], total: null };
    const tokens = r[2] === null && r[3] === null ? null : (r[2] ?? 0) + (r[3] ?? 0);
    if (tokens !== null) {
      const slot = slotOf.get(r[1]) ?? "other";
      const label = slot === "other" ? "Other" : r[1];
      const seg = day.segments.find((s) => s.slot === slot);
      if (seg) {
        seg.tokens += tokens;
        seg.cost = seg.cost === null && r[4] === null ? null : (seg.cost ?? 0) + (r[4] ?? 0);
      } else {
        day.segments.push({ label, slot, tokens, cost: r[4] });
      }
      day.total = (day.total ?? 0) + tokens;
    }
    days.set(r[0], day);
  }
  const data = [...days.values()].sort((a, b) => a.dt.localeCompare(b.dt));
  for (const d of data) {
    d.segments.sort((a, b) => slotRank(a.slot) - slotRank(b.slot));
  }

  const n = Math.max(data.length, 1);
  const slot = CHART_W / n;
  const barWidth = slot * (1 - BAR_GAP_RATIO);
  const plotH = CHART_H - PAD_TOP - PAD_BOTTOM;
  const max = Math.max(1, ...data.map((d) => d.total ?? 0));
  const labelStride = data.length > 10 ? 2 : 1;

  return (
    <div className="bar-chart">
      <div className="bar-chart__legend">
        {series
          .filter((s) => s.slot !== "other")
          .map((s) => (
            <span key={s.model} className="legend-item">
              <span className={`legend-swatch ${seriesClass(s.slot)}`} /> <span className="mono">{s.model}</span>
            </span>
          ))}
        {series.some((s) => s.slot === "other") && (
          <span className="legend-item">
            <span className="legend-swatch series--other" /> Other ({series.filter((s) => s.slot === "other").length})
          </span>
        )}
        <span className="legend-item legend-item--muted">
          <span className="legend-swatch legend-swatch--unknown" /> No data
        </span>
      </div>
      <svg className="bar-chart__svg" viewBox={`0 0 ${CHART_W} ${CHART_H}`} role="img" aria-label="Daily tokens per model">
        <line x1={0} x2={CHART_W} y1={CHART_H - PAD_BOTTOM} y2={CHART_H - PAD_BOTTOM} className="bar-chart__axis" />
        {data.map((d, i) => {
          const x = i * slot + (slot - barWidth) / 2;
          const label = i % labelStride === 0 && (
            <text x={x + barWidth / 2} y={CHART_H - PAD_BOTTOM + 14} className="bar-chart__label">
              {fmtDateShort(d.dt)}
            </text>
          );
          if (d.total === null) {
            const h = plotH * 0.06;
            return (
              <g key={d.dt}>
                <rect x={x} y={CHART_H - PAD_BOTTOM - h} width={barWidth} height={h} className="bar-chart__bar bar-chart__bar--unknown">
                  <title>{`${d.dt}\nno usage captured`}</title>
                </rect>
                {label}
              </g>
            );
          }
          const tooltip = [
            `${d.dt} · ${fmtNum(d.total)} tokens`,
            ...d.segments.map((s) => `${s.label}: ${fmtNum(s.tokens)} · ${fmtCost(s.cost)}`),
          ].join("\n");
          let y = CHART_H - PAD_BOTTOM;
          return (
            <g key={d.dt}>
              <title>{tooltip}</title>
              {d.segments.map((s) => {
                const h = (s.tokens / max) * plotH;
                y -= h;
                const gap = h > SEG_GAP * 2 ? SEG_GAP : 0;
                return (
                  <rect
                    key={String(s.slot)}
                    x={x}
                    y={y + gap}
                    width={barWidth}
                    height={Math.max(0, h - gap)}
                    className={`bar-chart__bar ${seriesClass(s.slot)}`}
                  />
                );
              })}
              {label}
            </g>
          );
        })}
      </svg>
    </div>
  );
}

function slotRank(slot: number | "other"): number {
  return slot === "other" ? MODEL_SERIES_SLOTS + 1 : slot;
}
