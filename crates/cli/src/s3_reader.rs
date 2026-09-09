//! Read-only S3 snapshots for the existing DuckDB dashboard. AWS credentials
//! remain with the user's AWS CLI. A complete, validated snapshot is published
//! only after every listed object has been downloaded successfully.
use crate::{
    config::{KikimimiConfig, S3SinkConfig},
    web::WebAppState,
};
use axum::{
    extract::{Extension, Request, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::AsyncReadExt,
    sync::{Mutex, RwLock},
};
use tower::ServiceExt;

const MAX_BYTES: u64 = 512 * 1024 * 1024;
const MAX_OBJECTS: usize = 10_000;
const SOURCE_COOKIE: &str = "kikimimi_view";

struct Snapshot {
    _directory: tempfile::TempDir,
    raw: PathBuf,
    data: PathBuf,
    objects: BTreeMap<String, Object>,
    refreshed_at: String,
}
#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "PascalCase")]
struct Object {
    key: String,
    size: u64,
    #[serde(rename = "ETag")]
    etag: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Listing {
    #[serde(default)]
    contents: Vec<Object>,
    #[serde(default)]
    is_truncated: bool,
    next_continuation_token: Option<String>,
}
struct ReaderState {
    config: Option<S3SinkConfig>,
    snapshot: Option<Arc<Snapshot>>,
    error: Option<String>,
}
pub(crate) struct Reader {
    state: RwLock<ReaderState>,
    refresh: Mutex<()>,
    root: PathBuf,
    aws: PathBuf,
    config_path: PathBuf,
}

pub(crate) fn selected(headers: &HeaderMap) -> bool {
    headers
        .get(header::COOKIE)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|value| value.split(';').any(|c| c.trim() == "kikimimi_view=s3"))
}
fn failure(code: StatusCode, message: &str) -> Response {
    (code, Json(serde_json::json!({"error": message}))).into_response()
}
fn cookie(kind: &str) -> String {
    format!("{SOURCE_COOKIE}={kind}; HttpOnly; SameSite=Strict; Path=/; Max-Age=2592000")
}

pub(crate) fn validate(config: &S3SinkConfig) -> Result<(String, String), String> {
    crate::sink_cmd::validate_s3_url(&config.url).map_err(|_| {
        "Enter an S3 bucket and optional prefix, such as s3://team-bucket/prefix".to_string()
    })?;
    let (bucket, prefix) = config.url[5..]
        .split_once('/')
        .unwrap_or((&config.url[5..], ""));
    if !bucket
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
        || prefix.split('/').any(|part| matches!(part, "." | ".."))
        || config.url.len() > 2048
        || config.url.contains(['?', '#'])
    {
        return Err("Invalid bucket or prefix".into());
    }
    if let Some(profile) = &config.profile {
        if profile.is_empty()
            || profile.len() > 128
            || !profile
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.@".contains(&b))
        {
            return Err("Invalid AWS profile name".into());
        }
    }
    if let Some(endpoint) = &config.endpoint_url {
        let url = reqwest::Url::parse(endpoint).map_err(|_| "Invalid storage endpoint")?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(
                "Use an HTTP(S) endpoint without credentials, query parameters or fragments".into(),
            );
        }
    }
    let prefix = prefix.trim_end_matches('/');
    Ok((
        bucket.into(),
        if prefix.is_empty() {
            "kikimimi.v1/events/".into()
        } else {
            format!("{prefix}/kikimimi.v1/events/")
        },
    ))
}

fn relative_key<'a>(key: &'a str, prefix: &str) -> Result<Option<&'a str>, String> {
    let relative = key
        .strip_prefix(prefix)
        .ok_or("S3 returned an object outside the requested prefix")?;
    if !relative.ends_with(".parquet") {
        return Ok(None);
    }
    let (partition, name) = relative
        .split_once('/')
        .ok_or("Invalid Parquet object layout")?;
    let date = partition
        .strip_prefix("dt=")
        .ok_or("Invalid date partition")?;
    if date.len() != 10
        || chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_err()
        || name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        return Err(
            "The bucket contains a Parquet object outside the kikimimi export layout".into(),
        );
    }
    Ok(Some(relative))
}

