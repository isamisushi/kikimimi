//! `[otel]` in `$CODEX_HOME/config.toml`: Codex's own log export, pointed at the
//! collector's OTLP receiver. `kikimimi init` writes it and `kikimimi uninstall`
//! removes it.
//!
//! Verified 2026-11-03 on Codex.app 26.1027.x (engine 0.166.0-alpha.5.1): with
//! this table the CLI, the desktop app's Codex mode and Work all export one
//! `codex.sse_event` per API call with the same token counts as the rollout. The
//! app reads it only at launch, so Codex has to be restarted. Chat mode runs on
//! OpenAI's servers and exports nothing.
//!
//! Only the logs exporter is set. Codex's own metrics and trace defaults are left
//! alone, and `log_user_prompt = false` keeps prompt text out of the export. A
//! `[otel]` table someone else wrote is never changed; the `environment` value
//! marks the table as kikimimi's.

use std::path::Path;

use toml_edit::{value, DocumentMut, InlineTable, Item, Table};

const ENVIRONMENT: &str = "kikimimi";

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Written,
    Unchanged,
    /// `[otel]` exists and was not written by kikimimi.
    Foreign,
}

pub fn endpoint(port: u16) -> String {
    format!("http://127.0.0.1:{port}/v1/logs")
}

fn is_ours(doc: &DocumentMut) -> Option<bool> {
    let otel = doc.get("otel")?;
    Some(otel.get("environment").and_then(Item::as_str) == Some(ENVIRONMENT))
}

/// Whether this machine's Codex exports to kikimimi, which makes the rollout
/// tailer a reconciler instead of the live source.
pub fn is_configured(config_toml: &Path) -> bool {
    crate::codex_router::load_document(config_toml)
        .ok()
        .and_then(|doc| is_ours(&doc))
        .unwrap_or(false)
}

pub fn apply(doc: &mut DocumentMut, port: u16, token: Option<&str>) -> Outcome {
    if is_ours(doc) == Some(false) {
        return Outcome::Foreign;
    }
    let mut http = InlineTable::new();
    http.insert("endpoint", endpoint(port).into());
    http.insert("protocol", "binary".into());
    if let Some(token) = token {
        let mut headers = InlineTable::new();
        headers.insert("Authorization", format!("Bearer {token}").into());
        http.insert("headers", headers.into());
    }
    let mut exporter = InlineTable::new();
    exporter.insert("otlp-http", http.into());

    let mut otel = Table::new();
    otel.insert("environment", value(ENVIRONMENT));
    otel.insert("log_user_prompt", value(false));
    otel.insert("exporter", value(exporter));
    // Compare semantically: formatting differs between a parsed and a built table.
    let current = doc.get("otel").map(|item| item.to_string());
    let next = Item::Table(otel);
    if current.as_deref().map(normalize) == Some(normalize(&next.to_string())) {
        return Outcome::Unchanged;
    }
    doc.insert("otel", next);
    Outcome::Written
}

fn normalize(text: &str) -> String {
    text.split_whitespace().collect()
}

/// How this Mac's Codex usage is collected, for the desktop status:
/// `absent` (no Codex), `otel` (kikimimi's export), `own_otel` (the user's own
/// `[otel]`, left alone) or `rollout` (session records only).
pub fn mode() -> &'static str {
    if !kikimimi_schema::paths::codex_home_dir().exists() {
        return "absent";
    }
    let doc = crate::codex_router::load_document(&crate::codex_router::config_toml_path());
    match doc.ok().as_ref().and_then(is_ours) {
        Some(true) => "otel",
        Some(false) => "own_otel",
        None => "rollout",
    }
}

/// Write kikimimi's `[otel]` with the collector's current port and token when
/// Codex is installed. `None` without Codex. Used when the desktop app resumes
/// collection, so a Mac set up before this existed gets it too; `kikimimi init`
/// goes through its own dry-run aware path.
pub fn ensure() -> anyhow::Result<Option<Outcome>> {
    if !kikimimi_schema::paths::codex_home_dir().exists() {
        return Ok(None);
    }
    let path = crate::codex_router::config_toml_path();
    let mut doc = crate::codex_router::load_document(&path)?;
    let token = crate::config::KikimimiConfig::load_without_secrets().otlp_token;
    let outcome = apply(&mut doc, crate::config::resolve_otlp_port(), token.as_deref());
    if outcome == Outcome::Written {
        crate::codex_router::write_document(&path, &doc)?;
    }
    Ok(Some(outcome))
}

/// Remove kikimimi's `[otel]`. Returns whether anything was removed.
pub fn remove(doc: &mut DocumentMut) -> bool {
    is_ours(doc) == Some(true) && doc.remove("otel").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    const USER: &str = "# my settings\nmodel = \"gpt-6-astra\"\n\n[features]\ncodex_hooks = true\n";

    #[test]
    fn apply_adds_the_logs_exporter_and_keeps_the_rest() {
        let mut doc: DocumentMut = USER.parse().unwrap();
        assert_eq!(apply(&mut doc, 4318, Some("tok")), Outcome::Written);
        let text = doc.to_string();
        assert!(text.starts_with(USER), "{text}");
        let otel = &doc["otel"];
        assert_eq!(otel["environment"].as_str(), Some("kikimimi"));
        assert_eq!(otel["log_user_prompt"].as_bool(), Some(false));
        let http = &otel["exporter"]["otlp-http"];
        assert_eq!(http["endpoint"].as_str(), Some("http://127.0.0.1:4318/v1/logs"));
        assert_eq!(http["protocol"].as_str(), Some("binary"));
        assert_eq!(http["headers"]["Authorization"].as_str(), Some("Bearer tok"));
        assert!(otel.get("metrics_exporter").is_none());

        let mut reparsed: DocumentMut = text.parse().unwrap();
        assert_eq!(apply(&mut reparsed, 4318, Some("tok")), Outcome::Unchanged);
        assert_eq!(apply(&mut reparsed, 4400, Some("tok")), Outcome::Written);
        assert!(reparsed.to_string().contains("127.0.0.1:4400"));

        assert!(remove(&mut reparsed));
        assert_eq!(reparsed.to_string().trim_end(), USER.trim_end());
        assert!(!remove(&mut reparsed));
    }

    #[test]
    fn a_users_own_otel_table_is_left_alone() {
        let own = format!("{USER}\n[otel]\nenvironment = \"prod\"\n");
        let mut doc: DocumentMut = own.parse().unwrap();
        assert_eq!(apply(&mut doc, 4318, None), Outcome::Foreign);
        assert!(!remove(&mut doc));
        assert_eq!(doc.to_string(), own);
    }

    #[test]
    fn is_configured_reads_the_marker() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        assert!(!is_configured(&path));
        let mut doc: DocumentMut = USER.parse().unwrap();
        apply(&mut doc, 4318, None);
        std::fs::write(&path, doc.to_string()).unwrap();
        assert!(is_configured(&path));
    }
}
