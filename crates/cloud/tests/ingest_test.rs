mod support;

use guru_schema::{Event, COLUMNS};
use support::{gzip, ingest_body_bytes, login_as, login_autoapprove, sample_event, SpawnOpts, TestApp};

async fn spawn_and_login(host_id: &str) -> (TestApp, reqwest::Client, support::Login) {
    let app = TestApp::spawn(SpawnOpts {
        dev_autoapprove: true,
        ..Default::default()
    })
    .await;
    let client = reqwest::Client::new();
    let login = login_autoapprove(&client, &app.base_url, host_id).await;
    (app, client, login)
}

#[tokio::test]
async fn ingest_reports_accepted_and_dedupes_a_resent_batch() {
    let (app, client, login) = spawn_and_login("host-ingest-1").await;

    let events = vec![
        sample_event("e1", "host-ingest-1", "sess-1"),
        sample_event("e2", "host-ingest-1", "sess-1"),
    ];
    let payload = gzip(&ingest_body_bytes(&events));

    let resp = client
        .post(format!("{}/v1/events", app.base_url))
        .bearer_auth(&login.token)
        .header("Content-Encoding", "gzip")
        .header("Content-Type", "application/json")
        .body(payload.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["accepted"], 2);
    assert_eq!(body["deduped"], 0);

    // Re-send the exact same batch: fully deduped by event_id, not an error.
    let resp2 = client
        .post(format!("{}/v1/events", app.base_url))
        .bearer_auth(&login.token)
        .header("Content-Encoding", "gzip")
        .header("Content-Type", "application/json")
        .body(payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp2.status(), 200);
    let body2: serde_json::Value = resp2.json().await.unwrap();
    assert_eq!(body2["accepted"], 0);
    assert_eq!(body2["deduped"], 2);

    app.teardown().await;
}

#[tokio::test]
async fn ingest_requires_gzip_content_encoding() {
    let (app, client, login) = spawn_and_login("host-ingest-2").await;
    let events = vec![sample_event("e1", "host-ingest-2", "sess-1")];
    let raw = ingest_body_bytes(&events); // NOT gzipped

    let resp = client
        .post(format!("{}/v1/events", app.base_url))
        .bearer_auth(&login.token)
        .header("Content-Type", "application/json")
        .body(raw)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    app.teardown().await;
}

#[tokio::test]
async fn ingest_rejects_batch_on_host_id_mismatch() {
    let (app, client, login) = spawn_and_login("host-real").await;

    // Token is bound to host-real; event claims a different host_id.
    let events = vec![sample_event("e1", "host-someone-else", "sess-1")];
    let payload = gzip(&ingest_body_bytes(&events));

    let resp = client
        .post(format!("{}/v1/events", app.base_url))
        .bearer_auth(&login.token)
        .header("Content-Encoding", "gzip")
        .body(payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 422);

    app.teardown().await;
}

#[tokio::test]
async fn ingest_rejects_batches_over_5000_events() {
    let (app, client, login) = spawn_and_login("host-oversize-count").await;

    let events: Vec<Event> = (0..5001)
        .map(|i| sample_event(&format!("e{i}"), "host-oversize-count", "sess-1"))
        .collect();
    let payload = gzip(&ingest_body_bytes(&events));

    let resp = client
        .post(format!("{}/v1/events", app.base_url))
        .bearer_auth(&login.token)
        .header("Content-Encoding", "gzip")
        .body(payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 413);

    app.teardown().await;
}

#[tokio::test]
async fn ingest_rejects_batches_over_32mb_decompressed() {
    let (app, client, login) = spawn_and_login("host-oversize-bytes").await;

    // One event with a ~33 MB field. Repetitive text compresses extremely
    // well, so the gzip body stays tiny (well under the 5 MB compressed
    // cap) and only the 32 MB *decompressed* cap can be what trips this.
    let mut ev = sample_event("e1", "host-oversize-bytes", "sess-1");
    ev.tool_input_json = Some("x".repeat(33 * 1024 * 1024));
    let payload = gzip(&ingest_body_bytes(&[ev]));
    assert!(
        payload.len() < 1024 * 1024,
        "test payload should compress to well under 1 MB, got {} bytes",
        payload.len()
    );

    let resp = client
        .post(format!("{}/v1/events", app.base_url))
        .bearer_auth(&login.token)
        .header("Content-Encoding", "gzip")
        .body(payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 413);

    app.teardown().await;
}

