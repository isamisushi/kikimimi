//! Two Postgres pools (architecture.md §6 "認証と権限", §11 "テナント分離"):
//!
//! - [`Pools::superuser`]: the DSN from `DATABASE_URL` as-is. Runs migrations at
//!   startup and is the *only* pool that ever touches `accounts` / `orgs` /
//!   `memberships` / `org_invites` / `audit_log` / `devices` / `device_codes`
//!   (the device.rs / web.rs / github.rs / orgs.rs auth+org endpoints).
//! - [`Pools::app`]: same host/port/database, but connects as the non-superuser
//!   `kikimimi_app` role created by the `0002_app_role.sql` migration. Every authed
//!   request (ingest/query/export) runs its DB work inside a transaction opened
//!   on this pool that starts with `SET LOCAL app.org_id = '<org from token>'`,
//!   which the Row-Level Security policy on `events` (0003_rls.sql) enforces.
//!   `kikimimi_app` has no grants at all on the auth tables, so a bug in a query
//!   handler cannot leak them even in principle.

use anyhow::Context;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{query, raw_sql, AssertSqlSafe, PgPool};
use std::str::FromStr;
use uuid::Uuid;

#[derive(Clone)]
pub struct Pools {
    pub superuser: PgPool,
    pub app: PgPool,
}

impl Pools {
    /// Connects the superuser pool, runs migrations on it (which is what
    /// creates/refreshes the `kikimimi_app` role — see `0002_app_role.sql`), and
    /// only then connects the `app` pool. Ordering matters: connecting `app`
    /// before migrations have run would try to authenticate as a role that
    /// doesn't exist yet on a fresh database.
    pub async fn connect(database_url: &str, app_db_password: &str) -> anyhow::Result<Self> {
        let superuser = PgPoolOptions::new()
            .max_connections(10)
            .connect(database_url)
            .await
            .with_context(|| {
                format!("connecting superuser pool to {}", redact_dsn(database_url))
            })?;

        run_migrations(&superuser, app_db_password)
            .await
            .context("running migrations")?;

        let superuser_opts = PgConnectOptions::from_str(database_url)
            .with_context(|| format!("parsing DATABASE_URL {}", redact_dsn(database_url)))?;
        let app_opts = superuser_opts
            .username("kikimimi_app")
            .password(app_db_password);
        let app = PgPoolOptions::new()
            .max_connections(10)
            .connect_with(app_opts)
            .await
            .context("connecting kikimimi_app pool")?;

        Ok(Self { superuser, app })
    }

    /// Begins a transaction on the `app` (RLS-scoped) pool and immediately
    /// pins it to `org_id` via `SET LOCAL app.org_id`. Every query run on the
    /// returned transaction only ever sees rows for this org (events' RLS
    /// policy: `org_id = current_setting('app.org_id')::uuid`, enforced even
    /// against `kikimimi_app` itself thanks to `FORCE ROW LEVEL SECURITY`).
    ///
    /// `org_id`'s `Display` impl only ever emits `[0-9a-f-]`, so interpolating
    /// it directly into the `SET LOCAL` text is safe — Postgres does not
    /// support bind parameters in `SET` statements at all.
    pub async fn org_scoped_tx(
        &self,
        org_id: Uuid,
    ) -> anyhow::Result<sqlx::Transaction<'static, sqlx::Postgres>> {
        let mut tx = self.app.begin().await.context("beginning app tx")?;
        query(AssertSqlSafe(format!("SET LOCAL app.org_id = '{org_id}'")))
            .execute(&mut *tx)
            .await
            .context("setting app.org_id")?;
        Ok(tx)
    }
}

