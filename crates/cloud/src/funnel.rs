//! Onboarding funnel (KKM-21, architecture.md §15-15): where do new machines
//! drop off between `kikimimi login` and the first insight, and how many are
//! still sending data 30 days later (§7.3 KPI)?
//!
//! What is recorded (`funnel_steps`, migration 0014): the *first* time a
//! subject reached a step, as a timestamp against an opaque id the cloud
//! already holds -- `devices.host_id` for the device steps, `accounts.id`
//! for the web step. Nothing else: no email, no hostname, no event content.
//! It is derived from requests the server receives anyway, so there is no
//! extra client telemetry, and a local-only install (no `kikimimi login`)
//! contributes nothing. `KIKIMIMI_FUNNEL=0` turns recording off for the
//! whole deployment (documented in the privacy page).
//!
//! What is exposed: `GET /web/q/funnel?days=N&scope=org|all` -- aggregate
//! counts only, at two scopes that follow the role model (roles.rs):
//! - `scope=org` (default): the hosts bound to the session's active org.
//!   Needs admin or owner there -- it is the team lead's "who on my team
//!   never finished `init`" view (a personal org's owner sees their own
//!   machines). Hosts that ran `login` but never got a token have no org
//!   yet, so they only appear at deployment scope.
//! - `scope=all`: every org on the deployment. Needs `accounts.operator`,
//!   the deployment-operator flag one level above an org owner; for anyone
//!   else the scope does not exist (404), so it doesn't advertise itself.
//!   The flag is set in SQL by the operator (see the teams docs) -- there is
//!   deliberately no HTTP path that grants it.
//!
//! Steps, in order:
//! - `login_started` (host): `POST /v1/device/code` -- the CLI ran `login`.
//! - `login_done` (host): the device token was minted.
//! - `first_events` (host): the first `POST /v1/events` batch that inserted
//!   at least one row -- `kikimimi init` succeeded and the daemon is
//!   flushing.
//! - `first_insight` (account): the first `GET /web/q/overview` (the SPA's
//!   landing page) or `GET /v1/query/<name>` (`kikimimi query --cloud`).
//!
//! "install" itself is not a step: nothing reaches the cloud before `login`.
//! The nearest proxy is the GitHub release download count, which the
//! operator reads separately (`gh api repos/.../releases`).

use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::AppError;
use crate::roles::require_role_at_least;
use crate::state::AppState;
use crate::web::WebSessionContext;
use crate::web_query::validate_range;

pub const KIND_HOST: &str = "host";
pub const KIND_ACCOUNT: &str = "account";

pub const STEP_LOGIN_STARTED: &str = "login_started";
pub const STEP_LOGIN_DONE: &str = "login_done";
pub const STEP_FIRST_EVENTS: &str = "first_events";
pub const STEP_FIRST_INSIGHT: &str = "first_insight";

/// Records that `subject` reached `step`, keeping the earliest timestamp.
/// Never fails the caller: a funnel write is bookkeeping, and the login /
/// ingest / query it piggybacks on must not turn into a 500 because of it,
/// so errors are logged at warn and swallowed. No-op when recording is
/// disabled (`KIKIMIMI_FUNNEL=0`).
pub async fn record<'e, E>(exec: E, enabled: bool, subject_kind: &str, subject_id: &str, step: &str)
where
    E: sqlx::PgExecutor<'e>,
{
    if !enabled {
        return;
    }
    let result = sqlx::query(
        "INSERT INTO funnel_steps (subject_kind, subject_id, step) VALUES ($1, $2, $3) \
         ON CONFLICT DO NOTHING",
    )
    .bind(subject_kind)
    .bind(subject_id)
    .bind(step)
    .execute(exec)
    .await;
    if let Err(e) = result {
        tracing::warn!(error = %e, subject_kind, step, "funnel step not recorded");
    }
}

