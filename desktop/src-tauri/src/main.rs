#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{path::PathBuf, time::Duration};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager,
};

mod collection;
mod dashboard;
mod update_bundle;
mod update_flow;
mod updates;

#[derive(serde::Serialize, serde::Deserialize)]
struct Status {
    running: bool,
    #[serde(default)]
    collection_target: Option<serde_json::Value>,
    #[serde(default)]
    applied_collection_target: Option<serde_json::Value>,
    service_installed: bool,
    dashboard_url: Option<String>,
    duckdb_available: bool,
    web_error: Option<String>,
    has_history: bool,
    #[serde(default)]
    has_s3_reader: bool,
    last_event_ts: Option<i64>,
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
async fn status(window: tauri::Webview, app: tauri::AppHandle) -> Result<Status, String> {
    local_setup(&window)?;
    let state = read_status().await?;
    if let Some(tray) = app.tray_by_id("kikimimi") {
        let summary = if state.running {
            "Collecting"
        } else {
            "Collection is off"
        };
        let _ = tray.set_tooltip(Some(format!("kikimimi — {summary}")));
    }
    Ok(state)
}

#[tauri::command]
async fn enable(
    window: tauri::Webview,
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
    cli(&["desktop", "enable"]).await?;
    wait_for_collector().await
}

#[tauri::command]
async fn resume(
    window: tauri::Webview,
    operations: tauri::State<'_, Operations>,
) -> Result<(), String> {
    local_setup(&window)?;
    let _guard = operations.0.lock().await;
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

fn navigate(app: &tauri::AppHandle, view: &str) {
    if let Some(window) = app.get_window("main") {
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
            dashboard::show_cloud,
            dashboard::cloud_workspaces,
            dashboard::select_cloud_workspace,
            dashboard::read_source,
            dashboard::read_storage,
            dashboard::set_source,
            status,
            enable,
            resume,
            disconnect,
            dashboard::show_dashboard,
            dashboard::hide_dashboard,
            updates::update_status,
            updates::check_updates,
            updates::install_update,
            updates::set_auto_update
        ])
        .setup(|app| {
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
            // Small monochrome template icon; no generated image asset required.
            let mut rgba = vec![0u8; 18 * 18 * 4];
            for y in 3..15 {
                for x in 3..15 {
                    if x == 4 || x == 8 || (x == 12 && (6..12).contains(&y)) {
                        rgba[(y * 18 + x) * 4 + 3] = 255;
                    }
                }
            }
            TrayIconBuilder::with_id("kikimimi")
                .icon(tauri::image::Image::new_owned(rgba, 18, 18))
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
                if matches!(
                    event,
                    tauri::WindowEvent::Resized(_) | tauri::WindowEvent::ScaleFactorChanged { .. }
                ) {
                    dashboard::resize(window);
                }
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
