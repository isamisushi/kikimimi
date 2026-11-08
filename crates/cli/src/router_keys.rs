use std::time::Duration;

use anyhow::Context;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct NewKeyResponse {
    pub(crate) id: String,
    pub(crate) key: String,
    pub(crate) base_url: Option<String>,
    pub(crate) auto_model: String,
}

fn client() -> anyhow::Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .context("building HTTP client")
}

pub(crate) fn create_opencode_key(endpoint: &str, token: &str) -> anyhow::Result<NewKeyResponse> {
    let endpoint = endpoint.trim_end_matches('/');
    let label = format!("OpenCode on {}", crate::login_cmd::hostname());
    let resp = client()?
        .post(format!("{endpoint}/v1/router/keys"))
        .bearer_auth(token)
        .json(&serde_json::json!({ "label": label }))
        .send()
        .context("POST /v1/router/keys")?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        anyhow::bail!("POST /v1/router/keys returned {status}: {body}");
    }
    resp.json().context("parsing /v1/router/keys response")
}

pub(crate) fn revoke_key(endpoint: &str, token: &str, id: &str) -> anyhow::Result<()> {
    let endpoint = endpoint.trim_end_matches('/');
    let resp = client()?
        .delete(format!("{endpoint}/v1/router/keys/{id}"))
        .bearer_auth(token)
        .send()
        .with_context(|| format!("DELETE /v1/router/keys/{id}"))?;
    // 404: already revoked (for example on the web).
    if !resp.status().is_success() && resp.status() != reqwest::StatusCode::NOT_FOUND {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        anyhow::bail!("DELETE /v1/router/keys/{id} returned {status}: {body}");
    }
    Ok(())
}
