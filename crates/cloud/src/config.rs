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
    /// Invite code gating `POST /activate` for public deployment. `None`
    /// when `KIKIMIMI_INVITE_CODE` is unset or empty. See device.rs module
    /// docs for the fail-closed rule this enables (no invite code and no
    /// `dev_autoapprove` ⇒ activation refuses with 503, never silently open
    /// registration).
    pub invite_code: Option<String>,
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
