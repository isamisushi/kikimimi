import { getCoverage } from "../api/client";
import { COVERAGE_DAYS, fmtPct, summarize } from "../api/coverage";
import { useAsync } from "../hooks/useAsync";
import { Link } from "../router/Router";

/** Topbar confidence badge (KKM-17): how much of the last 30 days the numbers
 * on every page can actually see. Links to the Overview's coverage panel. */
export function CoverageBadge() {
  const state = useAsync(() => getCoverage(COVERAGE_DAYS), []);
  if (state.status !== "ok" || state.data.rows.length === 0) return null;
  const s = summarize(state.data.rows[0]);
  const parts = [`usage ${fmtPct(s.usageKnownPct)}`, `hook↔otel ${fmtPct(s.hookOtelMatchPct)}`];
  if (s.hostsSilent > 0) parts.push(`${s.hostsSilent} host${s.hostsSilent === 1 ? "" : "s"} silent`);
  return (
    <span title={`Data coverage, last ${COVERAGE_DAYS} days. Click for the breakdown.`}>
      <Link to="/" className={"coverage-badge" + (s.warn ? " coverage-badge--warn" : "")}>
        <span className="coverage-badge__label">coverage</span> {parts.join(" · ")}
      </Link>
    </span>
  );
}
