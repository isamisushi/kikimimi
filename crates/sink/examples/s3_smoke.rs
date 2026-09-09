//! Synthetic events through the production S3 sink, for local integration tests.
use kikimimi_schema::Event;
use kikimimi_sink::{EventSink, S3Config, S3Sink};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    anyhow::ensure!(
        args.len() == 7,
        "usage: s3_smoke URL ENDPOINT STAGING HOST DATE EVENT_ID"
    );
    let mut sink = S3Sink::new(
        S3Config {
            url: args[1].clone(),
            endpoint_url: Some(args[2].clone()),
            ..Default::default()
        },
        args[4].clone(),
        args[3].clone().into(),
    );
    sink.push(Event {
        event_id: args[6].clone(),
        ts: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis() as i64,
        dt: args[5].clone(),
        host_id: args[4].clone(),
        session_id: Some(format!("{}-session", args[4])),
        agent: "claude-code".into(),
        source: "hook".into(),
        event_type: "tool.call".into(),
        tool_name: Some("Bash".into()),
        input_tokens: Some(10),
        output_tokens: Some(2),
        success: Some(true),
        ..Default::default()
    });
    sink.flush()?;
    anyhow::ensure!(
        sink.last_error().is_none(),
        "S3 upload failed: {:?}",
        sink.last_error()
    );
    Ok(())
}
