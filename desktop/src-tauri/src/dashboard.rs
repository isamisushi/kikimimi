//! Local history transport and an isolated, unprivileged remote Cloud webview.
use std::process::Stdio;
use tauri::{Emitter, Manager, Webview, WebviewUrl};
use tokio::io::{AsyncBufReadExt, BufReader};

// Keep in sync with the fixed app toolbar + collection bar in src/shell.css.
const HEADER_HEIGHT: f64 = 116.0;

struct Viewer {
    child: tokio::process::Child,
    url: tauri::Url,
}

#[derive(Default)]
pub(crate) struct Dashboard(tokio::sync::Mutex<Option<Viewer>>);

/// Visibility follows document readiness, not the completion of navigate().
/// A late load must never reveal a webview after the user switches to another tab.
#[derive(Default)]
pub(crate) struct CloudDisplay(std::sync::Mutex<Presentation>);

#[derive(Default)]
struct Presentation {
    requested: bool,
    pending: Option<tauri::Url>,
    reload: bool,
    generation: u64,
}

impl Presentation {
    fn begin(&mut self, url: tauri::Url) {
        self.generation += 1;
        self.pending = Some(url);
    }

    fn ready(&mut self, generation: u64) -> bool {
        if self.generation != generation || self.pending.is_none() {
            return false;
        }
        self.pending = None;
        self.requested
    }

    fn fail(&mut self, generation: u64) -> bool {
        if self.generation != generation || self.pending.is_none() {
            return false;
        }
        self.pending = None;
        self.reload = true;
        self.requested
    }

    fn finish(&mut self, url: &tauri::Url) -> bool {
        if self.pending.as_ref() != Some(url) {
            return false;
        }
        self.ready(self.generation)
    }
}

fn valid_url(url: &tauri::Url) -> bool {
    url.scheme() == "http"
        && url.host_str() == Some("127.0.0.1")
        && url.port().is_some()
        && url.username().is_empty()
        && url.password().is_none()
}

