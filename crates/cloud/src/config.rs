//! Server configuration from environment variables (architecture.md §12 Stage 0).
//!
//! All values have safe dev-time defaults so `cargo test` / a local `cargo run`
//! works against the dev Postgres without any env vars set, except `DATABASE_URL`
//! which still defaults to the documented dev connection string.
//!
//! Every `KIKIMIMI_*` var below also accepts the legacy `GURU_*` name as a
//! fallback (guru → kikimimi rename), printing a one-time deprecation warning
//! to stderr when the old name is what's actually set — see [`env_with_legacy`].

#[derive(Debug, Clone)]
pub struct Config {
    /// Address the HTTP server binds to.
    pub bind_addr: String,
    /// Base URL used to build `verification_url` in device-code responses.
    /// Defaults to `http://<bind_addr>`.
    pub public_base_url: String,
    /// Superuser Postgres DSN (used for migrations + the accounts/orgs/devices
    /// tables — see db.rs module docs for the two-pool design).
    pub database_url: String,
    /// Password for the non-superuser `kikimimi_app` role that RLS-scoped request
    /// transactions connect as.
    pub app_db_password: String,
    /// When `1`, `POST /v1/device/code` auto-approves immediately with
    /// `dev_email` — for tests/CI (contract §, architecture.md §12 Stage 0).
    pub dev_autoapprove: bool,
    /// Email used to auto-approve when `dev_autoapprove` is set.
    pub dev_email: String,
    /// Invite code gating the legacy `POST /web/login` for public deployment
    /// (architecture.md §6.1: "全体招待コード" -- superseded by GitHub OAuth +
    /// per-org invite links, but kept as the self-host bootstrap path). `None`
    /// when `KIKIMIMI_INVITE_CODE` is unset or empty.
    pub invite_code: Option<String>,
    /// GitHub OAuth app credentials (architecture.md §6.1 "主認証は GitHub
    /// OAuth"). Both must be set (non-empty) for `GET /auth/github` to do
    /// anything other than 503 -- see `github.rs`. `None` when the
    /// corresponding env var is unset or empty.
    pub github_client_id: Option<String>,
    pub github_client_secret: Option<String>,
    /// Base URL for GitHub's OAuth endpoints (`/login/oauth/authorize`,
    /// `/login/oauth/access_token`). Overridable via `GITHUB_OAUTH_BASE` so
    /// tests point this at a local mock instead of real `github.com` --
    /// architecture.md task contract: "no real GitHub calls in tests".
    pub github_oauth_base: String,
    /// Base URL for the GitHub REST API (`/user`, `/user/emails`).
    /// Overridable via `GITHUB_API_BASE`, same reasoning as
    /// `github_oauth_base`.
    pub github_api_base: String,
    /// `KIKIMIMI_LEGACY_INVITE=1` keeps the legacy email+invite `POST
    /// /web/login` reachable even once `github_client_id` is configured
    /// (normally that combination 404s it -- account-model contract: "the
    /// legacy email+invite login ... is disabled (404) when
    /// GITHUB_CLIENT_ID is set unless KIKIMIMI_LEGACY_INVITE=1").
    pub legacy_invite: bool,
}

/// Reads `new_key`; if unset, falls back to the pre-rename `old_key` (printing
/// a deprecation warning to stderr) so an operator's existing `GURU_*`
/// deployment env keeps working across the guru → kikimimi rename.
fn env_with_legacy(new_key: &str, old_key: &str) -> Option<String> {
    if let Ok(v) = std::env::var(new_key) {
        return Some(v);
    }
    let v = std::env::var(old_key).ok()?;
    eprintln!("warning: {old_key} is deprecated, use {new_key} instead (guru → kikimimi rename)");
    Some(v)
}

