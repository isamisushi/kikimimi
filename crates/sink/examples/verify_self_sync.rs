//! Cross-repository integration probe. Credentials arrive over stdin, never argv.
use kikimimi_schema::Event;
use kikimimi_sink::{store::Store, CloudSink, EventSink};
use serde::Deserialize;
use std::{
    io::Read,
    sync::{Arc, Mutex},
};
#[derive(Deserialize)]
struct Credentials {
    endpoint: String,
    email: String,
    org_id: String,
    token_a: String,
    token_b: String,
}
fn main() -> anyhow::Result<()> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let c: Credentials = serde_json::from_str(&input)?;
    anyhow::ensure!(
        c.endpoint.starts_with("http://127.0.0.1:"),
        "test service only"
    );
    let dir = tempfile::tempdir()?;
    let a = Arc::new(Mutex::new(Store::open(&dir.path().join("a.duckdb"))?));
    let bpath = dir.path().join("b.duckdb");
    let b = Arc::new(Mutex::new(Store::open(&bpath)?));
    let api = Event {
        event_id: "probe-api".into(),
        ts: 1793538000000,
        dt: "2026-11-01".into(),
        host_id: "mac-a".into(),
        agent: "claude".into(),
        client: Some("claude_desktop".into()),
        source: "hook".into(),
        event_type: "api.request".into(),
        session_id: Some("internal".into()),
        correlation_key: Some("response".into()),
        model: Some("claude-haiku-5-5".into()),
        input_tokens: Some(2),
        output_tokens: Some(16),
        usage_source: Some("hook".into()),
        ..Default::default()
    };
    let link = Event {
        event_id: "probe-link".into(),
        event_type: "message.usage.link".into(),
        session_id: Some("visible-chat".into()),
        parent_session_id: Some("internal".into()),
        turn_id: Some("visible-response".into()),
        correlation_confidence: Some("content_match".into()),
        input_tokens: None,
        output_tokens: None,
        ..api.clone()
    };
    for event in [&api, &link] {
        a.lock().unwrap().ingest(event, Some(("a", event)))?;
    }
    let make = |store, host: &str, token: &str, dest: &str| {
        CloudSink::database(
            c.endpoint.clone(),
            token.into(),
            host.into(),
            dest.into(),
            c.email.clone(),
            c.org_id.clone(),
            store,
        )
    };
    let mut sa = make(a.clone(), "mac-a", &c.token_a, "a");
    let mut sb = make(b.clone(), "mac-a2", &c.token_b, "b");
    sa.flush()?;
    sb.flush()?;
    let query="SELECT model,input_tokens,output_tokens,turn_id FROM resolved_events WHERE session_id='visible-chat'";
    let rows = b.lock().unwrap().query(query)?;
    anyhow::ensure!(
        rows.len() == 1
            && rows[0]["model"] == "claude-haiku-5-5"
            && rows[0]["input_tokens"] == 2
            && rows[0]["output_tokens"] == 16
            && rows[0]["turn_id"] == "visible-response",
        "cross-device usage/link mismatch"
    );
    let otel = Event {
        event_id: "probe-otel".into(),
        host_id: "mac-a2".into(),
        agent: "claude-code".into(),
        client: None,
        source: "otel".into(),
        session_id: Some("code-chat".into()),
        ..api.clone()
    };
    b.lock().unwrap().ingest(&otel, Some(("b", &otel)))?;
    sb.flush()?;
    sa.flush()?;
    anyhow::ensure!(a.lock().unwrap().query("SELECT event_id FROM resolved_events WHERE source='otel' AND event_id='probe-otel'")?.len()==1,"reverse sync failed");
    anyhow::ensure!(
        sa.pending() == 0 && sb.pending() == 0,
        "unacknowledged rows"
    );
    drop(sb);
    drop(b);
    let b = Arc::new(Mutex::new(Store::open(&bpath)?));
    let mut sb = make(b.clone(), "mac-a2", &c.token_b, "b");
    sb.flush()?;
    anyhow::ensure!(
        b.lock().unwrap().query(query)? == rows,
        "restart changed usage"
    );
    println!("two DuckDB devices: bidirectional sync, Haiku measured usage, conversation link, OTel source, restart: PASS");
    Ok(())
}