async fn ensure_viewer(app: &tauri::AppHandle) -> Result<tauri::Url, String> {
    let dashboard = app.state::<Dashboard>();
    let mut viewer = dashboard.0.lock().await;
    if let Some(existing) = viewer.as_mut() {
        if existing
            .child
            .try_wait()
            .map_err(|e| e.to_string())?
            .is_none()
        {
            return Ok(existing.url.clone());
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
    }
    // Do not echo the startup line: it contains the local authentication token.
    let ready: Ready = serde_json::from_str(&line).map_err(|_| "Invalid viewer startup status")?;
    if !valid_url(&ready.url) {
        return Err("The viewer must use a local address".into());
    }
    let url = ready.url.clone();
    *viewer = Some(Viewer {
        child,
        url: ready.url,
    });
    Ok(url)
}

// Child webviews use the window frame on macOS, while the shell's CSS begins
// below the title bar. Account for that inset (zero in fullscreen).
fn child_top(window: &tauri::Window) -> f64 {
    #[cfg(target_os = "macos")]
    if !window.is_fullscreen().unwrap_or(false) {
        // On macOS the public position API can report identical inner/outer
        // origins. Size differences include the standard decorated title bar.
        if let (Ok(inner), Ok(outer), Ok(scale)) = (
            window.inner_size(),
            window.outer_size(),
            window.scale_factor(),
        ) {
            let inset = outer.height.saturating_sub(inner.height) as f64 / scale;
            return HEADER_HEIGHT + if inset > 0.0 { inset } else { 32.0 };
        }
    }
    HEADER_HEIGHT
}

pub(crate) fn resize(window: &tauri::Window) {
    for label in ["cloud"] {
        if let (Some(view), Ok(size), Ok(scale)) = (
            window.app_handle().get_webview(label),
            window.inner_size(),
            window.scale_factor(),
        ) {
            let size = size.to_logical::<f64>(scale);
            let _ = view.set_bounds(tauri::Rect {
                position: tauri::LogicalPosition::new(0.0, child_top(window)).into(),
                size: tauri::LogicalSize::new(size.width, (size.height - HEADER_HEIGHT).max(1.0))
                    .into(),
            });
        }
    }
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
                "/web/me" | "/web/storage" | "/web/usage" | "/web/source" | "/web/marks"
            ) || matches!(
                route.strip_prefix("/web/q/"),
                Some(
                    "overview"
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
    super::local_setup(&window)?;
    if !history_endpoint(&path, &method) || !matches!(source.as_str(), "local" | "s3") {
        return Err("Unsupported history request".into());
    }
    let url = ensure_viewer(&app).await?;
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

#[tauri::command]
pub(crate) async fn hide_dashboard(window: Webview, app: tauri::AppHandle) -> Result<(), String> {
    super::local_setup(&window)?;
    app.state::<CloudDisplay>().0.lock().unwrap().requested = false;
    for label in ["cloud"] {
        if let Some(view) = app.get_webview(label) {
            view.hide().map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

// Only the privileged shell can configure the local reader. Requests are sent
// exclusively to the app-owned loopback server, never to a caller-provided URL.
async fn source_request(
    app: &tauri::AppHandle,
    selection: Option<&serde_json::Value>,
    kind: &str,
    storage: bool,
) -> Result<serde_json::Value, String> {
    let url = ensure_viewer(app).await?;
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
    let url = ensure_viewer(&app).await?;
    let cookie_view = app
        .get_webview("dashboard")
        .unwrap_or_else(|| window.clone());
    let is_s3 = cookie_view
        .cookies_for_url(url)
        .map_err(|_| "Could not read data source")?
        .iter()
        .any(|cookie| cookie.name() == "kikimimi_view" && cookie.value() == "s3");
    source_request(&app, None, if is_s3 { "s3" } else { "local" }, false).await
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

// Fixed Cloud endpoints only; session cookies never cross the native boundary.
async fn cloud_request(
    window: &Webview,
    app: &tauri::AppHandle,
    slug: Option<&str>,
) -> Result<serde_json::Value, String> {
    let origin: tauri::Url = "https://kikimimi.dev/".parse().unwrap();
    let view = app.get_webview("cloud").unwrap_or_else(|| window.clone());
    let cookies = view
        .cookies_for_url(origin.clone())
        .map_err(|_| "Could not read Cloud sign-in")?;
    let Some(session) = cookies.iter().find(|c| c.name() == "kikimimi_session") else {
        return Ok(serde_json::Value::Null);
    };
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|_| "Could not connect to Cloud")?;
    let request = if let Some(slug) = slug {
        client
            .post(origin.join("web/active-org").unwrap())
            .json(&serde_json::json!({ "slug": slug }))
    } else {
        client.get(origin.join("web/me").unwrap())
    };
    let response = request
        .header("Cookie", format!("kikimimi_session={}", session.value()))
        .send()
        .await
        .map_err(|_| "Could not reach Cloud. Check your connection and retry.")?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Ok(serde_json::Value::Null);
    }
    if !response.status().is_success() {
        return Err("Could not access this Cloud workspace. Sign in and try again.".into());
    }
    response
        .json()
        .await
        .map_err(|_| "Invalid Cloud response".into())
}

#[tauri::command]
pub(crate) async fn cloud_workspaces(
    window: Webview,
    app: tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    super::local_setup(&window)?;
    let session = cloud_request(&window, &app, None).await?;
    if session.is_null() {
        return Ok(session);
    }
    Ok(serde_json::json!({ "orgs": session["orgs"], "active_org": session["active_org"] }))
}

#[tauri::command]
pub(crate) async fn select_cloud_workspace(
    window: Webview,
    app: tauri::AppHandle,
    slug: String,
) -> Result<(), String> {
    super::local_setup(&window)?;
    if cloud_request(&window, &app, Some(&slug)).await?.is_null() {
        return Err("Sign in to Kikimimi Cloud to open this workspace.".into());
    }
    // Defer navigation to show_cloud so it can hide the old document first.
    app.state::<CloudDisplay>().0.lock().unwrap().reload = true;
    Ok(())
}

// Cloud and its GitHub sign-in run without native capabilities. Never load remote
// content into the privileged main webview or forward local authentication tokens.
fn cloud_navigation(url: &tauri::Url) -> bool {
    url.scheme() == "https"
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
        && matches!(url.host_str(), Some("kikimimi.dev" | "github.com"))
}

// Only known workspace pages may be requested by the local settings UI.
fn workspace_page(page: Option<&str>, cloud: bool) -> Result<&'static str, String> {
    match page {
        None | Some("overview") => Ok(if cloud { "/overview" } else { "/" }),
        Some("models") => Ok("/models"),
        Some("tools") => Ok("/tools"),
        Some("mcp") => Ok("/mcp"),
        Some("skills") => Ok("/skills"),
        Some("patterns") => Ok("/patterns"),
        Some("sessions") => Ok("/sessions"),
        Some("subagents") => Ok("/subagents"),
        Some("team") if cloud => Ok("/team"),
        Some("members") if cloud => Ok("/members"),
        Some("devices") if cloud => Ok("/devices"),
        Some("allocation") if cloud => Ok("/allocation"),
        Some("billing") if cloud => Ok("/billing"),
        Some("router") if cloud => Ok("/router"),
        _ => Err("Unknown workspace page".into()),
    }
}

#[tauri::command]
pub(crate) async fn show_cloud(
    window: Webview,
    app: tauri::AppHandle,
    page: Option<String>,
) -> Result<(), String> {
    super::local_setup(&window)?;
    let destination = format!(
        "https://kikimimi.dev{}",
        workspace_page(page.as_deref(), true)?
    );
    let destination: tauri::Url = destination.parse().unwrap();
    let display = app.state::<CloudDisplay>();
    let reload = {
        let mut state = display.0.lock().unwrap();
        state.requested = true;
        std::mem::take(&mut state.reload)
    };
    if let Some(cloud) = app.get_webview("cloud") {
        if reload || (page.is_some() && cloud.url().ok().as_ref() != Some(&destination)) {
            cloud.hide().map_err(|e| e.to_string())?;
            display.0.lock().unwrap().begin(destination.clone());
            cloud
                .navigate(destination)
                .map_err(|_| "Could not open Cloud settings")?;
        } else if display.0.lock().unwrap().pending.is_none() {
            cloud.show().map_err(|e| e.to_string())?;
        }
        return Ok(());
    }
    display.0.lock().unwrap().begin(destination.clone());
    let parent = app
        .get_window("main")
        .ok_or("The app window is unavailable")?;
    let size = parent
        .inner_size()
        .map_err(|e| e.to_string())?
        .to_logical::<f64>(parent.scale_factor().map_err(|e| e.to_string())?);
    let shell = app.clone();
    parent
        .add_child(
            tauri::webview::WebviewBuilder::new(
                "cloud",
                WebviewUrl::External(destination),
            )
            // UX hint only: all authentication remains on the Cloud origin.
            // The remote page has no native capabilities or local credentials.
            .initialization_script(include_str!("cloud-presentation.js"))
            .on_navigation(move |next| {
                if cloud_navigation(next) && next.host_str() == Some("kikimimi.dev")
                    && matches!(next.path(), "/__desktop_cloud_ready" | "/__desktop_cloud_failed") {
                    let generation = next.query_pairs().find(|(key, _)| key == "generation")
                        .and_then(|(_, value)| value.parse::<u64>().ok());
                    if let Some(generation) = generation {
                        let success = next.path() == "/__desktop_cloud_ready";
                        let act = {
                            let display = shell.state::<CloudDisplay>();
                            let mut state = display.0.lock().unwrap();
                            if success { state.ready(generation) } else { state.fail(generation) }
                        };
                        if act {
                            if success {
                                if let Some(view) = shell.get_webview("cloud") { let _ = view.show(); }
                            } else {
                                let _ = shell.emit_to("main", "cloud-load-error", ());
                            }
                        }
                    }
                    return false;
                }
                if cloud_navigation(next) && next.host_str() == Some("kikimimi.dev") && next.path() == "/__desktop_workspace" {
                    let _ = shell.emit_to("main", "navigate", "workspace");
                    return false;
                }
                if !cloud_navigation(next) { return false; }
                if let Some(view) = shell.get_webview("cloud") {
                    if view.url().ok().is_some_and(|current| current.origin() == next.origin()
                        && current.path() == next.path() && current.query() == next.query()
                        && current.fragment() != next.fragment()) {
                        return true;
                    }
                    let _ = view.hide();
                }
                shell.state::<CloudDisplay>().0.lock().unwrap().begin(next.clone());
                true
            })
            .on_page_load(|view, payload| {
                match payload.event() {
                    tauri::webview::PageLoadEvent::Started => { let _ = view.hide(); }
                    tauri::webview::PageLoadEvent::Finished => {
                        if payload.url().host_str() == Some("kikimimi.dev") {
                            let generation = {
                                let display = view.app_handle().state::<CloudDisplay>();
                                let state = display.0.lock().unwrap();
                                (state.pending.as_ref() == Some(payload.url())).then_some(state.generation)
                            };
                            if let Some(generation) = generation {
                                let script = format!("({})({generation});", include_str!("presentation-readiness.js"));
                                let _ = view.eval(script);
                            }
                        } else {
                            // OAuth pages are ordinary documents, outside the Cloud SPA.
                            let reveal = view.app_handle().state::<CloudDisplay>()
                                .0.lock().unwrap().finish(payload.url());
                            if reveal { let _ = view.show(); }
                        }
                    }
                }
            })
            .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny),
            tauri::LogicalPosition::new(0.0, child_top(&parent)),
            tauri::LogicalSize::new(size.width, (size.height - HEADER_HEIGHT).max(1.0)),
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_transport_cannot_change_connections_or_escape_loopback() {
        assert!(history_endpoint("/web/q/overview?days=14", "GET"));
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
    fn cloud_render_signal_must_belong_to_the_current_document() {
        let mut state = Presentation {
            requested: true,
            ..Default::default()
        };
        let url: tauri::Url = "https://kikimimi.dev/overview".parse().unwrap();
        state.begin(url.clone());
        let old = state.generation;
        state.begin(url);
        assert!(
            !state.ready(old),
            "even the same URL can have a stale render signal"
        );
        assert!(!state.fail(old));
        assert!(state.ready(state.generation));
        assert!(
            !state.ready(state.generation),
            "duplicate signals cannot reveal twice"
        );
    }

    #[test]
    fn cloud_render_timeout_allows_retry_and_ignores_hidden_tabs() {
        let mut state = Presentation {
            requested: true,
            ..Default::default()
        };
        state.begin("https://kikimimi.dev/overview".parse().unwrap());
        assert!(state.fail(state.generation));
        assert!(state.reload);
        state.begin("https://kikimimi.dev/overview".parse().unwrap());
        state.requested = false;
        assert!(!state.fail(state.generation));
    }

    #[test]
    fn cloud_stays_hidden_until_the_requested_document_finishes() {
        let mut state = Presentation {
            requested: true,
            ..Default::default()
        };
        let old: tauri::Url = "https://kikimimi.dev/workspace".parse().unwrap();
        let next: tauri::Url = "https://kikimimi.dev/overview".parse().unwrap();
        state.begin(next.clone());
        assert!(
            !state.finish(&old),
            "late completion of the old page must not reveal it"
        );
        assert!(state.pending.is_some());
        assert!(state.finish(&next));
        assert!(state.pending.is_none());
    }

    #[test]
    fn cloud_finishing_in_background_does_not_replace_settings() {
        let mut state = Presentation {
            requested: true,
            ..Default::default()
        };
        let url: tauri::Url = "https://kikimimi.dev/overview".parse().unwrap();
        state.begin(url.clone());
        state.requested = false;
        assert!(!state.finish(&url));
        assert!(
            state.pending.is_none(),
            "the loaded page can be reused later"
        );
    }

    #[test]
    fn cloud_redirects_wait_for_the_final_document() {
        let mut state = Presentation {
            requested: true,
            ..Default::default()
        };
        let overview: tauri::Url = "https://kikimimi.dev/overview".parse().unwrap();
        let login: tauri::Url = "https://kikimimi.dev/login".parse().unwrap();
        state.begin(overview.clone());
        state.begin(login.clone());
        assert!(!state.finish(&overview));
        assert!(state.finish(&login));
    }

    #[test]
    fn workspace_pages_are_explicit_and_source_scoped() {
        assert_eq!(workspace_page(Some("overview"), false).unwrap(), "/");
        assert_eq!(workspace_page(Some("overview"), true).unwrap(), "/overview");
        assert_eq!(workspace_page(Some("models"), false).unwrap(), "/models");
        assert_eq!(workspace_page(Some("members"), true).unwrap(), "/members");
        for page in [
            "members",
            "team",
            "devices",
            "allocation",
            "billing",
            "router",
        ] {
            assert!(workspace_page(Some(page), false).is_err());
            assert_eq!(
                workspace_page(Some(page), true).unwrap(),
                format!("/{page}")
            );
        }
        for page in [
            "https://example.com",
            "//example.com",
            "../settings",
            "models?token=test",
        ] {
            assert!(workspace_page(Some(page), true).is_err());
            assert!(workspace_page(Some(page), false).is_err());
        }
    }
    #[test]
    fn cloud_navigation_is_limited_to_cloud_and_github_over_https() {
        for url in [
            "https://kikimimi.dev/",
            "https://kikimimi.dev/auth/github/callback?code=test",
            "https://github.com/login/oauth/authorize",
        ] {
            assert!(cloud_navigation(&url.parse().unwrap()));
        }
        for url in [
            "http://kikimimi.dev/",
            "https://kikimimi.dev:444/",
            "https://kikimimi.dev.evil.test/",
            "https://github.com.evil.test/",
            "http://127.0.0.1:1234/",
            "tauri://localhost",
            "https://user@github.com/",
        ] {
            assert!(!cloud_navigation(&url.parse().unwrap()));
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