impl Config {
    pub fn from_env() -> Self {
        let bind_addr =
            std::env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:8787".to_string());
        let public_base_url =
            env_with_legacy("KIKIMIMI_PUBLIC_BASE_URL", "GURU_PUBLIC_BASE_URL")
                .unwrap_or_else(|| format!("http://{bind_addr}"));
        let dev_autoapprove = env_with_legacy("KIKIMIMI_DEV_AUTOAPPROVE", "GURU_DEV_AUTOAPPROVE")
            .map(|v| v == "1")
            .unwrap_or(false);
        if dev_autoapprove {
            // Loud on purpose: this bypasses both the invite-code gate and
            // per-mailbox proof entirely, so an operator who accidentally
            // ships it to a public deployment needs to see it in the logs.
            tracing::warn!(
                "!!! KIKIMIMI_DEV_AUTOAPPROVE=1 — every `kikimimi login` auto-approves with no \
                 invite code or email confirmation. For tests/CI only — NEVER set this on a \
                 public deployment. !!!"
            );
        }
        let invite_code = env_with_legacy("KIKIMIMI_INVITE_CODE", "GURU_INVITE_CODE")
            .filter(|v| !v.is_empty());
        Self {
            bind_addr,
            public_base_url,
            database_url: std::env::var("DATABASE_URL").unwrap_or_else(|_| {
                "postgres://postgres:guru-dev@127.0.0.1:5433/guru".to_string()
            }),
            app_db_password: env_with_legacy("KIKIMIMI_APP_DB_PASSWORD", "GURU_APP_DB_PASSWORD")
                .unwrap_or_else(|| "kikimimi-app-dev".to_string()),
            dev_autoapprove,
            dev_email: env_with_legacy("KIKIMIMI_DEV_EMAIL", "GURU_DEV_EMAIL")
                .unwrap_or_else(|| "dev@local".to_string()),
            invite_code,
            github_client_id: std::env::var("GITHUB_CLIENT_ID").ok().filter(|v| !v.is_empty()),
            github_client_secret: std::env::var("GITHUB_CLIENT_SECRET").ok().filter(|v| !v.is_empty()),
            github_oauth_base: std::env::var("GITHUB_OAUTH_BASE")
                .ok()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| "https://github.com".to_string()),
            github_api_base: std::env::var("GITHUB_API_BASE")
                .ok()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| "https://api.github.com".to_string()),
            legacy_invite: std::env::var("KIKIMIMI_LEGACY_INVITE").map(|v| v == "1").unwrap_or(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    struct EnvGuard(Vec<&'static str>);
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for k in &self.0 {
                std::env::remove_var(k);
            }
        }
    }

    #[test]
    #[serial]
    fn new_env_var_wins_over_legacy() {
        std::env::set_var("KIKIMIMI_DEV_EMAIL", "new@local");
        std::env::set_var("GURU_DEV_EMAIL", "old@local");
        let _g = EnvGuard(vec!["KIKIMIMI_DEV_EMAIL", "GURU_DEV_EMAIL"]);
        assert_eq!(
            env_with_legacy("KIKIMIMI_DEV_EMAIL", "GURU_DEV_EMAIL").as_deref(),
            Some("new@local")
        );
    }

    #[test]
    #[serial]
    fn legacy_env_var_used_as_fallback() {
        std::env::remove_var("KIKIMIMI_DEV_EMAIL");
        std::env::set_var("GURU_DEV_EMAIL", "old@local");
        let _g = EnvGuard(vec!["KIKIMIMI_DEV_EMAIL", "GURU_DEV_EMAIL"]);
        assert_eq!(
            env_with_legacy("KIKIMIMI_DEV_EMAIL", "GURU_DEV_EMAIL").as_deref(),
            Some("old@local")
        );
    }

    #[test]
    #[serial]
    fn neither_set_is_none() {
        std::env::remove_var("KIKIMIMI_DEV_EMAIL");
        std::env::remove_var("GURU_DEV_EMAIL");
        let _g = EnvGuard(vec!["KIKIMIMI_DEV_EMAIL", "GURU_DEV_EMAIL"]);
        assert_eq!(env_with_legacy("KIKIMIMI_DEV_EMAIL", "GURU_DEV_EMAIL"), None);
    }
}