/// Migrations run once at startup on the superuser pool, tracked in a
/// `_migrations` table (name, applied_at). Idempotent: safe to run against an
/// already-migrated database (each `.sql` file's statements use `IF NOT
/// EXISTS` / `DROP ... IF EXISTS` + `CREATE`, so re-applying a file that was
/// somehow re-listed is also harmless — though the `_migrations` table means
/// that in practice never happens).
///
/// `0002_app_role.sql` contains a `{{APP_PASSWORD}}` placeholder that gets
/// substituted with `app_db_password` (dollar-quoted, so no escaping footguns)
/// before being sent — the file on disk never has the real secret in it.
///
/// After the loop, an unconditional `ALTER ROLE kikimimi_app WITH PASSWORD`
/// reconciles the role's password with `app_db_password` on *every* boot, not
/// just the first (issue #3) — see the comment at that call site for why.
pub async fn run_migrations(pool: &PgPool, app_db_password: &str) -> anyhow::Result<()> {
    const MIGRATIONS: &[(&str, &str)] = &[
        ("0001_core", include_str!("../migrations/0001_core.sql")),
        (
            "0002_app_role",
            include_str!("../migrations/0002_app_role.sql"),
        ),
        ("0003_rls", include_str!("../migrations/0003_rls.sql")),
        (
            "0004_events_org_scoped_pk",
            include_str!("../migrations/0004_events_org_scoped_pk.sql"),
        ),
        (
            "0005_invite_attempts",
            include_str!("../migrations/0005_invite_attempts.sql"),
        ),
        (
            "0006_web_sessions",
            include_str!("../migrations/0006_web_sessions.sql"),
        ),
        (
            "0007_account_model",
            include_str!("../migrations/0007_account_model.sql"),
        ),
        (
            "0008_skill_name",
            include_str!("../migrations/0008_skill_name.sql"),
        ),
        (
            "0009_configured_mcp_servers",
            include_str!("../migrations/0009_configured_mcp_servers.sql"),
        ),
    ];

    raw_sql(
        "CREATE TABLE IF NOT EXISTS _migrations (\
            name TEXT PRIMARY KEY, \
            applied_at TIMESTAMPTZ NOT NULL DEFAULT now()\
        )",
    )
    .execute(pool)
    .await
    .context("creating _migrations table")?;

    for (name, sql) in MIGRATIONS {
        let already: Option<(String,)> =
            sqlx::query_as("SELECT name FROM _migrations WHERE name = $1")
                .bind(name)
                .fetch_optional(pool)
                .await
                .with_context(|| format!("checking whether migration {name} already applied"))?;
        if already.is_some() {
            continue;
        }

        // Dollar-quote the password so arbitrary characters in it (quotes,
        // backslashes) can never break out of the SQL string literal.
        let rendered = sql.replace(
            "{{APP_PASSWORD}}",
            &format!("$kikimimi_app_pw${app_db_password}$kikimimi_app_pw$"),
        );
        // `kikimimi_app` (0002_app_role.sql) is a role global to the whole
        // cluster, not scoped to one database. When several databases are
        // migrated at once (every test in this crate's suite does exactly
        // that, each against its own fresh database), Postgres can raise a
        // transient `tuple concurrently updated` on the shared `pg_authid`
        // catalog row even with the duplicate_object handling already in
        // that file. All these migrations are idempotent, so retrying a few
        // times with a short backoff is safe and clears the race.
        apply_with_retry(pool, AssertSqlSafe(rendered))
            .await
            .with_context(|| format!("applying migration {name}"))?;

        query("INSERT INTO _migrations (name) VALUES ($1)")
            .bind(name)
            .execute(pool)
            .await
            .with_context(|| format!("recording migration {name} as applied"))?;
    }

    // Issue #3: 0002_app_role only *creates* the role, and (like every
    // migration) only ever runs once per database — the `_migrations` guard
    // above skips it on every later boot. So nothing previously re-synced
    // the role's password after that first run. After the guru -> kikimimi
    // rename this meant a database migrated under the old role name kept
    // that old role's password forever: a freshly staged
    // KIKIMIMI_APP_DB_PASSWORD no longer matched it, and every connection on
    // the `app` pool crash-looped with "password authentication failed".
    // Running this ALTER ROLE unconditionally, outside the `_migrations`
    // gate, on every boot means a rotated secret (or a renamed role, same
    // fix) always heals itself instead of needing a manual GRANT/ALTER like
    // the one this repo's internal/ops.md addendum records having to do by
    // hand.
    let alter_role_sql = format!(
        "ALTER ROLE \"kikimimi_app\" WITH PASSWORD {}",
        pg_escape_literal(app_db_password)
    );
    match apply_with_retry(pool, AssertSqlSafe(alter_role_sql)).await {
        Ok(()) => {
            tracing::info!("kikimimi_app role password reconciled with KIKIMIMI_APP_DB_PASSWORD");
        }
        Err(e) => {
            // Fail-open (architecture.md §2.2): if the connecting user isn't
            // a superuser and doesn't own the role, ALTER ROLE fails with
            // insufficient_privilege. Before this fix, nothing tried to sync
            // the password past the first boot at all — proceeding anyway,
            // not crash-looping, was already the status quo for every boot
            // after the first — so a privilege error here must not become a
            // new hard failure that regresses that.
            tracing::warn!(
                error = %e,
                "could not reconcile kikimimi_app role password (insufficient privilege?) — continuing"
            );
        }
    }

    Ok(())
}

