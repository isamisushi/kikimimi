-- Persisted struggle-pattern detections (architecture.md §7.2 "苦戦検知 (pattern
-- library)", KKM-9). A background scanner (crates/cloud/src/patterns.rs) runs the
-- detection SQL per (org, dt) and writes one row per incident here, so the
-- ranking / before-after views (§7.3) read a table instead of re-deriving
-- everything from `events` on every request.
--
-- `pattern_hits` is org-scoped exactly like `events`: same RLS policy on
-- `org_id`, same `kikimimi_app` role, so `GET /v1/query/patterns` and the web
-- endpoints can never see another tenant's detections no matter what SQL they
-- run. The scanner writes through the same RLS-scoped transaction.
--
-- Rescans are idempotent: the primary key identifies an incident stably across
-- runs (`hit_key` is derived from the anchoring event ids), so a late-arriving
-- event (offline resend, ephemeral VM) updates the row in place and keeps
-- `first_detected_at`; incidents that vanish on rescan (their events were
-- deduped away) are deleted by `scan_seq`.

CREATE TABLE IF NOT EXISTS pattern_hits (
    org_id            UUID   NOT NULL REFERENCES orgs(id),
    dt                TEXT   NOT NULL,
    session_id        TEXT   NOT NULL,
    pattern_id        TEXT   NOT NULL,
    subject           TEXT   NOT NULL,
    hit_key           TEXT   NOT NULL,
    first_ts          BIGINT NOT NULL,
    last_ts           BIGINT NOT NULL,
    incidents         BIGINT NOT NULL DEFAULT 1,
    wasted_tokens_est BIGINT,
    detail            JSONB,
    scan_seq          BIGINT NOT NULL,
    first_detected_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, dt, session_id, pattern_id, subject, hit_key)
);

CREATE INDEX IF NOT EXISTS pattern_hits_org_dt_idx ON pattern_hits (org_id, dt);
CREATE INDEX IF NOT EXISTS pattern_hits_org_pattern_subject_idx ON pattern_hits (org_id, pattern_id, subject);

ALTER TABLE pattern_hits ENABLE ROW LEVEL SECURITY;
ALTER TABLE pattern_hits FORCE ROW LEVEL SECURITY;

DROP POLICY IF EXISTS pattern_hits_org_isolation ON pattern_hits;
CREATE POLICY pattern_hits_org_isolation ON pattern_hits
    USING (org_id = current_setting('app.org_id')::uuid)
    WITH CHECK (org_id = current_setting('app.org_id')::uuid);

GRANT SELECT, INSERT, UPDATE, DELETE ON pattern_hits TO kikimimi_app;

-- Scanner bookkeeping, superuser pool only (never exposed through the API).
-- One row per (org, dt) partition the scanner has looked at. `finalized` is the
-- §7.2 watermark: once a dt is older than the grace period (72h by default)
-- its last scan is the final one and later-arriving events are ignored
-- (counted in `late_events` so the gap stays visible, §7.1).
CREATE TABLE IF NOT EXISTS pattern_scan_state (
    org_id          UUID   NOT NULL REFERENCES orgs(id),
    dt              TEXT   NOT NULL,
    scan_seq        BIGINT NOT NULL,
    max_received_at TIMESTAMPTZ NOT NULL,
    finalized       BOOLEAN NOT NULL DEFAULT false,
    late_events     BIGINT NOT NULL DEFAULT 0,
    hits            BIGINT NOT NULL DEFAULT 0,
    scanned_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, dt)
);
