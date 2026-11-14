//! Read-only, machine-local health. Never return configuration secrets or raw sink errors.
use crate::state::AgentState;
use axum::{
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

const HEARTBEAT_MAX_AGE_MS: i64 = 30_000;

#[derive(Debug, Serialize)]
pub(crate) struct Health {
    pub checked_at_ms: i64,
    pub collector: &'static str,
    pub socket_reachable: bool,
    pub service_running: Option<bool>,
    pub heartbeat_at_ms: Option<i64>,
    pub snapshot_fresh: bool,
    pub last_event_ts: Option<i64>,
    pub last_flush_at_ms: Option<i64>,
    pub events_by_source: Option<crate::state::EventsBySource>,
    pub spool_pending: usize,
    pub issues: Vec<&'static str>,
    pub cloud: SyncHealth,
    pub s3: SyncHealth,
}

#[derive(Debug, Serialize)]
pub(crate) struct SyncHealth {
    pub status: &'static str,
    pub pending: Option<usize>,
    pub last_push_at: Option<i64>,
}

fn sync_health(
    enabled: bool,
    paused: bool,
    running: bool,
    fresh: bool,
    snapshot: Option<(usize, Option<i64>, bool)>,
) -> SyncHealth {
    let (pending, last_push_at, error) = snapshot
        .map(|(n, at, err)| (Some(n), at, err))
        .unwrap_or((None, None, false));
    let status = if !enabled {
        "off"
    } else if paused {
        "paused"
    } else if !running || !fresh {
        "unknown"
    } else if error {
        "error"
    } else if pending.is_none() {
        "waiting"
    } else if pending.unwrap_or(0) > 0 {
        "pending"
    } else if last_push_at.is_some() {
        "synced"
    } else {
        "waiting"
    };
    SyncHealth {
        status,
        pending,
        last_push_at,
    }
}

fn evaluate(
    state: Option<&AgentState>,
    reachable: bool,
    service_running: Option<bool>,
    now: i64,
    spool_pending: usize,
    cloud_enabled: bool,
    cloud_paused: bool,
    s3_enabled: bool,
) -> Health {
    let heartbeat = state.and_then(|s| s.saved_at_ms);
    let fresh = heartbeat.is_some_and(|at| (0..=HEARTBEAT_MAX_AGE_MS).contains(&(now - at)));
    let collector = if reachable && fresh {
        "running"
    } else if reachable || service_running == Some(true) {
        "unconfirmed"
    } else if service_running == Some(false) {
        "stopped"
    } else {
        "unknown"
    };
    let mut issues = vec![];
    if collector == "unconfirmed" {
        issues.push("collector_unconfirmed");
    }
    if collector == "stopped" {
        issues.push("collector_stopped");
    }
    if !fresh {
        issues.push("heartbeat_missing_or_stale");
    }
    // Old errors remain historical when the heartbeat is stale.
    if let Some(s) = state.filter(|_| fresh) {
        if s.otlp_error.is_some() {
            issues.push("telemetry_unavailable");
        }
        if s.last_flush_error.is_some() {
            issues.push("storage_error");
        }
        if s.codex.malformed_lines > 0 {
            issues.push("codex_malformed_records_since_start");
        }
        if s.claude_backfill.last_error.is_some() {
            issues.push("claude_read_error");
        }
        if s.claude_desktop.last_error.is_some() {
            issues.push("claude_desktop_read_error");
        }
        if s.otlp_rejected > 0 {
            issues.push("telemetry_rejected_since_start");
        }
    }
    if spool_pending > 20 {
        issues.push("spool_backlog");
    }
    Health {
        checked_at_ms: now,
        collector,
        socket_reachable: reachable,
        service_running,
        heartbeat_at_ms: heartbeat,
        snapshot_fresh: fresh,
        last_event_ts: state.and_then(|s| s.last_event_ts),
        last_flush_at_ms: state.and_then(|s| s.last_flush.as_ref().map(|f| f.at_ms)),
        events_by_source: state.map(|s| s.events_by_source.clone()),
        spool_pending,
        issues,
        cloud: sync_health(
            cloud_enabled,
            cloud_paused,
            collector == "running",
            fresh,
            state
                .and_then(|s| s.cloud.as_ref())
                .map(|s| (s.pending, s.last_push_at, s.last_error.is_some())),
        ),
        s3: sync_health(
            s3_enabled,
            false,
            collector == "running",
            fresh,
            state
                .and_then(|s| s.s3.as_ref())
                .map(|s| (s.pending, s.last_push_at, s.last_error.is_some())),
        ),
    }
}

pub(crate) fn snapshot() -> Health {
    let cfg = crate::config::KikimimiConfig::load_without_secrets();
    let state = crate::state::load_opt(&kikimimi_schema::paths::state_path());
    let service = crate::service::status();
    let mut health = evaluate(
        state.as_ref(),
        kikimimi_spool::send_control(b'n'),
        service.running,
        chrono::Utc::now().timestamp_millis(),
        kikimimi_spool::backlog(),
        cfg.cloud.is_some(),
        cfg.cloud_sync_paused,
        cfg.s3.is_some(),
    );
    let expected = crate::collection_cmd::describe(&cfg);
    if (cfg.cloud.is_some() || cfg.s3.is_some())
        && state.as_ref().and_then(|s| s.collection_target.as_ref()) != Some(&expected)
    {
        health.issues.push("destination_unconfirmed");
        if cfg.cloud.is_some() && !cfg.cloud_sync_paused {
            health.cloud.status = "unknown";
        }
        if cfg.s3.is_some() {
            health.s3.status = "unknown";
        }
    }
    health
}

pub(crate) async fn handle() -> Response {
    match tokio::task::spawn_blocking(snapshot).await {
        Ok(health) => (
            [(axum::http::header::CACHE_CONTROL, "no-store")],
            Json(health),
        )
            .into_response(),
        Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::CloudState;
    #[test]
    fn silence_is_not_a_stopped_collector_and_old_sync_is_not_current() {
        let mut s = AgentState::new(1, 0, 4318);
        s.saved_at_ms = Some(100_000);
        s.cloud = Some(CloudState {
            endpoint: "secret-not-returned".into(),
            pending: 0,
            last_push_at: Some(90_000),
            last_error: None,
        });
        let health = evaluate(Some(&s), true, Some(true), 101_000, 0, true, false, false);
        assert_eq!(health.collector, "running");
        assert_eq!(health.last_event_ts, None);
        assert_eq!(health.cloud.status, "synced");
        let old = evaluate(Some(&s), true, Some(true), 200_000, 0, true, false, false);
        assert_eq!(old.collector, "unconfirmed");
        assert_eq!(old.cloud.status, "unknown");
        assert!(!serde_json::to_string(&old)
            .unwrap()
            .contains("secret-not-returned"));
    }
    #[test]
    fn conflicting_service_and_socket_reports_are_never_healthy() {
        let mut s = AgentState::new(1, 0, 4318);
        s.saved_at_ms = Some(100_000);
        let h = evaluate(Some(&s), false, Some(true), 101_000, 0, true, false, false);
        assert_eq!(h.collector, "unconfirmed");
        assert_eq!(h.cloud.status, "unknown");
        s.saved_at_ms = None;
        assert_eq!(
            evaluate(Some(&s), true, None, 101_000, 0, false, false, false).collector,
            "unconfirmed"
        );
        assert_eq!(
            evaluate(None, false, None, 101_000, 0, false, false, false).collector,
            "unknown"
        );
    }
    #[test]
    fn errors_and_pending_sync_are_visible() {
        let mut s = AgentState::new(1, 0, 4318);
        s.saved_at_ms = Some(100_000);
        s.last_flush_error = Some("disk full".into());
        s.cloud = Some(CloudState {
            endpoint: "x".into(),
            pending: 7,
            last_push_at: None,
            last_error: None,
        });
        let h = evaluate(Some(&s), true, Some(true), 101_000, 21, true, false, false);
        assert!(h.issues.contains(&"storage_error"));
        assert!(h.issues.contains(&"spool_backlog"));
        assert_eq!(h.cloud.status, "pending");
        assert_eq!(h.cloud.pending, Some(7));
        assert_eq!(
            evaluate(Some(&s), true, None, 101_000, 0, true, true, false)
                .cloud
                .status,
            "paused"
        );
    }
}
