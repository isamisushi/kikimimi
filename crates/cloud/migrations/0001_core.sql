-- Core tables: accounts / orgs / org_members / devices / device_codes / events.
-- Accessed via the SUPERUSER pool only, except `events` which the `guru_app`
-- role (created in 0002_app_role.sql) also reads/writes under RLS.

CREATE TABLE IF NOT EXISTS accounts (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    email      TEXT NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS orgs (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name       TEXT NOT NULL,
    personal   BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS org_members (
    org_id     UUID NOT NULL REFERENCES orgs(id),
    account_id UUID NOT NULL REFERENCES accounts(id),
    role       TEXT NOT NULL DEFAULT 'member',
    PRIMARY KEY (org_id, account_id)
);

CREATE TABLE IF NOT EXISTS devices (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id       UUID NOT NULL REFERENCES orgs(id),
    account_id   UUID NOT NULL REFERENCES accounts(id),
    host_id      TEXT NOT NULL,
    hostname     TEXT,
    token_hash   BYTEA NOT NULL UNIQUE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at TIMESTAMPTZ,
    revoked      BOOLEAN NOT NULL DEFAULT false
);

-- Device authorization grant (RFC 8628-ish) flow state. Deliberately has no
-- token column: the plaintext token only ever exists transiently, minted
-- inside POST /v1/device/token at the moment `approved` is first observed
-- true, then the row is deleted so it can't be materialized twice.
CREATE TABLE IF NOT EXISTS device_codes (
    device_code   TEXT PRIMARY KEY,
    user_code     TEXT NOT NULL UNIQUE,
    host_id       TEXT NOT NULL,
    hostname      TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    approved      BOOLEAN NOT NULL DEFAULT false,
    account_email TEXT,
    expires_at    TIMESTAMPTZ NOT NULL
);

-- guru.v1 events (docs/design/architecture.md §5.1 / guru_schema::COLUMNS).
-- Column order and types mirror guru_schema::COLUMNS exactly, except `org_id`
-- which the API contract overrides: the client-submitted value is always
-- ignored and replaced with the authoritated org from the bearer token, so
-- here it is UUID NOT NULL (not the nullable TEXT the client-side schema
-- documents) and is the column Row-Level Security enforces on.
CREATE TABLE IF NOT EXISTS events (
    event_id             TEXT PRIMARY KEY,
    ts                   BIGINT NOT NULL,
    dt                   TEXT NOT NULL,
    org_id               UUID NOT NULL REFERENCES orgs(id),
    team_id              TEXT,
    user_id              TEXT,
    user_id_source       TEXT,
    host_id              TEXT NOT NULL,
    env_kind             TEXT,
    os                   TEXT,
    agent                TEXT NOT NULL,
    agent_version        TEXT,
    session_id           TEXT,
    parent_session_id    TEXT,
    turn_id              TEXT,
    cwd_hash             TEXT,
    repo                 TEXT,
    source               TEXT NOT NULL,
    correlation_key      TEXT,
    correlation_confidence TEXT,
    event_type           TEXT NOT NULL,
    tool_name            TEXT,
    tool_kind            TEXT,
    mcp_server           TEXT,
    mcp_tool             TEXT,
    duration_ms          BIGINT,
    success              BOOLEAN,
    error_type           TEXT,
    decision             TEXT,
    decision_source      TEXT,
    provider             TEXT,
    model                TEXT,
    effort               TEXT,
    thinking             BOOLEAN,
    input_tokens         BIGINT,
    output_tokens        BIGINT,
    cache_read_tokens    BIGINT,
    cache_write_tokens   BIGINT,
    reasoning_tokens     BIGINT,
    cost_usd             DOUBLE PRECISION,
    usage_source         TEXT,
    tool_input_json      TEXT,
    tool_output_excerpt  TEXT,
    prompt_text          TEXT,
    redaction_applied    BOOLEAN,
    received_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS events_org_ts_idx ON events (org_id, ts);
CREATE INDEX IF NOT EXISTS events_org_dt_idx ON events (org_id, dt);
