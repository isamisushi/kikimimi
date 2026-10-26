//! `kikimimi router enable|disable|status|token`: Codex の接続先を team router に
//! 切り替える (docs/design/v2.md §4.3 の Codex 行)。desktop app の設定画面もこれを呼ぶ。
//!
//! `enable` は `POST /v1/router/keys` で router key を発行させ、`$CODEX_HOME/config.toml`
//! の最上位に `model_provider = "kikimimi"` と `model = "kikimimi-auto"` を書き、
//! `[model_providers.kikimimi]` を足す。キーは `config.toml` にもシェルの環境変数にも
//! 置かない。Codex の command-backed auth (`[model_providers.kikimimi.auth]`) が
//! `kikimimi router token` を呼び、このコマンドが Keychain からキーを返す
//! (`crate::secret_store`)。
//! `env_key` にしないのは、Finder から起動した ChatGPT desktop app の Codex には
//! シェルの環境変数が渡らないため。
//!
//! 最上位に書くのは、`--profile` (`<name>.config.toml` を重ねる) が CLI のフラグで、
//! desktop app からは選べないため。`auth.json` (ChatGPT のログイン) には触らない。
//!
//! `disable` は、まだ kikimimi の値になっている最上位のキーだけを元の値に戻し
//! (ユーザーがその後で書き換えたものは残す)、`[model_providers.kikimimi]` を消して、
//! キーを `DELETE /v1/router/keys/{id}` で失効させる。
//!
//! キー名は OpenAI の Codex 設定ドキュメント (Advanced Configuration の Custom model
//! providers と Profiles の節) で確認 (2026-10-27)。Codex は設定を自動では読み直さないので、
//! 切り替えたあとは Codex の再起動が要る。

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use serde::Deserialize;
use toml_edit::{value, Array, DocumentMut, Item, Table};

use crate::config::{CodexRouterConfig, KikimimiConfig};

/// `[model_providers.<id>]` の id。Codex の予約 id (`openai`, `ollama`, `lmstudio`) と重ならない。
pub const PROVIDER_ID: &str = "kikimimi";

#[derive(Debug, Deserialize)]
struct NewKeyResponse {
    id: String,
    key: String,
    base_url: Option<String>,
    auto_model: String,
}

pub fn config_toml_path() -> PathBuf {
    kikimimi_schema::paths::codex_home_dir().join("config.toml")
}

/// `kikimimi router enable`.
pub fn enable() -> anyhow::Result<()> {
    let executable = std::env::current_exe()?.canonicalize()?;
    enable_with(&config_toml_path(), &executable)?;
    println!("Codex now uses your team's kikimimi router. Restart Codex to apply.");
    Ok(())
}

/// `kikimimi router disable`.
pub fn disable() -> anyhow::Result<()> {
    if disable_with(&config_toml_path())? {
        println!("Codex is back on its previous provider. Restart Codex to apply.");
    } else {
        println!("Codex was not using the kikimimi router.");
    }
    Ok(())
}

/// `kikimimi router status`: one JSON line, for people and for the desktop app.
pub fn status() -> anyhow::Result<()> {
    println!("{}", status_json(&config_toml_path()));
    Ok(())
}

/// `kikimimi router token`: Codex's auth command. Prints only the key.
pub fn token() -> anyhow::Result<()> {
    let cfg = KikimimiConfig::load();
    let router = cfg.codex_router.ok_or_else(|| {
        anyhow::anyhow!("the kikimimi router is not enabled; run `kikimimi router enable`")
    })?;
    anyhow::ensure!(
        !router.key.is_empty(),
        "the router key is missing from the Keychain; run `kikimimi router disable`, then `kikimimi router enable`"
    );
    println!("{}", router.key);
    Ok(())
}

pub fn status_json(config_toml: &Path) -> serde_json::Value {
    let cfg = KikimimiConfig::load_without_secrets();
    let applied = load_document(config_toml)
        .ok()
        .is_some_and(|doc| doc.get("model_provider").and_then(Item::as_str) == Some(PROVIDER_ID));
    serde_json::json!({
        "codex_found": config_toml.parent().is_some_and(Path::exists),
        "logged_in": cfg.cloud.is_some(),
        "org_slug": cfg.cloud.as_ref().map(|c| c.org_slug.clone()),
        "org_kind": cfg.cloud.as_ref().map(|c| c.org_kind.clone()),
        "enabled": cfg.codex_router.is_some(),
        "applied": applied,
        "base_url": cfg.codex_router.as_ref().map(|r| r.base_url.clone()),
    })
}

