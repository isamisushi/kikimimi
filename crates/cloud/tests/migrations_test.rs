mod support;

use support::{SpawnOpts, TestApp};

#[tokio::test]
async fn migrations_are_idempotent_and_create_a_migrations_table() {
    let app = TestApp::spawn(SpawnOpts::default()).await;

    let rows: Vec<(String,)> = sqlx::query_as("SELECT name FROM _migrations ORDER BY name")
        .fetch_all(&app.state.pools.superuser)
        .await
        .expect("select _migrations");
    let names: Vec<String> = rows.into_iter().map(|(n,)| n).collect();
    assert_eq!(
        names,
        vec![
            "0001_core".to_string(),
            "0002_app_role".to_string(),
            "0003_rls".to_string(),
            "0004_events_org_scoped_pk".to_string(),
            "0005_invite_attempts".to_string(),
            "0006_web_sessions".to_string(),
            "0007_account_model".to_string(),
        ]
    );

    // Re-running migrations against an already-migrated database must be a
    // no-op, not an error (task requirement: "migrations idempotent").
    kikimimi_cloud::db::run_migrations(&app.state.pools.superuser, support::TEST_APP_DB_PASSWORD)
        .await
        .expect("second migration run must succeed");
    kikimimi_cloud::db::run_migrations(&app.state.pools.superuser, support::TEST_APP_DB_PASSWORD)
        .await
        .expect("third migration run must succeed");

    let rows_after: Vec<(String,)> = sqlx::query_as("SELECT name FROM _migrations ORDER BY name")
        .fetch_all(&app.state.pools.superuser)
        .await
        .expect("select _migrations again");
    assert_eq!(rows_after.len(), 7, "no duplicate/new rows from re-running");

    // The server (and its kikimimi_app pool) must still work after re-migrating.
    let resp = reqwest::get(format!("{}/healthz", app.base_url))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    app.teardown().await;
}
