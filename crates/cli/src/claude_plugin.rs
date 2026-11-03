//! Metadata-only inbox for the supported Claude plugin. No browser/app patching.
use anyhow::{ensure, Result};
use kikimimi_schema::Event;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub session_id: String,
    pub response_id: String,
    pub model: String,
    pub timestamp_ms: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub message_uuid: Option<String>,
    pub parent_message_uuid: Option<String>,
    pub user_message_uuid: Option<String>,
    pub response_fingerprint: Option<String>,
}
fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 160
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
impl Response {
    pub fn event(&self, host: &str) -> Result<Event> {
        if let Some(fingerprint) = &self.response_fingerprint {
            ensure!(
                fingerprint.len() == 64 && fingerprint.bytes().all(|c| c.is_ascii_hexdigit()),
                "Invalid fingerprint"
            );
        }
        for value in [
            &self.message_uuid,
            &self.parent_message_uuid,
            &self.user_message_uuid,
        ]
        .into_iter()
        .flatten()
        {
            ensure!(uuid::Uuid::parse_str(value).is_ok(), "Invalid message UUID");
        }
        ensure!(
            identifier(&self.session_id) && identifier(&self.response_id),
            "Invalid identifier"
        );
        ensure!(
            identifier(&self.model) && self.model.starts_with("claude-"),
            "Invalid model"
        );
        ensure!(
            (1_600_000_000_000..4_102_444_800_000).contains(&self.timestamp_ms),
            "Invalid time"
        );
        for n in [
            Some(self.input_tokens),
            Some(self.output_tokens),
            self.cache_read_tokens,
            self.cache_write_tokens,
        ]
        .into_iter()
        .flatten()
        {
            ensure!((0..=100_000_000).contains(&n), "Invalid usage");
        }
        let key = format!("claude-plugin:{}:{}", self.session_id, self.response_id);
        Ok(Event {
            event_id: kikimimi_schema::event_id(host, "hook", "api.request", &key),
            ts: self.timestamp_ms,
            dt: kikimimi_schema::dt_of(self.timestamp_ms),
            host_id: host.into(),
            agent: "claude".into(),
            client: Some("claude_desktop".into()),
            env_kind: Some("cloud-vm".into()),
            source: "hook".into(),
            usage_source: Some("hook".into()),
            event_type: "api.request".into(),
            session_id: Some(format!("claude-plugin:{}", self.session_id)),
            correlation_key: Some(self.response_id.clone()),
            correlation_confidence: Some("exact".into()),
            provider: Some("anthropic".into()),
            model: Some(self.model.clone()),
            input_tokens: Some(self.input_tokens),
            output_tokens: Some(self.output_tokens),
            cache_read_tokens: self.cache_read_tokens,
            cache_write_tokens: self.cache_write_tokens,
            ..Default::default()
        })
    }
}
pub fn collect(dir: &Path, host: &str) -> Result<Vec<Event>> {
    let mut events = vec![];
    if !dir.exists() {
        return Ok(events);
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() || entry.path().extension().is_none_or(|s| s != "json") {
            continue;
        }
        if entry.metadata()?.len() > 4096 {
            continue;
        }
        let bytes = std::fs::read(entry.path())?;
        if let Ok(row) = serde_json::from_slice::<Response>(&bytes) {
            if let Ok(event) = row.event(host) {
                events.push(event);
            }
        }
    }
    Ok(events)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matching_is_private_unique_and_bounded() {
        assert_eq!(
            response_fingerprint("Hello\r\n世界", &"a".repeat(64)),
            "ebdad441e6740f8cb3c29d96f6b53f4ab7a9a0d1c77aefac70f5d7d033eca6b2"
        );
        let row: Response=serde_json::from_value(serde_json::json!({"session_id":"sdk", "response_id":"msg_one", "model":"claude-haiku-5-5", "timestamp_ms":1793531285000i64,"input_tokens":2,"output_tokens":12,"response_fingerprint":"a".repeat(64)})).unwrap();
        let target = Event {
            event_type: "message.fingerprint".into(),
            ts: row.timestamp_ms - 500,
            session_id: Some("chat".into()),
            turn_id: Some("message".into()),
            correlation_key: row.response_fingerprint.clone(),
            ..Default::default()
        };
        let links = link_responses(&[row], &[target.clone()], "host");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].session_id.as_deref(), Some("chat"));
        assert_eq!(
            links[0].correlation_confidence.as_deref(),
            Some("content_match")
        );
        assert!(links[0].input_tokens.is_none() && links[0].prompt_text.is_none());
        let make = || {
            serde_json::from_value::<Response>(serde_json::json!({"session_id":"sdk", "response_id":"msg_one", "model":"claude-haiku-5-5", "timestamp_ms":1793531285000i64,"input_tokens":2,"output_tokens":12,"response_fingerprint":"a".repeat(64)})).unwrap()
        };
        let mut duplicate = target.clone();
        duplicate.session_id = Some("another-chat".into());
        assert!(
            link_responses(&[make()], &[target.clone(), duplicate], "host")
                .iter()
                .all(|e| e.event_type == "message.usage.link.rejected")
        );
        assert!(link_responses(&[make(), make()], &[target.clone()], "host")
            .iter()
            .all(|e| e.event_type == "message.usage.link.rejected"));
        let mut old = target;
        old.ts -= 121_000;
        assert!(link_responses(&[make()], &[old], "host").is_empty());
        assert!(link_responses(&[make()], &[], "host").is_empty());
    }
    #[test]
    fn measured_identity_and_validation() {
        let mut r = Response {
            session_id: "session-1".into(),
            response_id: "msg_1".into(),
            model: "claude-haiku-5-5".into(),
            timestamp_ms: 1793531285000,
            input_tokens: 2,
            output_tokens: 16,
            cache_read_tokens: Some(119986),
            cache_write_tokens: Some(427),
            message_uuid: None,
            parent_message_uuid: None,
            user_message_uuid: None,
            response_fingerprint: None,
        };
        let e = r.event("host").unwrap();
        assert_eq!(e.output_tokens, Some(16));
        assert!(e.estimated_output_tokens.is_none());
        assert_eq!(e.event_id, r.event("host").unwrap().event_id);
        r.response_id = "msg_2".into();
        assert_ne!(e.event_id, r.event("host").unwrap().event_id);
        r.output_tokens = -1;
        assert!(r.event("host").is_err());
    }
}

