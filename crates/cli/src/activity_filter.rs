//! Request-scoped display filters. Never mutate stored data or sync scope.
use axum::{
    extract::{Query, Request},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub(crate) struct ActivityFilter {
    device: Option<String>,
    repository: Option<String>,
    #[serde(default)]
    without_repository: bool,
}
tokio::task_local! { static FILTER: ActivityFilter; }

impl ActivityFilter {
    fn validate(&self) -> bool {
        self.device
            .as_ref()
            .is_none_or(|v| v.len() <= 256 && !v.contains('\0'))
            && self
                .repository
                .as_ref()
                .is_none_or(|v| v.len() <= 2048 && !v.contains('\0'))
            && !(self.without_repository && self.repository.is_some())
    }
    fn predicate(&self, this_host: &str) -> String {
        let mut parts = vec![];
        if let Some(device) = self
            .device
            .as_deref()
            .filter(|v| !v.is_empty() && *v != "all")
        {
            let host = if device == "this-mac" {
                this_host
            } else {
                device
            };
            parts.push(format!("host_id='{}'", host.replace('\'', "''")));
        }
        if self.without_repository {
            parts.push("(repo IS NULL OR repo='')".to_string());
        } else if let Some(repo) = &self.repository {
            parts.push(format!("repo='{}'", repo.replace('\'', "''")));
        }
        if parts.is_empty() {
            "TRUE".into()
        } else {
            parts.join(" AND ")
        }
    }
}
pub(crate) async fn apply(request: Request, next: Next) -> Response {
    let filter = match Query::<ActivityFilter>::try_from_uri(request.uri()) {
        Ok(Query(filter)) if filter.validate() => filter,
        _ => return (StatusCode::BAD_REQUEST, "Invalid activity filter").into_response(),
    };
    FILTER.scope(filter, next.run(request)).await
}
pub(crate) fn source(source: &str) -> String {
    let filter = FILTER.try_with(Clone::clone).unwrap_or_default();
    let host = kikimimi_schema::paths::host_id().unwrap_or_default();
    format!("(SELECT * FROM {source} WHERE {})", filter.predicate(&host))
}

/// Stable scope for change records; a record must never follow unrelated display filters.
pub(crate) fn scope_key() -> String {
    let mut filter = FILTER.try_with(Clone::clone).unwrap_or_default();
    filter.device = filter.device.filter(|v| !v.is_empty() && v != "all").map(|v| {
        if v == "this-mac" { kikimimi_schema::paths::host_id().unwrap_or_default() } else { v }
    });
    if filter == ActivityFilter::default() { return String::new(); }
    serde_json::to_string(&filter).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn combined_filters_are_exact_and_escaped() {
        let filter = ActivityFilter {
            device: Some("this-mac".into()),
            repository: Some("owner/it's-repo".into()),
            without_repository: false,
        };
        assert_eq!(
            filter.predicate("mac-1"),
            "host_id='mac-1' AND repo='owner/it''s-repo'"
        );
        assert_eq!(ActivityFilter::default().predicate("mac-1"), "TRUE");
    }
    #[tokio::test]
    #[serial_test::serial]
    async fn filters_query_real_store_without_changing_options() {
        use axum::body::{to_bytes, Body};
        use tower::ServiceExt;
        let dir = tempfile::tempdir().unwrap();
        let store = kikimimi_sink::store::shared(dir.path()).unwrap();
        let host = kikimimi_schema::paths::host_id().unwrap();
        let ts = chrono::Utc::now().timestamp_millis();
        for (i, (device, repo)) in [
            (host.as_str(), Some("owner/it's-repo")),
            ("other-device", Some("owner/it's-repo")),
            ("other-device", Some("owner/second")),
            (host.as_str(), None),
        ]
        .into_iter()
        .enumerate()
        {
            store
                .lock()
                .unwrap()
                .ingest(
                    &kikimimi_schema::Event {
                        event_id: format!("filter-{i}"),
                        ts,
                        dt: kikimimi_schema::dt_of(ts),
                        host_id: device.into(),
                        repo: repo.map(str::to_owned),
                        agent: "claude-code".into(),
                        source: "hook".into(),
                        event_type: "api.request".into(),
                        ..Default::default()
                    },
                    None,
                )
                .unwrap();
        }
        let router = crate::web::query_router(false).with_state(crate::web::WebAppState {
            token: "test".into(),
            data_dir: dir.path().into(),
        });
        for (query, hosts, events) in [
            ("", 2, 4),
            ("?device=this-mac", 1, 2),
            ("?device=other-device&repository=owner%2Fit%27s-repo", 1, 1),
            ("?device=this-mac&without_repository=true", 1, 1),
            ("?device=unknown", 0, 0),
        ] {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/web/q/machines{query}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = response.status();
            let body: serde_json::Value =
                serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                    .unwrap();
            assert_eq!(status, StatusCode::OK, "{query}: {body}");
            let rows = body["rows"].as_array().unwrap();
            assert_eq!(rows.len(), hosts, "{query}: {body}");
            assert_eq!(
                rows.iter().map(|r| r[4].as_i64().unwrap()).sum::<i64>(),
                events,
                "{query}"
            );
        }
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/web/q/filter-options?device=this-mac&without_repository=true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(body["devices"].as_array().unwrap().len(), 2);
        assert_eq!(body["repositories"].as_array().unwrap().len(), 2);
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/web/q/machines?repository=x&without_repository=true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    #[tokio::test]
    async fn concurrent_requests_keep_their_own_filter() {
        let a = ActivityFilter {
            device: Some("a".into()),
            ..Default::default()
        };
        let b = ActivityFilter {
            device: Some("b".into()),
            without_repository: true,
            ..Default::default()
        };
        let (a, b) = tokio::join!(
            FILTER.scope(a, async {
                tokio::task::yield_now().await;
                source("events")
            }),
            FILTER.scope(b, async {
                tokio::task::yield_now().await;
                source("events")
            })
        );
        assert!(a.contains("host_id='a'"));
        assert!(!a.contains("host_id='b'"));
        assert!(b.contains("host_id='b' AND (repo IS NULL OR repo='')"));
        assert!(source("events").ends_with("WHERE TRUE)"));
    }
}
