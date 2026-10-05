mod support;
use serde_json::{json, Value};
use support::{web_login, SpawnOpts, TestApp};

#[tokio::test]
async fn inventory_roster_usage_permissions_and_tenant_isolation() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let c = reqwest::Client::new();
    let a = web_login(&c, &app.base_url, "owner@example.com").await;
    let b = web_login(&c, &app.base_url, "other@example.com").await;
    let url = format!("{}/web/inventory", app.base_url);
    assert_eq!(c.get(&url).send().await.unwrap().status(), 401);
    let org = uuid::Uuid::parse_str(&a.org_id).unwrap();
    let (account,): (uuid::Uuid,) =
        sqlx::query_as("SELECT id FROM accounts WHERE email='owner@example.com'")
            .fetch_one(&app.state.pools.superuser)
            .await
            .unwrap();
    let (foreign,): (uuid::Uuid,) =
        sqlx::query_as("SELECT id FROM accounts WHERE email='other@example.com'")
            .fetch_one(&app.state.pools.superuser)
            .await
            .unwrap();
    let person = json!({"email":"owner@example.com","name":"Owner","department":"Engineering","account_id":null,
        "assignments":[{"tool":"claude-code","plan":"Team","allocated_model":"Sonnet","monthly_amount":30,"currency":"USD"}]});
    let unobserved = json!({"email":"new@example.com","name":"New","department":"Operations","account_id":null,"assignments":[]});
    let r = c
        .post(&url)
        .header("Cookie", &a.cookie)
        .json(&json!({"people":[person.clone(),unobserved]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    // Duplicate captures must not double the estimated cost. Hook costs are
    // not added to API request costs; unknown pricing stays unknown.
    let now = chrono::Utc::now();
    for (id, source, kind, cost) in [
        ("otel", "otel", "api.request", Some(2.0)),
        ("log", "log", "api.request", Some(2.0)),
        ("hook", "hook", "tool.result", Some(8.0)),
        ("unknown", "otel", "api.request", None),
    ] {
        sqlx::query("INSERT INTO events(event_id,ts,dt,org_id,user_id,host_id,agent,source,session_id,event_type,model,cost_usd) VALUES($1,$2,$3,$4,$5,'host','claude-code',$6,'session',$7,'Sonnet',$8)")
            .bind(id).bind(now.timestamp_millis()).bind(now.format("%Y-%m-%d").to_string()).bind(org).bind(account.to_string()).bind(source).bind(kind).bind(cost)
            .execute(&app.state.pools.superuser).await.unwrap();
    }
    let result: Value = c
        .get(&url)
        .header("Cookie", &a.cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(result["people"].as_array().unwrap().len(), 2);
    let owner = result["people"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["email"] == "owner@example.com")
        .unwrap();
    assert_eq!(owner["matched_account_id"], account.to_string());
    assert_eq!(owner["usage"][0]["estimated_cost_usd"], 2.0);
    assert_eq!(owner["usage"][0]["sessions"], 1);
    assert_eq!(owner["usage"][0]["priced_requests"], 1);
    assert_eq!(owner["usage"][0]["total_requests"], 2);
    let new = result["people"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["email"] == "new@example.com")
        .unwrap();
    assert!(new["matched_account_id"].is_null());
    assert!(new["usage"].as_array().unwrap().is_empty());
    let id = owner["id"].as_str().unwrap();
    let other: Value = c
        .get(&url)
        .header("Cookie", &b.cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(other["people"].as_array().unwrap().is_empty());
    assert_eq!(
        c.delete(format!("{url}/{id}"))
            .header("Cookie", &b.cookie)
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    // RLS protects the table even if the handler forgets an org predicate.
    let mut tx = app
        .state
        .pools
        .org_scoped_tx(uuid::Uuid::parse_str(&b.org_id).unwrap())
        .await
        .unwrap();
    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM ai_inventory")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(count, 0);
    tx.commit().await.unwrap();
    let mut bad = person.clone();
    bad["account_id"] = json!(foreign);
    assert_eq!(
        c.post(&url)
            .header("Cookie", &a.cookie)
            .json(&json!({"people":[bad]}))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    let mut duplicate_link = person.clone();
    duplicate_link["email"] = json!("alias@example.com");
    duplicate_link["account_id"] = json!(account);
    assert_eq!(
        c.post(&url)
            .header("Cookie", &a.cookie)
            .json(&json!({"people":[duplicate_link]}))
            .send()
            .await
            .unwrap()
            .status(),
        400,
        "one usage account must not inflate two people's adoption counts"
    );
    let mut changed = person.clone();
    changed["name"] = json!("Changed");
    let mut invalid = person.clone();
    invalid["email"] = json!("invalid@example.com");
    invalid["assignments"][0]["monthly_amount"] = json!(-10);
    assert_eq!(
        c.post(&url)
            .header("Cookie", &a.cookie)
            .json(&json!({"people":[changed.clone(),invalid]}))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    let (name,): (String,) = sqlx::query_as("SELECT name FROM ai_inventory WHERE id=$1")
        .bind(uuid::Uuid::parse_str(id).unwrap())
        .fetch_one(&app.state.pools.superuser)
        .await
        .unwrap();
    assert_eq!(name, "Owner", "invalid batch must not partially update");
    changed["assignments"] = json!([]);
    assert_eq!(
        c.post(&url)
            .header("Cookie", &a.cookie)
            .json(&json!({"people":[changed]}))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    // Roles are checked afresh for reads and writes, even with a live cookie.
    sqlx::query("UPDATE memberships SET role='member' WHERE account_id=$1 AND org_id=$2")
        .bind(account)
        .bind(org)
        .execute(&app.state.pools.superuser)
        .await
        .unwrap();
    assert_eq!(
        c.get(&url)
            .header("Cookie", &a.cookie)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        c.post(&url)
            .header("Cookie", &a.cookie)
            .json(&json!({"people":[person]}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        c.delete(format!("{url}/{id}"))
            .header("Cookie", &a.cookie)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    sqlx::query("UPDATE memberships SET role='owner' WHERE account_id=$1 AND org_id=$2")
        .bind(account)
        .bind(org)
        .execute(&app.state.pools.superuser)
        .await
        .unwrap();
    assert_eq!(
        c.delete(format!("{url}/{id}"))
            .header("Cookie", &a.cookie)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    app.teardown().await;
}