pub fn enable_with(config_toml: &Path, executable: &Path) -> anyhow::Result<()> {
    let mut cfg = KikimimiConfig::load();
    let mut doc = load_document(config_toml)?;
    // Enabling twice keeps the first key and the first previous values.
    let (router, minted) = match cfg.codex_router.clone() {
        Some(router) => (router, false),
        None => {
            let cloud = cfg
                .cloud
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("not logged in; run `kikimimi login` first"))?;
            let created = create_key(&cloud.endpoint, &cloud.token)?;
            let base_url = created
                .base_url
                .ok_or_else(|| anyhow::anyhow!("this team's router has no URL yet"))?;
            let router = CodexRouterConfig {
                key_id: created.id,
                key: created.key,
                base_url,
                model: created.auto_model,
                previous_model_provider: root_str(&doc, "model_provider"),
                previous_model: root_str(&doc, "model"),
            };
            (router, true)
        }
    };
    let result = (|| {
        apply(&mut doc, &router, executable)?;
        cfg.codex_router = Some(router.clone());
        cfg.save()?;
        write_document(config_toml, &doc)
    })();
    if result.is_err() && minted {
        // Do not leave a live key that nothing points at.
        if let Some(cloud) = &cfg.cloud {
            let _ = revoke_key(&cloud.endpoint, &cloud.token, &router.key_id);
        }
        cfg.codex_router = None;
        let _ = cfg.save();
    }
    result
}

/// `Ok(false)` when the router was not enabled.
pub fn disable_with(config_toml: &Path) -> anyhow::Result<bool> {
    let mut cfg = KikimimiConfig::load();
    let Some(router) = cfg.codex_router.clone() else {
        return Ok(false);
    };
    if config_toml.exists() {
        let mut doc = load_document(config_toml)?;
        revert(&mut doc, &router);
        write_document(config_toml, &doc)?;
    }
    if let Some(cloud) = &cfg.cloud {
        if let Err(e) = revoke_key(&cloud.endpoint, &cloud.token, &router.key_id) {
            // Codex no longer uses the key; the team admin can still revoke it on the web.
            eprintln!("warning: could not revoke the router key ({e:#})");
        }
    }
    cfg.codex_router = None;
    cfg.save()?;
    Ok(true)
}

fn root_str(doc: &DocumentMut, key: &str) -> Option<String> {
    doc.get(key).and_then(Item::as_str).map(str::to_string)
}

/// Points `doc` at the router. Idempotent.
pub fn apply(
    doc: &mut DocumentMut,
    router: &CodexRouterConfig,
    executable: &Path,
) -> anyhow::Result<()> {
    let executable = executable
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("kikimimi path is not UTF-8"))?;
    doc["model_provider"] = value(PROVIDER_ID);
    doc["model"] = value(router.model.as_str());

    let providers = doc
        .entry("model_providers")
        .or_insert_with(|| {
            let mut t = Table::new();
            t.set_implicit(true);
            Item::Table(t)
        })
        .as_table_mut()
        .ok_or_else(|| {
            anyhow::anyhow!("`model_providers` in config.toml is not a table; edit it by hand")
        })?;
    let mut provider = Table::new();
    provider["name"] = value("kikimimi router");
    provider["base_url"] = value(format!("{}/v1", router.base_url.trim_end_matches('/')));
    provider["wire_api"] = value("responses");
    let mut auth = Table::new();
    auth["command"] = value(executable);
    auth["args"] = value(Array::from_iter(["router", "token"]));
    // The key does not expire: fetch it again only after a 401.
    auth["refresh_interval_ms"] = value(0);
    provider["auth"] = Item::Table(auth);
    providers.insert(PROVIDER_ID, Item::Table(provider));
    Ok(())
}

/// Undoes [`apply`], keeping anything the user changed since.
pub fn revert(doc: &mut DocumentMut, router: &CodexRouterConfig) {
    if root_str(doc, "model_provider").as_deref() == Some(PROVIDER_ID) {
        restore(
            doc,
            "model_provider",
            router.previous_model_provider.as_deref(),
        );
    }
    if root_str(doc, "model").as_deref() == Some(router.model.as_str()) {
        restore(doc, "model", router.previous_model.as_deref());
    }
    if let Some(providers) = doc.get_mut("model_providers").and_then(Item::as_table_mut) {
        providers.remove(PROVIDER_ID);
        if providers.is_empty() {
            doc.remove("model_providers");
        }
    }
}