impl Reader {
    pub(crate) fn new() -> Arc<Self> {
        let config_path = crate::config::config_path();
        let saved = KikimimiConfig::load_from(&config_path);
        let (config, error) = match saved {
            Ok(cfg) => match cfg.s3_reader {
                Some(c) => match validate(&c) {
                    Ok(_) => (Some(c), None),
                    Err(e) => (None, Some(e)),
                },
                None => (None, None),
            },
            Err(_) => (None, None),
        };
        Arc::new(Self {
            state: RwLock::new(ReaderState {
                config,
                snapshot: None,
                error,
            }),
            refresh: Mutex::new(()),
            root: kikimimi_schema::paths::kikimimi_dir().join("s3-reader-cache"),
            aws: "aws".into(),
            config_path,
        })
    }

    pub(crate) async fn info(&self, is_s3: bool) -> serde_json::Value {
        let state = self.state.read().await;
        serde_json::json!({"kind": if is_s3 {"s3"} else {"local"}, "connection": state.config,
            "refreshed_at": state.snapshot.as_ref().map(|s| &s.refreshed_at),
            "objects": state.snapshot.as_ref().map(|s| s.objects.len()), "error": state.error,
            "max_bytes": MAX_BYTES, "max_objects": MAX_OBJECTS})
    }

    async fn snapshot(&self) -> Result<Arc<Snapshot>, String> {
        if let Some(snapshot) = &self.state.read().await.snapshot {
            return Ok(snapshot.clone());
        }
        let _guard = self.refresh.lock().await;
        if let Some(snapshot) = &self.state.read().await.snapshot {
            return Ok(snapshot.clone());
        }
        if let Some(error) = &self.state.read().await.error {
            return Err(error.clone());
        }
        self.update(None, false).await
    }

    // Caller holds refresh: publish config and snapshot together; failed refresh
    // keeps the previous snapshot and its original timestamp visible.
    async fn update(
        &self,
        requested: Option<S3SinkConfig>,
        persist: bool,
    ) -> Result<Arc<Snapshot>, String> {
        let (config, previous) = {
            let state = self.state.read().await;
            let config = requested
                .or_else(|| state.config.clone())
                .ok_or("Connect an S3 read source in Storage & sharing")?;
            let previous = if state.config.as_ref() == Some(&config) {
                state.snapshot.clone()
            } else {
                None
            };
            (config, previous)
        };
        let result = tokio::time::timeout(
            Duration::from_secs(180),
            self.download(&config, previous.as_deref()),
        )
        .await
        .map_err(|_| "S3 refresh timed out; the previous snapshot has not changed".to_string())
        .and_then(|result| result);
        match result {
            Ok(snapshot) => {
                if persist {
                    let mut cfg = match std::fs::read(&self.config_path) {
                        Ok(bytes) => serde_json::from_slice::<KikimimiConfig>(&bytes)
                            .map_err(|_| "Cannot read existing settings")?,
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Default::default(),
                        Err(_) => return Err("Cannot read existing settings".into()),
                    };
                    cfg.s3_reader = Some(config.clone());
                    cfg.save_to(&self.config_path)
                        .map_err(|_| "Could not save the S3 connection")?;
                }
                let snapshot = Arc::new(snapshot);
                let mut state = self.state.write().await;
                state.config = Some(config);
                state.snapshot = Some(snapshot.clone());
                state.error = None;
                Ok(snapshot)
            }
            Err(error) => {
                self.state.write().await.error = Some(error.clone());
                Err(error)
            }
        }
    }