#[tokio::test]
async fn ingest_requires_a_bearer_token() {
    let app = TestApp::spawn(SpawnOpts {
        dev_autoapprove: true,
        ..Default::default()
    })
    .await;
    let client = reqwest::Client::new();
    let events = vec![sample_event("e1", "host-x", "sess-1")];
    let payload = gzip(&ingest_body_bytes(&events));

    let resp = client
        .post(format!("{}/v1/events", app.base_url))
        .header("Content-Encoding", "gzip")
        .body(payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    app.teardown().await;
}

#[tokio::test]
async fn ingest_defensively_nulls_body_columns() {
    let (app, client, login) = spawn_and_login("host-privacy").await;

    let ev = sample_event("e-privacy", "host-privacy", "sess-1");
    assert!(ev.tool_input_json.is_some(), "sanity: test event has body set");
    let payload = gzip(&ingest_body_bytes(&[ev]));

    let resp = client
        .post(format!("{}/v1/events", app.base_url))
        .bearer_auth(&login.token)
        .header("Content-Encoding", "gzip")
        .body(payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Read the row back directly (bypassing the query API) to confirm the
    // body columns are NULL in storage, not just omitted from some response.
    let app_pool = app.connect_as_app_role().await;
    let mut tx = app_pool.begin().await.unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!("SET LOCAL app.org_id = '{}'", login.org_id)))
        .execute(&mut *tx)
        .await
        .unwrap();
    let row: (Option<String>, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT tool_input_json, tool_output_excerpt, prompt_text FROM events WHERE event_id = $1",
    )
    .bind("e-privacy")
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(row, (None, None, None), "body columns must be NULLed by the server");
    tx.commit().await.unwrap();
    app_pool.close().await;

    app.teardown().await;
}

/// Security review: the semaphore-driven 429 path (`ingest.rs`'s
/// `try_acquire_owned`) had no regression test — a real HTTP test would need
/// to race 65 concurrent requests against a fast handler to reliably
/// saturate it. Calling the handler function directly, with every permit
/// already held, exercises the exact same code path deterministically.
#[tokio::test]
async fn ingest_returns_429_when_the_concurrency_semaphore_is_exhausted() {
    let (app, _client, login) = spawn_and_login("host-429").await;

    let mut permits = Vec::new();
    for _ in 0..guru_cloud::state::AppState::INGEST_CONCURRENCY {
        permits.push(
            app.state
                .ingest_semaphore
                .clone()
                .try_acquire_owned()
                .expect("semaphore starts with INGEST_CONCURRENCY permits available"),
        );
    }

    let auth = guru_cloud::auth::AuthContext {
        org_id: uuid::Uuid::parse_str(&login.org_id).unwrap(),
        account_id: uuid::Uuid::parse_str(&login.user_id).unwrap(),
        host_id: "host-429".to_string(),
        device_id: uuid::Uuid::new_v4(), // unused: the semaphore check runs before auth.device_id ever matters
    };
    let events = vec![sample_event("e-429", "host-429", "sess-1")];
    let payload = gzip(&ingest_body_bytes(&events));
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_ENCODING,
        axum::http::HeaderValue::from_static("gzip"),
    );

    let result = guru_cloud::ingest::ingest(
        axum::extract::State(app.state.clone()),
        auth,
        headers,
        axum::body::Bytes::from(payload),
    )
    .await;

    let err = result
        .err()
        .expect("must be rejected while every permit is held");
    let resp = axum::response::IntoResponse::into_response(err);
    assert_eq!(resp.status(), axum::http::StatusCode::TOO_MANY_REQUESTS);
    assert!(
        resp.headers().get("retry-after").is_some(),
        "429 must carry Retry-After"
    );

    drop(permits);
    app.teardown().await;
}

