#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{path::PathBuf, time::Duration};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager,
};

mod native_ui;
mod collection;
mod dashboard;
mod update_bundle;
mod update_flow;
mod updates;

#[derive(serde::Serialize, serde::Deserialize)]
struct Status {
    running: bool,
    #[serde(default)]
    health: Option<serde_json::Value>,
    #[serde(default)]
    collection_target: Option<serde_json::Value>,
    #[serde(default)]
    applied_collection_target: Option<serde_json::Value>,
    service_installed: bool,
    // Keep local authentication available to native history requests, never the UI.
    #[serde(skip_serializing)]
    dashboard_url: Option<String>,
    duckdb_available: bool,
    web_error: Option<String>,
    has_history: bool,
    #[serde(default)]
    has_s3_reader: bool,
    last_event_ts: Option<i64>,
    #[serde(default)]
    cloud_sync: Option<serde_json::Value>,
}

struct Operations(tokio::sync::Mutex<()>);

fn binaries() -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let directory = executable.parent().ok_or("Cannot locate application")?;
    // Tauri copies externalBin files beside the app executable, including in dev.
    Ok(directory.to_path_buf())
}

fn collector_command() -> Result<tokio::process::Command, String> {
    let directory = binaries()?;
    let mut paths = vec![
        directory.clone(),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
        PathBuf::from("/usr/sbin"),
        PathBuf::from("/sbin"),
    ];
    // Finder-launched apps do not inherit the user's shell PATH.
    if let Some(home) = std::env::var_os("HOME") {
        paths.push(PathBuf::from(home).join(".local/bin"));
    }
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let path = std::env::join_paths(paths).map_err(|e| e.to_string())?;
    let mut command = tokio::process::Command::new(directory.join("kikimimi"));
    command.env("PATH", path).kill_on_drop(true);
    Ok(command)
}