    async fn aws(&self, config: &S3SinkConfig, args: Vec<String>) -> Result<Vec<u8>, String> {
        let mut command = tokio::process::Command::new(&self.aws);
        command
            .args(args)
            .args([
                "--output",
                "json",
                "--no-cli-pager",
                "--cli-connect-timeout",
                "10",
                "--cli-read-timeout",
                "30",
            ])
            .env("AWS_CLI_AUTO_PROMPT", "off");
        if let Some(profile) = &config.profile {
            command.args(["--profile", profile]);
        }
        if let Some(endpoint) = &config.endpoint_url {
            command.args(["--endpoint-url", endpoint]);
        }
        run_bounded(
            command,
            45,
            "S3 request failed. Check AWS CLI sign-in, read permissions and the endpoint.",
        )
        .await
    }

    async fn download(
        &self,
        config: &S3SinkConfig,
        previous: Option<&Snapshot>,
    ) -> Result<Snapshot, String> {
        let (bucket, prefix) = validate(config)?;
        let mut objects = BTreeMap::new();
        let mut token: Option<String> = None;
        let mut seen = HashSet::new();
        let mut total = 0u64;
        loop {
            let mut args = vec![
                "s3api".into(),
                "list-objects-v2".into(),
                "--bucket".into(),
                bucket.clone(),
                "--prefix".into(),
                prefix.clone(),
                "--no-paginate".into(),
            ];
            if let Some(value) = &token {
                args.extend(["--continuation-token".into(), value.clone()]);
            }
            let page: Listing = serde_json::from_slice(&self.aws(config, args).await?)
                .map_err(|_| "Invalid S3 listing response")?;
            for object in page.contents {
                let Some(relative) = relative_key(&object.key, &prefix)? else {
                    continue;
                };
                total = total
                    .checked_add(object.size)
                    .ok_or("S3 dataset is too large")?;
                if total > MAX_BYTES || objects.len() >= MAX_OBJECTS {
                    return Err("This reader supports up to 512 MiB and 10,000 Parquet objects. Connect a smaller export prefix.".into());
                }
                if object.etag.is_empty()
                    || object.etag.len() > 256
                    || object.etag.chars().any(char::is_control)
                {
                    return Err("Missing or invalid S3 object version".into());
                }
                if objects.insert(relative.to_string(), object).is_some() {
                    return Err("S3 returned duplicate object keys".into());
                }
            }
            if !page.is_truncated {
                break;
            }
            let next = page
                .next_continuation_token
                .ok_or("S3 listing was truncated without a continuation token")?;
            if !seen.insert(next.clone()) || seen.len() > 100 {
                return Err("S3 listing pagination did not finish".into());
            }
            token = Some(next);
        }
        std::fs::create_dir_all(&self.root).map_err(|_| "Could not create S3 reader cache")?;
        let directory = tempfile::Builder::new()
            .prefix("snapshot-")
            .tempdir_in(&self.root)
            .map_err(|_| "Could not create S3 snapshot")?;
        let raw = directory.path().join("raw");
        let data = directory.path().join("data");
        let raw_path = &raw;
        let bucket_name = &bucket;
        let download_one = async |entry: Option<(&String, &Object)>| {
            let Some((relative, object)) = entry else {
                return Ok::<(), String>(());
            };
            let target = raw_path.join(relative);
            std::fs::create_dir_all(target.parent().unwrap())
                .map_err(|_| "Could not create cache partition")?;
            if previous.is_some_and(|old| old.objects.get(relative) == Some(object)) {
                std::fs::copy(previous.unwrap().raw.join(relative), &target)
                    .map_err(|_| "Could not reuse cached Parquet")?;
            } else {
                self.aws(
                    config,
                    vec![
                        "s3api".into(),
                        "get-object".into(),
                        "--bucket".into(),
                        bucket_name.clone(),
                        "--key".into(),
                        object.key.clone(),
                        "--if-match".into(),
                        object.etag.clone(),
                        target.to_string_lossy().into_owned(),
                    ],
                )
                .await?;
            }
            if std::fs::metadata(&target)
                .map_err(|_| "Missing downloaded object")?
                .len()
                != object.size
            {
                return Err("An S3 object changed during refresh. Try again.".into());
            }
            Ok(())
        };
        let entries: Vec<_> = objects.iter().collect();
        for batch in entries.chunks(4) {
            // Bound concurrency without detached tasks: cancellation drops and
            // kills all outstanding CLI children before the TempDir is removed.
            let (a, b, c, d) = tokio::join!(
                download_one(batch.first().copied()),
                download_one(batch.get(1).copied()),
                download_one(batch.get(2).copied()),
                download_one(batch.get(3).copied())
            );
            a?;
            b?;
            c?;
            d?;
        }
        std::fs::create_dir_all(data.join("dt=all"))
            .map_err(|_| "Could not create query snapshot")?;
        kikimimi_sink::ensure_schema_stub(&raw)
            .map_err(|_| "Could not prepare the Parquet schema")?;
        let glob = kikimimi_schema::paths::events_glob_sql_in(&raw);
        let output = data
            .join("dt=all/events.parquet")
            .display()
            .to_string()
            .replace('\'', "''");
        let sql = format!("SET memory_limit='256MB'; SET max_temp_directory_size='1GB'; CREATE VIEW raw_events AS SELECT * FROM read_parquet('{glob}', union_by_name=true, hive_partitioning=false); SELECT CASE WHEN count(*) FILTER (WHERE event_id IS NULL OR event_id='') > 0 THEN error('missing event IDs') ELSE 1 END FROM raw_events; COPY (SELECT * FROM raw_events QUALIFY row_number() OVER (PARTITION BY event_id ORDER BY ts DESC)=1) TO '{output}' (FORMAT PARQUET);");
        let mut command = tokio::process::Command::new("duckdb");
        command
            .current_dir(directory.path())
            .args(["-batch", "-no-stdin", "-c", &sql]);
        run_bounded(command, 60, "Could not read this Parquet dataset. Check DuckDB availability and the kikimimi schema.").await?;
        Ok(Snapshot {
            _directory: directory,
            raw,
            data,
            objects,
            refreshed_at: chrono::Utc::now().to_rfc3339(),
        })
    }
}

