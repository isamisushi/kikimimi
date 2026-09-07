-- Improvement markers for the §7.3 before/after view (KKM-12). A platform
-- team records "we changed MCP server X / tool Y on <day>" so the Struggles
-- page can split that subject's timeline into before and after. Org-scoped
-- exactly like pattern_hits (RLS on org_id, kikimimi_app role).
CREATE TABLE IF NOT EXISTS improvement_marks (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id     UUID NOT NULL REFERENCES orgs(id),
    pattern_id TEXT NOT NULL,
    subject    TEXT NOT NULL,
    marked_dt  TEXT NOT NULL,
    note       TEXT NOT NULL DEFAULT '',
    created_by UUID NOT NULL REFERENCES accounts(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS improvement_marks_org_subject_idx
    ON improvement_marks (org_id, pattern_id, subject, marked_dt);

ALTER TABLE improvement_marks ENABLE ROW LEVEL SECURITY;
ALTER TABLE improvement_marks FORCE ROW LEVEL SECURITY;

DROP POLICY IF EXISTS improvement_marks_org_isolation ON improvement_marks;
CREATE POLICY improvement_marks_org_isolation ON improvement_marks
    USING (org_id = current_setting('app.org_id')::uuid)
    WITH CHECK (org_id = current_setting('app.org_id')::uuid);

GRANT SELECT, INSERT, DELETE ON improvement_marks TO kikimimi_app;
