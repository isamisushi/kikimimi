//! Shared test harness: spins up a real `kikimimi-cloud` server (real axum
//! `Router`, real migrations) against a **fresh, dedicated Postgres
//! database** per test, on an OS-assigned port. Nothing here is mocked —
//! these are the "#[tokio::test] against the live PG" tests the task asks
//! for.
//!
//! `kikimimi_app` is a role global to the whole Postgres *cluster* (not scoped
//! per-database), so every test's migrations must agree on the same
//! password — otherwise two tests running concurrently would stomp on each
//! other's `ALTER ROLE ... PASSWORD` and randomly break each other's auth.
//! Hence the fixed [`TEST_APP_DB_PASSWORD`] instead of a per-test random one.

#![allow(dead_code)]

use flate2::write::GzEncoder;
use flate2::Compression;
use kikimimi_schema::Event;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{Executor, PgPool};
use std::io::Write as _;
use std::str::FromStr;

pub const TEST_APP_DB_PASSWORD: &str = "kikimimi-app-test-pw";

/// Invite code most tests spawn the server with (see [`SpawnOpts::default`])
/// so the pre-existing manual `/activate` flow tests (autoapprove off) keep
/// working under the new fail-closed gate. [`login_as`] submits this code.
pub const TEST_INVITE_CODE: &str = "test-invite-code-xyz";

pub struct TestApp {
    pub base_url: String,
    pub db_name: String,
    pub database_url: String,
    pub state: kikimimi_cloud::state::AppState,
    join: tokio::task::JoinHandle<()>,
}

pub struct SpawnOpts {
    pub dev_autoapprove: bool,
    pub dev_email: String,
    /// `None` means "KIKIMIMI_INVITE_CODE unset" (activation gate off, unless
    /// `dev_autoapprove` is also on it's fail-closed 503). Defaults to
    /// `Some(TEST_INVITE_CODE)` so the pre-existing manual-approve tests
    /// (autoapprove off, driven via [`login_as`]) keep passing under the
    /// new gate without every call site having to opt in explicitly.
    /// Tests that specifically exercise "neither env set" override this to
    /// `None`.
    pub invite_code: Option<String>,
}

impl Default for SpawnOpts {
    fn default() -> Self {
        Self {
            dev_autoapprove: false,
            dev_email: "dev@local".to_string(),
            invite_code: Some(TEST_INVITE_CODE.to_string()),
        }
    }
}

impl TestApp {
    pub async fn spawn(opts: SpawnOpts) -> Self {
        let db_name = format!("kikimimi_test_{}", uuid::Uuid::new_v4().simple());
        create_database(&db_name).await;
        let database_url = with_database(&base_database_url(), &db_name);

        let pools = kikimimi_cloud::db::Pools::connect(&database_url, TEST_APP_DB_PASSWORD)
            .await
            .expect("connect + migrate test database");

        let config = kikimimi_cloud::config::Config {
            bind_addr: "127.0.0.1:0".to_string(),
            public_base_url: "http://127.0.0.1:0".to_string(),
            database_url: database_url.clone(),
            app_db_password: TEST_APP_DB_PASSWORD.to_string(),
            dev_autoapprove: opts.dev_autoapprove,
            dev_email: opts.dev_email,
            invite_code: opts.invite_code,
        };
        let state = kikimimi_cloud::state::AppState::new(pools, config);
        let router = kikimimi_cloud::build_router(state.clone());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let port = listener.local_addr().unwrap().port();
        let base_url = format!("http://127.0.0.1:{port}");

        let join = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });

        Self {
            base_url,
            db_name,
            database_url,
            state,
            join,
        }
    }

    /// Aborts the server and closes both pools *before* dropping the
    /// database — Postgres refuses `DROP DATABASE` while any connection is
    /// still open against it.
    pub async fn teardown(self) {
        self.join.abort();
        let _ = self.join.await;
        self.state.pools.superuser.close().await;
        self.state.pools.app.close().await;
        drop_database(&self.db_name).await;
    }

    /// Opens a fresh connection **as `kikimimi_app`**, independent of the
    /// server's own pool — for RLS tests that want to drive the session GUC
    /// by hand.
    pub async fn connect_as_app_role(&self) -> PgPool {
        let opts = PgConnectOptions::from_str(&self.database_url)
            .unwrap()
            .username("kikimimi_app")
            .password(TEST_APP_DB_PASSWORD);
        PgPoolOptions::new()
            .max_connections(2)
            .connect_with(opts)
            .await
            .expect("connect as kikimimi_app")
    }
}