#[derive(Clone)]
struct Inbox {
    config: std::path::PathBuf,
    dir: std::path::PathBuf,
    lock: std::sync::Arc<tokio::sync::Mutex<()>>,
    rate: std::sync::Arc<std::sync::Mutex<(std::time::Instant, u32)>>,
    connections: std::sync::Arc<tokio::sync::Semaphore>,
}
impl Inbox {
    fn new(config: std::path::PathBuf, dir: std::path::PathBuf) -> Self {
        Self {
            config,
            dir,
            lock: Default::default(),
            rate: std::sync::Arc::new(std::sync::Mutex::new((std::time::Instant::now(), 0))),
            connections: std::sync::Arc::new(tokio::sync::Semaphore::new(8)),
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Batch {
    version: u8,
    responses: Vec<Response>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Config {
    token_sha256: String,
    port: u16,
    expires_at: Option<i64>,
    enabled: bool,
}
fn digest(token: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(token))
}

pub(crate) fn matching_key() -> Option<String> {
    load_config(&kikimimi_schema::paths::kikimimi_dir().join("claude-plugin-receiver.json"))
        .ok()
        .filter(|c| c.enabled)
        .map(|c| c.token_sha256)
}

/// HMAC-SHA256; the fixed 64-byte key is the receiver credential digest.
/// The body is never stored or transmitted. Domain separation prevents reuse.
pub(crate) fn response_fingerprint(body: &str, key: &str) -> String {
    use sha2::{Digest, Sha256};
    assert_eq!(key.len(), 64);
    let mut inner = Sha256::new();
    inner.update(key.bytes().map(|b| b ^ 0x36).collect::<Vec<_>>());
    inner.update(b"kikimimi-response-v1\0");
    inner.update(body.replace("\r\n", "\n").as_bytes());
    let mut outer = Sha256::new();
    outer.update(key.bytes().map(|b| b ^ 0x5c).collect::<Vec<_>>());
    outer.update(inner.finalize());
    hex::encode(outer.finalize())
}

/// Abstain on ambiguous text, missing history, or clock differences above 2m.
/// This is explicitly a content match, never claimed to be an identity match.
pub(crate) fn link_responses(rows: &[Response], candidates: &[Event], host: &str) -> Vec<Event> {
    let mut links = Vec::new();
    for row in rows {
        let Some(fp) = row.response_fingerprint.as_deref() else {
            continue;
        };
        let matches: Vec<_> = candidates
            .iter()
            .filter(|e| {
                e.event_type == "message.fingerprint"
                    && e.correlation_key.as_deref() == Some(fp)
                    && e.ts.abs_diff(row.timestamp_ms) <= 120_000
            })
            .collect();
        let ambiguous = matches.len() > 1
            || matches.first().is_some_and(|target| {
                rows.iter()
                    .filter(|other| {
                        other.response_fingerprint.as_deref() == Some(fp)
                            && other.timestamp_ms.abs_diff(target.ts) <= 120_000
                    })
                    .count()
                    > 1
            });
        if ambiguous {
            links.push(Event {
                event_id: kikimimi_schema::event_id(
                    host,
                    "log",
                    "message.usage.link.rejected",
                    &format!("{}:{}", row.session_id, row.response_id),
                ),
                ts: row.timestamp_ms,
                dt: kikimimi_schema::dt_of(row.timestamp_ms),
                host_id: host.into(),
                agent: "claude".into(),
                client: Some("claude_desktop".into()),
                source: "log".into(),
                event_type: "message.usage.link.rejected".into(),
                session_id: Some(format!("claude-plugin:{}", row.session_id)),
                correlation_key: Some(row.response_id.clone()),
                redaction_applied: Some(true),
                ..Default::default()
            });
            continue;
        }
        if matches.len() != 1 {
            continue;
        }
        let target = matches[0];
        if rows
            .iter()
            .filter(|other| {
                other.response_fingerprint.as_deref() == Some(fp)
                    && other.timestamp_ms.abs_diff(target.ts) <= 120_000
            })
            .count()
            != 1
        {
            continue;
        }
        let Some(chat) = target.session_id.as_deref() else {
            continue;
        };
        let Some(message) = target.turn_id.as_deref() else {
            continue;
        };
        links.push(Event {
            event_id: kikimimi_schema::event_id(
                host,
                "log",
                "message.usage.link",
                &format!("{}:{}:{chat}:{message}", row.session_id, row.response_id),
            ),
            ts: row.timestamp_ms,
            dt: kikimimi_schema::dt_of(row.timestamp_ms),
            host_id: host.into(),
            agent: "claude".into(),
            client: Some("claude_desktop".into()),
            source: "log".into(),
            event_type: "message.usage.link".into(),
            session_id: Some(chat.into()),
            parent_session_id: Some(format!("claude-plugin:{}", row.session_id)),
            turn_id: Some(message.into()),
            correlation_key: Some(row.response_id.clone()),
            correlation_confidence: Some("content_match".into()),
            redaction_applied: Some(true),
            ..Default::default()
        });
    }
    links
}

pub(crate) fn collect_links(dir: &Path, candidates: &[Event], host: &str) -> Result<Vec<Event>> {
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut rows = vec![];
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file()
            && entry.path().extension().is_some_and(|v| v == "json")
            && entry.metadata()?.len() <= 4096
        {
            if let Ok(row) = serde_json::from_slice::<Response>(&std::fs::read(entry.path())?) {
                if row.event(host).is_ok() {
                    rows.push(row);
                }
            }
        }
    }
    Ok(link_responses(&rows, candidates, host))
}
fn load_config(path: &Path) -> Result<Config> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::symlink_metadata(path)?;
    ensure!(
        m.is_file() && m.mode() & 0o077 == 0 && m.len() <= 4096,
        "Receiver config must be a private regular file"
    );
    let c: Config = serde_json::from_slice(&std::fs::read(path)?)?;
    ensure!(
        c.port != 0
            && c.token_sha256.len() == 64
            && c.token_sha256.bytes().all(|b| b.is_ascii_hexdigit()),
        "Invalid receiver config"
    );
    Ok(c)
}
fn private_json(path: &Path, value: &impl Serialize) -> Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Missing parent directory"))?;
    let mut f = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(&mut f, value)?;
    f.flush()?;
    f.as_file().sync_all()?;
    f.persist(path)?;
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}
#[derive(clap::Subcommand)]
pub enum Action {
    /// Generate/rotate a send-only credential in Keychain, without an expiry (revocable immediately).
    Enable {
        #[arg(long)]
        url: String,
        #[arg(long, default_value_t = 43192)]
        port: u16,
    },
    /// Export a private packaging input; delete it after packaging/upload.
    Export {
        #[arg(long)]
        url: String,
        #[arg(long)]
        output: std::path::PathBuf,
    },
    /// Immediately reject further uploads, including in the running receiver.
    Revoke,
}
fn validate_url(url: &str) -> Result<()> {
    let u = reqwest::Url::parse(url)?;
    ensure!(
        u.scheme() == "https"
            && u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none()
            && u.query().is_none()
            && u.fragment().is_none()
            && u.path() == "/v1/claude-plugin",
        "Expected a dedicated HTTPS ingestion URL"
    );
    Ok(())
}
pub fn configure(action: Action) -> Result<()> {
    use crate::secret_store::{self, Secret};
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let root = kikimimi_schema::paths::kikimimi_dir();
    std::fs::create_dir_all(&root)?;
    let path = root.join("claude-plugin-receiver.json");
    match action {
        Action::Enable { url, port } => {
            validate_url(&url)?;
            ensure!(port > 0, "Invalid port");
            ensure!(secret_store::enabled(), "A supported Keychain is required");
            use rand::RngExt as _;
            let mut bytes = [0u8; 32];
            rand::rng().fill(&mut bytes);
            let token = hex::encode(bytes);
            secret_store::set(Secret::ClaudePluginToken, &root, &token)?;
            private_json(
                &path,
                &Config {
                    token_sha256: digest(token.as_bytes()),
                    port,
                    expires_at: None,
                    enabled: true,
                },
            )?;
            println!("Receiver credential rotated (no expiry; immediately revocable). Restart the collector if its listener is not running; repackage and update the Claude plugin.");
        }
        Action::Export { url, output } => {
            validate_url(&url)?;
            let c = load_config(&path)?;
            ensure!(
                c.enabled
                    && c.expires_at
                        .is_none_or(|at| at > chrono::Utc::now().timestamp()),
                "Receiver disabled or expired"
            );
            let token = secret_store::get(Secret::ClaudePluginToken, &root)?
                .ok_or_else(|| anyhow::anyhow!("Missing Keychain credential"))?;
            ensure!(
                digest(token.as_bytes()) == c.token_sha256,
                "Credential mismatch"
            );
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(output)?;
            f.write_all(
                serde_json::to_string(
                    &serde_json::json!({"url":url,"token":token,"expires_at":c.expires_at}),
                )?
                .as_bytes(),
            )?;
            f.sync_all()?;
            println!("Private packaging input exported. Delete it after upload.");
        }
        Action::Revoke => {
            let mut c = load_config(&path)?;
            c.enabled = false;
            private_json(&path, &c)?;
            secret_store::delete(Secret::ClaudePluginToken, &root)?;
            println!("Receiver credential revoked.");
        }
    }
    Ok(())
}
fn router(inbox: Inbox) -> axum::Router {
    axum::Router::new()
        .route("/v1/claude-plugin", axum::routing::post(receive))
        .with_state(inbox)
}
async fn receive(
    axum::extract::State(inbox): axum::extract::State<Inbox>,
    request: axum::extract::Request,
) -> axum::http::StatusCode {
    use axum::http::StatusCode as S;
    // Bound concurrent uploads and work even for unauthenticated requests. Do not
    // trust forwarded IP headers; this limit applies to the entire dedicated receiver.
    let Ok(_permit) = inbox.connections.try_acquire() else {
        return S::TOO_MANY_REQUESTS;
    };
    {
        let Ok(mut rate) = inbox.rate.lock() else {
            return S::SERVICE_UNAVAILABLE;
        };
        if rate.0.elapsed().as_secs() >= 60 {
            *rate = (std::time::Instant::now(), 0);
        }
        if rate.1 >= 120 {
            return S::TOO_MANY_REQUESTS;
        }
        rate.1 += 1;
    }
    // Reload on each request so revocation/rotation takes effect without restart.
    let Ok(config) = load_config(&inbox.config) else {
        return S::UNAUTHORIZED;
    };
    if !config.enabled
        || config
            .expires_at
            .is_some_and(|at| at <= chrono::Utc::now().timestamp())
    {
        return S::UNAUTHORIZED;
    }
    let token = request
        .headers()
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("");
    if token.len() != 64 {
        return S::UNAUTHORIZED;
    }
    let hash = digest(token.as_bytes());
    if hash
        .bytes()
        .zip(config.token_sha256.bytes())
        .fold(0u8, |a, (x, y)| a | (x ^ y))
        != 0
    {
        return S::UNAUTHORIZED;
    }
    let body = match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        axum::body::to_bytes(request.into_body(), 65536),
    )
    .await
    {
        Ok(Ok(b)) => b,
        Ok(Err(_)) => return S::PAYLOAD_TOO_LARGE,
        Err(_) => return S::REQUEST_TIMEOUT,
    };
    let Ok(batch) = serde_json::from_slice::<Batch>(&body) else {
        return S::BAD_REQUEST;
    };
    if batch.version != 1
        || batch.responses.is_empty()
        || batch.responses.len() > 32
        || batch.responses.iter().any(|r| r.event("validate").is_err())
    {
        return S::BAD_REQUEST;
    }
    let _guard = inbox.lock.lock().await;
    let result = (|| -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(&inbox.dir)?;
        ensure!(
            std::fs::symlink_metadata(&inbox.dir)?.is_dir(),
            "Invalid inbox"
        );
        std::fs::set_permissions(&inbox.dir, std::fs::Permissions::from_mode(0o700))?;
        let count = std::fs::read_dir(&inbox.dir)?.count();
        let mut pending = vec![];
        for row in batch.responses {
            let path = inbox
                .dir
                .join(format!("{}.json", row.event("inbox")?.event_id));
            if let Ok(m) = std::fs::symlink_metadata(&path) {
                ensure!(m.is_file() && m.len() <= 4096, "Invalid inbox entry");
                let previous: Response = serde_json::from_slice(&std::fs::read(&path)?)?;
                let mut old = serde_json::to_value(&previous)?;
                let mut new = serde_json::to_value(&row)?;
                for field in [
                    "message_uuid",
                    "parent_message_uuid",
                    "user_message_uuid",
                    "response_fingerprint",
                ] {
                    let a = old
                        .as_object_mut()
                        .unwrap()
                        .remove(field)
                        .unwrap_or_default();
                    let b = new
                        .as_object_mut()
                        .unwrap()
                        .remove(field)
                        .unwrap_or_default();
                    ensure!(
                        a.is_null() || b.is_null() || a == b,
                        "Conflicting response metadata"
                    );
                }
                ensure!(old == new, "Conflicting response identity");
                if previous.response_fingerprint.is_none() && row.response_fingerprint.is_some() {
                    pending.push((path, row));
                }
            } else {
                pending.push((path, row));
            }
        }
        ensure!(count + pending.len() <= 20000, "Inbox full");
        for (path, row) in pending {
            private_json(&path, &row)?;
        }
        Ok(())
    })();
    if result.is_ok() {
        S::OK
    } else {
        S::SERVICE_UNAVAILABLE
    }
}
/// Dedicated loopback port; never shares the dashboard's routes or credential.
pub fn spawn() {
    let root = kikimimi_schema::paths::kikimimi_dir();
    let path = root.join("claude-plugin-receiver.json");
    if !path.exists() {
        return;
    }
    let Ok(config) = load_config(&path) else {
        eprintln!("Claude plugin receiver: invalid configuration");
        return;
    };
    let state = Inbox::new(path, root.join("claude-plugin-inbox"));
    tokio::spawn(async move {
        let address = std::net::SocketAddr::from(([127, 0, 0, 1], config.port));
        match tokio::net::TcpListener::bind(address).await {
            Ok(listener) => {
                if axum::serve(listener, router(state)).await.is_err() {
                    eprintln!("Claude plugin receiver stopped");
                }
            }
            Err(_) => eprintln!("Claude plugin receiver: port unavailable"),
        }
    });
}

