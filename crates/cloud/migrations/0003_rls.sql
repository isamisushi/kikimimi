-- Row-Level Security on events (architecture.md §6, §11 "テナント分離").
-- `guru_app` gets exactly the grants it needs (USAGE + SELECT/INSERT on
-- events) and nothing on accounts/orgs/org_members/devices/device_codes —
-- those stay reachable only through the superuser pool, in the auth
-- endpoints (device.rs).
ALTER TABLE events ENABLE ROW LEVEL SECURITY;
ALTER TABLE events FORCE ROW LEVEL SECURITY;

DROP POLICY IF EXISTS events_org_isolation ON events;
CREATE POLICY events_org_isolation ON events
    USING (org_id = current_setting('app.org_id')::uuid)
    WITH CHECK (org_id = current_setting('app.org_id')::uuid);

GRANT USAGE ON SCHEMA public TO guru_app;
GRANT SELECT, INSERT ON events TO guru_app;