fn base_database_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:guru-dev@127.0.0.1:5433/guru".to_string())
}

/// Swaps the trailing `/<database>` path segment of a Postgres DSN.
fn with_database(url: &str, db_name: &str) -> String {
    let (prefix, _old_db) = url.rsplit_once('/').expect("DSN must contain a /<db>");
    format!("{prefix}/{db_name}")
}

async fn admin_pool() -> PgPool {
    let opts = PgConnectOptions::from_str(&base_database_url())
        .unwrap()
        .database("postgres");
    PgPoolOptions::new()
        .max_connections(2)
        .connect_with(opts)
        .await
        .expect("connect admin pool (maintenance db)")
}

async fn create_database(db_name: &str) {
    let admin = admin_pool().await;
    admin
        .execute(sqlx::AssertSqlSafe(format!("CREATE DATABASE \"{db_name}\"")))
        .await
        .expect("CREATE DATABASE");
    admin.close().await;
}

async fn drop_database(db_name: &str) {
    let admin = admin_pool().await;
    // best-effort: a leaked connection somewhere must never fail the test.
    let _ = admin
        .execute(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS \"{db_name}\" WITH (FORCE)"
        )))
        .await;
    admin.close().await;
}

// ---------------------------------------------------------------------------
// Event / gzip helpers
// ---------------------------------------------------------------------------

pub fn sample_event(event_id: &str, host_id: &str, session_id: &str) -> Event {
    Event {
        event_id: event_id.to_string(),
        ts: 1_700_000_000_000,
        dt: "2023-11-14".to_string(),
        host_id: host_id.to_string(),
        agent: "claude-code".to_string(),
        source: "hook".to_string(),
        session_id: Some(session_id.to_string()),
        event_type: kikimimi_schema::event_type::TOOL_CALL.to_string(),
        tool_name: Some("Bash".to_string()),
        tool_kind: Some("bash".to_string()),
        duration_ms: Some(120),
        success: Some(true),
        input_tokens: Some(100),
        output_tokens: Some(50),
        cost_usd: Some(0.01),
        model: Some("claude-sonnet".to_string()),
        usage_source: Some("hook".to_string()),
        // Body columns: set on purpose so tests can assert the server NULLs
        // them regardless (architecture.md §5.2).
        tool_input_json: Some(r#"{"command":"ls"}"#.to_string()),
        tool_output_excerpt: Some("must never reach the DB".to_string()),
        prompt_text: Some("must never reach the DB".to_string()),
        ..Default::default()
    }
}

pub fn ingest_body_bytes(events: &[Event]) -> Vec<u8> {
    let body = serde_json::json!({ "schema": kikimimi_schema::SCHEMA_VERSION, "events": events });
    serde_json::to_vec(&body).unwrap()
}

pub fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut e = GzEncoder::new(Vec::new(), Compression::default());
    e.write_all(bytes).unwrap();
    e.finish().unwrap()
}

// ---------------------------------------------------------------------------
// Device-flow login helper
// ---------------------------------------------------------------------------

pub struct Login {
    pub token: String,
    pub org_id: String,
    pub user_id: String,
    pub email: String,
}

