-- Scope the events dedup/idempotency key to (org_id, event_id) instead of a
-- global `event_id` PRIMARY KEY (spec review finding #2: a client-supplied
-- `event_id` colliding across two different orgs would silently block one
-- org's insert via `ON CONFLICT (event_id) DO NOTHING` keyed on the other
-- org's row, reported as `deduped` rather than surfaced — a real gap in
-- defense-in-depth for §11 "テナント分離" beyond RLS, which only protects
-- reads, not the uniqueness constraint itself). `ingest.rs`'s
-- `ON CONFLICT (org_id, event_id)` matches this migration.
ALTER TABLE events DROP CONSTRAINT IF EXISTS events_pkey;

DO $do$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'events_org_event_pkey'
    ) THEN
        ALTER TABLE events ADD CONSTRAINT events_org_event_pkey PRIMARY KEY (org_id, event_id);
    END IF;
END
$do$;
