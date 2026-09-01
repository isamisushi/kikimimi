-- Account model v2 (architecture.md §6.1, "2026-09-01 確定"): GitHub OAuth
-- identity on accounts, org kind + slug, a wider membership role set
-- (owner/admin/member/viewer), per-org invite links, and an audit log for
-- admin drilldowns. Superuser-pool only, same club as accounts/orgs/
-- org_members/devices/device_codes/web_sessions (0001_core.sql,
-- 0006_web_sessions.sql module docs) -- none of the tables touched below
-- ever get a `kikimimi_app` grant.
--
-- Everything here is additive/idempotent so it applies cleanly both to a
-- brand-new database (fresh off 0001-0006) and to one that already has real
-- accounts/orgs/org_members rows from before this migration existed -- the
-- backfill blocks below are exactly what makes the latter case safe.

-- ---------------------------------------------------------------------------
-- accounts: GitHub identity (nullable -- a pre-existing email-only account
-- keeps working until it links via GitHub OAuth, see github.rs).
-- ---------------------------------------------------------------------------
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS github_id BIGINT;
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS github_login TEXT;

DO $do$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'accounts_github_id_key') THEN
        ALTER TABLE accounts ADD CONSTRAINT accounts_github_id_key UNIQUE (github_id);
    END IF;
END
$do$;

-- ---------------------------------------------------------------------------
-- orgs: kind ('personal'|'team') + slug (unique, url-safe).
-- ---------------------------------------------------------------------------
ALTER TABLE orgs ADD COLUMN IF NOT EXISTS kind TEXT;

-- Backfill: every org that existed before this migration was created by
-- ensure_personal_org (the only org-creation path that existed), so it is a
-- personal org; `personal` (the pre-existing boolean) is kept in sync and
-- used here as the source of truth in case a future backfill needs it, but
-- in practice this always evaluates to 'personal' pre-migration.
UPDATE orgs SET kind = CASE WHEN personal THEN 'personal' ELSE 'team' END WHERE kind IS NULL;
ALTER TABLE orgs ALTER COLUMN kind SET NOT NULL;

DO $do$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'orgs_kind_check') THEN
        ALTER TABLE orgs ADD CONSTRAINT orgs_kind_check CHECK (kind IN ('personal', 'team'));
    END IF;
END
$do$;

ALTER TABLE orgs ADD COLUMN IF NOT EXISTS slug TEXT;

-- Backfill slug for pre-existing orgs from the owning account's email
-- local-part (sanitized to url-safe: runs of non-alphanumeric collapse to a
-- single hyphen, leading/trailing hyphens trimmed, lowercased), plus an
-- 8-hex-char suffix from the org's own id for uniqueness. `org_members` is
-- still the live table name at this point in the migration -- the rename to
-- `memberships` happens further down, after this backfill runs.
UPDATE orgs o
SET slug = NULLIF(
                trim(both '-' from lower(regexp_replace(split_part(a.email, '@', 1), '[^a-zA-Z0-9]+', '-', 'g'))),
                ''
            ) || '-' || substr(replace(o.id::text, '-', ''), 1, 8)
FROM org_members m
JOIN accounts a ON a.id = m.account_id
WHERE m.org_id = o.id AND o.slug IS NULL AND m.role = 'owner';

-- Fallback for any org still without a slug (no owner membership found, or
-- the sanitized local-part was empty) -- derive one from the org id alone so
-- the NOT NULL constraint below can never fail.
UPDATE orgs SET slug = 'org-' || substr(replace(id::text, '-', ''), 1, 12) WHERE slug IS NULL;

ALTER TABLE orgs ALTER COLUMN slug SET NOT NULL;

DO $do$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'orgs_slug_key') THEN
        ALTER TABLE orgs ADD CONSTRAINT orgs_slug_key UNIQUE (slug);
    END IF;
END
$do$;

