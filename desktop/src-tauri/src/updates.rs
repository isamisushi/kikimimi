use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{Manager, State, Webview};
use tauri_plugin_updater::UpdaterExt;

const ENDPOINT: &str =
    "https://github.com/isamisushi/kikimimi/releases/download/desktop-updates/latest.json";
const PUBLIC_KEY: Option<&str> = option_env!("KIKIMIMI_UPDATER_PUBLIC_KEY");

#[derive(Clone, serde::Serialize)]
pub(crate) struct View {
    current_version: String,
    available_version: Option<String>,
    auto_update: bool,
    configured: bool,
    phase: String,
    error: Option<String>,
    last_checked: Option<u64>,
}
#[derive(serde::Serialize, serde::Deserialize)]
struct Preferences {
    auto_update: bool,
}

pub(crate) struct Updates {
    view: Mutex<View>,
    directory: PathBuf,
}

fn change(app: &tauri::AppHandle, f: impl FnOnce(&mut View)) {
    f(&mut app.state::<Updates>().view.lock().unwrap());
}
fn save(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut temp =
        tempfile::NamedTempFile::new_in(path.parent().unwrap()).map_err(|e| e.to_string())?;
    temp.write_all(bytes)
        .and_then(|_| temp.as_file().sync_all())
        .map_err(|e| e.to_string())?;
    temp.persist(path).map_err(|e| e.to_string())?;
    fs::File::open(path.parent().unwrap())
        .and_then(|file| file.sync_all())
        .map_err(|e| e.to_string())
}
fn clear_marker(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

pub(crate) fn configured_build() -> bool {
    PUBLIC_KEY.is_some_and(|key| !key.trim().is_empty()) && !cfg!(debug_assertions)
}

pub(crate) fn start(app: &tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let directory = app.path().app_config_dir()?;
    fs::create_dir_all(&directory)?;
    let preferences = match fs::read(directory.join("updates.json")) {
        Ok(bytes) => serde_json::from_slice::<Preferences>(&bytes).map_err(|e| e.to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Preferences { auto_update: true }),
        Err(e) => Err(e.to_string()),
    };
    let configured = configured_build();
    app.manage(Updates {
        directory,
        view: Mutex::new(View {
            current_version: app.package_info().version.to_string(),
            available_version: None,
            auto_update: preferences.as_ref().is_ok_and(|p| p.auto_update),
            configured,
            phase: "idle".into(),
            error: preferences.err(),
            last_checked: None,
        }),
    });
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Repair a collector restart interrupted by a crash, even if auto-update is off.
        let operations = app.state::<super::Operations>();
        {
            let _guard = operations.0.lock().await;
            if let Err(error) = recover(&app).await {
                change(&app, |v| {
                    v.phase = "error".into();
                    v.error = Some(error);
                });
            }
        }
        tokio::time::sleep(Duration::from_secs(60)).await;
        loop {
            // Serialize against enabling/disconnecting and manual updates.
            let _guard = operations.0.lock().await;
            let run = {
                let state = app.state::<Updates>();
                let v = state.view.lock().unwrap();
                v.configured && v.auto_update
            };
            if run {
                let _ = perform(&app, true).await;
            }
            drop(_guard);
            tokio::time::sleep(Duration::from_secs(24 * 60 * 60)).await;
        }
    });
    Ok(())
}

#[tauri::command]
pub(crate) fn update_status(window: Webview, state: State<'_, Updates>) -> Result<View, String> {
    super::local_setup(&window)?;
    Ok(state.view.lock().unwrap().clone())
}

#[tauri::command]
pub(crate) async fn set_auto_update(
    window: Webview,
    app: tauri::AppHandle,
    enabled: bool,
) -> Result<(), String> {
    super::local_setup(&window)?;
    let operations = app.state::<super::Operations>();
    let _guard = operations.0.lock().await;
    let state = app.state::<Updates>();
    save(
        &state.directory.join("updates.json"),
        &serde_json::to_vec(&Preferences {
            auto_update: enabled,
        })
        .map_err(|e| e.to_string())?,
    )?;
    change(&app, |v| v.auto_update = enabled);
    Ok(())
}

#[tauri::command]
pub(crate) async fn check_updates(window: Webview, app: tauri::AppHandle) -> Result<(), String> {
    super::local_setup(&window)?;
    let operations = app.state::<super::Operations>();
    let _guard = operations.0.lock().await;
    perform(&app, false).await
}
#[tauri::command]
pub(crate) async fn install_update(window: Webview, app: tauri::AppHandle) -> Result<(), String> {
    super::local_setup(&window)?;
    let operations = app.state::<super::Operations>();
    let _guard = operations.0.lock().await;
    perform(&app, true).await
}

async fn recover(app: &tauri::AppHandle) -> Result<(), String> {
    let path = app.state::<Updates>().directory.join("update-pending");
    if path.try_exists().map_err(|e| e.to_string())? {
        super::cli(&["desktop", "restart-after-update"]).await?;
        clear_marker(&path)?;
    }
    Ok(())
}