/// Security review: `ingest_rejects_batches_over_32mb_decompressed` only
/// exercises the *decompressed* cap (its payload is deliberately tiny on the
/// wire, since repetitive text compresses extremely well). The 5 MB
/// *compressed*-body cap (`ingest.rs`: `body.len() > MAX_COMPRESSED_BYTES`,
/// checked before any attempt to decompress) had no test of its own.
#[tokio::test]
async fn ingest_rejects_bodies_over_5mb_compressed() {
    let (app, client, login) = spawn_and_login("host-oversize-compressed").await;

    // Doesn't need to be valid gzip: the compressed-size check runs first and
    // rejects on `body.len()` alone, before `decode_gzip_capped` ever runs.
    // Kept under the router's 8 MB `DefaultBodyLimit` so this exercises
    // ingest.rs's own 5 MB check, not axum's outer backstop.
    let body = vec![0u8; ingest_max_compressed_bytes() + 1];

    let resp = client
        .post(format!("{}/v1/events", app.base_url))
        .bearer_auth(&login.token)
        .header("Content-Encoding", "gzip")
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 413);

    app.teardown().await;
}

fn ingest_max_compressed_bytes() -> usize {
    guru_cloud::ingest::MAX_COMPRESSED_BYTES
}

/// Spec review finding: `events.event_id` was a *global* `PRIMARY KEY`, so
/// `ON CONFLICT (event_id) DO NOTHING` could silently dedupe one org's insert
/// against a different org's row of the same id (migrations/
/// 0004_events_org_scoped_pk.sql + ingest.rs's `ON CONFLICT (org_id,
/// event_id)` fix this). This proves both orgs' rows land, unmerged, with
/// their own data.
#[tokio::test]
async fn ingest_same_event_id_in_two_orgs_does_not_cross_tenant_dedupe() {
    let app = TestApp::spawn(SpawnOpts::default()).await; // autoapprove OFF: real two-tenant login
    let client = reqwest::Client::new();

    let org_a = login_as(&client, &app.base_url, "host-dedup-a", "dedup-a@example.com").await;
    let org_b = login_as(&client, &app.base_url, "host-dedup-b", "dedup-b@example.com").await;
    assert_ne!(org_a.org_id, org_b.org_id, "sanity: two distinct orgs");

    let mut ev_a = sample_event("shared-id", "host-dedup-a", "sess-a");
    ev_a.tool_name = Some("FromOrgA".to_string());
    let mut ev_b = sample_event("shared-id", "host-dedup-b", "sess-b");
    ev_b.tool_name = Some("FromOrgB".to_string());

    for (login, ev) in [(&org_a, ev_a), (&org_b, ev_b)] {
        let payload = gzip(&ingest_body_bytes(&[ev]));
        let resp = client
            .post(format!("{}/v1/events", app.base_url))
            .bearer_auth(&login.token)
            .header("Content-Encoding", "gzip")
            .body(payload)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(
            body["accepted"], 1,
            "each org's insert of the same event_id must be accepted on its own \
             merits, not deduped against the other org's row: {body:?}"
        );
    }

    for (login, expected_tool) in [(&org_a, "FromOrgA"), (&org_b, "FromOrgB")] {
        let app_pool = app.connect_as_app_role().await;
        let mut tx = app_pool.begin().await.unwrap();
        sqlx::query(sqlx::AssertSqlSafe(format!("SET LOCAL app.org_id = '{}'", login.org_id)))
            .execute(&mut *tx)
            .await
            .unwrap();
        let row: (String,) =
            sqlx::query_as("SELECT tool_name FROM events WHERE event_id = 'shared-id'")
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        assert_eq!(row.0, expected_tool, "org {} must see its own row's data", login.org_id);
        tx.rollback().await.unwrap();
        app_pool.close().await;
    }

    app.teardown().await;
}

