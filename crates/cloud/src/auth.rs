//! Bearer token auth (architecture.md §6 "認証と権限", API contract):
//! opaque 43-char base64url token, server stores only `sha256(token)`, each
//! token bound to `(org_id, account_id, host_id)`.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use rand::RngExt as _;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;

/// 32 random bytes, base64url (no padding) encoded, is exactly 43 chars —
/// `ceil(32 * 8 / 6) == 43`.
const TOKEN_BYTES: usize = 32;

/// Generates a fresh bearer token. Returns `(plaintext, sha256_hash)` — the
/// plaintext is handed to the client exactly once (device/token materialize,
/// or the activate response) and never stored; only the hash is persisted.
pub fn generate_token() -> (String, Vec<u8>) {
    let mut bytes = [0u8; TOKEN_BYTES];
    rand::rng().fill(&mut bytes);
    let token = URL_SAFE_NO_PAD.encode(bytes);
    let hash = Sha256::digest(token.as_bytes()).to_vec();
    (token, hash)
}

pub fn hash_token(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// Identity attached to a request by the auth extractor once the bearer
/// token has been resolved to a non-revoked device row.
#[derive(Debug, Clone)]
pub struct AuthContext {
    pub org_id: Uuid,
    pub account_id: Uuid,
    pub host_id: String,
    /// The `devices.id` row the bearer token resolved to — lets a handler
    /// (e.g. `POST /v1/device/revoke`) act on exactly the token that
    /// authenticated the request, without re-deriving it from `host_id`
    /// (which is not unique: repeated `kikimimi login` on the same host creates
    /// a new `devices` row each time, so `host_id` alone could match more
    /// than one device/token).
    pub device_id: Uuid,
}

impl FromRequestParts<AppState> for AuthContext {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or(AppError::Unauthorized("missing Authorization header"))?;
        let token = header
            .strip_prefix("Bearer ")
            .ok_or(AppError::Unauthorized("Authorization must be a Bearer token"))?;
        if token.is_empty() {
            return Err(AppError::Unauthorized("empty bearer token"));
        }

        let hash = hash_token(token);

        let row: Option<(Uuid, Uuid, Uuid, String, bool)> = sqlx::query_as(
            "SELECT id, org_id, account_id, host_id, revoked FROM devices WHERE token_hash = $1",
        )
        .bind(&hash)
        .fetch_optional(&state.pools.superuser)
        .await
        .map_err(anyhow::Error::from)?;

        let (device_id, org_id, account_id, host_id, revoked) =
            row.ok_or(AppError::Unauthorized("invalid or unknown token"))?;
        if revoked {
            return Err(AppError::Unauthorized("token revoked"));
        }

        // Throttled last_seen_at update: at most once/minute per device, so a
        // chatty client doesn't turn every request into two writes.
        sqlx::query(
            "UPDATE devices SET last_seen_at = now() WHERE id = $1 \
             AND (last_seen_at IS NULL OR last_seen_at < now() - interval '60 seconds')",
        )
        .bind(device_id)
        .execute(&state.pools.superuser)
        .await
        .map_err(anyhow::Error::from)?;

        Ok(AuthContext {
            org_id,
            account_id,
            host_id,
            device_id,
        })
    }
}
