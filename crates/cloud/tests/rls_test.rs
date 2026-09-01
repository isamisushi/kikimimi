mod support;

use support::{gzip, ingest_body_bytes, login_as, sample_event, SpawnOpts, TestApp};

#[tokio::test]
async fn cross_tenant_isolation_via_query_api_and_direct_app_role_connection() {
    let app = TestApp::spawn(SpawnOpts::default()).await; // autoapprove OFF: real two-tenant login
    let client = reqwest::Client::new();

    let org_a = login_as(&client, &app.base_url, "host-a", "alice@example.com").await;
    let org_b = login_as(&client, &app.base_url, "host-b", "bob@example.com").await;
    assert_ne!(org_a.org_id, org_b.org_id, "sanity: two distinct orgs");

    // Ingest one event into org A only.
    let ev_a = sample_event("secret-a", "host-a", "sess-a");
    let payload_a = gzip(&ingest_body_bytes(&[ev_a]));
    let resp = client
        .post(format!("{}/v1/events", app.base_url))
        .bearer_auth(&org_a.token)
        .header("Content-Encoding", "gzip")
        .body(payload_a)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // --- Path 1: org B's own bearer token, through GET /v1/query/today ---
    let resp = client
        .get(format!(
            "{}/v1/query/today?dt_from=2000-01-01&dt_to=2100-01-01",
            app.base_url
        ))
        .bearer_auth(&org_b.token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let rows = body["rows"].as_array().unwrap();
    let total_events: i64 = rows.iter().map(|r| r[0].as_i64().unwrap_or(0)).sum();
    assert_eq!(
        total_events, 0,
        "org B must see zero of org A's rows via the query API, got {body:?}"
    );

    // Org A, same query, must see its own row (sanity check the query isn't
    // just broken/always-empty).
    let resp = client
        .get(format!(
            "{}/v1/query/today?dt_from=2000-01-01&dt_to=2100-01-01",
            app.base_url
        ))
        .bearer_auth(&org_a.token)
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let rows = body["rows"].as_array().unwrap();
    let total_events: i64 = rows.iter().map(|r| r[0].as_i64().unwrap_or(0)).sum();
    assert_eq!(total_events, 1, "org A must see its own row: {body:?}");

    // --- Path 2: a direct kikimimi_app connection, RLS enforced at the DB level ---
    let app_pool = app.connect_as_app_role().await;
    let mut tx = app_pool.begin().await.unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "SET LOCAL app.org_id = '{}'",
        org_b.org_id
    )))
    .execute(&mut *tx)
    .await
    .unwrap();
    let rows: Vec<(String,)> = sqlx::query_as("SELECT event_id FROM events")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert!(
        rows.is_empty(),
        "a direct kikimimi_app connection scoped to org B must not see org A's event, got {rows:?}"
    );
    tx.rollback().await.unwrap();

    // Same direct connection, scoped to org A this time, does see the row —
    // proves the emptiness above is RLS, not a broken query.
    let mut tx = app_pool.begin().await.unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "SET LOCAL app.org_id = '{}'",
        org_a.org_id
    )))
    .execute(&mut *tx)
    .await
    .unwrap();
    let rows: Vec<(String,)> = sqlx::query_as("SELECT event_id FROM events")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert_eq!(rows, vec![("secret-a".to_string(),)]);
    tx.rollback().await.unwrap();
    app_pool.close().await;

    app.teardown().await;
}