/// Security review, finding #5: `insert_event` binds ~44 columns positionally
/// in an order that must exactly mirror `guru_schema::COLUMNS` — nothing
/// enforces that alignment other than careful maintenance, and a future
/// out-of-order column addition could silently write a value under the wrong
/// column name (a swapped `Option<String>`/`Option<i64>` pair of the same
/// nullability could still bind "successfully"). This round-trips every
/// non-body, non-token-derived column through a real ingest and checks each
/// one lands under its own name, not just the 3 body columns
/// `ingest_defensively_nulls_body_columns` already covers.
#[tokio::test]
async fn ingest_writes_every_column_under_its_own_name() {
    let (app, client, login) = spawn_and_login("host-roundtrip").await;

    let mut ev = sample_event("e-roundtrip", "host-roundtrip", "sess-roundtrip");
    ev.team_id = Some("team-rt".to_string());
    ev.user_id_source = Some("account".to_string());
    ev.env_kind = Some("laptop".to_string());
    ev.os = Some("linux".to_string());
    ev.agent_version = Some("2.1.251".to_string());
    ev.parent_session_id = Some("parent-rt".to_string());
    ev.turn_id = Some("turn-rt".to_string());
    ev.cwd_hash = Some("cwdhash-rt".to_string());
    ev.repo = Some("repo-rt".to_string());
    ev.correlation_key = Some("corr-rt".to_string());
    ev.correlation_confidence = Some("exact".to_string());
    ev.tool_kind = Some("mcp".to_string());
    ev.mcp_server = Some("mcp-server-rt".to_string());
    ev.mcp_tool = Some("mcp-tool-rt".to_string());
    ev.error_type = Some("timeout".to_string());
    ev.decision = Some("accept".to_string());
    ev.decision_source = Some("user".to_string());
    ev.provider = Some("anthropic".to_string());
    ev.effort = Some("high".to_string());
    ev.thinking = Some(true);
    ev.cache_read_tokens = Some(11);
    ev.cache_write_tokens = Some(22);
    ev.reasoning_tokens = Some(33);
    ev.usage_source = Some("otel".to_string());
    ev.redaction_applied = Some(true); // NOT a body column — must round-trip, unlike tool_input_json etc.

    let payload = gzip(&ingest_body_bytes(&[ev.clone()]));
    let resp = client
        .post(format!("{}/v1/events", app.base_url))
        .bearer_auth(&login.token)
        .header("Content-Encoding", "gzip")
        .body(payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let app_pool = app.connect_as_app_role().await;
    let mut tx = app_pool.begin().await.unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!("SET LOCAL app.org_id = '{}'", login.org_id)))
        .execute(&mut *tx)
        .await
        .unwrap();

    // org_id/user_id come from the token, not the client-submitted event, and
    // the 3 real body columns are defensively NULLed — all covered by other,
    // more targeted tests. Everything else in COLUMNS must round-trip as-is.
    let skip: &[&str] = &["org_id", "user_id", "tool_input_json", "tool_output_excerpt", "prompt_text"];

    for &col in COLUMNS {
        if skip.contains(&col) {
            continue;
        }
        let actual = fetch_column_as_string(&mut tx, col).await;
        let expected = expected_text(&ev, col);
        assert_eq!(actual, expected, "column {col:?} round-tripped incorrectly");
    }

    let (org_id, user_id): (String, String) =
        sqlx::query_as("SELECT org_id::text, user_id FROM events WHERE event_id = 'e-roundtrip'")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(org_id, login.org_id, "org_id must come from the token, not the client");
    assert_eq!(user_id, login.user_id, "user_id must come from the token, not the client");

    tx.rollback().await.unwrap();
    app_pool.close().await;
    app.teardown().await;
}

/// `guru_schema::COLUMNS`' type per column (mirrors `crates/sink/src/lib.rs`'s
/// and `crates/cloud/src/export.rs`'s private `column_data_type` — duplicated
/// here rather than made `pub` purely for this test, to avoid widening either
/// crate's public surface just for test reuse).
fn column_is_int8(col: &str) -> bool {
    matches!(
        col,
        "ts" | "duration_ms"
            | "input_tokens"
            | "output_tokens"
            | "cache_read_tokens"
            | "cache_write_tokens"
            | "reasoning_tokens"
    )
}
fn column_is_float8(col: &str) -> bool {
    col == "cost_usd"
}
fn column_is_bool(col: &str) -> bool {
    matches!(col, "success" | "thinking" | "redaction_applied")
}

