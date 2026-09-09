//! Privileged collection switching. Device tokens stay in native memory and stdin.
use serde_json::{json, Value};
use tauri::Manager;
use tokio::io::AsyncWriteExt;

async fn authorize_cloud(
    window: &tauri::Webview,
    app: &tauri::AppHandle,
    host: &Value,
    slug: &str,
    repos: &Value,
) -> Result<Value, String> {
    let origin: tauri::Url = "https://kikimimi.dev/".parse().unwrap();
    let view = app.get_webview("cloud").unwrap_or_else(|| window.clone());
    let cookies = view
        .cookies_for_url(origin.clone())
        .map_err(|_| "Could not read Cloud sign-in")?;
    let session = cookies
        .iter()
        .find(|c| c.name() == "kikimimi_session")
        .ok_or("Sign in to Cloud before changing collection")?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|_| "Could not connect to Cloud")?;
    let code: Value = client
        .post(origin.join("v1/device/code").unwrap())
        .json(&json!({"host_id":host,"org_hint":slug}))
        .send()
        .await
        .map_err(|_| "Could not connect to Cloud")?
        .error_for_status()
        .map_err(|_| "Could not request device authorization")?
        .json()
        .await
        .map_err(|_| "Invalid Cloud authorization response")?;
    let user_code = code["user_code"]
        .as_str()
        .ok_or("Missing device authorization code")?;
    let device_code = code["device_code"]
        .as_str()
        .ok_or("Missing device authorization code")?;
    client
        .post(origin.join("activate").unwrap())
        .header("Cookie", format!("kikimimi_session={}", session.value()))
        .json(&json!({"code":user_code,"org_slug":slug}))
        .send()
        .await
        .map_err(|_| "Could not authorize this Mac")?
        .error_for_status()
        .map_err(|_| "Your account cannot collect into this workspace")?;
    let body: Value = client
        .post(origin.join("v1/device/token").unwrap())
        .json(&json!({"device_code":device_code}))
        .send()
        .await
        .map_err(|_| "Could not finish device authorization")?
        .error_for_status()
        .map_err(|_| "Device authorization failed")?
        .json()
        .await
        .map_err(|_| "Invalid device authorization response")?;
    if body["status"] != "ok" || body["org_slug"] != slug {
        return Err("Cloud did not authorize the selected workspace".into());
    }
    for key in ["token", "org_id", "org_kind", "email"] {
        if body[key].as_str().is_none() {
            return Err("Incomplete Cloud authorization".into());
        }
    }
    Ok(
        json!({"endpoint":"https://kikimimi.dev","token":body["token"],"org_id":body["org_id"],"org_slug":slug,"org_kind":body["org_kind"],"email":body["email"],"repo_patterns":repos}),
    )
}

#[tauri::command]
pub(crate) async fn switch_collection(
    window: tauri::Webview,
    app: tauri::AppHandle,
    selection: Value,
) -> Result<(), String> {
    super::local_setup(&window)?;
    let operations = app.state::<super::Operations>();
    let _guard = operations.0.lock().await;
    let info: Value = serde_json::from_str(&super::cli(&["desktop", "collection-info"]).await?)
        .map_err(|_| "Could not read collection settings")?;
    if info["target"] != selection["expected"] {
        return Err("Collection settings changed. Review the destination again.".into());
    }
    let kind = selection["kind"].as_str().ok_or("Choose a destination")?;
    let mut request = json!({"kind":kind,"expected":info["target"]});
    match kind {
        "local" => {}
        "s3" => {
            request["s3"] = selection["s3"].clone();
        }
        "cloud" => {
            let slug = selection["slug"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or("Choose a Cloud workspace")?;
            let repos = selection["repo_patterns"]
                .as_array()
                .ok_or("Choose repositories to share")?;
            if repos.len() > 100
                || repos
                    .iter()
                    .any(|r| r.as_str().is_none_or(|s| s.is_empty() || s.len() > 2048))
            {
                return Err("Invalid repository selection".into());
            }
            request["cloud"] = authorize_cloud(
                &window,
                &app,
                &info["host_id"],
                slug,
                &selection["repo_patterns"],
            )
            .await?;
        }
        _ => return Err("Unknown collection destination".into()),
    }
    let mut command = super::collector_command()?;
    let mut child = command
        .args(["desktop", "switch-collection"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|_| "Could not update collection")?;
    let mut input = child.stdin.take().ok_or("Could not update collection")?;
    input
        .write_all(&serde_json::to_vec(&request).map_err(|_| "Invalid collection request")?)
        .await
        .map_err(|_| "Could not update collection")?;
    drop(input);
    let result = tokio::time::timeout(std::time::Duration::from_secs(15), child.wait())
        .await
        .map_err(|_| "Collection update timed out. Check the current destination before retrying.")?
        .map_err(|_| "Could not update collection")?;
    if !result.success() {
        return Err("Could not change collection. Check the connection and repository patterns, then review the current destination.".into());
    }
    Ok(())
}