async fn run_bounded(
    mut command: tokio::process::Command,
    seconds: u64,
    error: &str,
) -> Result<Vec<u8>, String> {
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|_| error.to_string())?;
    let stdout = child.stdout.take().ok_or(error)?;
    tokio::time::timeout(Duration::from_secs(seconds), async {
        let mut bytes = Vec::new();
        stdout
            .take(8 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| error.to_string())?;
        if bytes.len() > 8 * 1024 * 1024 {
            return Err("S3 response exceeded the reader limit".into());
        }
        if !child.wait().await.map_err(|_| error.to_string())?.success() {
            return Err(error.to_string());
        }
        Ok(bytes)
    })
    .await
    .map_err(|_| "Storage operation timed out".to_string())?
}

pub(crate) async fn info(
    Extension(reader): Extension<Arc<Reader>>,
    headers: HeaderMap,
) -> Response {
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(reader.info(selected(&headers)).await),
    )
        .into_response()
}

pub(crate) async fn delete_mark(
    State(state): State<WebAppState>,
    headers: HeaderMap,
    path: axum::extract::Path<String>,
) -> Response {
    if selected(&headers) {
        return failure(StatusCode::FORBIDDEN, "S3 dashboards are read-only");
    }
    crate::web_query::delete_mark(State(state), path).await
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub(crate) enum Selection {
    Local,
    S3 {
        url: String,
        profile: Option<String>,
        endpoint_url: Option<String>,
    },
}

pub(crate) async fn select(
    Extension(reader): Extension<Arc<Reader>>,
    Json(selection): Json<Selection>,
) -> Response {
    let kind = match selection {
        Selection::Local => "local",
        Selection::S3 {
            url,
            profile,
            endpoint_url,
        } => {
            let config = S3SinkConfig {
                url: url.trim_end_matches('/').into(),
                profile,
                endpoint_url,
            };
            if let Err(error) = validate(&config) {
                return failure(StatusCode::BAD_REQUEST, &error);
            }
            let _guard = reader.refresh.lock().await;
            if let Err(error) = reader.update(Some(config), true).await {
                return failure(StatusCode::BAD_GATEWAY, &error);
            }
            "s3"
        }
    };
    (
        [(header::SET_COOKIE, cookie(kind))],
        Json(serde_json::json!({"ok":true})),
    )
        .into_response()
}

pub(crate) async fn refresh(Extension(reader): Extension<Arc<Reader>>) -> Response {
    let _guard = reader.refresh.lock().await;
    match reader.update(None, false).await {
        Ok(_) => Json(serde_json::json!({"ok":true})).into_response(),
        Err(error) => failure(StatusCode::BAD_GATEWAY, &error),
    }
}

pub(crate) async fn dispatch(
    State(mut state): State<WebAppState>,
    Extension(reader): Extension<Arc<Reader>>,
    request: Request,
) -> Response {
    let s3 = selected(request.headers());
    // Retain the Arc until the whole query finishes, including concurrent refresh.
    let snapshot = if s3 {
        if request.uri().path().starts_with("/web/marks") {
            return if request.method() == axum::http::Method::GET {
                Json(serde_json::json!({"marks":[]})).into_response()
            } else {
                failure(StatusCode::FORBIDDEN, "S3 dashboards are read-only")
            };
        }
        if request.uri().path() == "/web/usage" {
            return Json(serde_json::json!([])).into_response();
        }
        match reader.snapshot().await {
            Ok(value) => Some(value),
            Err(error) => return failure(StatusCode::BAD_GATEWAY, &error),
        }
    } else {
        None
    };
    if let Some(snapshot) = &snapshot {
        state.data_dir = snapshot.data.clone();
    }
    let response = crate::web::query_router(s3)
        .with_state(state)
        .oneshot(request)
        .await
        .unwrap();
    drop(snapshot);
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reader_prefix_and_endpoint_validation() {
        let mut config = S3SinkConfig {
            url: "s3://team-bucket/shared/".into(),
            profile: Some("team-reader".into()),
            endpoint_url: None,
        };
        assert_eq!(
            validate(&config).unwrap(),
            ("team-bucket".into(), "shared/kikimimi.v1/events/".into())
        );
        for url in [
            "s3://",
            "s3://bucket/../private",
            "s3://user:secret@bucket/prefix",
            "s3://bucket/prefix?key=x",
        ] {
            config.url = url.into();
            assert!(validate(&config).is_err());
        }
        config.url = "s3://team-bucket".into();
        for endpoint in [
            "file:///tmp",
            "https://user:secret@storage.example",
            "https://storage.example/?key=x",
        ] {
            config.endpoint_url = Some(endpoint.into());
            assert!(validate(&config).is_err());
        }
        config.endpoint_url = Some("http://127.0.0.1:9000".into());
        assert!(validate(&config).is_ok());
    }
    #[test]
    fn object_keys_cannot_escape_the_snapshot_or_fake_a_date() {
        let prefix = "shared/kikimimi.v1/events/";
        assert_eq!(
            relative_key(&format!("{prefix}dt=2026-09-09/host-1.parquet"), prefix).unwrap(),
            Some("dt=2026-09-09/host-1.parquet")
        );
        for suffix in [
            "dt=2026-09-09/../../escape.parquet",
            "dt=2026-99-99/host.parquet",
            "dt=2026-09-09/host\n.parquet",
            "dt=2026-09-09/host\\evil.parquet",
        ] {
            assert!(relative_key(&format!("{prefix}{suffix}"), prefix).is_err());
        }
        assert!(relative_key("other/events/file.parquet", prefix).is_err());
        assert!(relative_key(&format!("{prefix}README.txt"), prefix)
            .unwrap()
            .is_none());
    }
    #[test]
    fn source_selection_requires_the_exact_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            "kikimimi_view=s3evil; other=s3".parse().unwrap(),
        );
        assert!(!selected(&headers));
        headers.insert(
            header::COOKIE,
            "other=local; kikimimi_view=s3".parse().unwrap(),
        );
        assert!(selected(&headers));
    }
}