-- ---------------------------------------------------------------------------
-- memberships (renamed from org_members): wider role set + created_at.
-- architecture.md §6.1's literal shape: "memberships(account_id, org_id,
-- role in owner/admin/member/viewer, created_at, PK(account_id,org_id))".
-- ---------------------------------------------------------------------------
DO $do$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_class WHERE relname = 'org_members')
       AND NOT EXISTS (SELECT 1 FROM pg_class WHERE relname = 'memberships') THEN
        ALTER TABLE org_members RENAME TO memberships;
    END IF;
END
$do$;

ALTER TABLE memberships ADD COLUMN IF NOT EXISTS created_at TIMESTAMPTZ NOT NULL DEFAULT now();

-- Backfill: "every existing account gets membership owner on its personal
-- org" -- already true (org_members already had an 'owner' row per personal
-- org from ensure_personal_org); this only normalizes any row that somehow
-- has a role outside the new enum so the CHECK constraint below can't fail.
UPDATE memberships SET role = 'owner' WHERE role NOT IN ('owner', 'admin', 'member', 'viewer');

DO $do$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'memberships_role_check') THEN
        ALTER TABLE memberships ADD CONSTRAINT memberships_role_check CHECK (role IN ('owner', 'admin', 'member', 'viewer'));
    END IF;
END
$do$;

-- Re-key the PK to (account_id, org_id) -- was (org_id, account_id) under
-- the old name (same column set, just matching the documented order).
DO $do$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'org_members_pkey') THEN
        ALTER TABLE memberships DROP CONSTRAINT org_members_pkey;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'memberships_pkey') THEN
        ALTER TABLE memberships ADD CONSTRAINT memberships_pkey PRIMARY KEY (account_id, org_id);
    END IF;
END
$do$;

-- ---------------------------------------------------------------------------
-- org_invites: role + expiry + use-count limited invite links
-- (POST /web/orgs/:slug/invites -> GET/POST /join/<token>). `token_hash` is
-- sha256(token), same convention as devices.token_hash/web_sessions.token_hash
-- -- the plaintext token is only ever handed to the admin who created the
-- invite (in the POST response), never stored.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS org_invites (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id     UUID NOT NULL REFERENCES orgs(id),
    role       TEXT NOT NULL CHECK (role IN ('owner', 'admin', 'member', 'viewer')),
    token_hash BYTEA NOT NULL UNIQUE,
    expires_at TIMESTAMPTZ NOT NULL,
    max_uses   INTEGER,
    uses       INTEGER NOT NULL DEFAULT 0,
    created_by UUID NOT NULL REFERENCES accounts(id),
    revoked    BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS org_invites_org_id_idx ON org_invites (org_id);

-- ---------------------------------------------------------------------------
-- audit_log: admin drilldown trail (architecture.md §11 "admin のドリルダウン
-- はロールで制限し、閲覧を監査ログに残す"). `actor`/`org_id`/`action`/`target`/`at`
-- per the account-model contract's literal column list.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS audit_log (
    id     BIGSERIAL PRIMARY KEY,
    actor  UUID NOT NULL REFERENCES accounts(id),
    org_id UUID NOT NULL REFERENCES orgs(id),
    action TEXT NOT NULL,
    target TEXT,
    at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS audit_log_org_id_idx ON audit_log (org_id);

-- ---------------------------------------------------------------------------
-- device_codes: an approval now binds directly to (account_id, org_id)
-- resolved from the approver's web session + chosen org (device.rs's
-- `/activate` no longer collects a raw email) instead of `account_email`.
-- `org_hint` carries `kikimimi login --org <slug>` through to the dropdown
-- (device::device_code / device::activate_get).
-- ---------------------------------------------------------------------------
ALTER TABLE device_codes ADD COLUMN IF NOT EXISTS account_id UUID REFERENCES accounts(id);
ALTER TABLE device_codes ADD COLUMN IF NOT EXISTS org_id UUID REFERENCES orgs(id);
ALTER TABLE device_codes ADD COLUMN IF NOT EXISTS org_hint TEXT;
