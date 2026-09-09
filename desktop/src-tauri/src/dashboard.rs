//! A separate, unprivileged webview backed by an app-owned history server.
use std::process::Stdio;
use tauri::{Emitter, Manager, Webview, WebviewUrl};
use tokio::io::{AsyncBufReadExt, BufReader};

// Keep in sync with the fixed app toolbar + collection bar in ui/style.css.
const HEADER_HEIGHT: f64 = 116.0;

fn workspace_navigation_script() -> String {
    format!("window.__desktopWorkspaceStyles = {};\n{}", serde_json::json!(include_str!("../../ui/workspace.css")), include_str!("workspace-navigation.js"))
}

struct Viewer {
    child: tokio::process::Child,
    url: tauri::Url,
}

#[derive(Default)]
pub(crate) struct Dashboard(tokio::sync::Mutex<Option<Viewer>>);

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
    if let Some(previous) = app.get_webview("dashboard") {
        previous.close().map_err(|e| e.to_string())?;
    }
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
    for label in ["dashboard", "cloud"] {
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

#[tauri::command]
pub(crate) async fn show_dashboard(window: Webview, app: tauri::AppHandle, page: Option<String>) -> Result<(), String> {
    #[cfg(debug_assertions)]
    eprintln!("desktop: opening history viewer");
    super::local_setup(&window)?;
    let path = workspace_page(page.as_deref(), false)?;
    let mut url = ensure_viewer(&app).await?;
    if page.is_some() { url.set_path(path); }
    if let Some(cloud) = app.get_webview("cloud") {
        cloud.hide().map_err(|e| e.to_string())?;
    }
    if let Some(existing) = app.get_webview("dashboard") {
        if page.is_some() {
            // The existing view already has its viewer session cookie.
            url.set_query(None);
            existing.navigate(url).map_err(|_| "Could not open workspace page")?;
        }
        existing.show().map_err(|e| e.to_string())?;
        return Ok(());
    }
    let parent = app
        .get_window("main")
        .ok_or("The app window is unavailable")?;
    let size = parent
        .inner_size()
        .map_err(|e| e.to_string())?
        .to_logical::<f64>(parent.scale_factor().map_err(|e| e.to_string())?);
    let origin = url.origin();
    let shell = app.clone();
    parent
        .add_child(
            tauri::webview::WebviewBuilder::new("dashboard", WebviewUrl::External(url))
                .initialization_script(workspace_navigation_script())
                .initialization_script(r#"
                    document.addEventListener('click', event => {
                        const link = event.target instanceof Element ? event.target.closest('a[href]') : null;
                        if (!link) return;
                        const target = new URL(link.href, location.href);
                        if (target.origin === location.origin && target.pathname === '/storage') {
                            event.preventDefault(); event.stopImmediatePropagation();
                            location.assign(['Change source', 'S3 workspace', 'This machine', 'This Mac'].includes(link.textContent.trim()) ? '/__desktop_connection' : '/__desktop_settings');
                        }
                    }, true);
                "#)
                .on_navigation(move |next| {
                    if next.as_str() == "https://kikimimi.dev/"
                        || (next.origin() == origin && matches!(next.path(), "/storage" | "/__desktop_settings" | "/__desktop_connection" | "/__desktop_workspace"))
                    {
                        let _ = shell.emit_to("main", "navigate", if next.path() == "/__desktop_connection" { "connection" } else { "workspace" });
                        return false;
                    }
                    next.origin() == origin
                }),
            tauri::LogicalPosition::new(0.0, child_top(&parent)),
            tauri::LogicalSize::new(size.width, (size.height - HEADER_HEIGHT).max(1.0)),
        )
        .map_err(|e| e.to_string())?;
    #[cfg(debug_assertions)]
    eprintln!("desktop: history webview ready");
    Ok(())
}

#[tauri::command]
pub(crate) async fn hide_dashboard(window: Webview, app: tauri::AppHandle) -> Result<(), String> {
    super::local_setup(&window)?;
    for label in ["dashboard", "cloud"] {
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
    source_request(&app, Some(&selection), kind, false).await?;
    window
        .set_cookie(
            tauri::webview::Cookie::build(("kikimimi_view", kind.to_owned()))
                .domain("127.0.0.1")
                .path("/")
                .http_only(true)
                .same_site(tauri::webview::cookie::SameSite::Strict)
                .max_age(tauri::webview::cookie::time::Duration::days(30))
                .build(),
        )
        .map_err(|_| "Could not apply the data source")?;
    if let Some(view) = app.get_webview("dashboard") {
        view.navigate(ensure_viewer(&app).await?)
            .map_err(|_| "Could not reload the dashboard")?;
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
    if let Some(view) = app.get_webview("cloud") {
        view.navigate("https://kikimimi.dev/overview".parse().unwrap())
            .map_err(|_| "Could not open the workspace")?;
    }
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
    let destination = format!("https://kikimimi.dev{}", workspace_page(page.as_deref(), true)?);
    if let Some(local) = app.get_webview("dashboard") {
        local.hide().map_err(|e| e.to_string())?;
    }
    if let Some(cloud) = app.get_webview("cloud") {
        if page.is_some() {
            cloud
                .navigate(destination.parse().unwrap())
                .map_err(|_| "Could not open Cloud settings")?;
        }
        cloud.show().map_err(|e| e.to_string())?;
        return Ok(());
    }
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
                WebviewUrl::External(destination.parse().unwrap()),
            )
            .initialization_script(workspace_navigation_script())
            .initialization_script(
                r#"
                document.addEventListener('DOMContentLoaded', () => {
                    const style = document.createElement('style');
                    style.textContent = '.org-switcher { display: none !important; }';
                    document.head.appendChild(style);
                });
            "#,
            )
            .on_navigation(move |next| {
                if cloud_navigation(next) && next.host_str() == Some("kikimimi.dev") && next.path() == "/__desktop_workspace" {
                    let _ = shell.emit_to("main", "navigate", "workspace");
                    return false;
                }
                cloud_navigation(next)
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
    fn workspace_pages_are_explicit_and_source_scoped() {
        assert_eq!(workspace_page(Some("overview"), false).unwrap(), "/");
        assert_eq!(workspace_page(Some("overview"), true).unwrap(), "/overview");
        assert_eq!(workspace_page(Some("models"), false).unwrap(), "/models");
        assert_eq!(workspace_page(Some("members"), true).unwrap(), "/members");
        for page in ["members", "team", "devices"] { assert!(workspace_page(Some(page), false).is_err()); }
        for page in ["https://example.com", "//example.com", "../settings", "models?token=test"] {
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
