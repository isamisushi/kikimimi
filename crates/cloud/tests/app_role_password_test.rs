//! Issue #3: reconciling the `kikimimi_app` role's password on every boot,
//! not just the first.
//!
//! `0002_app_role.sql` only ever runs once per database — like every
//! migration, it's guarded by the `_migrations` table (see `db.rs`'s
//! `run_migrations`) — so on its own it cannot heal a password mismatch
//! introduced *after* the first boot. That's exactly what happened during
//! the guru -> kikimimi rename: the app role got recreated under its new
//! name, but an already-migrated database still had the old role's old
//! password, so a freshly staged `KIKIMIMI_APP_DB_PASSWORD` no longer
//! matched and the `app` pool crash-looped on "password authentication
//! failed". This drives `run_migrations` directly against a real database
//! with two different passwords and proves only the *second* one still
//! authenticates afterward.

mod support;

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::str::FromStr;

/// Opens (and immediately hands back, unopened-pool-closed-by-caller) a
/// two-connection pool authenticating as `kikimimi_app` with `password`
/// against `database_url` — mirrors `support::TestApp::connect_as_app_role`,
/// but parameterized over the password since this test needs two different
/// ones against the *same* database, which that fixed-password helper can't
/// do.
async fn try_connect_as_app_role(
    database_url: &str,
    password: &str,
) -> Result<sqlx::PgPool, sqlx::Error> {
    let opts = PgConnectOptions::from_str(database_url)
        .expect("parse database_url")
        .username("kikimimi_app")
        .password(password);
    PgPoolOptions::new()
        .max_connections(2)
        .connect_with(opts)
        .await
}

#[tokio::test]
async fn boot_reconciles_app_role_password_with_current_env_value() {
    let db_name = format!("kikimimi_test_{}", uuid::Uuid::new_v4().simple());
    support::create_database(&db_name).await;
    let database_url = support::with_database(&support::base_database_url(), &db_name);

    const PASSWORD_A: &str = "kikimimi-app-pw-a-9f3c";
    const PASSWORD_B: &str = "kikimimi-app-pw-b-71ee";

    // Superuser pool driving run_migrations directly — this test is about
    // run_migrations specifically, so it skips TestApp/AppState/the HTTP
    // server entirely.
    let superuser = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
        .expect("connect superuser pool");

    kikimimi_cloud::db::run_migrations(&superuser, PASSWORD_A)
        .await
        .expect("first migration run (password A) must succeed");

    // Sanity check: password A authenticates as kikimimi_app right after the
    // role is first created.
    let pool_a = try_connect_as_app_role(&database_url, PASSWORD_A)
        .await
        .expect("password A must work right after the first boot");
    pool_a.close().await;

    // Second boot with a *different* password. `0002_app_role` is already
    // recorded in `_migrations` and so is skipped entirely on this run —
    // only the unconditional `ALTER ROLE` this fix adds at the end of
    // run_migrations can be what reconciles the password below.
    kikimimi_cloud::db::run_migrations(&superuser, PASSWORD_B)
        .await
        .expect("second migration run (password B) must succeed");

    // New connections must now use B ...
    let pool_b = try_connect_as_app_role(&database_url, PASSWORD_B)
        .await
        .expect("password B must work once the second boot reconciles it");
    let row: (i32,) = sqlx::query_as("SELECT 1")
        .fetch_one(&pool_b)
        .await
        .expect("SELECT 1 as kikimimi_app with the reconciled password");
    assert_eq!(row.0, 1);
    pool_b.close().await;

    // ... and A must no longer authenticate at all.
    let err = try_connect_as_app_role(&database_url, PASSWORD_A)
        .await
        .expect_err("password A must have been superseded by the reconciliation");
    let msg = err.to_string();
    assert!(
        msg.contains("password authentication failed") || msg.contains("28P01"),
        "expected a password-authentication error for the stale password, got: {msg}"
    );

    superuser.close().await;
    support::drop_database(&db_name).await;
}
