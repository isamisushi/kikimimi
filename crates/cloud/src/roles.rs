//! Role-based access checks for the org/team management endpoints
//! (`orgs.rs`, `web_query.rs`'s `/web/q/sessions`) — architecture.md §6.1
//! "ロールと目的限定": owner/admin > member > viewer, ranked in that order.
//!
//! There is no role field cached on `WebSessionContext`: a membership's role
//! can change between two requests from the same session (e.g. an admin
//! demoted to member), so every admin-gated handler looks it up fresh via
//! [`require_role_at_least`] rather than trusting anything set at login time.

use sqlx::PgPool;
use uuid::Uuid;

use crate::error::AppError;

/// Higher number = more privileged. Anything not in the known set (there
/// shouldn't be any, `memberships.role` has a CHECK constraint) ranks below
/// `viewer` so an unrecognized value fails closed rather than open.
pub fn role_rank(role: &str) -> u8 {
    match role {
        "owner" => 4,
        "admin" => 3,
        "member" => 2,
        "viewer" => 1,
        _ => 0,
    }
}

pub fn role_at_least(role: &str, min: &str) -> bool {
    role_rank(role) >= role_rank(min)
}

/// `account_id`'s role on `org_id`, or `None` if they aren't a member at all
/// — distinct from an `AppError` so callers can decide for themselves
/// whether "not a member" is a 403 or a 404.
pub async fn membership_role(
    pool: &PgPool,
    account_id: Uuid,
    org_id: Uuid,
) -> Result<Option<String>, AppError> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT role FROM memberships WHERE account_id = $1 AND org_id = $2")
            .bind(account_id)
            .bind(org_id)
            .fetch_optional(pool)
            .await
            .map_err(anyhow::Error::from)?;
    Ok(row.map(|(r,)| r))
}

/// Looks up `account_id`'s role on `org_id` and requires it to be at least
/// `min`. 404 (not 403) when they aren't a member at all — doesn't confirm
/// to a non-member whether the org itself exists. 403 when they are a member
/// but with too low a role.
pub async fn require_role_at_least(
    pool: &PgPool,
    account_id: Uuid,
    org_id: Uuid,
    min: &str,
) -> Result<String, AppError> {
    let role = membership_role(pool, account_id, org_id)
        .await?
        .ok_or_else(|| AppError::NotFound("org not found".into()))?;
    if !role_at_least(&role, min) {
        return Err(AppError::Forbidden(format!(
            "requires role {min} or higher, caller has {role}"
        )));
    }
    Ok(role)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_rank_orders_owner_above_admin_above_member_above_viewer() {
        assert!(role_rank("owner") > role_rank("admin"));
        assert!(role_rank("admin") > role_rank("member"));
        assert!(role_rank("member") > role_rank("viewer"));
    }

    #[test]
    fn unrecognized_role_ranks_below_viewer() {
        assert!(role_rank("bogus") < role_rank("viewer"));
    }

    #[test]
    fn role_at_least_is_reflexive_and_monotonic() {
        assert!(role_at_least("admin", "admin"));
        assert!(role_at_least("owner", "member"));
        assert!(!role_at_least("member", "admin"));
        assert!(!role_at_least("viewer", "member"));
    }
}
