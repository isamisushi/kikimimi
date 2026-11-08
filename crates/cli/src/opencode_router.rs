//! Global OpenCode routing. JSONC trivia is preserved; credentials never cross the UI bridge.
use crate::config::{CloudConfig, KikimimiConfig, OpenCodeRouterConfig, OpenCodeTitleRestore};
use anyhow::Context;
use jsonc_parser::cst::{CstInputValue, CstObject, CstRootNode};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

fn cloud(cfg: &KikimimiConfig) -> Option<&CloudConfig> {
    cfg.opencode_router_cloud.as_ref().or(cfg.cloud.as_ref())
}

pub fn connect() -> anyhow::Result<()> {
    let connection: CloudConfig = serde_json::from_reader(std::io::stdin().lock())?;
    anyhow::ensure!(
        connection.org_kind == "team"
            && !connection.token.is_empty()
            && !connection.org_slug.is_empty(),
        "Choose a team workspace"
    );
    let mut cfg = KikimimiConfig::load();
    anyhow::ensure!(
        cfg.opencode_router.is_none(),
        "Restore the previous provider before changing teams"
    );
    cfg.opencode_router_cloud = Some(connection);
    cfg.save()
}

fn config_path() -> anyhow::Result<PathBuf> {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => {
            PathBuf::from(std::env::var_os("HOME").context("HOME is unavailable")?).join(".config")
        }
    };
    let dir = base.join("opencode");
    // OpenCode merges JSONC after JSON, so edit the last global layer.
    let jsonc = dir.join("opencode.jsonc");
    Ok(if jsonc.exists() {
        jsonc
    } else {
        dir.join("opencode.json")
    })
}
fn key_path(path: &Path) -> PathBuf {
    path.with_file_name(".kikimimi-router-key")
}
fn load(path: &Path) -> anyhow::Result<CstRootNode> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "{}".into(),
        Err(e) => return Err(e.into()),
    };
    let root = CstRootNode::parse(&raw, &Default::default())
        .context("Invalid OpenCode JSON/JSONC settings")?;
    anyhow::ensure!(
        root.object_value().is_some(),
        "OpenCode settings must be an object"
    );
    Ok(root)
}
fn input(value: Value) -> CstInputValue {
    match value {
        Value::Null => CstInputValue::Null,
        Value::Bool(v) => CstInputValue::Bool(v),
        Value::Number(v) => CstInputValue::Number(v.to_string()),
        Value::String(v) => CstInputValue::String(v),
        Value::Array(v) => CstInputValue::Array(v.into_iter().map(input).collect()),
        Value::Object(v) => {
            CstInputValue::Object(v.into_iter().map(|(k, v)| (k, input(v))).collect())
        }
    }
}
fn set(object: &CstObject, name: &str, value: Value) {
    if let Some(prop) = object.get(name) {
        prop.set_value(input(value));
    } else {
        object.append(name, input(value));
    }
}
fn provider(router: &OpenCodeRouterConfig) -> Value {
    let mut provider = json!({
        "npm": "@ai-sdk/openai-compatible", "name": "kikimimi router",
        "options": { "baseURL": format!("{}/v1", router.base_url.trim_end_matches('/')),
            "apiKey": format!("{{file:{}}}", key_path(&router.config_path).display()) },
        "models": { (router.model.clone()): { "name": "Team router" } }
    });
    if router.title_restore.is_some() {
        provider["models"]["kikimimi-low"] = json!({"name":"Team router (low)"});
    }
    provider
}
const TITLE_MODEL: &str = "kikimimi/kikimimi-low";

