//! Migration backfill (`migrations/0007_account_model.sql`) exercised
//! against a database seeded with a *pre-existing* account/org/org_members
//! row shaped exactly like migrations 0001-0006 produced it -- i.e. before
//! `kind`/`slug`/`memberships`/`github_id` existed at all. This is the "real
//! upgrade" scenario the backfill blocks in 0007 exist for, as opposed to
//! every other test in this crate, which only ever sees a brand-new database
//! that runs all seven migrations from a clean slate.
//!
//! Approach: apply migrations 0001-0006 by hand (their raw SQL, same files
//! `db.rs` embeds), seed one legacy account + personal org + org_members
//! row directly, record those six migrations as already-applied in
//! `_migrations`, then call the real (production) `kikimimi_cloud::db::
//! run_migrations`, which sees 0001-0006 done and applies exactly 0007 --
//! so the backfill logic under test is the actual shipped SQL, not a
//! reimplementation of it.

mod support;

use sqlx::{postgres::PgPoolOptions, PgPool};
use uuid::Uuid;

const MIGRATIONS_0001_TO_0006: &[(&str, &str)] = &[
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
];

async fn apply_legacy_schema(pool: &PgPool) {
    sqlx::raw_sql(
        "CREATE TABLE IF NOT EXISTS _migrations (\
            name TEXT PRIMARY KEY, \
            applied_at TIMESTAMPTZ NOT NULL DEFAULT now()\
        )",
    )
    .execute(pool)
    .await
    .expect("create _migrations");

    for (name, sql) in MIGRATIONS_0001_TO_0006 {
        let rendered = sql.replace(
            "{{APP_PASSWORD}}",
            &format!(
                "$kikimimi_app_pw${}$kikimimi_app_pw$",
                support::TEST_APP_DB_PASSWORD
            ),
        );
        sqlx::raw_sql(sqlx::AssertSqlSafe(rendered))
            .execute(pool)
            .await
            .unwrap_or_else(|e| panic!("applying legacy migration {name}: {e}"));
        sqlx::query("INSERT INTO _migrations (name) VALUES ($1)")
            .bind(name)
            .execute(pool)
            .await
            .unwrap_or_else(|e| panic!("recording legacy migration {name}: {e}"));
    }
}

#[tokio::test]
async fn backfill_gives_a_pre_existing_account_a_kind_slug_and_owner_membership() {
    let db_name = format!("kikimimi_backfill_{}", Uuid::new_v4().simple());
    support::create_database(&db_name).await;
    let database_url = support::with_database(&support::base_database_url(), &db_name);

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
        .expect("connect to fresh database");

    apply_legacy_schema(&pool).await;

    // Seed a legacy account + personal org + org_members row, exactly the
    // shape 0001-0006 produced (no kind/slug/github_id/memberships yet).
    let account_id: (Uuid,) =
        sqlx::query_as("INSERT INTO accounts (email) VALUES ($1) RETURNING id")
            .bind("legacy-user@example.com")
            .fetch_one(&pool)
            .await
            .expect("seed legacy account");
    let org_id: (Uuid,) =
        sqlx::query_as("INSERT INTO orgs (name, personal) VALUES ($1, true) RETURNING id")
            .bind("legacy-user@example.com (personal)")
            .fetch_one(&pool)
            .await
            .expect("seed legacy org");
    sqlx::query("INSERT INTO org_members (org_id, account_id, role) VALUES ($1, $2, 'owner')")
        .bind(org_id.0)
        .bind(account_id.0)
        .execute(&pool)
        .await
        .expect("seed legacy org_members row");

    // Sanity: the pre-migration shape really has none of 0007's columns/
    // tables yet.
    let pre_has_memberships: (bool,) =
        sqlx::query_as("SELECT to_regclass('memberships') IS NOT NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        !pre_has_memberships.0,
        "memberships must not exist before 0007 runs"
    );

    // Now run the real migration runner -- it sees 0001-0006 already
    // recorded and applies exactly 0007.
    kikimimi_cloud::db::run_migrations(&pool, support::TEST_APP_DB_PASSWORD)
        .await
        .expect("0007_account_model must apply cleanly over the seeded legacy schema");

    let applied: Vec<(String,)> = sqlx::query_as("SELECT name FROM _migrations ORDER BY name")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(applied.last().unwrap().0, "0008_skill_name");
    assert_eq!(applied.len(), 8);

    // org_members is gone, memberships has the same row plus created_at.
    let post_has_org_members: (bool,) =
        sqlx::query_as("SELECT to_regclass('org_members') IS NOT NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        !post_has_org_members.0,
        "org_members must be renamed away, not left behind"
    );

    let membership: (String, chrono::DateTime<chrono::Utc>) = sqlx::query_as(
        "SELECT role, created_at FROM memberships WHERE account_id = $1 AND org_id = $2",
    )
    .bind(account_id.0)
    .bind(org_id.0)
    .fetch_one(&pool)
    .await
    .expect("the pre-existing owner membership survived the rename");
    assert_eq!(
        membership.0, "owner",
        "backfill: every existing account keeps its owner membership"
    );

    // orgs.kind/slug backfilled.
    let (kind, slug): (String, String) =
        sqlx::query_as("SELECT kind, slug FROM orgs WHERE id = $1")
            .bind(org_id.0)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(kind, "personal");
    assert!(
        slug.starts_with("legacy-user-"),
        "slug should derive from the email local-part: {slug}"
    );
    assert!(
        slug.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
        "slug must be url-safe: {slug}"
    );

    // accounts.github_id/github_login stay NULL for an account that has
    // never done GitHub OAuth.
    let (github_id, github_login): (Option<i64>, Option<String>) =
        sqlx::query_as("SELECT github_id, github_login FROM accounts WHERE id = $1")
            .bind(account_id.0)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(github_id.is_none());
    assert!(github_login.is_none());

    // org_invites / audit_log exist and are empty.
    let (invites, audit): (i64, i64) = {
        let i: (i64,) = sqlx::query_as("SELECT count(*) FROM org_invites")
            .fetch_one(&pool)
            .await
            .unwrap();
        let a: (i64,) = sqlx::query_as("SELECT count(*) FROM audit_log")
            .fetch_one(&pool)
            .await
            .unwrap();
        (i.0, a.0)
    };
    assert_eq!(invites, 0);
    assert_eq!(audit, 0);

    // Re-running migrations (idempotency, same guarantee as
    // migrations_test.rs) must be a no-op.
    kikimimi_cloud::db::run_migrations(&pool, support::TEST_APP_DB_PASSWORD)
        .await
        .expect("re-running migrations over the now-current schema must be a no-op");

    pool.close().await;
    support::drop_database(&db_name).await;
}
