//! Local history transport and fixed external Cloud links.
use std::process::Stdio;
use tauri::{Manager, Webview};
use tokio::io::{AsyncBufReadExt, BufReader};

struct Viewer {
    child: tokio::process::Child,
    url: tauri::Url,
    local_history: bool,
}

#[derive(Default)]
pub(crate) struct Dashboard(tokio::sync::Mutex<Option<Viewer>>);

pub(crate) async fn close_viewer(app: &tauri::AppHandle) {
    let dashboard = app.state::<Dashboard>();
    let viewer = dashboard.0.lock().await.take();
    if let Some(mut viewer) = viewer {
        let _ = viewer.child.kill().await;
        let _ = viewer.child.wait().await;
    }
}

fn valid_url(url: &tauri::Url) -> bool {
    url.scheme() == "http"
        && url.host_str() == Some("127.0.0.1")
        && url.port().is_some()
        && url.username().is_empty()
        && url.password().is_none()
}

async fn ensure_viewer(app: &tauri::AppHandle, require_local_history: bool) -> Result<tauri::Url, String> {
    let dashboard = app.state::<Dashboard>();
    let mut viewer = dashboard.0.lock().await;
    if let Some(existing) = viewer.as_mut() {
        if existing
            .child
            .try_wait()
            .map_err(|e| e.to_string())?
            .is_none()
        {
            if !require_local_history || existing.local_history {
                return Ok(existing.url.clone());
            }
            // A viewer started during collection does not own the local ledger.
            // Reopen it after collection stops, rather than returning empty history.
            existing.child.kill().await.map_err(|e| e.to_string())?;
            existing.child.wait().await.map_err(|e| e.to_string())?;
        }
    }
    *viewer = None;
    let mut command = super::collector_command()?;
    let mut child = command
        .args(["desktop", "serve-dashboard"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Could not start the history viewer: {e}"))?;
    let mut lines = BufReader::new(child.stdout.take().ok_or("Missing viewer output")?).lines();
    let line = tokio::time::timeout(std::time::Duration::from_secs(10), lines.next_line())
        .await
        .map_err(|_| "The history viewer took too long to start")?
        .map_err(|_| "Could not read viewer startup status")?
        .ok_or("The history viewer stopped before it was ready")?;
    #[derive(serde::Deserialize)]
    struct Ready {
        url: tauri::Url,
        local_history: bool,
    }
    // Do not echo the startup line: it contains the local authentication token.
    let ready: Ready = serde_json::from_str(&line).map_err(|_| "Invalid viewer startup status")?;
    if !valid_url(&ready.url) {
        return Err("The viewer must use a local address".into());
    }
    if require_local_history && !ready.local_history {
        return Err("Collection restarted while opening saved history. Try again.".into());
    }
    let url = ready.url.clone();
    *viewer = Some(Viewer {
        child,
        url: ready.url,
        local_history: ready.local_history,
    });
    Ok(url)
}

// The bundled React tree renders history. Rust owns the loopback credential and
// exposes only the dashboard API, never a general-purpose HTTP proxy.
fn history_endpoint(path: &str, method: &str) -> bool {
    let route = path.split('?').next().unwrap_or("");
    if path.contains("..") || path.contains('\\') || path.contains('#') {
        return false;
    }
    match method {
        "GET" => {
            matches!(
                route,
                "/web/me" | "/web/storage" | "/web/usage" | "/web/source" | "/web/marks" | "/web/collection-health"
            ) || matches!(
                route.strip_prefix("/web/q/"),
                Some(
                    "overview"
                        | "filter-options"
                        | "models"
                        | "machines"
                        | "tools"
                        | "mcp"
                        | "unused-mcp"
                        | "skills"
                        | "unused-skills"
                        | "sessions"
                        | "session"
                        | "patterns"
                        | "pattern-hits"
                        | "pattern-timeline"
                        | "coverage"
                        | "subagents"
                )
            )
        }
        "POST" => matches!(route, "/web/marks" | "/web/source/refresh"),
        "DELETE" => route.strip_prefix("/web/marks/").is_some_and(|id| {
            !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        }),
        _ => false,
    }
}

#[derive(serde::Serialize)]
pub(crate) struct HistoryResponse {
    status: u16,
    body: String,
}

#[tauri::command]
pub(crate) async fn history_request(
    window: Webview,
    app: tauri::AppHandle,
    path: String,
    method: String,
    source: String,
    body: Option<serde_json::Value>,
) -> Result<HistoryResponse, String> {
    // Bootstrap can race with collector restart/token rotation. Retry this read
    // only; never replay mutations or fall back to stale local Parquet.
    let attempts = if source == "local" && method == "GET" && path == "/web/me" { 12 } else { 1 };
    for attempt in 0..attempts {
        let result = history_request_once(window.clone(), app.clone(), path.clone(), method.clone(), source.clone(), body.clone()).await;
        let retry = match &result {
            Err(_) => true,
            Ok(response) => response.status == 401 || response.status >= 500,
        };
        if !retry || attempt + 1 == attempts { return result; }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    unreachable!()
}

async fn history_request_once(
    window: Webview,
    app: tauri::AppHandle,
    path: String,
    method: String,
    source: String,
    body: Option<serde_json::Value>,
) -> Result<HistoryResponse, String> {
    super::local_setup(&window)?;
    if !history_endpoint(&path, &method) || !matches!(source.as_str(), "local" | "s3") {
        return Err("Unsupported history request".into());
    }
    // The collector owns the live DuckDB connection. The standalone viewer is
    // for S3/offline history, not a second, stale local Parquet reader.
    let url = if source == "local" {
        let status = super::read_status().await?;
        if status.running {
            let url: tauri::Url = status.dashboard_url
                .ok_or("Local history service is not ready")?
                .parse().map_err(|_| "Invalid local history address")?;
            if !valid_url(&url) {
                return Err("Local history must use a local address".into());
            }
            url
        } else {
            ensure_viewer(&app, true).await?
        }
    } else {
        ensure_viewer(&app, false).await?
    };
    let token = url
        .query_pairs()
        .find(|(k, _)| k == "t")
        .map(|(_, v)| v.into_owned())
        .ok_or("Missing viewer authentication")?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(190))
        .build()
        .map_err(|_| "Could not initialize history")?;
    let endpoint = url.join(&path).map_err(|_| "Invalid history path")?;
    if endpoint.origin() != url.origin() {
        return Err("Invalid history origin".into());
    }
    let mut request = client
        .request(method.parse().map_err(|_| "Invalid method")?, endpoint)
        .header(
            "Cookie",
            format!("kikimimi_local={token}; kikimimi_view={source}"),
        );
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request
        .send()
        .await
        .map_err(|_| "Could not reach the history service")?;
    let status = response.status().as_u16();
    let body = response
        .text()
        .await
        .map_err(|_| "Could not read history")?;
    Ok(HistoryResponse { status, body })
}

// Only the privileged shell can configure the local reader. Requests are sent
// exclusively to the app-owned loopback server, never to a caller-provided URL.
async fn source_request(
    app: &tauri::AppHandle,
    selection: Option<&serde_json::Value>,
    kind: &str,
    storage: bool,
) -> Result<serde_json::Value, String> {
    let url = ensure_viewer(app, false).await?;
    let token = url
        .query_pairs()
        .find(|(k, _)| k == "t")
        .map(|(_, v)| v.into_owned())
        .ok_or("Missing viewer authentication")?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(190))
        .build()
        .map_err(|_| "Could not initialize source settings")?;
    let endpoint = url
        .join(if storage {
            "/web/storage"
        } else {
            "/web/source"
        })
        .map_err(|_| "Invalid viewer address")?;
    let request = if let Some(body) = selection {
        client.post(endpoint).json(body)
    } else {
        client.get(endpoint)
    };
    let response = request
        .header(
            "Cookie",
            format!("kikimimi_local={token}; kikimimi_view={kind}"),
        )
        .send()
        .await
        .map_err(|_| "Could not reach source settings. Try again.")?;
    let success = response.status().is_success();
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|_| "Invalid source settings response")?;
    if !success {
        return Err(body["error"]
            .as_str()
            .unwrap_or("Could not change data source")
            .to_owned());
    }
    Ok(body)
}

