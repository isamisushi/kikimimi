-- Company roster is independent of accounts: include people without telemetry.
CREATE TABLE IF NOT EXISTS ai_inventory (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id UUID NOT NULL REFERENCES orgs(id),
    email TEXT NOT NULL,
    name TEXT NOT NULL,
    department TEXT NOT NULL DEFAULT '',
    account_id UUID REFERENCES accounts(id),
    assignments JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(assignments) = 'array'),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (org_id, email)
);
ALTER TABLE ai_inventory ENABLE ROW LEVEL SECURITY;
ALTER TABLE ai_inventory FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS ai_inventory_org_isolation ON ai_inventory;
CREATE POLICY ai_inventory_org_isolation ON ai_inventory
    USING (org_id = current_setting('app.org_id')::uuid)
    WITH CHECK (org_id = current_setting('app.org_id')::uuid);
GRANT SELECT, INSERT, UPDATE, DELETE ON ai_inventory TO kikimimi_app;
