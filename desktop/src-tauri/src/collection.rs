//! Privileged collection switching. Device tokens stay in native memory and stdin.
use serde_json::{json, Value};
use tauri::Manager;
use tokio::io::AsyncWriteExt;

// A login grant is reused once, for its exact workspace, without exposing it to JS.
static LOGIN_GRANT: std::sync::Mutex<Option<(std::time::Instant, Value)>> = std::sync::Mutex::new(None);
fn take_login_grant(slug: &str) -> Option<Value> {
    let (created, grant) = LOGIN_GRANT.lock().ok()?.take()?;
    (created.elapsed() < std::time::Duration::from_secs(300) && grant["org_slug"] == slug).then_some(grant)
}

pub(crate) async fn authorize_cloud(
    host: &Value,
    slug: &str,
    repos: &Value,
) -> Result<Value, String> {
    let origin: tauri::Url = "https://kikimimi.dev/".parse().unwrap();
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|_| "Could not connect to Cloud")?;
    let code: Value = client
        .post(origin.join("v1/device/code").unwrap())
        .json(&json!({"host_id":host,"hostname":"kikimimi desktop","org_hint":slug}))
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
    let mut verification = origin.join("activate").unwrap();
    verification
        .query_pairs_mut()
        .append_pair("code", user_code);
    super::dashboard::open_browser(&verification).await?;
    let interval =
        std::time::Duration::from_secs(code["interval_secs"].as_u64().unwrap_or(5).clamp(1, 30));
    let body = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        poll_token(&client, &origin, device_code, interval),
    )
    .await
    .map_err(|_| "Authorization timed out. Try again.")??;
    authorized_connection(&body, slug, repos)
}

async fn poll_token(
    client: &reqwest::Client,
    origin: &tauri::Url,
    device_code: &str,
    interval: std::time::Duration,
) -> Result<Value, String> {
    loop {
        let response = client
            .post(origin.join("v1/device/token").unwrap())
            .json(&json!({"device_code":device_code}))
            .send()
            .await
            .map_err(|_| "Could not finish device authorization")?;
        if response.status() == reqwest::StatusCode::GONE {
            return Err("Authorization expired. Try again.".to_string());
        }
        let body: Value = response
            .error_for_status()
            .map_err(|_| "Device authorization failed")?
            .json()
            .await
            .map_err(|_| "Invalid device authorization response")?;
        match body["status"].as_str() {
            Some("pending") => tokio::time::sleep(interval).await,
            Some("ok") => return Ok(body),
            _ => return Err("Authorization was declined or expired. Try again.".into()),
        }
    }
}