async fn cli(args: &[&str]) -> Result<String, String> {
    let mut command = collector_command()?;
    command.args(args);
    let output = tokio::time::timeout(Duration::from_secs(45), command.output())
        .await
        .map_err(|_| "Operation timed out. Refresh status before retrying.".to_string())?
        .map_err(|e| format!("Cannot run bundled collector: {e}"))?;
    if !output.status.success() {
        // init's stdout may contain the local OTLP bearer token. Never display it.
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    String::from_utf8(output.stdout).map_err(|e| e.to_string())
}

fn local_setup(window: &tauri::Webview) -> Result<(), String> {
    let url = window.url().map_err(|e| e.to_string())?;
    // tauri dev serves frontendDist through its configured loopback dev server.
    // Trust that exact configured origin only in debug builds, never any loopback URL.
    let development_origin = if cfg!(debug_assertions) {
        window.app_handle().config().build.dev_url.as_ref()
    } else {
        None
    };
    if !trusted_shell(window.label(), &url, development_origin) {
        return Err("Only the setup window may manage collection".into());
    }
    Ok(())
}

fn trusted_shell(label: &str, url: &tauri::Url, development_origin: Option<&tauri::Url>) -> bool {
    label == "main"
        && ((url.scheme() == "tauri" && url.host_str() == Some("localhost"))
            || development_origin.is_some_and(|expected| expected.origin() == url.origin()))
}

async fn read_status() -> Result<Status, String> {
    serde_json::from_str(&cli(&["desktop", "status"]).await?).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_ui_language(window: tauri::Webview, app: tauri::AppHandle, locale: String) -> Result<(), String> {
    local_setup(&window)?;
    native_ui::apply(&app, &locale)
}

#[tauri::command]
async fn status(window: tauri::Webview, app: tauri::AppHandle) -> Result<Status, String> {
    local_setup(&window)?;
    let state = read_status().await?;
    if let Some(tray) = app.tray_by_id("kikimimi") {
        let summary = if state.running {
            if native_ui::is_japanese(&app) { "収集中" } else { "Collecting" }
        } else {
            if native_ui::is_japanese(&app) { "収集はオフです" } else { "Collection is off" }
        };
        let _ = tray.set_tooltip(Some(format!("kikimimi — {summary}")));
    }
    Ok(state)
}

#[tauri::command]
async fn enable(
    window: tauri::Webview,
    app: tauri::AppHandle,
    operations: tauri::State<'_, Operations>,
) -> Result<(), String> {
    local_setup(&window)?;
    let _guard = operations.0.lock().await;
    if !cfg!(debug_assertions)
        && !std::env::current_exe()
            .map_err(|e| e.to_string())?
            .starts_with("/Applications/kikimimi.app/Contents/MacOS")
    {
        return Err(
            "Move kikimimi to Applications and reopen it before enabling collection.".into(),
        );
    }
    dashboard::close_viewer(&app).await;
    cli(&["desktop", "enable"]).await?;
    wait_for_collector().await
}

#[tauri::command]
async fn resume(
    window: tauri::Webview,
    app: tauri::AppHandle,
    operations: tauri::State<'_, Operations>,
) -> Result<(), String> {
    local_setup(&window)?;
    let _guard = operations.0.lock().await;
    dashboard::close_viewer(&app).await;
    cli(&["desktop", "resume"]).await?;
    wait_for_collector().await
}

async fn wait_for_collector() -> Result<(), String> {
    for _ in 0..30 {
        if read_status().await?.running {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err("Collection did not start. Your settings were saved; try resuming collection.".into())
}

#[tauri::command]
async fn disconnect(
    window: tauri::Webview,
    operations: tauri::State<'_, Operations>,
) -> Result<(), String> {
    local_setup(&window)?;
    let _guard = operations.0.lock().await;
    disconnect_collection().await
}

#[tauri::command]
async fn reset_setup(
    window: tauri::Webview,
    app: tauri::AppHandle,
    operations: tauri::State<'_, Operations>,
) -> Result<(), String> {
    local_setup(&window)?;
    let _guard = operations.0.lock().await;
    dashboard::close_viewer(&app).await;
    disconnect_collection().await?;
    cli(&["desktop", "reset-collection-settings"]).await?;
    Ok(())
}

async fn disconnect_collection() -> Result<(), String> {
    cli(&["uninstall"]).await?;
    for _ in 0..20 {
        let state = read_status().await?;
        if !state.running && !state.service_installed {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err("Settings were removed, but the collector or login service is still active. Refresh status before removing the app.".into())
}

#[tauri::command]
async fn opencode_router_status(window: tauri::Webview) -> Result<serde_json::Value, String> {
    local_setup(&window)?;
    serde_json::from_str(&cli(&["desktop", "open-code-router-status"]).await?)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn connect_opencode_router(
    window: tauri::Webview,
    app: tauri::AppHandle,
    slug: String,
) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    local_setup(&window)?;
    let operations = app.state::<Operations>();
    let _guard = operations.0.lock().await;
    let status: serde_json::Value =
        serde_json::from_str(&cli(&["desktop", "open-code-router-status"]).await?)
            .map_err(|_| "Could not read OpenCode settings")?;
    if status["enabled"] == true {
        return Err("Restore the previous provider before changing teams".into());
    }
    if slug.is_empty() || status["host_id"].as_str().is_none() {
        return Err("Choose a team workspace".into());
    }
    let cloud =
        collection::authorize_cloud(&status["host_id"], &slug, &serde_json::json!([])).await?;
    if cloud["org_kind"] != "team" {
        return Err("Choose a team workspace".into());
    }
    let mut command = collector_command()?;
    command
        .args(["desktop", "open-code-router-connect"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|_| "Could not save router connection")?;
    let request = serde_json::to_vec(&cloud).map_err(|_| "Invalid router connection")?;
    let operation = async {
        let mut input = child
            .stdin
            .take()
            .ok_or("Could not save router connection")?;
        input
            .write_all(&request)
            .await
            .map_err(|_| "Could not save router connection")?;
        drop(input);
        let result = child
            .wait()
            .await
            .map_err(|_| "Could not save router connection")?;
        if !result.success() {
            return Err("Could not save router connection".to_string());
        }
        Ok(())
    };
    tokio::time::timeout(Duration::from_secs(30), operation)
        .await
        .map_err(|_| {
            "Saving router connection timed out. Refresh its status before retrying.".to_string()
        })?
}

/// Updates OpenCode's global provider and its private key file.
#[tauri::command]
async fn set_opencode_router(
    window: tauri::Webview,
    operations: tauri::State<'_, Operations>,
    enabled: bool,
) -> Result<(), String> {
    local_setup(&window)?;
    let _guard = operations.0.lock().await;
    let action = if enabled {
        "open-code-router-enable"
    } else {
        "open-code-router-disable"
    };
    cli(&["desktop", action]).await.map(|_| ())
}

fn navigate(app: &tauri::AppHandle, view: &str) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
    let _ = app.emit_to("main", "navigate", view);
}

fn main() {
    let builder = tauri::Builder::default();
    // Unsigned/development builds have no updater configuration or trusted key.
    // Do not initialize a plugin which would reject that absent configuration.
    let builder = if updates::configured_build() {
        builder.plugin(tauri_plugin_updater::Builder::new().build())
    } else {
        builder
    };
    builder
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            navigate(app, "dashboard")
        }))
        .manage(Operations(tokio::sync::Mutex::new(())))
        .manage(dashboard::Dashboard::default())
        .invoke_handler(tauri::generate_handler![
            collection::switch_collection,
            collection::collection_workspaces,
            dashboard::show_cloud,
            dashboard::read_source,
            dashboard::read_storage,
            dashboard::set_source,
            status,
            enable,
            resume,
            disconnect,
            reset_setup,
            opencode_router_status,
            connect_opencode_router,
            set_opencode_router,
            dashboard::history_request,
            updates::update_status,
            updates::check_updates,
            updates::install_update,
            updates::set_auto_update,
            set_ui_language
        ])
        .setup(|app| {
            app.manage(native_ui::Language::default());
            updates::start(app.handle())?;
            let show = MenuItem::with_id(app, "show", "Open Dashboard", true, None::<&str>)?;
            let settings =
                MenuItem::with_id(app, "settings", "App Settings…", true, Some("CmdOrCtrl+,"))?;
            let native_menu = tauri::menu::MenuBuilder::new(app)
                .item(
                    &tauri::menu::SubmenuBuilder::new(app, "kikimimi")
                        .item(&settings)
                        .separator()
                        .quit()
                        .build()?,
                )
                .item(
                    &tauri::menu::SubmenuBuilder::new(app, "Edit")
                        .undo()
                        .redo()
                        .separator()
                        .cut()
                        .copy()
                        .paste()
                        .select_all()
                        .build()?,
                )
                .build()?;
            app.set_menu(native_menu)?;
            let quit = MenuItem::with_id(
                app,
                "quit",
                "Quit app (collection continues)",
                true,
                None::<&str>,
            )?;
            let menu = Menu::with_items(app, &[&show, &settings, &quit])?;
            // Retina-resolution monochrome logo; macOS applies the menu bar tint.
            let rgba = include_bytes!("../../tray.rgba").to_vec();
            TrayIconBuilder::with_id("kikimimi")
                .icon(tauri::image::Image::new_owned(rgba, 36, 36))
                .icon_as_template(true)
                .tooltip("kikimimi")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => navigate(app, "dashboard"),
                    "settings" => navigate(app, "settings"),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;
            Ok(())
        })
        .on_menu_event(|app, event| {
            if event.id.as_ref() == "settings" {
                navigate(app, "settings");
            }
        })
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("Unable to build kikimimi desktop")
        .run(|app, event| {
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                navigate(app, "dashboard");
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_keeps_dashboard_auth_native_and_preserves_public_fields() {
        for dashboard_url in [
            serde_json::json!("http://127.0.0.1:1234/?t=test-only-token"),
            serde_json::Value::Null,
        ] {
            let mut payload = serde_json::json!({
                "running": dashboard_url.is_string(),
                "health": {"collector": "running", "snapshot_fresh": true},
                "collection_target": {"cloud": null, "s3": null},
                "applied_collection_target": {"cloud": null, "s3": null},
                "service_installed": true,
                "dashboard_url": dashboard_url,
                "duckdb_available": true,
                "web_error": null,
                "has_history": true,
                "has_s3_reader": false,
                "last_event_ts": 123,
                "cloud_sync": {"pending": 0, "last_error": null, "last_push_at": null}
            });
            let state: Status = serde_json::from_value(payload.clone()).unwrap();
            assert_eq!(state.dashboard_url.as_deref(), dashboard_url.as_str());

            payload.as_object_mut().unwrap().remove("dashboard_url");
            let public_status = serde_json::to_value(&state).unwrap();
            assert_eq!(public_status, payload);
            assert!(!public_status.to_string().contains("test-only-token"));
        }
    }

    #[test]
    fn management_commands_only_accept_the_shell_and_its_configured_dev_origin() {
        let development: tauri::Url = "http://127.0.0.1:1420".parse().unwrap();
        assert!(trusted_shell(
            "main",
            &"tauri://localhost".parse().unwrap(),
            None
        ));
        assert!(trusted_shell("main", &development, Some(&development)));
        assert!(!trusted_shell("main", &development, None));
        assert!(!trusted_shell(
            "main",
            &"http://127.0.0.1:9999".parse().unwrap(),
            Some(&development)
        ));
        assert!(!trusted_shell(
            "dashboard",
            &development,
            Some(&development)
        ));
        assert!(!trusted_shell(
            "main",
            &"https://example.com".parse().unwrap(),
            Some(&development)
        ));
    }
}