fn restore(doc: &mut DocumentMut, key: &str, previous: Option<&str>) {
    match previous {
        Some(v) => doc[key] = value(v),
        None => {
            doc.remove(key);
        }
    }
}

fn load_document(path: &Path) -> anyhow::Result<DocumentMut> {
    match std::fs::read_to_string(path) {
        Ok(text) => text
            .parse()
            .with_context(|| format!("parsing {} as TOML", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(DocumentMut::new()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Backs up the original once (`config.toml.kikimimi-backup`), then writes atomically.
fn write_document(path: &Path, doc: &DocumentMut) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let backup = crate::claude_settings::backup_path(path);
    if path.exists() && !backup.exists() {
        std::fs::copy(path, &backup).with_context(|| format!("backing up {}", path.display()))?;
    }
    crate::state::write_atomic(path, doc.to_string().as_bytes())
}

fn client() -> anyhow::Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .context("building HTTP client")
}

fn create_key(endpoint: &str, token: &str) -> anyhow::Result<NewKeyResponse> {
    let endpoint = endpoint.trim_end_matches('/');
    let label = format!("Codex on {}", crate::login_cmd::hostname());
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

fn revoke_key(endpoint: &str, token: &str, id: &str) -> anyhow::Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn router(previous_provider: Option<&str>, previous_model: Option<&str>) -> CodexRouterConfig {
        CodexRouterConfig {
            key_id: "k1".into(),
            key: "sk-member".into(),
            base_url: "https://acme-router.example/".into(),
            model: "kikimimi-auto".into(),
            previous_model_provider: previous_provider.map(str::to_string),
            previous_model: previous_model.map(str::to_string),
        }
    }

    const EXE: &str = "/Applications/kikimimi.app/Contents/MacOS/kikimimi";

    #[test]
    fn apply_writes_root_keys_and_the_provider_and_keeps_the_rest() {
        let original = r#"# my settings
model = "gpt-5"
approval_policy = "on-request"

[mcp_servers.github]
command = "npx"
"#;
        let mut doc: DocumentMut = original.parse().unwrap();
        apply(&mut doc, &router(None, Some("gpt-5")), Path::new(EXE)).unwrap();
        let text = doc.to_string();
        assert!(
            text.starts_with("# my settings\nmodel = \"kikimimi-auto\"\n"),
            "{text}"
        );
        assert!(
            text.contains("[mcp_servers.github]\ncommand = \"npx\""),
            "{text}"
        );
        let reparsed: toml_edit::DocumentMut = text.parse().unwrap();
        assert_eq!(reparsed["model_provider"].as_str(), Some("kikimimi"));
        let p = &reparsed["model_providers"]["kikimimi"];
        assert_eq!(
            p["base_url"].as_str(),
            Some("https://acme-router.example/v1")
        );
        assert_eq!(p["wire_api"].as_str(), Some("responses"));
        assert_eq!(p["auth"]["command"].as_str(), Some(EXE));
        let args: Vec<_> = p["auth"]["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(args, ["router", "token"]);
        // The key itself never goes into config.toml.
        assert!(!text.contains("sk-member"));
        // A model_provider written after the tables would land inside one.
        assert!(
            text.find("model_provider =").unwrap() < text.find('[').unwrap(),
            "{text}"
        );

        let mut again = reparsed.clone();
        apply(&mut again, &router(None, Some("gpt-5")), Path::new(EXE)).unwrap();
        assert_eq!(again.to_string(), text);
    }

    #[test]
    fn revert_restores_previous_values_and_removes_only_our_provider() {
        let original = r#"model_provider = "azure"
model = "gpt-5"

[model_providers.azure]
base_url = "https://azure.example"
"#;
        let mut doc: DocumentMut = original.parse().unwrap();
        let r = router(Some("azure"), Some("gpt-5"));
        apply(&mut doc, &r, Path::new(EXE)).unwrap();
        revert(&mut doc, &r);
        assert_eq!(doc.to_string(), original);
    }

    #[test]
    fn revert_removes_keys_that_did_not_exist_and_an_empty_providers_table() {
        let mut doc = DocumentMut::new();
        let r = router(None, None);
        apply(&mut doc, &r, Path::new(EXE)).unwrap();
        revert(&mut doc, &r);
        assert_eq!(doc.to_string(), "");
    }

    #[test]
    fn revert_keeps_values_the_user_changed_after_enable() {
        let mut doc: DocumentMut = "model = \"o3\"\n".parse().unwrap();
        let r = router(None, Some("o3"));
        apply(&mut doc, &r, Path::new(EXE)).unwrap();
        doc["model"] = value("gpt-5-codex");
        doc["model_provider"] = value("openai");
        revert(&mut doc, &r);
        assert_eq!(doc["model"].as_str(), Some("gpt-5-codex"));
        assert_eq!(doc["model_provider"].as_str(), Some("openai"));
        assert!(doc.get("model_providers").is_none());
    }

    mod against_a_mock_cloud {
        use super::*;
        use crate::config::CloudConfig;
        use httpmock::prelude::*;
        use serde_json::json;
        use serial_test::serial;

        fn login_with(server: &MockServer) {
            let mut cfg = KikimimiConfig::load();
            cfg.cloud = Some(CloudConfig {
                endpoint: server.base_url(),
                token: "tok-device".into(),
                org_slug: "acme".into(),
                org_kind: "team".into(),
                ..Default::default()
            });
            cfg.save().unwrap();
        }

        #[test]
        #[serial]
        fn enable_then_disable_round_trips_config_toml_and_revokes_the_key() {
            let dir = tempfile::tempdir().unwrap();
            std::env::set_var("KIKIMIMI_DIR", dir.path().join("kikimimi"));
            let config_toml = dir.path().join("codex").join("config.toml");
            std::fs::create_dir_all(config_toml.parent().unwrap()).unwrap();
            let original = "# mine\nmodel = \"gpt-5\"\n";
            std::fs::write(&config_toml, original).unwrap();

            let server = MockServer::start();
            login_with(&server);
            let create = server.mock(|when, then| {
                when.method(POST)
                    .path("/v1/router/keys")
                    .header("authorization", "Bearer tok-device");
                then.status(200).json_body(json!({
                    "id": "key-1", "label": "Codex", "key": "sk-member-1",
                    "base_url": "https://acme-router.example",
                    "auto_model": "kikimimi-auto", "models": ["kikimimi-auto"],
                }));
            });
            let revoke = server.mock(|when, then| {
                when.method(DELETE)
                    .path("/v1/router/keys/key-1")
                    .header("authorization", "Bearer tok-device");
                then.status(200).json_body(json!({ "revoked": true }));
            });

            enable_with(&config_toml, Path::new(EXE)).unwrap();
            // A second enable reuses the key.
            enable_with(&config_toml, Path::new(EXE)).unwrap();
            create.assert_calls(1);
            let text = std::fs::read_to_string(&config_toml).unwrap();
            assert!(text.contains("model_provider = \"kikimimi\""), "{text}");
            assert!(!text.contains("sk-member-1"));
            let backup = crate::claude_settings::backup_path(&config_toml);
            assert_eq!(std::fs::read_to_string(backup).unwrap(), original);
            let status = status_json(&config_toml);
            assert_eq!(status["enabled"], true);
            assert_eq!(status["applied"], true);
            assert_eq!(
                KikimimiConfig::load().codex_router.unwrap().key,
                "sk-member-1"
            );

            assert!(disable_with(&config_toml).unwrap());
            revoke.assert_calls(1);
            assert_eq!(std::fs::read_to_string(&config_toml).unwrap(), original);
            assert!(KikimimiConfig::load().codex_router.is_none());
            assert!(!disable_with(&config_toml).unwrap());

            std::env::remove_var("KIKIMIMI_DIR");
        }

        #[test]
        #[serial]
        fn enable_surfaces_a_team_without_a_router() {
            let dir = tempfile::tempdir().unwrap();
            std::env::set_var("KIKIMIMI_DIR", dir.path().join("kikimimi"));
            let config_toml = dir.path().join("codex").join("config.toml");
            let server = MockServer::start();
            login_with(&server);
            server.mock(|when, then| {
                when.method(POST).path("/v1/router/keys");
                then.status(404).body("this team has no active router");
            });

            let err = enable_with(&config_toml, Path::new(EXE)).unwrap_err();
            assert!(format!("{err:#}").contains("no active router"), "{err:#}");
            assert!(!config_toml.exists());
            assert!(KikimimiConfig::load().codex_router.is_none());

            std::env::remove_var("KIKIMIMI_DIR");
        }
    }

    #[test]
    fn apply_refuses_an_inline_providers_table() {
        let mut doc: DocumentMut = "model_providers = { x = { base_url = \"u\" } }\n"
            .parse()
            .unwrap();
        assert!(apply(&mut doc, &router(None, None), Path::new(EXE)).is_err());
    }
}
