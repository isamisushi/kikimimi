-- Non-superuser role the server's RLS-scoped request pool connects as
-- (architecture.md §11 "テナント分離"). Idempotent: creates the role if
-- missing, otherwise resyncs its password to the current
-- GURU_APP_DB_PASSWORD env value (the {{APP_PASSWORD}} placeholder below is
-- substituted by the Rust migration runner before this file is executed —
-- it never appears in the file on disk with a real secret in it).
--
-- `guru_app` is a role global to the whole Postgres *cluster*, not scoped to
-- one database, so this migration can legitimately run concurrently from
-- several different databases at once (e.g. the test suite, which migrates
-- a fresh database per test). The nested BEGIN/EXCEPTION below turns the
-- CREATE-then-ALTER check into something safe under that race: if another
-- session's CREATE ROLE commits between our catalog check and our own
-- CREATE ROLE, Postgres raises `duplicate_object`, caught here and treated
-- the same as the role having already existed (fall back to ALTER ROLE).
DO $do$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'guru_app') THEN
        BEGIN
            CREATE ROLE guru_app LOGIN PASSWORD {{APP_PASSWORD}};
        EXCEPTION WHEN duplicate_object THEN
            ALTER ROLE guru_app WITH LOGIN PASSWORD {{APP_PASSWORD}};
        END;
    ELSE
        ALTER ROLE guru_app WITH LOGIN PASSWORD {{APP_PASSWORD}};
    END IF;
END
$do$;