async fn perform(app: &tauri::AppHandle, install: bool) -> Result<(), String> {
    let result = perform_inner(app, install).await;
    if let Err(error) = &result {
        change(app, |v| {
            v.phase = "error".into();
            v.error = Some(error.clone());
        });
    }
    result
}
async fn perform_inner(app: &tauri::AppHandle, install: bool) -> Result<(), String> {
    if !app.state::<Updates>().view.lock().unwrap().configured {
        return Err("Updates are unavailable in this development build (release signing key not configured).".into());
    }
    recover(app).await?;
    change(app, |v| {
        v.phase = "checking".into();
        v.error = None;
    });
    let updater = app
        .updater_builder()
        .pubkey(PUBLIC_KEY.unwrap().trim())
        .endpoints(vec![ENDPOINT.parse().unwrap()])
        .map_err(|e| e.to_string())?
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| e.to_string())?;
    let update = tokio::time::timeout(Duration::from_secs(30), updater.check())
        .await
        .map_err(|_| "Update check timed out")?
        .map_err(|e| e.to_string())?;
    change(app, |v| {
        v.last_checked = Some(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        );
        v.available_version = update.as_ref().map(|u| u.version.clone());
        v.phase = if update.is_some() {
            "available"
        } else {
            "current"
        }
        .into();
    });
    if install {
        if let Some(update) = update {
            // Do not invoke an installer from a DMG/translocated or development path.
            if super::binaries()? != Path::new("/Applications/kikimimi.app/Contents/MacOS") {
                return Err("Move kikimimi to Applications before updating.".into());
            }
            // The signature authenticates the bytes; require HTTPS for the download too.
            if update.download_url.scheme() != "https" {
                return Err("Update download must use HTTPS".into());
            }
            let mut host = Install { app, update };
            super::update_flow::apply(&mut host).await?;
            change(app, |v| v.phase = "restarting".into());
            app.restart();
        }
    }
    Ok(())
}

struct Install<'a> {
    app: &'a tauri::AppHandle,
    update: tauri_plugin_updater::Update,
}
impl super::update_flow::Host for Install<'_> {
    async fn download_verified(&mut self) -> Result<Vec<u8>, String> {
        change(self.app, |v| v.phase = "downloading".into());
        self.update
            .download(|_, _| {}, || {})
            .await
            .map_err(|e| e.to_string())
    }
    fn mark_pending(&mut self) -> Result<(), String> {
        save(
            &self.app.state::<Updates>().directory.join("update-pending"),
            self.update.version.as_bytes(),
        )
    }
    fn install_atomically(&mut self, bytes: Vec<u8>) -> Result<(), String> {
        change(self.app, |v| v.phase = "installing".into());
        install_macos(&bytes, &self.update.version)
    }
    async fn restart_collector(&mut self) -> Result<(), String> {
        super::cli(&["desktop", "restart-after-update"])
            .await
            .map(|_| ())
    }
    fn clear_pending(&mut self) -> Result<(), String> {
        clear_marker(&self.app.state::<Updates>().directory.join("update-pending"))
    }
}

/// Stage on the same volume, verify the signed app, then atomically exchange the
/// two directories. There is no interval where launchd/hooks point to a missing
/// bundle. A failed swap leaves the installed app and running collector intact.
#[cfg(target_os = "macos")]
fn install_macos(bytes: &[u8], expected_version: &str) -> Result<(), String> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let staging = tempfile::Builder::new()
        .prefix(".kikimimi-update-")
        .tempdir_in("/Applications")
        .map_err(|e| e.to_string())?;
    tar::Archive::new(flate2::read::GzDecoder::new(bytes))
        .unpack(staging.path())
        .map_err(|e| e.to_string())?;
    let incoming = staging.path().join("kikimimi.app");
    super::update_bundle::validate(&incoming, expected_version)?;
    let arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x86_64"
    };
    for name in super::update_bundle::EXECUTABLES {
        let compatible = super::update_bundle::architecture_check(
            &incoming.join("Contents/MacOS").join(name),
            arch,
        )
        .output()
        .map_err(|e| e.to_string())?;
        if !compatible.status.success() {
            return Err(format!(
                "Update executable {name} does not support this Mac's architecture"
            ));
        }
    }
    let verified = std::process::Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(&incoming)
        .output()
        .map_err(|e| e.to_string())?;
    if !verified.status.success() {
        return Err("Updated app failed macOS code signature verification".into());
    }
    let source = CString::new(incoming.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    let destination = CString::new("/Applications/kikimimi.app").unwrap();
    let result = unsafe {
        libc::renameatx_np(
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            libc::RENAME_SWAP,
        )
    };
    if result != 0 {
        return Err(format!(
            "Could not replace app: {}",
            std::io::Error::last_os_error()
        ));
    }
    // The old app is now inside the temporary directory. TempDir removes only it.
    Ok(())
}
#[cfg(not(target_os = "macos"))]
fn install_macos(_: &[u8], _: &str) -> Result<(), String> {
    Err("Desktop updates currently require macOS".into())
}
