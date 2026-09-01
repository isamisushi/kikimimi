//! `kikimimi sink add|list|remove` — BYO sink 設定 (architecture.md §4「sink (出口)」、
//! §6「BYO sink (任意)」)。
//!
//! **kikimimi はここに認証情報を一切保存しない**: `s3` sink の `config.json` エントリは
//! `url`/`profile`/`endpoint_url` だけを持つ (`crate::config::S3SinkConfig`)。アップロード
//! は `kikimimi agent` が起動する `kikimimi_sink::S3Sink` が `aws` CLI にシェルアウトして行い、
//! ユーザーの既存プロファイル/SSO/IAM ロールをそのまま使う。
//!
//! `add`/`remove` はどちらも、書き込み後に稼働中の `kikimimi agent` へ制御バイト `b'r'`
//! (reload) を送る (ベストエフォート — デーモンが起動していなくても失敗にはしない)。
//! これにより `kikimimi agent` を再起動しなくても新しい sink 設定がすぐ効く (agent.rs)。

use anyhow::Context;

use crate::config::{KikimimiConfig, S3SinkConfig};

/// `url` を `kikimimi sink add s3` の入力として受け付けてよいか検証する:
///
/// - `s3://` で始まること (それ以外のスキームは `aws s3 cp` の宛先として意味を成さない)。
/// - 空白文字・制御文字 (改行/タブ/ESC 等、`char::is_control` / ASCII space) を含まないこと。
///   kikimimi はこの `url` を (a) そのまま `Command::args` の 1 要素として `aws` CLI に渡し
///   (シェル文字列には決して埋め込まない — 別クレートの `S3Sink::run_uploader` 参照。
///   そちら側は spawn 時点で argv 分割済みなのでシェルインジェクションの経路はそもそも
///   無いが) (b) `kikimimi status`/`kikimimi sink list` の出力にそのまま `println!` する。
///   ANSI エスケープや改行を含む `url` を許すと、後者の端末出力を細工できてしまう
///   (例: 出力を上書き/隠す端末エスケープシーケンス注入) ので、そもそも受け付けない。
pub(crate) fn validate_s3_url(url: &str) -> anyhow::Result<()> {
    if !url.starts_with("s3://") {
        anyhow::bail!("s3 sink url must start with s3:// (got {url:?})");
    }
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        anyhow::bail!(
            "s3 sink url must not contain whitespace or control characters (got {url:?})"
        );
    }
    Ok(())
}

/// `kikimimi sink add s3 <url> [--profile P] [--endpoint-url E]`。
pub fn add_s3(
    url: String,
    profile: Option<String>,
    endpoint_url: Option<String>,
) -> anyhow::Result<()> {
    validate_s3_url(&url)?;
    let url = url.trim_end_matches('/').to_string();

    let mut cfg = KikimimiConfig::load();
    cfg.s3 = Some(S3SinkConfig {
        url: url.clone(),
        profile,
        endpoint_url,
    });
    cfg.save().context("saving config.json")?;

    println!("s3 sink configured: {url}");
    notify_daemon_reload();
    Ok(())
}

/// `kikimimi sink remove s3`。
pub fn remove(kind: &str) -> anyhow::Result<()> {
    match kind {
        "s3" => remove_s3(),
        other => anyhow::bail!("unknown sink kind {other:?} (supported: s3)"),
    }
}

fn remove_s3() -> anyhow::Result<()> {
    let mut cfg = KikimimiConfig::load();
    if cfg.s3.take().is_none() {
        println!("no s3 sink configured");
        return Ok(());
    }
    cfg.save().context("saving config.json")?;
    println!("s3 sink removed");
    notify_daemon_reload();
    Ok(())
}

/// `kikimimi sink list`。BYO sink の実際の pending/last_push/last_error は
/// `kikimimi status` が state.json から出す (agent.rs `sync_s3_state`) — ここは
/// config.json に何が設定されているかだけを見せる。
pub fn list() -> anyhow::Result<()> {
    let cfg = KikimimiConfig::load();
    println!("sinks:");
    println!(
        "  file: always on (local Parquet, {})",
        kikimimi_schema::paths::data_dir().display()
    );
    match &cfg.cloud {
        Some(c) => println!("  cloud: {}", c.endpoint),
        None => println!("  cloud: not logged in (run `kikimimi login`)"),
    }
    match &cfg.s3 {
        Some(s) => {
            let mut line = format!("  s3: {}", s.url);
            if let Some(p) = &s.profile {
                line.push_str(&format!(" (profile={p})"));
            }
            if let Some(e) = &s.endpoint_url {
                line.push_str(&format!(" (endpoint-url={e})"));
            }
            println!("{line}");
        }
        None => println!("  s3: not configured (run `kikimimi sink add s3 <s3://bucket/prefix>`)"),
    }
    println!();
    println!("run `kikimimi status` for pending/last_push/last_error on each sink");
    Ok(())
}

