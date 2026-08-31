import { fmtCost, fmtDateShort, fmtNum } from "../api/format";

export interface TokenBarDatum {
  dt: string;
  input: number | null;
  output: number | null;
  cost: number | null;
}

const CHART_W = 700;
const CHART_H = 220;
const PAD_TOP = 12;
const PAD_BOTTOM = 28;
const BAR_GAP_RATIO = 0.35;

/** Hand-rolled stacked SVG bar chart: input+output tokens per day, cost in the tooltip. */
export function TokenBarChart({ data }: { data: TokenBarDatum[] }) {
  const n = Math.max(data.length, 1);
  const width = CHART_W;
  const slot = width / n;
  const barWidth = slot * (1 - BAR_GAP_RATIO);
  const plotH = CHART_H - PAD_TOP - PAD_BOTTOM;

  const totals = data.map((d) =>
    d.input === null && d.output === null ? null : (d.input ?? 0) + (d.output ?? 0),
  );
  const max = Math.max(1, ...totals.filter((t): t is number => t !== null));

  // Show every label when few bars, thin them out otherwise to avoid overlap.
  const labelStride = data.length > 10 ? 2 : 1;

  return (
    <div className="bar-chart">
      <div className="bar-chart__legend">
        <span className="legend-item">
          <span className="legend-swatch legend-swatch--input" /> input tokens
        </span>
        <span className="legend-item">
          <span className="legend-swatch legend-swatch--output" /> output tokens
        </span>
        <span className="legend-item legend-item--muted">
          <span className="legend-swatch legend-swatch--unknown" /> No data
        </span>
      </div>
      <svg
        className="bar-chart__svg"
        viewBox={`0 0 ${width} ${CHART_H}`}
        role="img"
        aria-label="Daily token usage"
      >
        <line
          x1={0}
          x2={width}
          y1={CHART_H - PAD_BOTTOM}
          y2={CHART_H - PAD_BOTTOM}
          className="bar-chart__axis"
        />
        {data.map((d, i) => {
          const x = i * slot + (slot - barWidth) / 2;
          const total = totals[i];
          const title = `${d.dt}\ninput: ${fmtNum(d.input)}\noutput: ${fmtNum(d.output)}\ncost: ${fmtCost(d.cost)}`;

          if (total === null) {
            const h = plotH * 0.06;
            const y = CHART_H - PAD_BOTTOM - h;
            return (
              <g key={d.dt}>
                <rect
                  x={x}
                  y={y}
                  width={barWidth}
                  height={h}
                  className="bar-chart__bar bar-chart__bar--unknown"
                >
                  <title>{title}</title>
                </rect>
                {i % labelStride === 0 && (
                  <text
                    x={x + barWidth / 2}
                    y={CHART_H - PAD_BOTTOM + 14}
                    className="bar-chart__label"
                  >
                    {fmtDateShort(d.dt)}
                  </text>
                )}
              </g>
            );
          }

          const inputVal = d.input ?? 0;
          const outputVal = d.output ?? 0;
          const inputH = max > 0 ? (inputVal / max) * plotH : 0;
          const outputH = max > 0 ? (outputVal / max) * plotH : 0;
          const yInput = CHART_H - PAD_BOTTOM - inputH;
          const yOutput = yInput - outputH;

          return (
            <g key={d.dt}>
              {inputH > 0 && (
                <rect
                  x={x}
                  y={yInput}
                  width={barWidth}
                  height={inputH}
                  className="bar-chart__bar bar-chart__bar--input"
                >
                  <title>{title}</title>
                </rect>
              )}
              {outputH > 0 && (
                <rect
                  x={x}
                  y={yOutput}
                  width={barWidth}
                  height={outputH}
                  className="bar-chart__bar bar-chart__bar--output"
                >
                  <title>{title}</title>
                </rect>
              )}
              {inputH === 0 && outputH === 0 && (
                <rect
                  x={x}
                  y={CHART_H - PAD_BOTTOM - 1}
                  width={barWidth}
                  height={1}
                  className="bar-chart__bar bar-chart__bar--zero"
                >
                  <title>{title}</title>
                </rect>
              )}
              {i % labelStride === 0 && (
                <text
                  x={x + barWidth / 2}
                  y={CHART_H - PAD_BOTTOM + 14}
                  className="bar-chart__label"
                >
                  {fmtDateShort(d.dt)}
                </text>
              )}
            </g>
          );
        })}
      </svg>
    </div>
  );
}