/// Runs the full device flow via HTTP. `dev_autoapprove` must be on for the
/// server (see [`SpawnOpts`]) or the first poll will just say "pending"
/// forever since nothing here drives the `/activate` form.
pub async fn login_autoapprove(client: &reqwest::Client, base_url: &str, host_id: &str) -> Login {
    let code_resp: serde_json::Value = client
        .post(format!("{base_url}/v1/device/code"))
        .json(&serde_json::json!({ "host_id": host_id, "hostname": "test-host" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let device_code = code_resp["device_code"].as_str().unwrap().to_string();

    let token_resp: serde_json::Value = client
        .post(format!("{base_url}/v1/device/token"))
        .json(&serde_json::json!({ "device_code": device_code }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        token_resp["status"], "ok",
        "expected immediate approval with KIKIMIMI_DEV_AUTOAPPROVE, got {token_resp:?}"
    );

    Login {
        token: token_resp["token"].as_str().unwrap().to_string(),
        org_id: token_resp["org_id"].as_str().unwrap().to_string(),
        user_id: token_resp["user_id"].as_str().unwrap().to_string(),
        email: token_resp["email"].as_str().unwrap().to_string(),
    }
}

/// Logs in via the manual `/activate` flow with a specific email, so two
/// calls with two different emails land in two different personal orgs —
/// unlike [`login_autoapprove`], which always uses the same configured
/// `dev_email` and so would put both "tenants" in the same org. The server
/// must be spawned with `dev_autoapprove: false` or the first poll will just
/// say "pending" forever since nothing here drives the `/activate` form.
///
/// Submits [`TEST_INVITE_CODE`] as the `invite_code` field — matches
/// [`SpawnOpts::default`], so this only works unmodified against a server
/// spawned with that default (or an explicit `invite_code:
/// Some(TEST_INVITE_CODE.to_string())`).
pub async fn login_as(client: &reqwest::Client, base_url: &str, host_id: &str, email: &str) -> Login {
    let code_resp: serde_json::Value = client
        .post(format!("{base_url}/v1/device/code"))
        .json(&serde_json::json!({ "host_id": host_id, "hostname": "h" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let device_code = code_resp["device_code"].as_str().unwrap().to_string();
    let user_code = code_resp["user_code"].as_str().unwrap().to_string();

    let approve = client
        .post(format!("{base_url}/activate"))
        .form(&[
            ("code", user_code.as_str()),
            ("email", email),
            ("invite_code", TEST_INVITE_CODE),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(approve.status(), 200);

    let ok: serde_json::Value = client
        .post(format!("{base_url}/v1/device/token"))
        .json(&serde_json::json!({ "device_code": device_code }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ok["status"], "ok", "{ok:?}");

    Login {
        token: ok["token"].as_str().unwrap().to_string(),
        org_id: ok["org_id"].as_str().unwrap().to_string(),
        user_id: ok["user_id"].as_str().unwrap().to_string(),
        email: ok["email"].as_str().unwrap().to_string(),
    }
}

// ---------------------------------------------------------------------------
// Web login helper (POST /web/login)
// ---------------------------------------------------------------------------

pub struct WebLogin {
    /// Just the `kikimimi_session=<value>` pair (the `Set-Cookie` response
    /// header's attributes -- `HttpOnly`, `Secure`, ... -- stripped off), so
    /// tests can pass this straight through as a `Cookie:` request header
    /// value (reqwest in this crate's dev-deps has no `cookies` feature
    /// enabled, so there's no automatic cookie jar -- tests forward it by
    /// hand).
    pub cookie: String,
    pub org_id: String,
    pub email: String,
}

/// `POST /web/login {email, invite_code}` with [`TEST_INVITE_CODE`] (matches
/// [`SpawnOpts::default`], same convention as [`login_as`]). Panics (via
/// `assert_eq!`) if the login doesn't come back 200 -- callers that want to
/// exercise the *failure* paths (wrong invite, rate limit) should call
/// `POST /web/login` directly instead of this helper.
pub async fn web_login(client: &reqwest::Client, base_url: &str, email: &str) -> WebLogin {
    let resp = client
        .post(format!("{base_url}/web/login"))
        .json(&serde_json::json!({ "email": email, "invite_code": TEST_INVITE_CODE }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "web login for {email}");
    let set_cookie = resp
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .expect("Set-Cookie on successful /web/login")
        .to_str()
        .unwrap()
        .to_string();
    // Keep only the `name=value` pair, dropping `; HttpOnly; Secure; ...`.
    let cookie = set_cookie.split(';').next().unwrap().trim().to_string();
    let body: serde_json::Value = resp.json().await.unwrap();
    WebLogin {
        cookie,
        org_id: body["org_id"].as_str().unwrap().to_string(),
        email: body["email"].as_str().unwrap().to_string(),
    }
}