fn authorized_connection(body: &Value, slug: &str, repos: &Value) -> Result<Value, String> {
    if body["status"] != "ok" || (!slug.is_empty() && body["org_slug"] != slug) {
        return Err("Cloud did not authorize the selected workspace".into());
    }
    for key in ["token", "org_id", "org_kind", "email"] {
        if body[key].as_str().is_none_or(str::is_empty) {
            return Err("Incomplete Cloud authorization".into());
        }
    }
    Ok(
        json!({"endpoint":"https://kikimimi.dev","token":body["token"],"org_id":body["org_id"],"org_slug":body["org_slug"],"org_kind":body["org_kind"],"email":body["email"],"repo_patterns":repos}),
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
    let mut request = json!({"kind":kind,"expected":info["target"],"upload_history":selection["upload_history"].as_bool().unwrap_or(false)});
    match kind {
        "local" | "sync-on" | "sync-off" => {}
        "s3" => {
            request["s3"] = selection["s3"].clone();
        }
        "cloud" => {
            let slug = selection["slug"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or("Choose a Cloud workspace")?;
            request["cloud"] = match take_login_grant(slug) {
                Some(grant) => grant,
                None => authorize_cloud(&info["host_id"], slug, &json!([])).await?,
            };
            let expected_kind = if selection["team"] == true {
                "team"
            } else {
                "personal"
            };
            if request["cloud"]["org_kind"] != expected_kind {
                return Err("Workspace type changed. Review the destination again.".into());
            }
        }
        _ => return Err("Unknown collection destination".into()),
    }
    let mut command = super::collector_command()?;
    command.kill_on_drop(true);
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
    let result = tokio::time::timeout(std::time::Duration::from_secs(3600), child.wait())
        .await
        .map_err(|_| "Collection update timed out. Check the current destination before retrying.")?
        .map_err(|_| "Could not update collection")?;
    if !result.success() {
        return Err("Collection or history upload failed. Check the current destination. You can retry history upload safely; previously accepted events will not be counted twice.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn login_grant_is_single_use_workspace_bound_and_short_lived() {
        let grant = json!({"org_slug":"one","token":"test-only"});
        *LOGIN_GRANT.lock().unwrap() = Some((std::time::Instant::now(), grant.clone()));
        assert!(take_login_grant("other").is_none());
        *LOGIN_GRANT.lock().unwrap() = Some((std::time::Instant::now(), grant.clone()));
        assert_eq!(take_login_grant("one"), Some(grant.clone()));
        assert!(take_login_grant("one").is_none());
        *LOGIN_GRANT.lock().unwrap() = Some((std::time::Instant::now() - std::time::Duration::from_secs(301), grant));
        assert!(take_login_grant("one").is_none());
    }

    fn server(responses: Vec<(u16, Value)>) -> (tauri::Url, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}/", listener.local_addr().unwrap())
            .parse()
            .unwrap();
        let task = std::thread::spawn(move || {
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                    .unwrap();
                let mut request = [0; 4096];
                let count = stream.read(&mut request).unwrap();
                assert!(String::from_utf8_lossy(&request[..count])
                    .starts_with("POST /v1/device/token "));
                let body = body.to_string();
                write!(stream, "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        (origin, task)
    }

    #[tokio::test]
    async fn browser_approval_polls_pending_then_accepts_token() {
        let (origin, task) = server(vec![
            (200, json!({"status":"pending"})),
            (200, json!({"status":"ok","token":"test"})),
        ]);
        let result = poll_token(
            &reqwest::Client::new(),
            &origin,
            "device-test",
            std::time::Duration::ZERO,
        )
        .await
        .unwrap();
        assert_eq!(result["token"], "test");
        task.join().unwrap();
    }

    #[tokio::test]
    async fn expired_or_denied_approval_cannot_connect() {
        for response in [
            (410, json!({})),
            (200, json!({"status":"denied"})),
            (200, json!({})),
        ] {
            let (origin, task) = server(vec![response]);
            assert!(poll_token(
                &reqwest::Client::new(),
                &origin,
                "device-test",
                std::time::Duration::ZERO
            )
            .await
            .is_err());
            task.join().unwrap();
        }
    }

    #[test]
    fn browser_cannot_replace_the_reviewed_workspace_or_omit_credentials() {
        let body = json!({"status":"ok","org_slug":"team-a","org_kind":"team","org_id":"id","token":"secret","email":"test@example.com"});
        assert!(authorized_connection(&body, "team-b", &json!([])).is_err());
        let approved =
            authorized_connection(&body, "team-a", &json!(["github.com/team/*"])).unwrap();
        assert_eq!(approved["repo_patterns"], json!(["github.com/team/*"]));
        for key in ["token", "org_id", "org_kind", "email"] {
            let mut missing = body.clone();
            missing[key] = json!("");
            assert!(authorized_connection(&missing, "team-a", &json!([])).is_err());
        }
    }
}

#[tauri::command]
pub(crate) async fn collection_workspaces(window: tauri::Webview) -> Result<Value, String> {
    super::local_setup(&window)?;
    *LOGIN_GRANT.lock().map_err(|_| "Could not start sign-in")? = None;
    if let Ok(result) = super::cli(&["desktop", "workspaces"]).await {
        return serde_json::from_str(&result).map_err(|_| "Invalid workspace list".into());
    }
    let info: Value = serde_json::from_str(&super::cli(&["desktop", "collection-info"]).await?)
        .map_err(|_| "Could not read this Mac".to_string())?;
    let auth = authorize_cloud(&info["host_id"], "", &json!([])).await?;
    let mut result: Value = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(15)).build().map_err(|_| "Could not connect")?
        .get("https://kikimimi.dev/v1/orgs").bearer_auth(auth["token"].as_str().ok_or("Missing authorization")?)
        .send().await.map_err(|_| "Could not load workspaces")?
        .error_for_status().map_err(|_| "Could not load workspaces")?
        .json().await.map_err(|_| "Invalid workspace list".to_string())?;
    result["authorized_slug"] = auth["org_slug"].clone();
    *LOGIN_GRANT.lock().map_err(|_| "Could not finish sign-in")? = Some((std::time::Instant::now(), auth));
    Ok(result)
}