/// The aggregate. `$1` = cohort window in days: a host is in the cohort
/// when its earliest recorded step is within the window. `$2` = org id, or
/// NULL for the whole deployment; at org scope a host counts when it has a
/// device in that org, and an account's `first_insight` counts for the
/// org's hosts that belong to it. Retention ignores the window on purpose
/// -- it needs hosts old enough to *have* a 30th day.
pub const FUNNEL_SQL: &str = "\
WITH org_hosts AS (
    SELECT DISTINCT host_id FROM devices WHERE $2::uuid IS NULL OR org_id = $2::uuid
),
hosts AS (
    SELECT subject_id AS host_id,
           min(at) FILTER (WHERE step = 'login_started') AS login_started,
           min(at) FILTER (WHERE step = 'login_done')    AS login_done,
           min(at) FILTER (WHERE step = 'first_events')  AS first_events
    FROM funnel_steps
    WHERE subject_kind = 'host'
      AND ($2::uuid IS NULL OR subject_id IN (SELECT host_id FROM org_hosts))
    GROUP BY subject_id
),
cohort AS (
    SELECT *
    FROM hosts
    WHERE LEAST(login_started, login_done, first_events) >= now() - make_interval(days => $1::int)
),
insight AS (
    SELECT DISTINCT d.host_id
    FROM devices d
    JOIN funnel_steps f
      ON f.subject_kind = 'account' AND f.step = 'first_insight'
     AND f.subject_id = d.account_id::text
    WHERE $2::uuid IS NULL OR d.org_id = $2::uuid
),
retention AS (
    SELECT host_id, min(created_at) AS first_seen, max(last_seen_at) AS last_seen
    FROM devices
    WHERE $2::uuid IS NULL OR org_id = $2::uuid
    GROUP BY host_id
)
SELECT
    (SELECT count(*) FROM cohort WHERE login_started IS NOT NULL)::bigint AS hosts_login_started,
    (SELECT count(*) FROM cohort WHERE login_done IS NOT NULL)::bigint    AS hosts_login_done,
    (SELECT count(*) FROM cohort WHERE first_events IS NOT NULL)::bigint  AS hosts_first_events,
    (SELECT count(*) FROM cohort c JOIN insight i USING (host_id))::bigint AS hosts_first_insight,
    (SELECT percentile_cont(0.5) WITHIN GROUP (
         ORDER BY extract(epoch FROM (first_events - login_started)) / 60.0)
     FROM cohort
     WHERE login_started IS NOT NULL AND first_events IS NOT NULL)::float8 AS median_minutes_login_to_first_events,
    (SELECT count(*) FROM retention
     WHERE first_seen <= now() - interval '30 days')::bigint AS hosts_eligible_30d,
    (SELECT count(*) FROM retention
     WHERE first_seen <= now() - interval '30 days'
       AND last_seen >= first_seen + interval '30 days')::bigint AS hosts_retained_30d";

#[derive(Debug, Deserialize)]
pub struct FunnelQuery {
    days: Option<u32>,
    scope: Option<String>,
}

pub async fn is_operator(state: &AppState, account_id: Uuid) -> Result<bool, AppError> {
    let (operator,): (bool,) = sqlx::query_as("SELECT operator FROM accounts WHERE id = $1")
        .bind(account_id)
        .fetch_one(&state.pools.superuser)
        .await
        .map_err(anyhow::Error::from)?;
    Ok(operator)
}

/// `GET /web/q/funnel?days=N&scope=org|all` -- see module docs.
pub async fn funnel(
    State(state): State<AppState>,
    session: WebSessionContext,
    Query(q): Query<FunnelQuery>,
) -> Result<Json<Value>, AppError> {
    let days = validate_range(q.days, 30, 1, 3650, "days")?;
    let scope = q.scope.as_deref().unwrap_or("org");
    let org_filter: Option<Uuid> = match scope {
        "org" => {
            require_role_at_least(
                &state.pools.superuser,
                session.account_id,
                session.org_id,
                "admin",
            )
            .await?;
            Some(session.org_id)
        }
        "all" => {
            if !is_operator(&state, session.account_id).await? {
                return Err(AppError::NotFound("not found".into()));
            }
            None
        }
        other => {
            return Err(AppError::BadRequest(format!(
                "scope must be \"org\" or \"all\", got {other:?}"
            )))
        }
    };

    let row: (i64, i64, i64, i64, Option<f64>, i64, i64) = sqlx::query_as(FUNNEL_SQL)
        .bind(days as i32)
        .bind(org_filter)
        .fetch_one(&state.pools.superuser)
        .await
        .map_err(anyhow::Error::from)?;
    let (started, done, events, insight, median_minutes, eligible, retained) = row;

    Ok(Json(json!({
        "days": days,
        "scope": scope,
        "tracking": state.config.funnel_tracking,
        "steps": [
            { "step": STEP_LOGIN_STARTED, "hosts": started },
            { "step": STEP_LOGIN_DONE, "hosts": done },
            { "step": STEP_FIRST_EVENTS, "hosts": events },
            { "step": STEP_FIRST_INSIGHT, "hosts": insight },
        ],
        "median_minutes_login_to_first_events": median_minutes,
        "retention_30d": {
            "hosts_eligible": eligible,
            "hosts_retained": retained,
        },
    })))
}