/// Escapes `value` as a Postgres `E''` (C-style escape) string literal, safe
/// to splice directly into a statement — `ALTER ROLE ... PASSWORD` chief
/// among them — that Postgres gives no bind-parameter support for at all.
/// Doubling both `'` and `\` and using the `E` prefix makes this correct
/// regardless of the server's `standard_conforming_strings` setting: per the
/// Postgres string literal grammar, an `E'...'` string *always* processes
/// backslash escapes, whereas a plain `'...'` literal only does when
/// `standard_conforming_strings` is off — so there is no server-config
/// assumption to get right or wrong here.
fn pg_escape_literal(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('\'', "''");
    format!("E'{escaped}'")
}

/// Runs one `raw_sql` statement, retrying a handful of times on failure with
/// a short randomized backoff. Only used for migrations, which are all
/// idempotent by construction, so a retry after a transient failure (e.g. the
/// concurrent-DDL-on-a-global-role race described above) is always safe.
async fn apply_with_retry(pool: &PgPool, sql: AssertSqlSafe<String>) -> Result<(), sqlx::Error> {
    const MAX_ATTEMPTS: u32 = 5;
    let mut last_err = None;
    for attempt in 0..MAX_ATTEMPTS {
        if attempt > 0 {
            let jitter_ms = 20 * attempt as u64 + (rand::random::<u64>() % 30);
            tokio::time::sleep(std::time::Duration::from_millis(jitter_ms)).await;
        }
        match raw_sql(AssertSqlSafe(sql.0.clone())).execute(pool).await {
            Ok(_) => return Ok(()),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.expect("loop runs at least once"))
}

/// Masks the password segment of a `postgres://user:pass@host:port/db` DSN
/// (security review: `DATABASE_URL`, password included, was being
/// interpolated straight into `anyhow::Context` strings — a startup
/// connection failure's error chain is printed in full, DSN included, by
/// `main`'s default `anyhow::Result` handler, which typically lands in
/// container/systemd logs). Best-effort: any DSN shape this doesn't
/// recognize (no `://`, no `@`, no `:` before the `@`) is returned
/// unchanged rather than panicking — never worse than the un-redacted
/// status quo, but redacts every DSN shape this codebase actually produces.
fn redact_dsn(url: &str) -> String {
    let Some(after_scheme) = url.find("://").map(|i| i + 3) else {
        return url.to_string();
    };
    let Some(at_idx) = url[after_scheme..].find('@').map(|i| after_scheme + i) else {
        return url.to_string();
    };
    let Some(colon_idx) = url[after_scheme..at_idx]
        .find(':')
        .map(|i| after_scheme + i)
    else {
        return url.to_string();
    };
    format!("{}:***{}", &url[..colon_idx], &url[at_idx..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_dsn_masks_the_password_only() {
        assert_eq!(
            redact_dsn("postgres://postgres:guru-dev@127.0.0.1:5433/guru"),
            "postgres://postgres:***@127.0.0.1:5433/guru"
        );
    }

    #[test]
    fn redact_dsn_handles_password_with_special_characters() {
        assert_eq!(
            redact_dsn("postgres://u:p@ss:w0rd!@host/db"),
            // The *first* '@' after the scheme ends the userinfo section per
            // the URL grammar, so a literal '@' in the password isn't
            // representable un-escaped in a real DSN — this DSN is already
            // ambiguous input, and redaction still masks up to that first
            // '@', which is the conservative (more redaction, not less) side
            // to fail on.
            "postgres://u:***@ss:w0rd!@host/db"
        );
    }

    #[test]
    fn redact_dsn_leaves_unrecognized_shapes_unchanged() {
        assert_eq!(redact_dsn("not-a-dsn"), "not-a-dsn");
        assert_eq!(redact_dsn("postgres://host/db"), "postgres://host/db");
    }

    #[test]
    fn pg_escape_literal_plain_value() {
        assert_eq!(pg_escape_literal("hunter2"), "E'hunter2'");
    }

    #[test]
    fn pg_escape_literal_doubles_single_quotes() {
        assert_eq!(pg_escape_literal("o'brien"), "E'o''brien'");
    }

    #[test]
    fn pg_escape_literal_doubles_backslashes() {
        assert_eq!(pg_escape_literal(r"pa\ss"), r"E'pa\\ss'");
    }

    #[test]
    fn pg_escape_literal_handles_quotes_and_backslashes_together() {
        // A pathological password an attacker (or a very unlucky random
        // generator) could hand us: mixes both special characters, including
        // a backslash immediately followed by a quote, which is exactly the
        // sequence that would matter if standard_conforming_strings were off
        // and this weren't an E'' literal.
        assert_eq!(pg_escape_literal(r"a\'b"), r"E'a\\''b'");
    }
}
