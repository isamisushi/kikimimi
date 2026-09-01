//! `kikimimi repos allow|list|remove` — architecture.md §6.1's team-org repo allowlist:
//! "'kikimimi repos allow <glob>' / 'kikimimi repos list' / 'kikimimi repos remove <glob>'".
//!
//! Purely local: this only edits `config.json`'s `cloud.repo_patterns` (no network call).
//! The daemon (`kikimimi agent`) applies the list via `crate::repo_filter::RepoFilter`, and
//! only when the device's org is `org_kind == "team"` — see that module's docs for the exact
//! semantics (empty list = send everything; an event with no `repo` never matches a
//! non-empty list).
//!
//! `allow`/`remove` both notify a running `kikimimi agent` to reload (the same control byte
//! `b'r'` `kikimimi sink add s3`/`kikimimi sink remove s3` already use, see
//! `sink_cmd::notify_daemon_reload`'s docs) so the change takes effect immediately, without
//! requiring a daemon restart.

use anyhow::Context;

use crate::config::KikimimiConfig;

/// A repo glob must be non-empty and free of whitespace/control characters. Not a shell-
/// injection concern (this string is never shelled out — it only ever gets pattern-matched
/// in-process against `event.repo`), but it does get `println!`-ed verbatim by `kikimimi
/// repos list`/`kikimimi status`, so the same terminal-escape-injection defense as
/// `sink_cmd::validate_s3_url` applies here too.
pub(crate) fn validate_glob(glob: &str) -> anyhow::Result<()> {
    if glob.is_empty() {
        anyhow::bail!("repo glob must not be empty");
    }
    if glob.chars().any(|c| c.is_whitespace() || c.is_control()) {
        anyhow::bail!("repo glob must not contain whitespace or control characters (got {glob:?})");
    }
    Ok(())
}

/// `kikimimi repos allow <glob>`.
pub fn allow(glob: String) -> anyhow::Result<()> {
    validate_glob(&glob)?;

    let mut cfg = KikimimiConfig::load();
    let cloud = cfg.cloud.as_mut().ok_or_else(|| {
        anyhow::anyhow!(
            "not logged in; run `kikimimi login --org <team-slug>` first \
             (the repo allowlist only has an effect for a team org)"
        )
    })?;

    if cloud.repo_patterns.iter().any(|p| p == &glob) {
        println!("{glob} is already allowed");
        return Ok(());
    }
    cloud.repo_patterns.push(glob.clone());
    let org_kind = cloud.org_kind.clone();
    cfg.save().context("saving config.json")?;

    println!("repo allowed: {glob}");
    if org_kind != "team" {
        println!(
            "note: this device's active org is not a team org (org_kind={org_kind:?}) — the \
             repo filter only applies to team orgs, so this pattern has no effect yet"
        );
    }
    crate::sink_cmd::notify_daemon_reload();
    Ok(())
}

/// `kikimimi repos remove <glob>`.
pub fn remove(glob: &str) -> anyhow::Result<()> {
    let mut cfg = KikimimiConfig::load();
    let Some(cloud) = cfg.cloud.as_mut() else {
        println!("no repo patterns configured");
        return Ok(());
    };

    let before = cloud.repo_patterns.len();
    cloud.repo_patterns.retain(|p| p != glob);
    if cloud.repo_patterns.len() == before {
        println!("{glob} was not in the allowlist");
        return Ok(());
    }

    cfg.save().context("saving config.json")?;
    println!("repo removed: {glob}");
    crate::sink_cmd::notify_daemon_reload();
    Ok(())
}