#[cfg(test)]
mod receiver_tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    #[tokio::test]
    async fn rejects_unauthorized_and_content_and_deduplicates() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("config");
        private_json(
            &cfg,
            &Config {
                token_sha256: digest("a".repeat(64).as_bytes()),
                port: 43192,
                expires_at: None,
                enabled: true,
            },
        )
        .unwrap();
        let inbox = Inbox::new(cfg, dir.path().join("inbox"));
        let app = router(inbox);
        let row = serde_json::json!({"session_id":"session-1","response_id":"msg_1","model":"claude-haiku-5-5","timestamp_ms":1793531285000i64,"input_tokens":2,"output_tokens":16,"cache_read_tokens":119986,"cache_write_tokens":427});
        let batch = serde_json::json!({"version":1,"responses":[row]});
        let request = |auth: bool, body: &serde_json::Value| {
            let mut b = Request::post("/v1/claude-plugin");
            if auth {
                b = b.header("authorization", format!("Bearer {}", "a".repeat(64)));
            }
            b.body(Body::from(body.to_string())).unwrap()
        };
        assert_eq!(
            app.clone()
                .oneshot(request(false, &batch))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        for _ in 0..2 {
            assert_eq!(
                app.clone()
                    .oneshot(request(true, &batch))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::OK
            );
        }
        assert_eq!(collect(&dir.path().join("inbox"), "host").unwrap().len(), 1);
        let mut bad = batch.clone();
        bad["responses"][0]["prompt"] = "must not persist".into();
        assert_eq!(
            app.oneshot(request(true, &bad)).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
    }
}

