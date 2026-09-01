import { getSkills } from "../api/client";
import { fmtDateShort, fmtNum } from "../api/format";
import { useAsync } from "../hooks/useAsync";
import { QueryBoundary } from "../components/QueryBoundary";
import { SortableTable, type ColumnDef } from "../components/SortableTable";
import type { SkillRow } from "../api/types";

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

export function Skills() {
  const skills = useAsync(() => getSkills(DAYS), [DAYS]);

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
    </div>
  );
}
