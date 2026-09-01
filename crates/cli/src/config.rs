//! `~/.kikimimi/config.json` — `kikimimi init`/`kikimimi login` が確定させた設定を `kikimimi agent` に
//! 橋渡しする小さな永続設定ファイル (architecture.md §4 「OTLP レシーバ」、§6 「デーモン
//! → cloud」)。
//!
//! 持ち回るのは 2 つ:
//! - `otlp_port`: `kikimimi init` はポート使用状況を検査し、衝突していれば別ポートを選んで
//!   Claude Code の `settings.json` に書き込む (`init_cmd.rs`) が、選んだポートを
//!   `kikimimi agent` 自身にも伝えないと、次に agent を起動したときにまた既定の 4318 で
//!   bind を試みて再度衝突してしまう。そこで選んだポートをここに永続化し、`kikimimi agent`
//!   起動時に読む (`KIKIMIMI_OTLP_PORT` 環境変数が明示的に設定されていれば、そちらが常に
//!   優先される)。
//! - `cloud`: `kikimimi login` が発行させたデバイストークンと、それに紐づく `org_id`/
//!   `email`。`kikimimi agent` はこれがあれば `CloudSink` を立ち上げる (agent.rs)。
//!
//! `token` は秘密情報なので、このファイルは常に 0600 (owner のみ読み書き) で書く
//! (`save_to`)。

use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

/// `kikimimi login` が確定させた cloud 認証情報 (architecture.md §6「デーモン → cloud」、
/// §6.1 アカウントモデル)。`token` は平文でここに保存される — 保存先ファイル自体を 0600 に
/// 絞ることで守る (macOS Keychain 等への格納は Stage 0 では未実装、将来の TODO)。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CloudConfig {
    pub endpoint: String,
    pub token: String,
    pub email: String,
    pub org_id: String,
    /// この端末がアクティブに紐づいている org の slug (§6.1「1 マシン = 1 アクティブ org」)。
    /// `POST /v1/device/token` の "ok" レスポンスが返す `org_slug` をそのまま保存する。
    /// `#[serde(default)]`: account-model 対応前の旧い config.json (このキーが無い) も読める
    /// — その場合は空文字列になり、`kikimimi orgs`/`kikimimi status` 側が「未確認」として扱う。
    #[serde(default)]
    pub org_slug: String,
    /// 上記 org の種別 ("personal" | "team")。空文字列は「不明 (旧い config.json)」を意味し、
    /// リポジトリフィルタ (`repo_patterns`) は `"team"` のときだけ効く
    /// (`crate::repo_filter::RepoFilter` 参照) — 空文字列や `"personal"` は素通し。
    #[serde(default)]
    pub org_kind: String,
    /// `kikimimi repos allow/remove` が編集するリポジトリ許可リスト (glob のリスト)。
    /// §6.1: "team org へは端末側の「リポジトリパターン許可リスト」に一致する repo の
    /// イベントだけ送信" — `org_kind == "team"` のときだけデーモンが適用する
    /// (`crate::repo_filter`)。personal org では常に無視される (パターンが残っていても
    /// 送信を絞らない)。`#[serde(default)]`: 旧い config.json も読める。
    #[serde(default)]
    pub repo_patterns: Vec<String>,
}

/// `kikimimi sink add s3` が確定させた BYO S3 sink の設定 (architecture.md §4「sink (出口)」、
/// §6「BYO sink (任意)」)。**認証情報は一切保存しない** — アップロードは常に `aws` CLI
/// (`kikimimi_sink::S3Sink`) にシェルアウトし、ユーザーの既存プロファイル/SSO/IAM ロールを
/// そのまま使わせる。ここに書くのは宛先とオプションの `--profile`/`--endpoint-url` だけ
/// (どちらも秘密情報ではない — `url` は `kikimimi status`/`kikimimi sink list` にそのまま出す)。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct S3SinkConfig {
    /// `s3://bucket/prefix` (末尾の `/` の有無は問わない)。
    pub url: String,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub endpoint_url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct KikimimiConfig {
    /// `kikimimi init` が (衝突検知の結果) 選んだ OTLP ポート。未設定なら既定値を使う。
    #[serde(default)]
    pub otlp_port: Option<u16>,
    /// `kikimimi agent` が (衝突検知の結果) 選んだローカル web UI のポート
    /// (architecture.md §8)。`otlp_port` と同じ役割・同じ持ち回り方だが、選定は
    /// `kikimimi init` ではなく `kikimimi agent` 自身の起動時に行う (§8 には別コマンドが
    /// 無いため)。未設定なら既定値 4319 を使う。
    #[serde(default)]
    pub web_port: Option<u16>,
    /// `kikimimi login` していなければ `None`。`kikimimi logout` はこれを `None` に戻す。
    #[serde(default)]
    pub cloud: Option<CloudConfig>,
    /// `kikimimi sink add s3` していなければ `None`。`kikimimi sink remove s3` はこれを
    /// `None` に戻す。
    #[serde(default)]
    pub s3: Option<S3SinkConfig>,
}

