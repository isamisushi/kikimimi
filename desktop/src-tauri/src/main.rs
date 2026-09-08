#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{path::PathBuf, time::Duration};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Manager,
};

mod update_bundle;
mod update_flow;
mod updates;

#[derive(serde::Serialize, serde::Deserialize)]
struct Status {
    running: bool,
    service_installed: bool,
    dashboard_url: Option<String>,
    duckdb_available: bool,
    web_error: Option<String>,
}

struct Operations(tokio::sync::Mutex<()>);

fn binaries() -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let directory = executable.parent().ok_or("Cannot locate application")?;
    // Tauri copies externalBin files beside the app executable, including in dev.
    Ok(directory.to_path_buf())
}

async fn cli(args: &[&str]) -> Result<String, String> {
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
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let path = std::env::join_paths(paths).map_err(|e| e.to_string())?;
    let mut command = tokio::process::Command::new(directory.join("kikimimi"));
    command.args(args).env("PATH", path).kill_on_drop(true);
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

fn local_setup(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() != "main" {
        return Err("Only the setup window may manage collection".into());
    }
    Ok(())
}

async fn read_status() -> Result<Status, String> {
    serde_json::from_str(&cli(&["desktop", "status"]).await?).map_err(|e| e.to_string())
}

#[tauri::command]
async fn status(window: tauri::WebviewWindow) -> Result<Status, String> {
    local_setup(&window)?;
    read_status().await
}

#[tauri::command]
async fn enable(
    window: tauri::WebviewWindow,
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
    for _ in 0..30 {
        if read_status().await?.dashboard_url.is_some() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err("Settings were saved, but the dashboard is not ready. Check status and retry.".into())
}

#[tauri::command]
async fn disconnect(
    window: tauri::WebviewWindow,
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

#[tauri::command]
async fn open_dashboard(window: tauri::WebviewWindow, app: tauri::AppHandle) -> Result<(), String> {
    local_setup(&window)?;
    let url: tauri::Url = read_status()
        .await?
        .dashboard_url
        .ok_or("Collector is not ready")?
        .parse()
        .map_err(|_| "Invalid dashboard URL")?;
    if url.scheme() != "http" || url.host_str() != Some("127.0.0.1") || url.port().is_none() {
        return Err("Dashboard must use a loopback address".into());
    }
    let label = format!("dashboard-{}", url.port().unwrap());
    if let Some(existing) = app.get_webview_window(&label) {
        existing.navigate(url).map_err(|e| e.to_string())?;
        existing.show().map_err(|e| e.to_string())?;
        existing.set_focus().map_err(|e| e.to_string())?;
        return Ok(());
    }
    for (name, previous) in app.webview_windows() {
        if name.starts_with("dashboard-") {
            previous.close().map_err(|e| e.to_string())?;
        }
    }
    let origin = url.origin();
    tauri::WebviewWindowBuilder::new(&app, label, tauri::WebviewUrl::External(url))
        .title("kikimimi — Dashboard")
        .inner_size(1200.0, 800.0)
        .on_navigation(move |next| next.origin() == origin)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn show_setup(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            show_setup(app)
        }))
        .manage(Operations(tokio::sync::Mutex::new(())))
        .invoke_handler(tauri::generate_handler![
            status,
            enable,
            disconnect,
            open_dashboard,
            updates::update_status,
            updates::check_updates,
            updates::install_update,
            updates::set_auto_update
        ])
        .setup(|app| {
            updates::start(app.handle())?;
            let show = MenuItem::with_id(app, "show", "Open kikimimi", true, None::<&str>)?;
            let quit = MenuItem::with_id(
                app,
                "quit",
                "Quit app (collection continues)",
                true,
                None::<&str>,
            )?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            // Small monochrome template icon; no generated image asset required.
            let mut rgba = vec![0u8; 18 * 18 * 4];
            for y in 3..15 {
                for x in 3..15 {
                    if x == 4 || x == 8 || (x == 12 && (6..12).contains(&y)) {
                        rgba[(y * 18 + x) * 4 + 3] = 255;
                    }
                }
            }
            TrayIconBuilder::new()
                .icon(tauri::image::Image::new_owned(rgba, 18, 18))
                .icon_as_template(true)
                .tooltip("kikimimi")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_setup(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("Unable to run kikimimi desktop");
}