/// `kikimimi repos list`.
pub fn list() -> anyhow::Result<()> {
    let cfg = KikimimiConfig::load();
    let Some(cloud) = cfg.cloud.as_ref() else {
        println!("not logged in (repo allowlist only has an effect for a team org)");
        return Ok(());
    };

    println!(
        "active org: {} [{}]",
        if cloud.org_slug.is_empty() {
            "-"
        } else {
            &cloud.org_slug
        },
        if cloud.org_kind.is_empty() {
            "unknown"
        } else {
            &cloud.org_kind
        }
    );
    if cloud.org_kind != "team" {
        println!("(repo filter only applies to team orgs; this org's events are never filtered)");
    }
    if cloud.repo_patterns.is_empty() {
        println!(
            "repo patterns: none configured{}",
            if cloud.org_kind == "team" {
                " — every event is currently sent to the team cloud unfiltered"
            } else {
                ""
            }
        );
    } else {
        println!("repo patterns:");
        for p in &cloud.repo_patterns {
            println!("  {p}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CloudConfig;
    use serial_test::serial;

    fn logged_in_team_config() -> KikimimiConfig {
        KikimimiConfig {
            cloud: Some(CloudConfig {
                endpoint: "http://127.0.0.1:8787".into(),
                token: "tok".into(),
                email: "dev@example.com".into(),
                org_id: "org-1".into(),
                org_slug: "acme".into(),
                org_kind: "team".into(),
                repo_patterns: Vec::new(),
            }),
            ..Default::default()
        }
    }

    #[test]
    fn validate_glob_rejects_empty_and_whitespace() {
        assert!(validate_glob("").is_err());
        assert!(validate_glob("github.com/acme/ api").is_err());
        assert!(validate_glob("github.com/acme/api\n").is_err());
        assert!(validate_glob("github.com/acme/api\x1b[2K").is_err());
    }

    #[test]
    fn validate_glob_accepts_a_plain_glob() {
        assert!(validate_glob("github.com/acme/*").is_ok());
    }

    #[test]
    #[serial]
    fn allow_errors_when_not_logged_in() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());
        let result = allow("github.com/acme/*".to_string());
        assert!(result.is_err());
        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    #[serial]
    fn allow_rejects_an_invalid_glob() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());
        logged_in_team_config().save().unwrap();

        let result = allow("bad glob".to_string());
        assert!(result.is_err());
        assert!(KikimimiConfig::load()
            .cloud
            .unwrap()
            .repo_patterns
            .is_empty());

        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    #[serial]
    fn allow_appends_a_new_pattern_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());
        logged_in_team_config().save().unwrap();

        allow("github.com/acme/*".to_string()).unwrap();
        allow("github.com/acme/internal-*".to_string()).unwrap();
        // Re-adding the same pattern must not duplicate it.
        allow("github.com/acme/*".to_string()).unwrap();

        let cfg = KikimimiConfig::load();
        assert_eq!(
            cfg.cloud.unwrap().repo_patterns,
            vec![
                "github.com/acme/*".to_string(),
                "github.com/acme/internal-*".to_string()
            ]
        );

        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    #[serial]
    fn remove_deletes_exactly_the_matching_pattern() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());
        let mut cfg = logged_in_team_config();
        cfg.cloud.as_mut().unwrap().repo_patterns = vec![
            "github.com/acme/*".to_string(),
            "github.com/other/*".to_string(),
        ];
        cfg.save().unwrap();

        remove("github.com/acme/*").unwrap();

        let after = KikimimiConfig::load();
        assert_eq!(
            after.cloud.unwrap().repo_patterns,
            vec!["github.com/other/*".to_string()]
        );

        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    #[serial]
    fn remove_when_not_present_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());
        logged_in_team_config().save().unwrap();

        assert!(remove("github.com/nonexistent/*").is_ok());
        assert!(KikimimiConfig::load()
            .cloud
            .unwrap()
            .repo_patterns
            .is_empty());

        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    #[serial]
    fn remove_when_not_logged_in_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());
        assert!(remove("github.com/acme/*").is_ok());
        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    #[serial]
    fn list_does_not_panic_when_not_logged_in() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());
        assert!(list().is_ok());
        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    #[serial]
    fn list_does_not_panic_with_patterns_configured() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());
        let mut cfg = logged_in_team_config();
        cfg.cloud.as_mut().unwrap().repo_patterns = vec!["github.com/acme/*".to_string()];
        cfg.save().unwrap();

        assert!(list().is_ok());

        std::env::remove_var("KIKIMIMI_DIR");
    }
}
