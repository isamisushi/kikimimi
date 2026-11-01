import { t } from "../i18n";
import { getMcp, getUnusedMcp } from "../api/client";
import { fmtDateShort, fmtNum } from "../api/format";
import { useAsync } from "../hooks/useAsync";
import { QueryBoundary } from "../components/QueryBoundary";
import { SortableTable, type ColumnDef } from "../components/SortableTable";
import type { McpRow, UnusedMcpRow } from "../api/types";

const DAYS = 14;

/** Merged view row: `getMcp`'s per-server stats (which only ever lists
 * servers with at least one event) unioned with `getUnusedMcp`'s
 * configured/never-called servers, keyed by `mcp_server`. */
interface McpPageRow {
  mcp_server: string;
  configured: boolean;
  calls: number | null;
  failures: number | null;
  distinct_sessions: number | null;
  last_called_dt: string | null;
}

interface McpPageData {
  rows: McpPageRow[];
  /** Dataset-level (same on every `getUnusedMcp` row, see `UnusedMcpRow`'s
   * doc comment) -- false means `configured` fell back to the old
   * observed-in-the-last-30-days proxy instead of a real config snapshot. */
  configuredFromSnapshot: boolean;
}

function mergeMcpRows(mcp: McpRow[], unused: UnusedMcpRow[]): McpPageData {
  const byServer = new Map<string, McpPageRow>();
  for (const [
    mcp_server,
    calls,
    failures,
    distinct_sessions,
    last_called_dt,
  ] of mcp) {
    byServer.set(mcp_server, {
      mcp_server,
      configured: false,
      calls,
      failures,
      distinct_sessions,
      last_called_dt,
    });
  }
  for (const [
    mcp_server,
    configured,
    calls,
    distinct_sessions,
    last_called_dt,
  ] of unused) {
    const existing = byServer.get(mcp_server);
    if (existing) {
      // getMcp only ever lists servers with at least one event, so every
      // row it has takes precedence for calls/failures/sessions (it's the
      // richer table -- it alone carries `failures`); getUnusedMcp only
      // adds the "configured" flag on top for these.
      existing.configured = configured;
    } else {
      // Present only in getUnusedMcp: configured, never observed at all --
      // calls is therefore 0, so 0 failures is a known fact (not
      // "unknown"), unlike a genuinely unmeasured cell.
      byServer.set(mcp_server, {
        mcp_server,
        configured,
        calls,
        failures: 0,
        distinct_sessions,
        last_called_dt,
      });
    }
  }
  const configuredFromSnapshot = unused.length === 0 ? true : unused[0][6];
  return { rows: [...byServer.values()], configuredFromSnapshot };
}

/** The product's core question: configured, but nobody's called it in the window. */
function isUnused(r: McpPageRow): boolean {
  return r.configured && (r.calls === 0 || r.calls === null);
}

const columns: ColumnDef<McpPageRow>[] = [
  {
    key: "mcp_server",
    get label() {
      return t("MCP server");
    },
    sortValue: (r) => r.mcp_server,
    render: (r) => (
      <span className="mcp-name">
        <span className="mono">{r.mcp_server}</span>
        {isUnused(r) && (
          <span className="badge badge--warn">{t("Unused")}</span>
        )}
      </span>
    ),
  },
  {
    key: "configured",
    get label() {
      return t("Configured");
    },
    sortValue: (r) => (r.configured ? 1 : 0),
    render: (r) =>
      r.configured ? (
        <span className="badge badge--neutral">{t("Configured")}</span>
      ) : (
        "–"
      ),
  },
  {
    key: "calls",
    get label() {
      return t("Calls");
    },
    align: "right",
    sortValue: (r) => r.calls,
    render: (r) => fmtNum(r.calls),
  },
  {
    key: "failures",
    get label() {
      return t("Failures");
    },
    align: "right",
    sortValue: (r) => r.failures,
    render: (r) => (
      <span
        className={
          r.failures !== null && r.failures > 0 ? "text-danger" : undefined
        }
      >
        {fmtNum(r.failures)}
      </span>
    ),
  },
  {
    key: "distinct_sessions",
    get label() {
      return t("Sessions using it");
    },
    align: "right",
    sortValue: (r) => r.distinct_sessions,
    render: (r) => fmtNum(r.distinct_sessions),
  },
  {
    key: "last_called_dt",
    get label() {
      return t("Last called");
    },
    sortValue: (r) => r.last_called_dt,
    render: (r) => (r.last_called_dt ? fmtDateShort(r.last_called_dt) : "–"),
  },
];

export function Mcp() {
  const mcp = useAsync<McpPageData>(async () => {
    const [mcpResult, unusedResult] = await Promise.all([
      getMcp(DAYS),
      getUnusedMcp(DAYS),
    ]);
    return mergeMcpRows(mcpResult.rows, unusedResult.rows);
  }, [DAYS]);

  return (
    <div className="page">
      <div className="page__header">
        <h1>{t("MCP servers")}</h1>
        <p className="page__subtitle">
          {t("Usage over the last")} {DAYS}{" "}
          {t(
            "days. Unused servers are candidates for removal or consolidation.",
          )}
        </p>
      </div>

      <section className="panel">
        <QueryBoundary
          state={mcp}
          isEmpty={(d) => d.rows.length === 0}
          emptyLabel={t("No MCP servers connected")}
          onRetry={mcp.reload}
        >
          {(data) => {
            const unusedCount = data.rows.filter(isUnused).length;
            return (
              <>
                {!data.configuredFromSnapshot && (
                  <div className="callout callout--info">
                    {t(
                      "No config snapshot from this window yet — showing observed servers only (upgrade kikimimi and start a new session).",
                    )}
                  </div>
                )}
                {unusedCount > 0 && (
                  <div className="callout callout--warn">
                    {t(
                      "{0} configured MCP server{1} not called in the last {2} days. Consider whether it's still worth giving agents access.",
                      [unusedCount, unusedCount === 1 ? "" : "s", DAYS],
                    )}
                  </div>
                )}
                <SortableTable
                  columns={columns}
                  rows={data.rows}
                  rowKey={(r) => r.mcp_server}
                  defaultSortKey="calls"
                  defaultSortDir="asc"
                  rowClassName={(r) => (isUnused(r) ? "row-warn" : undefined)}
                  caption={t(
                    "Per-MCP-server usage: whether it's currently configured, call count, failure count, sessions using it, and last called date.",
                  )}
                />
              </>
            );
          }}
        </QueryBoundary>
      </section>
    </div>
  );
}
