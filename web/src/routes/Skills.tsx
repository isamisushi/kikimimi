import { getSkills, getUnusedSkills } from "../api/client";
import { fmtDateShort, fmtNum } from "../api/format";
import { useAsync } from "../hooks/useAsync";
import { QueryBoundary } from "../components/QueryBoundary";
import { SortableTable, type ColumnDef } from "../components/SortableTable";
import type { SkillRow, UnusedSkillRow } from "../api/types";

const DAYS = 14;

const columns: ColumnDef<SkillRow>[] = [
  {
    key: "skill_name",
    label: "Skill",
    sortValue: (r) => r[0],
    render: (r) => <span className="mono">{r[0]}</span>,
  },
  {
    key: "calls",
    label: "Invocations",
    align: "right",
    sortValue: (r) => r[1],
    render: (r) => fmtNum(r[1]),
  },
  {
    key: "failures",
    label: "Failures",
    align: "right",
    sortValue: (r) => r[2],
    render: (r) => (
      <span className={r[2] !== null && r[2] > 0 ? "text-danger" : undefined}>
        {fmtNum(r[2])}
      </span>
    ),
  },
  {
    key: "distinct_sessions",
    label: "Sessions using it",
    align: "right",
    sortValue: (r) => r[3],
    render: (r) => fmtNum(r[3]),
  },
  {
    key: "last_used_dt",
    label: "Last used",
    sortValue: (r) => r[4],
    render: (r) => (r[4] ? fmtDateShort(r[4]) : "\u2013"),
  },
];

const unusedColumns: ColumnDef<UnusedSkillRow>[] = [
  {
    key: "skill_name",
    label: "Skill",
    sortValue: (r) => r[0],
    render: (r) => (
      <span className="mono">
        {r[0]}
        {r[1] && r[3] === 0 && <span className="badge badge--warn">Unused</span>}
      </span>
    ),
  },
  {
    key: "sessions_configured",
    label: "Sessions listing it",
    align: "right",
    sortValue: (r) => r[2],
    render: (r) => fmtNum(r[2]),
  },
  {
    key: "calls",
    label: "Invocations",
    align: "right",
    sortValue: (r) => r[3],
    render: (r) => fmtNum(r[3]),
  },
  {
    key: "last_used_dt",
    label: "Last used",
    sortValue: (r) => r[5],
    render: (r) => (r[5] ? fmtDateShort(r[5]) : "\u2013"),
  },
];

export function Skills() {
  const skills = useAsync(() => getSkills(DAYS), [DAYS]);
  const unused = useAsync(() => getUnusedSkills(DAYS), [DAYS]);

  return (
    <div className="page">
      <div className="page__header">
        <h1>Skills</h1>
        <p className="page__subtitle">
          Skill invocations over the last {DAYS} days, extracted from agent hooks (name only, never skill arguments).
        </p>
      </div>

      <section className="panel">
        <QueryBoundary
          state={skills}
          isEmpty={(d) => d.rows.length === 0}
          emptyLabel="No skill invocations recorded"
          onRetry={skills.reload}
        >
          {(data) => (
            <SortableTable
              columns={columns}
              rows={data.rows}
              rowKey={(r) => r[0]}
              defaultSortKey="calls"
              defaultSortDir="asc"
              caption="Per-skill usage: invocation count, failure count, sessions using it, and last used date."
            />
          )}
        </QueryBoundary>
      </section>

      <section className="panel">
        <h2 className="panel__title">Configured vs invoked</h2>
        <p className="panel__note">
          "Configured" means Claude Code listed the skill in a session's transcript (bundled skills,
          ~/.claude/skills, plugins). Only sessions with a transcript backfill carry that list; a
          hooks-only session says nothing about what was configured.
        </p>
        <QueryBoundary
          state={unused}
          isEmpty={(d) => d.rows.length === 0}
          emptyLabel="No skill listings in this period"
          onRetry={unused.reload}
        >
          {(data) => {
            const unusedCount = data.rows.filter((r) => r[1] && r[3] === 0).length;
            return (
              <>
                {unusedCount > 0 && (
                  <div className="callout callout--warn">
                    <strong>{unusedCount}</strong> configured skill{unusedCount === 1 ? "" : "s"} never
                    invoked in the last {DAYS} days.
                  </div>
                )}
                <SortableTable
                  columns={unusedColumns}
                  rows={data.rows}
                  rowKey={(r) => r[0]}
                  defaultSortKey="calls"
                  defaultSortDir="asc"
                  rowClassName={(r) => (r[1] && r[3] === 0 ? "row-warn" : undefined)}
                  caption="Skills Claude Code listed per session versus skills actually invoked."
                />
              </>
            );
          }}
        </QueryBoundary>
      </section>
    </div>
  );
}