/// `kikimimi agent` が起動していれば制御バイト `b'r'` で sink 設定の再読み込みを頼む
/// (agent.rs)。起動していなければ次回起動時にどのみち最新の config.json を読むので、
/// 送れなくても何もしない (fail-open, `kikimimi flush` 等と同じ形)。
///
/// `pub(crate)`: `repos_cmd.rs`'s `allow`/`remove` reuse this exact same "ask the running
/// daemon to re-read config.json" signal (agent.rs's `b'r'` handler reloads both the s3 sink
/// and the repo filter from the same config load) rather than duplicating the helper.
pub(crate) fn notify_daemon_reload() {
    if kikimimi_spool::send_control(b'r') {
        println!("kikimimi agent: reloaded sinks");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    #[serial]
    fn add_s3_rejects_non_s3_url() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());

        let result = add_s3("https://example.com/bucket".to_string(), None, None);
        assert!(result.is_err());
        assert!(KikimimiConfig::load().s3.is_none());

        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    fn validate_s3_url_rejects_whitespace_and_control_chars() {
        // Space, tab, newline, CR, and a raw ESC (start of an ANSI escape sequence --
        // must not be smugglable into `kikimimi status`/`kikimimi sink list` output).
        for bad in [
            "s3://bucket/pre fix",
            "s3://bucket/pre\tfix",
            "s3://bucket/pre\nfix",
            "s3://bucket/pre\rfix",
            "s3://bucket/pre\x1bfix",
            "s3://bucket/pre\x00fix",
        ] {
            assert!(
                validate_s3_url(bad).is_err(),
                "expected {bad:?} to be rejected"
            );
        }
    }

    #[test]
    fn validate_s3_url_accepts_a_plain_bucket_prefix_url() {
        assert!(validate_s3_url("s3://my-bucket/team").is_ok());
    }

    #[test]
    #[serial]
    fn add_s3_rejects_url_with_embedded_control_char() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());

        let result = add_s3("s3://bucket/pre\nfix".to_string(), None, None);
        assert!(result.is_err());
        assert!(KikimimiConfig::load().s3.is_none());

        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    #[serial]
    fn add_s3_saves_config_and_trims_trailing_slash() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());

        add_s3(
            "s3://my-bucket/team/".to_string(),
            Some("myprofile".to_string()),
            Some("http://127.0.0.1:9000".to_string()),
        )
        .unwrap();

        let cfg = KikimimiConfig::load();
        let s3 = cfg.s3.expect("s3 sink must be saved");
        assert_eq!(s3.url, "s3://my-bucket/team");
        assert_eq!(s3.profile.as_deref(), Some("myprofile"));
        assert_eq!(s3.endpoint_url.as_deref(), Some("http://127.0.0.1:9000"));

        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    #[serial]
    fn remove_s3_clears_config_but_preserves_cloud() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());

        let mut cfg = KikimimiConfig::load();
        cfg.cloud = Some(crate::config::CloudConfig {
            endpoint: "http://127.0.0.1:8787".into(),
            token: "tok".into(),
            email: "dev@example.com".into(),
            org_id: "org-1".into(),
            ..Default::default()
        });
        cfg.s3 = Some(S3SinkConfig {
            url: "s3://my-bucket/team".into(),
            profile: None,
            endpoint_url: None,
        });
        cfg.save().unwrap();

        remove("s3").unwrap();

        let after = KikimimiConfig::load();
        assert_eq!(after.s3, None);
        assert!(
            after.cloud.is_some(),
            "removing s3 must not touch cloud config"
        );

        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    #[serial]
    fn remove_unknown_kind_errors() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());
        assert!(remove("postgres").is_err());
        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    #[serial]
    fn remove_s3_when_not_configured_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());
        assert!(remove("s3").is_ok());
        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    #[serial]
    fn list_does_not_panic_with_nothing_configured() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());
        assert!(list().is_ok());
        std::env::remove_var("KIKIMIMI_DIR");
    }
}