#[cfg(test)]
mod security_tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode as S},
    };
    use tower::ServiceExt;
    #[tokio::test]
    async fn live_revocation_rotation_expiry_and_limits() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.json");
        let mut config = Config {
            token_sha256: digest("a".repeat(64).as_bytes()),
            port: 43192,
            expires_at: None,
            enabled: true,
        };
        private_json(&path, &config).unwrap();
        let inbox = Inbox::new(path.clone(), root.path().join("inbox"));
        let rate = inbox.rate.clone();
        let app = router(inbox);
        let send = |token: &str, body: String| {
            Request::post("/v1/claude-plugin")
                .header("authorization", format!("Bearer {}", token.repeat(64)))
                .body(Body::from(body))
                .unwrap()
        };
        let batch = serde_json::json!({"version":1,"responses":[{"session_id":"session","response_id":"msg_1","model":"claude-haiku-5-5","timestamp_ms":1793531285000i64,"input_tokens":2,"output_tokens":16}]}).to_string();
        assert_eq!(
            app.clone()
                .oneshot(send("b", batch.clone()))
                .await
                .unwrap()
                .status(),
            S::UNAUTHORIZED
        );
        assert!(!root.path().join("inbox").exists());
        assert_eq!(
            app.clone()
                .oneshot(send("a", "x".repeat(65537)))
                .await
                .unwrap()
                .status(),
            S::PAYLOAD_TOO_LARGE
        );
        config.expires_at = Some(1);
        private_json(&path, &config).unwrap();
        assert_eq!(
            app.clone()
                .oneshot(send("a", batch.clone()))
                .await
                .unwrap()
                .status(),
            S::UNAUTHORIZED
        );
        config.expires_at = None;
        config.enabled = false;
        private_json(&path, &config).unwrap();
        assert_eq!(
            app.clone()
                .oneshot(send("a", batch.clone()))
                .await
                .unwrap()
                .status(),
            S::UNAUTHORIZED
        );
        config.enabled = true;
        config.token_sha256 = digest("b".repeat(64).as_bytes());
        private_json(&path, &config).unwrap();
        assert_eq!(
            app.clone()
                .oneshot(send("a", batch.clone()))
                .await
                .unwrap()
                .status(),
            S::UNAUTHORIZED
        );
        assert_eq!(
            app.clone()
                .oneshot(send("b", batch.clone()))
                .await
                .unwrap()
                .status(),
            S::OK
        );
        let mut changed: serde_json::Value = serde_json::from_str(&batch).unwrap();
        changed["responses"][0]["output_tokens"] = 99.into();
        assert_eq!(
            app.clone()
                .oneshot(send("b", changed.to_string()))
                .await
                .unwrap()
                .status(),
            S::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            collect(&root.path().join("inbox"), "host").unwrap()[0].output_tokens,
            Some(16)
        );
        rate.lock().unwrap().1 = 120;
        assert_eq!(
            app.oneshot(send("b", batch)).await.unwrap().status(),
            S::TOO_MANY_REQUESTS
        );
    }
}
