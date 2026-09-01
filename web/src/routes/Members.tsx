import { getMemberUsage, getMembers } from "../api/client";
import { fmtCost, fmtNum } from "../api/format";
import { useAsync } from "../hooks/useAsync";
import { useSession } from "../hooks/useSession";
import { QueryBoundary } from "../components/QueryBoundary";
import { SortableTable, type ColumnDef } from "../components/SortableTable";
import type { MemberRow, OrgKind, Role } from "../api/types";

const DAYS = 30;
const ROLE_RANK: Record<Role, number> = { owner: 4, admin: 3, member: 2, viewer: 1 };

interface MembersData {
  rows: MemberRow[];
  /** account_id -> email, from GET /web/orgs/:slug/members -- so the table
   * can show who a row actually is instead of a raw user_id (account uuid). */
  emailByUserId: Map<string, string>;
}

function loopSuspectCount(r: MemberRow): number {
  return r[9] ?? 0;
}

function memberLabel(r: MemberRow, emailByUserId: Map<string, string>): string {
  return emailByUserId.get(r[0]) ?? r[0];
}

function buildColumns(emailByUserId: Map<string, string>): ColumnDef<MemberRow>[] {
  return [
    {
      key: "member",
      label: "Member",
      sortValue: (r) => memberLabel(r, emailByUserId),
      render: (r) => <span className="mono">{memberLabel(r, emailByUserId)}</span>,
    },
    {
      key: "sessions",
      label: "Sessions",
      align: "right",
      sortValue: (r) => r[1],
      render: (r) => fmtNum(r[1]),
    },
    {
      key: "api_requests",
      label: "API requests",
      align: "right",
      sortValue: (r) => r[2],
      render: (r) => fmtNum(r[2]),
    },
    {
      key: "tool_calls",
      label: "Tool calls",
      align: "right",
      sortValue: (r) => r[3],
      render: (r) => fmtNum(r[3]),
    },
    {
      key: "tool_failures",
      label: "Failures",
      align: "right",
      sortValue: (r) => r[4],
      render: (r) => (
        <span className={r[4] !== null && r[4] > 0 ? "text-danger" : undefined}>{fmtNum(r[4])}</span>
      ),
    },
    {
      key: "input_tokens",
      label: "Input tok",
      align: "right",
      sortValue: (r) => r[5],
      render: (r) => fmtNum(r[5]),
    },
    {
      key: "output_tokens",
      label: "Output tok",
      align: "right",
      sortValue: (r) => r[6],
      render: (r) => fmtNum(r[6]),
    },
    {
      key: "cache_read_tokens",
      label: "Cache read",
      align: "right",
      sortValue: (r) => r[7],
      render: (r) => fmtNum(r[7]),
    },
    {
      key: "cost_usd",
      label: "Est. cost",
      align: "right",
      sortValue: (r) => r[8],
      render: (r) => fmtCost(r[8]),
    },
    {
      key: "loop_suspect_sessions",
      label: "Loop-suspect sessions",
      align: "right",
      sortValue: (r) => r[9],
      render: (r) => (
        <span className="mcp-name">
          {fmtNum(r[9])}
          {loopSuspectCount(r) > 0 && <span className="badge badge--warn">Check for loops</span>}
        </span>
      ),
    },
  ];
}

export function Members() {
  const { session } = useSession();
  if (!session) return null;

  const active = session.orgs.find((o) => o.slug === session.active_org);

  return (
    <div className="page">
      <div className="page__header">
        <h1>Member usage</h1>
        <p className="page__subtitle">
          Understand what drives each member's usage — high totals usually mean loops or heavy cache re-reads,
          not "overuse". Sorted by member, not by cost: this is an explanation, not a leaderboard.
        </p>
      </div>

      {active ? (
        <MembersGate slug={active.slug} kind={active.kind} role={active.role} />
      ) : (
        <section className="panel">
          <div className="state-panel state-panel--empty">No active workspace</div>
        </section>
      )}
    </div>
  );
}

/** Mirrors Team.tsx's `TeamAdminOrMember` gating: in a team org, a role
 * below admin can't view this even before hitting the network (the server
 * enforces the same admin/owner-only rule and would 403 anyway, see
 * `crates/cloud/src/web_query.rs`'s `members` handler doc comment) -- a
 * `personal` org has no "other members" to gate away from, so it's always
 * shown. */
function MembersGate({ slug, kind, role }: { slug: string; kind: OrgKind; role: Role }) {
  if (kind === "team" && ROLE_RANK[role] < ROLE_RANK.admin) {
    return (
      <section className="panel">
        <p className="page__subtitle">
          You're a <span className="badge badge--neutral">{role}</span> of this team. Ask an admin to view
          member usage.
        </p>
      </section>
    );
  }
  return <MembersPanel activeOrgSlug={slug} />;
}

function MembersPanel({ activeOrgSlug }: { activeOrgSlug: string }) {
  const usage = useAsync<MembersData>(async () => {
    const [usageResult, roster] = await Promise.all([getMemberUsage(DAYS), getMembers(activeOrgSlug)]);
    const emailByUserId = new Map(roster.members.map((m) => [m.account_id, m.email]));
    return { rows: usageResult.rows, emailByUserId };
  }, [activeOrgSlug]);

  return (
    <section className="panel">
      <QueryBoundary
        state={usage}
        isEmpty={(d) => d.rows.length === 0}
        emptyLabel="No usage in this window"
        onRetry={usage.reload}
      >
        {(data) => {
          const columns = buildColumns(data.emailByUserId);
          const loopSuspectMembers = data.rows.filter((r) => loopSuspectCount(r) > 0).length;
          return (
            <>
              {loopSuspectMembers > 0 && (
                <div className="callout callout--warn">
                  <strong>{loopSuspectMembers}</strong> member{loopSuspectMembers === 1 ? "" : "s"} ha
                  {loopSuspectMembers === 1 ? "s" : "ve"} at least one session with 50+ API requests. Worth a
                  look for a runaway loop -- not necessarily a problem on its own.
                </div>
              )}
              <SortableTable
                columns={columns}
                rows={data.rows}
                rowKey={(r) => r[0]}
                defaultSortKey="member"
                defaultSortDir="asc"
                rowClassName={(r) => (loopSuspectCount(r) > 0 ? "row-warn" : undefined)}
                caption={`Per-member usage over the last ${DAYS} days: sessions, API requests, tool calls, failures, token totals, estimated cost, and loop-suspect session count.`}
              />
            </>
          );
        }}
      </QueryBoundary>
    </section>
  );
}
