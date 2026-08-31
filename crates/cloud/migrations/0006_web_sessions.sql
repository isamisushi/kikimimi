-- Server-side sessions for the hosted web UI login flow (POST /web/login,
-- architecture.md §8 / WEB API CONTRACT). Distinct from `devices` (CLI
-- bearer tokens): a `web_sessions` row is created by `POST /web/login` and
-- looked up by the `guru_session` cookie on every `/web/q/*` request and
-- `GET /web/me` (see web.rs's `WebSessionContext`).
--
-- Reachable only via the SUPERUSER pool (module docs pattern from
-- 0001_core.sql: accounts/orgs/org_members/devices/device_codes are never
-- touched by the RLS-scoped `guru_app` pool -- `web_sessions` joins the same
-- club). `guru_app` gets no grants on this table at all (see 0003_rls.sql
-- for the equivalent reasoning re: `events`).
CREATE TABLE IF NOT EXISTS web_sessions (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    account_id UUID NOT NULL REFERENCES accounts(id),
    org_id     UUID NOT NULL REFERENCES orgs(id),
    -- sha256(token), same as devices.token_hash -- the plaintext (opaque
    -- 43-char base64url token, `auth::generate_token`) only ever exists
    -- transiently: handed to the browser once as the `guru_session` cookie
    -- value, in the `POST /web/login` response, then never stored.
    token_hash BYTEA NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- 30 days from creation (WEB API CONTRACT). Checked in `WebSessionContext`
    -- on every request -- there is no background sweeper for expired rows
    -- yet (Stage 0; a stale row is harmless, just dead weight in the table).
    expires_at TIMESTAMPTZ NOT NULL,
    -- Set true by POST /web/logout, so a cookie copied out of a browser
    -- before logout can't be replayed afterwards even though the row itself
    -- still exists (kept, not deleted, for audit purposes).
    revoked    BOOLEAN NOT NULL DEFAULT false
);

CREATE INDEX IF NOT EXISTS web_sessions_account_id_idx ON web_sessions (account_id);