fn child(obj: &CstObject, key: &str) -> Option<CstObject> {
    obj.get(key)?.value()?.as_object()
}
fn ensure_child(obj: &CstObject, key: &str) -> CstObject {
    if obj.get(key).is_none() {
        set(obj, key, json!({}));
    }
    child(obj, key).unwrap()
}
fn model(router: &OpenCodeRouterConfig) -> String {
    format!("kikimimi/{}", router.model)
}
fn validate(root: &CstRootNode) -> anyhow::Result<()> {
    let value = root.to_serde_value().unwrap();
    anyhow::ensure!(
        value.get("agent").is_none_or(Value::is_object)
            && value.pointer("/agent/title").is_none_or(Value::is_object),
        "OpenCode agent and title settings must be objects"
    );
    anyhow::ensure!(
        value.get("provider").is_none_or(Value::is_object),
        "OpenCode provider settings must be an object"
    );
    anyhow::ensure!(
        value.pointer("/provider/kikimimi").is_none(),
        "An existing kikimimi provider is configured. Remove or rename it before connecting."
    );
    anyhow::ensure!(
        !value["disabled_providers"]
            .as_array()
            .is_some_and(|a| a.iter().any(|v| v == "kikimimi")),
        "kikimimi is in OpenCode disabled_providers"
    );
    anyhow::ensure!(
        !value["enabled_providers"]
            .as_array()
            .is_some_and(|a| !a.iter().any(|v| v == "kikimimi")),
        "Add kikimimi to OpenCode enabled_providers before connecting"
    );
    Ok(())
}
fn apply(root: &CstRootNode, router: &OpenCodeRouterConfig) {
    let obj = root.object_value().unwrap();
    if obj.get("provider").is_none() {
        set(&obj, "provider", json!({}));
    }
    let providers = obj
        .get("provider")
        .unwrap()
        .value()
        .unwrap()
        .as_object()
        .unwrap();
    set(&providers, "kikimimi", provider(router));
    set(&obj, "model", json!(model(router)));
    let title = ensure_child(&ensure_child(&obj, "agent"), "title");
    set(&title, "model", json!(TITLE_MODEL));
}
fn revert(root: &CstRootNode, router: &OpenCodeRouterConfig) {
    let obj = root.object_value().unwrap();
    let value = root.to_serde_value().unwrap();
    if value.get("model") == Some(&json!(model(router))) {
        if let Some(previous) = &router.previous_model {
            set(&obj, "model", previous.clone());
        } else if let Some(prop) = obj.get("model") {
            prop.remove();
        }
    }
    if let Some(previous) = &router.title_restore {
        if value.pointer("/agent/title/model") == Some(&json!(TITLE_MODEL)) {
            let agent = child(&obj, "agent").unwrap();
            let title = child(&agent, "title").unwrap();
            match &previous.previous_model {
                Some(m) => set(&title, "model", m.clone()),
                None => title.get("model").unwrap().remove(),
            }
            if !previous.had_title && title.properties().is_empty() {
                agent.get("title").unwrap().remove();
            }
            if !previous.had_agent && agent.properties().is_empty() {
                obj.get("agent").unwrap().remove();
            }
        }
    }
    // Do not discard manual provider edits made since connecting.
    if value.pointer("/provider/kikimimi") == Some(&provider(router)) {
        if let Some(providers) = obj
            .get("provider")
            .and_then(|p| p.value())
            .and_then(|v| v.as_object())
        {
            if let Some(prop) = providers.get("kikimimi") {
                prop.remove();
            }
        }
    }
}
fn write(path: &Path, root: &CstRootNode) -> anyhow::Result<()> {
    std::fs::create_dir_all(path.parent().context("Invalid config path")?)?;
    let backup = crate::claude_settings::backup_path(path);
    if path.exists() && !backup.exists() {
        crate::state::write_atomic(&backup, &std::fs::read(path)?)?;
    }
    crate::state::write_atomic(path, root.to_string().as_bytes())
}

