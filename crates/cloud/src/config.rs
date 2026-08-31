//! Server configuration from environment variables (architecture.md §12 Stage 0).
//!
//! All values have safe dev-time defaults so `cargo test` / a local `cargo run`
//! works against the dev Postgres without any env vars set, except `DATABASE_URL`
//! which still defaults to the documented dev connection string.

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
    /// Password for the non-superuser `guru_app` role that RLS-scoped request
    /// transactions connect as.
    pub app_db_password: String,
    /// When `1`, `POST /v1/device/code` auto-approves immediately with
    /// `dev_email` — for tests/CI (contract §, architecture.md §12 Stage 0).
    pub dev_autoapprove: bool,
    /// Email used to auto-approve when `dev_autoapprove` is set.
    pub dev_email: String,
    /// Invite code gating `POST /activate` for public deployment. `None`
    /// when `GURU_INVITE_CODE` is unset or empty. See device.rs module docs
    /// for the fail-closed rule this enables (no invite code and no
    /// `dev_autoapprove` ⇒ activation refuses with 503, never silently open
    /// registration).
    pub invite_code: Option<String>,
}

impl Config {
    pub fn from_env() -> Self {
        let bind_addr =
            std::env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:8787".to_string());
        let public_base_url = std::env::var("GURU_PUBLIC_BASE_URL")
            .unwrap_or_else(|_| format!("http://{bind_addr}"));
        let dev_autoapprove = std::env::var("GURU_DEV_AUTOAPPROVE")
            .map(|v| v == "1")
            .unwrap_or(false);
        if dev_autoapprove {
            // Loud on purpose: this bypasses both the invite-code gate and
            // per-mailbox proof entirely, so an operator who accidentally
            // ships it to a public deployment needs to see it in the logs.
            tracing::warn!(
                "!!! GURU_DEV_AUTOAPPROVE=1 — every `guru login` auto-approves with no invite \
                 code or email confirmation. For tests/CI only — NEVER set this on a public \
                 deployment. !!!"
            );
        }
        let invite_code = std::env::var("GURU_INVITE_CODE")
            .ok()
            .filter(|v| !v.is_empty());
        Self {
            bind_addr,
            public_base_url,
            database_url: std::env::var("DATABASE_URL").unwrap_or_else(|_| {
                "postgres://postgres:guru-dev@127.0.0.1:5433/guru".to_string()
            }),
            app_db_password: std::env::var("GURU_APP_DB_PASSWORD")
                .unwrap_or_else(|_| "guru-app-dev".to_string()),
            dev_autoapprove,
            dev_email: std::env::var("GURU_DEV_EMAIL").unwrap_or_else(|_| "dev@local".to_string()),
            invite_code,
        }
    }
}
