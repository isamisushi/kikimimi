//! Atomic desktop collection-destination changes. No credentials on argv/stdout.
use crate::config::{CloudConfig, KikimimiConfig, S3SinkConfig};
use serde::Deserialize;
use serde_json::{json, Value};

pub(crate) fn load() -> anyhow::Result<KikimimiConfig> {
    let path = crate::config::config_path();
    if !path.exists() {
        return Ok(KikimimiConfig::default());
    }
    KikimimiConfig::load_from(&path)
}

pub(crate) fn describe(cfg: &KikimimiConfig) -> Value {
    json!({
        "cloud": cfg.cloud.as_ref().map(|c| json!({"org_slug":c.org_slug,"org_kind":c.org_kind,"hosted":c.endpoint.trim_end_matches('/') == "https://kikimimi.dev","repo_patterns":c.repo_patterns})),
        "s3": cfg.s3.as_ref().map(|s| json!({"url":s.url.trim_end_matches('/'),"endpoint_url":s.endpoint_url.as_ref().filter(|e| reqwest::Url::parse(e).is_ok_and(|u| u.username().is_empty() && u.password().is_none() && u.query().is_none() && u.fragment().is_none()))})),
    })
}

#[derive(Deserialize)]
pub(crate) struct Selection {
    kind: String,
    expected: Value,
    cloud: Option<CloudConfig>,
    s3: Option<S3SinkConfig>,
}

fn apply(cfg: &mut KikimimiConfig, selection: Selection) -> anyhow::Result<()> {
    anyhow::ensure!(
        describe(cfg) == selection.expected,
        "Collection settings changed. Review the destination again."
    );
    let (cloud, s3) = match selection.kind.as_str() {
        "local" => (None, None),
        "s3" => {
            let mut s3 = selection
                .s3
                .ok_or_else(|| anyhow::anyhow!("Choose an S3 connection"))?;
            crate::s3_reader::validate(&s3).map_err(anyhow::Error::msg)?;
            s3.url = s3.url.trim_end_matches('/').to_string();
            (None, Some(s3))
        }
        "cloud" => {
            let cloud = selection
                .cloud
                .ok_or_else(|| anyhow::anyhow!("Cloud authorization is required"))?;
            anyhow::ensure!(
                cloud.endpoint == "https://kikimimi.dev"
                    && !cloud.token.is_empty()
                    && !cloud.org_id.is_empty()
                    && !cloud.org_slug.is_empty()
                    && matches!(cloud.org_kind.as_str(), "personal" | "team"),
                "Invalid Cloud authorization"
            );
            anyhow::ensure!(
                cloud.org_kind != "team" || !cloud.repo_patterns.is_empty(),
                "Choose the repositories to share with this team"
            );
            for pattern in &cloud.repo_patterns {
                crate::repos_cmd::validate_glob(pattern)?;
            }
            (Some(cloud), None)
        }
        _ => anyhow::bail!("Unknown collection destination"),
    };
    cfg.cloud = cloud;
    cfg.s3 = s3;
    Ok(())
}

pub(crate) fn switch() -> anyhow::Result<()> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::io::stdin().take(65537).read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() <= 65536, "Collection request too large");
    let selection: Selection = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("Invalid collection request"))?;
    let mut cfg = load()?;
    apply(&mut cfg, selection)?;
    cfg.save()?;
    crate::sink_cmd::notify_daemon_reload();
    Ok(())
}

pub(crate) fn info() -> anyhow::Result<()> {
    let cfg = load()?;
    println!(
        "{}",
        json!({"host_id":kikimimi_schema::paths::host_id()?,"target":describe(&cfg)})
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn selection(cfg: &KikimimiConfig, kind: &str) -> Selection {
        Selection {
            kind: kind.into(),
            expected: describe(cfg),
            cloud: None,
            s3: None,
        }
    }
    #[test]
    fn switching_is_exclusive_and_preserves_reader_and_local_settings() {
        let mut cfg = KikimimiConfig::default();
        cfg.otlp_port = Some(1234);
        cfg.s3_reader = Some(S3SinkConfig {
            url: "s3://reader/shared".into(),
            ..Default::default()
        });
        cfg.cloud = Some(CloudConfig::default());
        let mut next = selection(&cfg, "s3");
        next.s3 = Some(S3SinkConfig {
            url: "s3://team/shared/".into(),
            ..Default::default()
        });
        apply(&mut cfg, next).unwrap();
        assert!(cfg.cloud.is_none());
        assert_eq!(cfg.s3.as_ref().unwrap().url, "s3://team/shared");
        let next = selection(&cfg, "local");
        apply(&mut cfg, next).unwrap();
        assert!(cfg.s3.is_none());
        assert_eq!(cfg.otlp_port, Some(1234));
        assert_eq!(cfg.s3_reader.unwrap().url, "s3://reader/shared");
    }
    #[test]
    fn stale_confirmation_and_invalid_input_do_not_modify_config() {
        let mut cfg = KikimimiConfig::default();
        let next = selection(&cfg, "local");
        cfg.s3 = Some(S3SinkConfig {
            url: "s3://existing/shared".into(),
            ..Default::default()
        });
        let before = cfg.clone();
        assert!(apply(&mut cfg, next).is_err());
        assert_eq!(cfg, before);
        let mut next = selection(&cfg, "s3");
        next.s3 = Some(S3SinkConfig {
            url: "https://wrong".into(),
            ..Default::default()
        });
        assert!(apply(&mut cfg, next).is_err());
        assert_eq!(cfg, before);
    }
    #[test]
    fn authorized_cloud_switch_replaces_s3_and_hides_credentials() {
        let mut cfg = KikimimiConfig::default();
        cfg.s3 = Some(S3SinkConfig {
            url: "s3://previous/shared".into(),
            ..Default::default()
        });
        let mut next = selection(&cfg, "cloud");
        next.cloud = Some(CloudConfig {
            endpoint: "https://kikimimi.dev".into(),
            token: "private-test-token".into(),
            org_id: "id".into(),
            org_slug: "team".into(),
            org_kind: "team".into(),
            repo_patterns: vec!["github.com/team/*".into()],
            ..Default::default()
        });
        apply(&mut cfg, next).unwrap();
        assert!(cfg.s3.is_none());
        assert_eq!(
            cfg.cloud.as_ref().unwrap().repo_patterns,
            vec!["github.com/team/*"]
        );
        assert!(!describe(&cfg).to_string().contains("private-test-token"));
        let next = selection(&cfg, "local");
        apply(&mut cfg, next).unwrap();
        assert!(cfg.cloud.is_none());
    }

    #[test]
    fn cloud_team_requires_explicit_sharing_scope() {
        let mut cfg = KikimimiConfig::default();
        let mut next = selection(&cfg, "cloud");
        next.cloud = Some(CloudConfig {
            endpoint: "https://kikimimi.dev".into(),
            token: "test".into(),
            org_id: "id".into(),
            org_slug: "team".into(),
            org_kind: "team".into(),
            ..Default::default()
        });
        assert!(apply(&mut cfg, next).is_err());
        assert!(cfg.cloud.is_none());
    }
}