pub fn enable() -> anyhow::Result<()> {
    enable_with(&config_path()?)
}
fn enable_with(path: &Path) -> anyhow::Result<()> {
    let path = path.to_path_buf();
    let mut cfg = KikimimiConfig::load();
    anyhow::ensure!(
        cfg.opencode_router.is_none(),
        "OpenCode routing is already enabled. Restore the previous provider first."
    );
    let root = load(&path)?;
    validate(&root)?;
    // Also inspect JSON when JSONC is the selected layer, to avoid merging an old provider.
    if path.extension().is_some_and(|e| e == "jsonc") {
        validate(&load(&path.with_extension("json"))?)?;
    }
    anyhow::ensure!(
        !key_path(&path).exists(),
        "An OpenCode router key file already exists; restore the previous connection first"
    );
    let connection = cloud(&cfg).context("Authorize a team first")?.clone();
    anyhow::ensure!(connection.org_kind == "team", "Choose a team workspace");
    let created = crate::router_keys::create_opencode_key(&connection.endpoint, &connection.token)?;
    let result = (|| {
        let base_url = created
            .base_url
            .clone()
            .context("This team's router has no URL yet")?;
        anyhow::ensure!(
            !created.key.is_empty() && !created.auto_model.is_empty(),
            "Incomplete router response"
        );
        let router = OpenCodeRouterConfig {
            key_id: created.id.clone(),
            base_url,
            model: created.auto_model,
            config_path: path.clone(),
            title_restore: Some(OpenCodeTitleRestore {
                had_agent: root.to_serde_value().unwrap().get("agent").is_some(),
                had_title: root
                    .to_serde_value()
                    .unwrap()
                    .pointer("/agent/title")
                    .is_some(),
                previous_model: root
                    .to_serde_value()
                    .unwrap()
                    .pointer("/agent/title/model")
                    .cloned(),
            }),
            previous_model: root.to_serde_value().unwrap().get("model").cloned(),
        };
        apply(&root, &router);
        std::fs::create_dir_all(path.parent().unwrap())?;
        crate::state::write_atomic(&key_path(&path), created.key.as_bytes())?;
        // Save recovery state before changing OpenCode; failed writes remain reversible.
        cfg.opencode_router = Some(router);
        cfg.save()?;
        write(&path, &root)
    })();
    if result.is_err() {
        let _ =
            crate::router_keys::revoke_key(&connection.endpoint, &connection.token, &created.id);
        let _ = std::fs::remove_file(key_path(&path));
        cfg.opencode_router = None;
        let _ = cfg.save();
    }
    result
}
pub fn disable() -> anyhow::Result<()> {
    let mut cfg = KikimimiConfig::load();
    let Some(router) = cfg.opencode_router.clone() else {
        return Ok(());
    };
    if router.config_path.exists() {
        let root = load(&router.config_path)?;
        revert(&root, &router);
        write(&router.config_path, &root)?;
    }
    if let Some(connection) = cloud(&cfg) {
        if let Err(e) =
            crate::router_keys::revoke_key(&connection.endpoint, &connection.token, &router.key_id)
        {
            eprintln!("warning: could not revoke the OpenCode router key ({e:#})");
        }
    }
    match std::fs::remove_file(key_path(&router.config_path)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    cfg.opencode_router = None;
    cfg.save()
}
pub fn status() -> anyhow::Result<()> {
    let cfg = KikimimiConfig::load_without_secrets();
    let path = cfg
        .opencode_router
        .as_ref()
        .map(|r| r.config_path.clone())
        .unwrap_or(config_path()?);
    let value = load(&path)?.to_serde_value().unwrap();
    let applied = cfg.opencode_router.as_ref().is_some_and(|r| {
        value.pointer("/agent/title/model") == Some(&json!(TITLE_MODEL))
            && r.title_restore.is_some()
            && value["model"] == model(r)
            && value.pointer("/provider/kikimimi") == Some(&provider(r))
            && key_path(&path).exists()
    });
    println!(
        "{}",
        json!({ "logged_in": cloud(&cfg).is_some(),
        "org_slug": cloud(&cfg).map(|c| &c.org_slug), "org_kind": cloud(&cfg).map(|c| &c.org_kind),
        "host_id": kikimimi_schema::paths::host_id().ok(), "enabled": cfg.opencode_router.is_some(),
        "applied": applied, "current_model": value.get("model"), "config_path": path })
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn router() -> OpenCodeRouterConfig {
        OpenCodeRouterConfig {
            key_id: "test".into(),
            base_url: "https://router.example".into(),
            model: "kikimimi-auto".into(),
            config_path: "/tmp/opencode.jsonc".into(),
            previous_model: Some(json!("anthropic/previous")),
            title_restore: Some(Default::default()),
        }
    }
    #[test]
    fn preserves_comments_settings_and_restores_model() {
        let root = CstRootNode::parse("{ // keep me\n \"model\": \"anthropic/previous\", \"permission\": {\"edit\": \"ask\"}, \"provider\": {\"other\": {}}, }", &Default::default()).unwrap();
        let original = root.to_serde_value();
        validate(&root).unwrap();
        apply(&root, &router());
        assert!(root.to_string().contains("// keep me"));
        assert_eq!(
            root.to_serde_value().unwrap()["provider"]["kikimimi"]["options"]["baseURL"],
            "https://router.example/v1"
        );
        revert(&root, &router());
        assert_eq!(root.to_serde_value(), original);
    }
    #[test]
    fn restores_title_model_without_discarding_new_title_settings() {
        let root = CstRootNode::parse(
            r#"{"agent":{"title":{"model":"custom/old"}}}"#,
            &Default::default(),
        )
        .unwrap();
        let mut router = router();
        router.title_restore = Some(OpenCodeTitleRestore {
            had_agent: true,
            had_title: true,
            previous_model: Some(json!("custom/old")),
        });
        apply(&root, &router);
        let title = child(
            &child(&root.object_value().unwrap(), "agent").unwrap(),
            "title",
        )
        .unwrap();
        assert_eq!(
            root.to_serde_value().unwrap().pointer("/agent/title/model"),
            Some(&json!(TITLE_MODEL))
        );
        set(&title, "temperature", json!(0.5));
        revert(&root, &router);
        assert_eq!(
            root.to_serde_value().unwrap()["agent"]["title"],
            json!({"model":"custom/old","temperature":0.5})
        );
    }
    #[test]
    fn preserves_manual_changes() {
        let root = CstRootNode::parse("{}", &Default::default()).unwrap();
        apply(&root, &router());
        set(&root.object_value().unwrap(), "model", json!("other/new"));
        revert(&root, &router());
        assert_eq!(root.to_serde_value().unwrap()["model"], "other/new");
    }
    #[test]
    fn refuses_conflicts_and_provider_filters() {
        for raw in [
            r#"{"provider":{"kikimimi":{}}}"#,
            r#"{"provider":null}"#,
            r#"{"disabled_providers":["kikimimi"]}"#,
            r#"{"enabled_providers":["openai"]}"#,
        ] {
            let root = CstRootNode::parse(raw, &Default::default()).unwrap();
            assert!(validate(&root).is_err());
        }
    }
    #[test]
    fn rejects_invalid_config_without_overwriting() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.json");
        std::fs::write(&path, "{invalid").unwrap();
        assert!(load(&path).is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "{invalid");
    }
    #[test]
    fn restores_absent_model_and_keeps_changed_provider() {
        let root = CstRootNode::parse("{}", &Default::default()).unwrap();
        let mut r = router();
        r.previous_model = None;
        apply(&root, &r);
        let providers = root
            .object_value()
            .unwrap()
            .get("provider")
            .unwrap()
            .value()
            .unwrap()
            .as_object()
            .unwrap();
        set(&providers, "kikimimi", json!({"name": "manual edit"}));
        revert(&root, &r);
        assert!(root.to_serde_value().unwrap().get("model").is_none());
        assert_eq!(
            root.to_serde_value().unwrap()["provider"]["kikimimi"]["name"],
            "manual edit"
        );
    }

    #[test]
    #[serial_test::serial]
    fn lifecycle_uses_separate_authorization_and_private_key_and_recovers_failures() {
        use httpmock::prelude::*;
        let dir = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("KIKIMIMI_DIR");
        std::env::set_var("KIKIMIMI_DIR", dir.path().join("kikimimi"));
        let server = MockServer::start();
        let mut cfg = KikimimiConfig::default();
        cfg.opencode_router_cloud = Some(CloudConfig {
            endpoint: server.base_url(),
            token: "opencode-device".into(),
            org_kind: "team".into(),
            org_slug: "team".into(),
            ..Default::default()
        });
        cfg.cloud = Some(CloudConfig {
            endpoint: "https://uploads.example".into(),
            token: "upload-secret".into(),
            ..Default::default()
        });
        cfg.save().unwrap();
        let mut create = server.mock(|when, then| {
            when.method(POST).path("/v1/router/keys").header("authorization", "Bearer opencode-device");
            then.status(200).json_body(json!({"id":"oc-key", "key":"test-router-secret", "base_url":"https://router.example", "auto_model":"kikimimi-auto"}));
        });
        let revoke = server.mock(|when, then| {
            when.method(DELETE)
                .path("/v1/router/keys/oc-key")
                .header("authorization", "Bearer opencode-device");
            then.status(500);
        });
        let path = dir.path().join("opencode.jsonc");
        let original = "{ // original comment\n \"model\": \"other/model\", \"permission\": {\"edit\": \"ask\"}, }";
        std::fs::write(&path, original).unwrap();
        enable_with(&path).unwrap();
        assert!(enable_with(&path).is_err());
        create.assert_calls(1);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("test-router-secret"));
        assert!(text.contains("original comment"));
        assert_eq!(
            std::fs::read_to_string(key_path(&path)).unwrap(),
            "test-router-secret"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(key_path(&path))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        assert_eq!(
            std::fs::read_to_string(crate::claude_settings::backup_path(&path)).unwrap(),
            original
        );
        disable().unwrap(); // Remote revocation failure must not prevent local restoration.
        revoke.assert_calls(1);
        assert!(!key_path(&path).exists());
        let restored = load(&path).unwrap().to_serde_value().unwrap();
        assert_eq!(restored["model"], "other/model");
        assert_eq!(restored["permission"]["edit"], "ask");
        let loaded = KikimimiConfig::load();
        assert!(loaded.opencode_router.is_none());
        assert_eq!(loaded.cloud, cfg.cloud);
        // A malformed key response is revoked without leaving local credentials or state.
        create.delete();
        let invalid = server.mock(|when, then| {
            when.method(POST).path("/v1/router/keys");
            then.status(200).json_body(json!({"id":"oc-key", "key":"test-router-secret", "base_url":null, "auto_model":"kikimimi-auto"}));
        });
        assert!(enable_with(&path).is_err());
        invalid.assert_calls(1);
        revoke.assert_calls(2);
        assert!(!key_path(&path).exists());
        assert!(KikimimiConfig::load().opencode_router.is_none());
        match previous {
            Some(value) => std::env::set_var("KIKIMIMI_DIR", value),
            None => std::env::remove_var("KIKIMIMI_DIR"),
        }
    }
}
