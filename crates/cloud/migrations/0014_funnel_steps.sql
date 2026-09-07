-- Onboarding funnel (KKM-21, architecture.md §15-15 / §12 Stage 0 "install →
-- first insight in 2 minutes"). One row per (subject, step): the first time
-- a host or an account reached a step. Timestamps and opaque ids only -- no
-- email, no hostname, no event content -- and only ever read as aggregate
-- counts by `GET /web/q/funnel` (operator allowlist). SUPERUSER pool only,
-- like `devices`: the RLS-scoped `kikimimi_app` role never touches it.
--
-- subject_kind = 'host'    subject_id = devices.host_id
--   steps: login_started (POST /v1/device/code), login_done (token minted),
--          first_events (first accepted POST /v1/events batch)
-- subject_kind = 'account' subject_id = accounts.id::text
--   steps: first_insight (first GET /web/q/overview or GET /v1/query/<name>)
-- Deployment operator (architecture.md §6.1 roles, one level above an org
-- owner): may read the funnel across every org (`scope=all`). Granted by
-- the operator themselves, in SQL -- there is deliberately no HTTP path
-- that sets it. Org admins/owners read their own org's funnel without it.
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS operator BOOLEAN NOT NULL DEFAULT false;

CREATE TABLE IF NOT EXISTS funnel_steps (
    subject_kind TEXT NOT NULL,
    subject_id   TEXT NOT NULL,
    step         TEXT NOT NULL,
    at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (subject_kind, subject_id, step)
);

-- Backfill from what the deployment already knew before this migration, so
-- the funnel doesn't start empty: every existing device is a host that
-- finished login (at its first token), and every host that has ever sent
-- events reached first_events (at its earliest event).
INSERT INTO funnel_steps (subject_kind, subject_id, step, at)
SELECT 'host', host_id, 'login_done', min(created_at) FROM devices GROUP BY host_id
ON CONFLICT DO NOTHING;
INSERT INTO funnel_steps (subject_kind, subject_id, step, at)
SELECT 'host', host_id, 'first_events', to_timestamp(min(ts) / 1000.0) FROM events GROUP BY host_id
ON CONFLICT DO NOTHING;