pub fn config_path() -> PathBuf {
    kikimimi_schema::paths::kikimimi_dir().join("config.json")
}

impl KikimimiConfig {
    /// ファイルが無い・壊れている場合は既定値 (`otlp_port: None`) を返す
    /// (config.json は補助的な永続化であり、これを読めないこと自体で失敗させない)。
    pub fn load() -> Self {
        Self::load_from(&config_path()).unwrap_or_default()
    }

    pub fn load_from(path: &Path) -> anyhow::Result<Self> {
        let raw =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn save(&self) -> anyhow::Result<()> {
        self.save_to(&config_path())
    }

    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let bytes = serde_json::to_vec_pretty(self).context("serializing config.json")?;
        crate::state::write_atomic(path, &bytes)?;
        // May carry a cloud token (§6): restrict to owner-only, same as an SSH key.
        // `write_atomic`'s tmp-then-rename means `path` only exists once this is reachable.
        set_owner_only_permissions(path)
            .with_context(|| format!("setting owner-only permissions on {}", path.display()))
    }
}

fn set_owner_only_permissions(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

/// `kikimimi agent` が実際に bind すべき OTLP ポートを決める。
/// 優先順位: `KIKIMIMI_OTLP_PORT` 環境変数 (明示的な上書き。テスト/smoke.sh 用) >
/// `config.json` の `otlp_port` (`kikimimi init` が衝突検知の結果として選んだ値) >
/// 既定の 4318。
/// Whether the OTLP port comes from an explicit env override (`KIKIMIMI_OTLP_PORT`,
/// or the legacy `GURU_OTLP_PORT`) rather than config.json/the default -- `init_cmd.rs`
/// uses this to decide whether to trust the port verbatim (skip conflict probing)
/// or to actively probe/pick a free one.
pub fn otlp_port_env_override() -> Option<u16> {
    kikimimi_schema::env_compat::env_u16_with_legacy("KIKIMIMI_OTLP_PORT", "GURU_OTLP_PORT")
}

pub fn resolve_otlp_port() -> u16 {
    if let Some(p) = otlp_port_env_override() {
        return p;
    }
    if let Some(p) = KikimimiConfig::load().otlp_port {
        return p;
    }
    kikimimi_otlp::default_addr().port()
}

/// `kikimimi agent` の web UI (architecture.md §8) が bind すべき *希望* ポートを決める。
/// `resolve_otlp_port` と同じ優先順位: `KIKIMIMI_WEB_PORT` 環境変数 > `config.json` の
/// `web_port` (前回起動時に衝突検知の結果として選んだ値) > 既定の 4319。
///
/// あくまで「希望」であって最終決定ではない点が `resolve_otlp_port` と異なる:
/// OTLP は `kikimimi init` が事前に衝突検知・別ポート選定・持ち回りを済ませるのに対し、
/// web UI 用の別コマンドは無いため、衝突検知と別ポート選定は `kikimimi agent` 自身の
/// 起動時 (agent.rs) が行う — ここは常に「今設定されている値」を返すだけ。
/// Whether the web UI port comes from an explicit env override (`KIKIMIMI_WEB_PORT`,
/// or the legacy `GURU_WEB_PORT`) -- `agent.rs` uses this the same way
/// [`otlp_port_env_override`] is used by `init_cmd.rs`.
pub fn web_port_env_override() -> Option<u16> {
    kikimimi_schema::env_compat::env_u16_with_legacy("KIKIMIMI_WEB_PORT", "GURU_WEB_PORT")
}

pub fn resolve_web_port_preferred() -> u16 {
    if let Some(p) = web_port_env_override() {
        return p;
    }
    if let Some(p) = KikimimiConfig::load().web_port {
        return p;
    }
    DEFAULT_WEB_PORT
}

/// architecture.md §8: "127.0.0.1:$KIKIMIMI_WEB_PORT (default 4319 ...)".
pub const DEFAULT_WEB_PORT: u16 = 4319;

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let cfg = KikimimiConfig {
            otlp_port: Some(54321),
            web_port: Some(4319),
            cloud: None,
            s3: None,
        };
        cfg.save_to(&path).unwrap();
        let loaded = KikimimiConfig::load_from(&path).unwrap();
        assert_eq!(loaded, cfg);
    }

    #[test]
    fn load_missing_file_defaults_to_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nope.json");
        assert!(KikimimiConfig::load_from(&path).is_err());
    }

    #[test]
    #[serial]
    fn resolve_otlp_port_prefers_env_over_config_over_default() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());
        std::env::remove_var("KIKIMIMI_OTLP_PORT");

        // Nothing set at all: falls back to kikimimi_otlp's default (4318, since
        // KIKIMIMI_OTLP_PORT is unset here too).
        assert_eq!(resolve_otlp_port(), 4318);

        // config.json alone: used.
        KikimimiConfig {
            otlp_port: Some(15000),
            ..Default::default()
        }
        .save()
        .unwrap();
        assert_eq!(resolve_otlp_port(), 15000);

        // KIKIMIMI_OTLP_PORT env var present: wins over config.json.
        std::env::set_var("KIKIMIMI_OTLP_PORT", "16000");
        assert_eq!(resolve_otlp_port(), 16000);

        std::env::remove_var("KIKIMIMI_OTLP_PORT");
        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    #[serial]
    fn resolve_otlp_port_falls_back_to_legacy_guru_env_var() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());
        std::env::remove_var("KIKIMIMI_OTLP_PORT");
        std::env::set_var("GURU_OTLP_PORT", "17000");

        assert_eq!(resolve_otlp_port(), 17000);
        assert_eq!(otlp_port_env_override(), Some(17000));

        std::env::remove_var("GURU_OTLP_PORT");
        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    #[serial]
    fn resolve_web_port_preferred_prefers_env_over_config_over_default() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());
        std::env::remove_var("KIKIMIMI_WEB_PORT");

        // Nothing set at all: falls back to the 4319 default.
        assert_eq!(resolve_web_port_preferred(), 4319);

        // config.json alone: used.
        KikimimiConfig {
            web_port: Some(15001),
            ..Default::default()
        }
        .save()
        .unwrap();
        assert_eq!(resolve_web_port_preferred(), 15001);

        // KIKIMIMI_WEB_PORT env var present: wins over config.json.
        std::env::set_var("KIKIMIMI_WEB_PORT", "16001");
        assert_eq!(resolve_web_port_preferred(), 16001);

        std::env::remove_var("KIKIMIMI_WEB_PORT");
        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    #[serial]
    fn resolve_web_port_preferred_falls_back_to_legacy_guru_env_var() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("KIKIMIMI_DIR", dir.path());
        std::env::remove_var("KIKIMIMI_WEB_PORT");
        std::env::set_var("GURU_WEB_PORT", "17001");

        assert_eq!(resolve_web_port_preferred(), 17001);
        assert_eq!(web_port_env_override(), Some(17001));

        std::env::remove_var("GURU_WEB_PORT");
        std::env::remove_var("KIKIMIMI_DIR");
    }

    #[test]
    fn save_leaves_no_tmp_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        KikimimiConfig::default().save_to(&path).unwrap();
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["config.json"]);
    }

    #[test]
    fn save_writes_the_file_owner_only_0600() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        KikimimiConfig {
            otlp_port: None,
            cloud: Some(CloudConfig {
                endpoint: "http://127.0.0.1:8787".into(),
                token: "super-secret-token".into(),
                email: "dev@local".into(),
                org_id: "org-1".into(),
                ..Default::default()
            }),
            ..Default::default()
        }
        .save_to(&path)
        .unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "config.json (may hold a cloud token) must be owner-only, got {mode:o}"
        );
    }

    #[test]
    fn cloud_config_roundtrips_through_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let cfg = KikimimiConfig {
            otlp_port: Some(4318),
            cloud: Some(CloudConfig {
                endpoint: "https://cloud.example".into(),
                token: "tok-abc".into(),
                email: "me@example.com".into(),
                org_id: "org-xyz".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        cfg.save_to(&path).unwrap();
        assert_eq!(KikimimiConfig::load_from(&path).unwrap(), cfg);
    }

    /// account-model contract (architecture.md §6.1): `org_slug`/`org_kind`/`repo_patterns`
    /// must all round-trip through save/load exactly like the pre-existing `CloudConfig`
    /// fields.
    #[test]
    fn cloud_config_org_and_repo_patterns_roundtrip_through_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let cfg = KikimimiConfig {
            cloud: Some(CloudConfig {
                endpoint: "https://cloud.example".into(),
                token: "tok-abc".into(),
                email: "me@example.com".into(),
                org_id: "org-xyz".into(),
                org_slug: "acme".into(),
                org_kind: "team".into(),
                repo_patterns: vec!["github.com/acme/*".into(), "*/internal-*".into()],
            }),
            ..Default::default()
        };
        cfg.save_to(&path).unwrap();
        assert_eq!(KikimimiConfig::load_from(&path).unwrap(), cfg);
    }

    /// backward-compat: a `cloud` object written before the account-model contract (no
    /// "org_slug"/"org_kind"/"repo_patterns" keys at all) must still load, defaulting to
    /// empty string / empty string / empty vec respectively.
    #[test]
    fn old_cloud_config_without_org_or_repo_pattern_keys_loads_with_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            br#"{"cloud": {"endpoint": "https://cloud.example", "token": "tok", "email": "me@example.com", "org_id": "org-1"}}"#,
        )
        .unwrap();
        let loaded = KikimimiConfig::load_from(&path).unwrap();
        let cloud = loaded.cloud.expect("cloud must still load");
        assert_eq!(cloud.org_slug, "");
        assert_eq!(cloud.org_kind, "");
        assert!(cloud.repo_patterns.is_empty());
    }

    #[test]
    fn old_config_json_without_cloud_key_loads_with_cloud_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, br#"{"otlp_port": 4318}"#).unwrap();
        let loaded = KikimimiConfig::load_from(&path).unwrap();
        assert_eq!(loaded.cloud, None);
        assert_eq!(loaded.otlp_port, Some(4318));
    }

    /// backward-compat: config.json written before the `s3` BYO sink field existed
    /// (no "s3" key at all) must still load, with `s3: None`.
    #[test]
    fn old_config_json_without_s3_key_loads_with_s3_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, br#"{"otlp_port": 4318}"#).unwrap();
        let loaded = KikimimiConfig::load_from(&path).unwrap();
        assert_eq!(loaded.s3, None);
    }

    #[test]
    fn s3_config_roundtrips_through_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let cfg = KikimimiConfig {
            s3: Some(S3SinkConfig {
                url: "s3://my-bucket/team".into(),
                profile: Some("myprofile".into()),
                endpoint_url: Some("http://127.0.0.1:9000".into()),
            }),
            ..Default::default()
        };
        cfg.save_to(&path).unwrap();
        assert_eq!(KikimimiConfig::load_from(&path).unwrap(), cfg);
    }

    /// `S3SinkConfig` never carries credentials -- only the destination and the
    /// (non-secret) `--profile`/`--endpoint-url` overrides.
    #[test]
    fn s3_config_has_no_credential_fields() {
        let cfg = S3SinkConfig {
            url: "s3://bucket/prefix".into(),
            profile: None,
            endpoint_url: None,
        };
        let json = serde_json::to_value(&cfg).unwrap();
        let obj = json.as_object().unwrap();
        let keys: std::collections::BTreeSet<String> = obj.keys().cloned().collect();
        let expected: std::collections::BTreeSet<String> = ["url", "profile", "endpoint_url"]
            .into_iter()
            .map(String::from)
            .collect();
        assert_eq!(keys, expected);
    }
}