async fn fetch_column_as_string(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    col: &str,
) -> Option<String> {
    // `col` always comes from the fixed, trusted `guru_schema::COLUMNS` list
    // (never attacker-controlled input), so building this dynamically is safe.
    let sql = format!("SELECT {col} FROM events WHERE event_id = 'e-roundtrip'");
    if column_is_int8(col) {
        let (v,): (Option<i64>,) = sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .fetch_one(&mut **tx)
            .await
            .unwrap();
        v.map(|n| n.to_string())
    } else if column_is_float8(col) {
        let (v,): (Option<f64>,) = sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .fetch_one(&mut **tx)
            .await
            .unwrap();
        v.map(|n| n.to_string())
    } else if column_is_bool(col) {
        let (v,): (Option<bool>,) = sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .fetch_one(&mut **tx)
            .await
            .unwrap();
        v.map(|b| b.to_string())
    } else {
        let (v,): (Option<String>,) = sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .fetch_one(&mut **tx)
            .await
            .unwrap();
        v
    }
}

/// What `col` should hold after ingesting `ev`, rendered the same way
/// [`fetch_column_as_string`] renders the value actually stored.
fn expected_text(ev: &Event, col: &str) -> Option<String> {
    match col {
        "event_id" => Some(ev.event_id.clone()),
        "ts" => Some(ev.ts.to_string()),
        "dt" => Some(ev.dt.clone()),
        "team_id" => ev.team_id.clone(),
        "user_id_source" => ev.user_id_source.clone(),
        "host_id" => Some(ev.host_id.clone()),
        "env_kind" => ev.env_kind.clone(),
        "os" => ev.os.clone(),
        "agent" => Some(ev.agent.clone()),
        "agent_version" => ev.agent_version.clone(),
        "session_id" => ev.session_id.clone(),
        "parent_session_id" => ev.parent_session_id.clone(),
        "turn_id" => ev.turn_id.clone(),
        "cwd_hash" => ev.cwd_hash.clone(),
        "repo" => ev.repo.clone(),
        "source" => Some(ev.source.clone()),
        "correlation_key" => ev.correlation_key.clone(),
        "correlation_confidence" => ev.correlation_confidence.clone(),
        "event_type" => Some(ev.event_type.clone()),
        "tool_name" => ev.tool_name.clone(),
        "tool_kind" => ev.tool_kind.clone(),
        "mcp_server" => ev.mcp_server.clone(),
        "mcp_tool" => ev.mcp_tool.clone(),
        "duration_ms" => ev.duration_ms.map(|v| v.to_string()),
        "success" => ev.success.map(|v| v.to_string()),
        "error_type" => ev.error_type.clone(),
        "decision" => ev.decision.clone(),
        "decision_source" => ev.decision_source.clone(),
        "provider" => ev.provider.clone(),
        "model" => ev.model.clone(),
        "effort" => ev.effort.clone(),
        "thinking" => ev.thinking.map(|v| v.to_string()),
        "input_tokens" => ev.input_tokens.map(|v| v.to_string()),
        "output_tokens" => ev.output_tokens.map(|v| v.to_string()),
        "cache_read_tokens" => ev.cache_read_tokens.map(|v| v.to_string()),
        "cache_write_tokens" => ev.cache_write_tokens.map(|v| v.to_string()),
        "reasoning_tokens" => ev.reasoning_tokens.map(|v| v.to_string()),
        "cost_usd" => ev.cost_usd.map(|v| v.to_string()),
        "usage_source" => ev.usage_source.clone(),
        "redaction_applied" => ev.redaction_applied.map(|v| v.to_string()),
        other => panic!("expected_text: unhandled column {other:?} — update this test"),
    }
}