#[tauri::command]
pub(crate) async fn read_storage(
    window: Webview,
    app: tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    super::local_setup(&window)?;
    source_request(&app, None, "local", true).await
}

#[tauri::command]
pub(crate) async fn read_source(
    window: Webview,
    app: tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    super::local_setup(&window)?;
    source_request(&app, None, "local", false).await
}

#[tauri::command]
pub(crate) async fn set_source(
    window: Webview,
    app: tauri::AppHandle,
    selection: serde_json::Value,
) -> Result<(), String> {
    super::local_setup(&window)?;
    let kind = selection["kind"].as_str().ok_or("Choose a data source")?;
    if !matches!(kind, "local" | "s3") {
        return Err("Unknown data source".into());
    }
    // Configuration validates/persists a connection, but never navigates or
    // changes the currently selected reader. Browsing owns those operations.
    if kind == "s3" {
        source_request(&app, Some(&selection), kind, false).await?;
    }
    Ok(())
}

// Only fixed HTTPS Cloud links can leave the application.
pub(crate) async fn open_browser(url: &tauri::Url) -> Result<(), String> {
    if url.scheme() != "https"
        || url.host_str() != Some("kikimimi.dev")
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("Invalid Cloud address".into());
    }
    let status = tokio::process::Command::new("/usr/bin/open")
        .arg(url.as_str())
        .status()
        .await
        .map_err(|_| "Could not open your browser")?;
    if !status.success() {
        return Err("Could not open your browser".into());
    }
    Ok(())
}

