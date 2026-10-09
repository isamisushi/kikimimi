//! Admin-owned roster/contracts, independent from telemetry. No billing or
//! provisioning side effects. Missing events never prove that someone is idle.
use crate::{
    error::AppError, roles::require_role_at_least, state::AppState, web::WebSessionContext,
};
use axum::{
    extract::{Path, State},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use uuid::Uuid;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Assignment {
    pub tool: String,
    pub plan: String,
    pub allocated_model: String,
    pub monthly_amount: Option<f64>,
    pub currency: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Person {
    pub email: String,
    pub name: String,
    pub department: String,
    pub account_id: Option<Uuid>,
    pub assignments: Vec<Assignment>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveRequest {
    pub people: Vec<Person>,
}

fn validate(body: &mut SaveRequest) -> Result<(), AppError> {
    let bad = |s: &str| AppError::BadRequest(s.into());
    if body.people.is_empty() || body.people.len() > 1000 {
        return Err(bad("Provide 1–1000 people per import"));
    }
    let mut emails = HashSet::new();
    for p in &mut body.people {
        p.email = p.email.trim().to_lowercase();
        p.name = p.name.trim().into();
        p.department = p.department.trim().into();
        if p.email.len() > 254
            || !p
                .email
                .split_once('@')
                .is_some_and(|(a, b)| !a.is_empty() && !b.is_empty())
            || p.email.chars().any(char::is_whitespace)
        {
            return Err(bad("Each person needs a valid email"));
        }
        if !emails.insert(p.email.clone()) {
            return Err(bad("Duplicate person email in import"));
        }
        if p.name.is_empty()
            || p.name.len() > 200
            || p.department.len() > 200
            || p.assignments.len() > 20
        {
            return Err(bad("Name/department too long or too many assignments"));
        }
        let mut tools = HashSet::new();
        for a in &mut p.assignments {
            a.tool = a.tool.trim().to_lowercase();
            a.currency = a.currency.trim().to_uppercase();
            if ![
                "claude-code",
                "codex",
                "cursor",
                "opencode",
                "copilot",
                "other",
            ]
            .contains(&a.tool.as_str())
                || !tools.insert(a.tool.clone())
            {
                return Err(bad("Use one assignment per tool per person"));
            }
            if a.plan.len() > 200 || a.allocated_model.len() > 200 {
                return Err(bad("Plan/model too long"));
            }
            if !["USD", "JPY", "EUR", "GBP"].contains(&a.currency.as_str()) {
                return Err(bad("Currency must be USD, JPY, EUR or GBP"));
            }
            if a.monthly_amount
                .is_some_and(|n| !n.is_finite() || !(0.0..=1_000_000_000.0).contains(&n))
            {
                return Err(bad("Monthly amount must be non-negative and finite"));
            }
        }
    }
    Ok(())
}

async fn authorize(state: &AppState, s: &WebSessionContext, action: &str) -> Result<(), AppError> {
    require_role_at_least(&state.pools.superuser, s.account_id, s.org_id, "admin").await?;
    sqlx::query("INSERT INTO audit_log (actor, org_id, action) VALUES ($1,$2,$3)")
        .bind(s.account_id)
        .bind(s.org_id)
        .bind(action)
        .execute(&state.pools.superuser)
        .await?;
    Ok(())
}

pub async fn save(
    State(state): State<AppState>,
    s: WebSessionContext,
    Json(mut body): Json<SaveRequest>,
) -> Result<Json<Value>, AppError> {
    authorize(&state, &s, "inventory_save_attempt").await?;
    validate(&mut body)?;
    // Explicit links may only name existing members of this organization.
    for p in &body.people {
        if let Some(id) = p.account_id {
            let (exists,): (bool,) = sqlx::query_as(
                "SELECT EXISTS(SELECT 1 FROM memberships WHERE org_id=$1 AND account_id=$2)",
            )
            .bind(s.org_id)
            .bind(id)
            .fetch_one(&state.pools.superuser)
            .await?;
            if !exists {
                return Err(AppError::BadRequest(
                    "Linked account must be a member of this workspace".into(),
                ));
            }
        }
    }
    let mut tx = state.pools.org_scoped_tx(s.org_id).await?;
    for p in &body.people {
        let assignments = serde_json::to_string(&p.assignments).map_err(anyhow::Error::from)?;
        sqlx::query("INSERT INTO ai_inventory (org_id,email,name,department,account_id,assignments) VALUES ($1,$2,$3,$4,$5,$6::jsonb) ON CONFLICT (org_id,email) DO UPDATE SET name=EXCLUDED.name, department=EXCLUDED.department, account_id=EXCLUDED.account_id, assignments=EXCLUDED.assignments, updated_at=now()")
            .bind(s.org_id).bind(&p.email).bind(&p.name).bind(&p.department).bind(p.account_id).bind(assignments)
            .execute(&mut *tx).await?;
    }
    // A person's activity must not appear under two roster entries.
    let members: Vec<(Uuid, String)> = sqlx::query_as("SELECT a.id,a.email FROM memberships m JOIN accounts a ON a.id=m.account_id WHERE m.org_id=$1")
        .bind(s.org_id).fetch_all(&state.pools.superuser).await?;
    let links: Vec<(String, Option<Uuid>)> =
        sqlx::query_as("SELECT email,account_id FROM ai_inventory")
            .fetch_all(&mut *tx)
            .await?;
    let mut linked = HashSet::new();
    for (email, explicit) in links {
        let resolved = members.iter().find(|(id, member_email)| {
            explicit.map_or_else(
                || member_email.eq_ignore_ascii_case(&email),
                |explicit| explicit == *id,
            )
        });
        if let Some((id, _)) = resolved {
            if !linked.insert(*id) {
                return Err(AppError::BadRequest(
                    "A usage account can only be linked to one roster person".into(),
                ));
            }
        }
    }
    tx.commit().await?;
    Ok(Json(json!({"saved": body.people.len()})))
}

pub async fn remove(
    State(state): State<AppState>,
    s: WebSessionContext,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    authorize(&state, &s, "inventory_delete_attempt").await?;
    let mut tx = state.pools.org_scoped_tx(s.org_id).await?;
    let result = sqlx::query("DELETE FROM ai_inventory WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    if result.rows_affected() == 0 {
        return Err(AppError::NotFound("Person not found".into()));
    }
    tx.commit().await?;
    Ok(Json(json!({"ok": true})))
}

pub async fn list(
    State(state): State<AppState>,
    s: WebSessionContext,
) -> Result<Json<Value>, AppError> {
    authorize(&state, &s, "inventory_view").await?;
    let members: Vec<(Uuid, String)> = sqlx::query_as("SELECT a.id,a.email FROM memberships m JOIN accounts a ON a.id=m.account_id WHERE m.org_id=$1")
        .bind(s.org_id).fetch_all(&state.pools.superuser).await?;
    let mut tx = state.pools.org_scoped_tx(s.org_id).await?;
    let raw: Vec<(String,)> = sqlx::query_as(
        "SELECT row_to_json(i)::text FROM ai_inventory i ORDER BY department,name,email",
    )
    .fetch_all(&mut *tx)
    .await?;
    let now = chrono::Utc::now();
    let from = (now.date_naive() - chrono::Duration::days(29))
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp_millis();
    let usage: Vec<(String,)> = sqlx::query_as(USAGE_SQL)
        .bind(from)
        .bind(now.timestamp_millis())
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;
    let usage: Vec<Value> = usage
        .into_iter()
        .map(|(v,)| serde_json::from_str(&v))
        .collect::<Result<_, _>>()
        .map_err(anyhow::Error::from)?;
    let mut people = Vec::new();
    for (raw,) in raw {
        let mut p: Value = serde_json::from_str(&raw).map_err(anyhow::Error::from)?;
        let explicit = p["account_id"].as_str();
        let matched = members.iter().find(|(id, email)| match explicit {
            Some(link) => id.to_string() == link,
            None => email.eq_ignore_ascii_case(p["email"].as_str().unwrap_or("")),
        });
        let account = matched.map(|(id, _)| id.to_string());
        p["matched_account_id"] = json!(account);
        p["usage"] = json!(usage
            .iter()
            .filter(|u| account
                .as_deref()
                .is_some_and(|id| u["user_id"].as_str() == Some(id)))
            .collect::<Vec<_>>());
        people.push(p);
    }
    // Membership changes can introduce a new ambiguous email match after an
    // import. Do not count either record until the admin fixes the mapping.
    let mut counts = std::collections::HashMap::<String, usize>::new();
    for p in &people {
        if let Some(id) = p["matched_account_id"].as_str() {
            *counts.entry(id.into()).or_default() += 1;
        }
    }
    for p in &mut people {
        let conflict = p["matched_account_id"]
            .as_str()
            .is_some_and(|id| counts.get(id).copied().unwrap_or(0) > 1);
        p["identity_conflict"] = json!(conflict);
        if conflict {
            p["matched_account_id"] = Value::Null;
            p["usage"] = json!([]);
        }
    }
    Ok(Json(
        json!({"people": people, "from": from, "to": now.timestamp_millis(), "members": members.into_iter().map(|(id,email)| json!({"id":id,"email":email})).collect::<Vec<_>>()}),
    ))
}

// Preserve missing cost as null. Prefer OTel over transcript per person/host/
// agent/session so two captures of one request do not inflate spend. Activity
// includes all sources; costs only include the chosen api.request source.
const USAGE_SQL: &str = r#"
WITH e AS (SELECT * FROM events WHERE ts >= $1 AND ts <= $2 AND user_id IS NOT NULL),
pref AS (
 SELECT user_id,host_id,agent,session_id,min(CASE source WHEN 'otel' THEN 0 WHEN 'log' THEN 1 ELSE 2 END) rank
 FROM e WHERE event_type='api.request' GROUP BY 1,2,3,4
), cost AS (
 SELECT e.user_id,e.agent,sum(e.cost_usd) estimated_cost_usd,
 count(*) FILTER (WHERE e.cost_usd IS NOT NULL) priced_requests,count(*) total_requests
 FROM e JOIN pref p ON p.user_id=e.user_id AND p.host_id=e.host_id AND p.agent=e.agent AND p.session_id IS NOT DISTINCT FROM e.session_id
 WHERE e.event_type='api.request' AND (CASE e.source WHEN 'otel' THEN 0 WHEN 'log' THEN 1 ELSE 2 END)=p.rank
 GROUP BY 1,2
), activity AS (
 SELECT user_id,agent,count(DISTINCT (host_id,session_id)) FILTER (WHERE session_id IS NOT NULL) sessions,
 count(DISTINCT dt) active_days,max(ts) last_seen_at,array_agg(DISTINCT model) FILTER (WHERE model IS NOT NULL) models
 FROM e GROUP BY 1,2
)
SELECT row_to_json(r)::text FROM (
 SELECT a.*,c.estimated_cost_usd,coalesce(c.priced_requests,0) priced_requests,coalesce(c.total_requests,0) total_requests
 FROM activity a LEFT JOIN cost c USING(user_id,agent)
) r
"#;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn import_rejects_duplicates_and_bad_money() {
        let person = json!({"email":"A@EXAMPLE.COM", "name":"A", "department":"Eng", "account_id":null, "assignments":[]});
        let mut body: SaveRequest =
            serde_json::from_value(json!({"people":[person.clone(),person.clone()]})).unwrap();
        assert!(validate(&mut body).is_err());
        let mut body: SaveRequest = serde_json::from_value(json!({"people":[person]})).unwrap();
        body.people[0].assignments.push(Assignment {
            tool: "codex".into(),
            plan: "Pro".into(),
            allocated_model: "".into(),
            monthly_amount: Some(-1.0),
            currency: "USD".into(),
        });
        assert!(validate(&mut body).is_err());
        body.people[0].assignments[0].monthly_amount = None;
        assert!(validate(&mut body).is_ok());
        assert_eq!(body.people[0].email, "a@example.com");
    }
}