// Only known workspace pages may be requested by the local settings UI.
fn cloud_page(page: Option<&str>) -> Result<&'static str, String> {
    match page {
        None | Some("overview") => Ok("/overview"),
        Some("models") => Ok("/models"),
        Some("tools") => Ok("/tools"),
        Some("mcp") => Ok("/mcp"),
        Some("skills") => Ok("/skills"),
        Some("patterns") => Ok("/patterns"),
        Some("sessions") => Ok("/sessions"),
        Some("subagents") => Ok("/subagents"),
        Some("team") => Ok("/team"),
        Some("members") => Ok("/members"),
        Some("devices") => Ok("/devices"),
        Some("allocation") => Ok("/allocation"),
        Some("billing") => Ok("/billing"),
        Some("router") => Ok("/router"),
        _ => Err("Unknown workspace page".into()),
    }
}

#[tauri::command]
pub(crate) async fn show_cloud(window: Webview, page: Option<String>) -> Result<(), String> {
    super::local_setup(&window)?;
    let destination = format!("https://kikimimi.dev{}", cloud_page(page.as_deref())?);
    open_browser(&destination.parse().unwrap()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_transport_cannot_change_connections_or_escape_loopback() {
        assert!(history_endpoint("/web/q/overview?days=14", "GET"));
        assert!(history_endpoint("/web/collection-health", "GET"));
        assert!(!history_endpoint("/web/collection-health", "POST"));
        assert!(history_endpoint("/web/marks", "POST"));
        assert!(history_endpoint("/web/source/refresh", "POST"));
        for (path, method) in [
            ("https://example.com/web/me", "GET"),
            ("//example.com/web/me", "GET"),
            ("/web/source", "POST"),
            ("/web/logout", "POST"),
            ("/web/q/../source", "GET"),
            ("/web/marks/../../source", "DELETE"),
        ] {
            assert!(!history_endpoint(path, method), "{path}");
        }
    }

    #[test]
    fn external_cloud_pages_cannot_escape_the_fixed_origin() {
        assert_eq!(cloud_page(None).unwrap(), "/overview");
        for page in [
            "overview",
            "models",
            "tools",
            "mcp",
            "skills",
            "patterns",
            "sessions",
            "subagents",
            "team",
            "members",
            "devices",
            "allocation",
            "billing",
            "router",
        ] {
            assert_eq!(cloud_page(Some(page)).unwrap(), format!("/{page}"));
        }
        for page in [
            "https://example.com",
            "//example.com",
            "../settings",
            "models?token=test",
        ] {
            assert!(cloud_page(Some(page)).is_err());
        }
    }
    #[test]
    fn viewer_navigation_requires_an_explicit_loopback_port() {
        assert!(valid_url(&"http://127.0.0.1:1234/?t=test".parse().unwrap()));
        for url in [
            "https://example.com:1234",
            "http://127.0.0.1",
            "http://localhost:1234",
            "http://user@127.0.0.1:1234",
        ] {
            assert!(!valid_url(&url.parse().unwrap()));
        }
    }
}
