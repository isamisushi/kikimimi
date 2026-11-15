//! Reconcile local request-time auth diagnostics with measured usage. Keep login state,
//! quota snapshots, message contents and credentials out of historical classification.
use anyhow::Result;
use kikimimi_sink::store::SharedStore;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::Command,
    sync::{Mutex, OnceLock},
    time::SystemTime,
};

pub const MATCH: &str = r#"
(u.agent='codex' AND u.provider='openai' AND u.source IN ('otel','log')
 AND EXISTS (SELECT 1 FROM codex_request_auth a WHERE a.host=u.host_id
   AND a.session_id=u.session_id AND a.turn_id=u.turn_id GROUP BY a.session_id
   HAVING bool_and(a.subscribed) AND min(abs(a.ts-u.ts)) <= 60000))
OR (u.agent='claude-code' AND u.source IN ('otel','log') AND (u.provider IS NULL OR u.provider='anthropic')
 AND EXISTS (SELECT 1 FROM claude_request_auth a WHERE a.host=u.host_id AND a.session_id=u.session_id
   AND a.model=u.model AND a.i=u.input_tokens AND a.o=u.output_tokens AND a.r=u.cache_read_tokens
   AND a.w=u.cache_write_tokens AND abs(a.ts-u.ts)<=60000 GROUP BY a.session_id
   HAVING count(DISTINCT a.request_id)=1 AND bool_and(a.subscribed)))"#;

#[derive(Default)]
struct Cache {
    files: HashMap<PathBuf, (u64, SystemTime)>,
    dbs: HashMap<PathBuf, i64>,
}

pub fn refresh(store: SharedStore) -> Result<()> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    let mut cache = CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let host = kikimimi_schema::paths::host_id()?;
    let mut stack = vec![kikimimi_schema::paths::claude_projects_dir()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for item in entries.flatten() {
            let Ok(ty) = item.file_type() else { continue };
            let path = item.path();
            if ty.is_dir() {
                stack.push(path);
                continue;
            }
            if !ty.is_file() || path.extension().and_then(|x| x.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(meta) = item.metadata() else { continue };
            let stamp = (meta.len(), meta.modified()?);
            if cache.files.get(&path) == Some(&stamp) {
                continue;
            }
            let Ok(file) = fs::File::open(&path) else {
                continue;
            };
            let rows = claude_rows(BufReader::new(file))?;
            record(&store, &host, true, &rows)?;
            cache.files.insert(path, stamp);
        }
    }
    if let Ok(entries) = fs::read_dir(kikimimi_schema::paths::codex_home_dir()) {
        for item in entries.flatten() {
            let path = item.path();
            let name = item.file_name();
            if !name
                .to_str()
                .and_then(|n| n.strip_prefix("logs_"))
                .and_then(|n| n.strip_suffix(".sqlite"))
                .is_some_and(|n| n.parse::<u32>().is_ok())
            {
                continue;
            }
            let cursor = cache.dbs.get(&path).copied().unwrap_or(0);
            if let Ok((rows, last)) = codex_rows(&path, cursor) {
                record(&store, &host, false, &rows)?;
                cache.dbs.insert(path, last);
            }
        }
    }
    Ok(())
}
fn record(store: &SharedStore, host: &str, claude: bool, rows: &[Value]) -> Result<()> {
    if !rows.is_empty() {
        let data = serde_json::to_string(rows)?;
        store
            .lock()
            .map_err(|_| anyhow::anyhow!("store poisoned"))?
            .record_request_auth(host, claude, &data)?;
    }
    Ok(())
}

fn allows_claude_subscription(v: &Value) -> bool {
    matches!(v["billing_source"].as_str(), None | Some("subscription"))
        && matches!(v["provider"].as_str(), None | Some("anthropic"))
        && matches!(v["auth_mode"].as_str(), None | Some("oauth" | "claude.ai"))
        && v["fallback_credit"].is_null()
}

fn claude_rows(reader: impl BufRead) -> Result<Vec<Value>> {
    let mut requests = HashMap::new();
    let mut responses = Vec::new();
    for line in reader.lines() {
        let line = line?;
        if !line.contains("\"api-request\"") && !line.contains("\"assistant\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(sid) = v["sessionId"].as_str() else {
            continue;
        };
        if v["type"] == "api-request" {
            let Some(id) = v["id"].as_str() else { continue };
            let oauth = allows_claude_subscription(&v)
                && allows_claude_subscription(&v["params"])
                && v["params"]["betas"]
                    .as_array()
                    .is_some_and(|betas| betas.iter().any(|b| b == "oauth-2025-04-20"));
            requests
                .entry((sid.to_owned(), id.to_owned()))
                .and_modify(|ok: &mut bool| *ok &= oauth)
                .or_insert(oauth);
        } else if v["type"] == "assistant" && v["isApiErrorMessage"] != true {
            let m = &v["message"];
            let u = &m["usage"];
            let (Some(id), Some(model), Some(ts)) = (
                v["requestId"].as_str(),
                m["model"].as_str(),
                v["timestamp"]
                    .as_str()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()),
            ) else {
                continue;
            };
            let (Some(i), Some(o), Some(r), Some(w)) = (
                u["input_tokens"].as_i64(),
                u["output_tokens"].as_i64(),
                u["cache_read_input_tokens"].as_i64(),
                u["cache_creation_input_tokens"].as_i64(),
            ) else {
                continue;
            };
            let direct = id.starts_with("req_")
                && matches!(v["entrypoint"].as_str(), Some("cli" | "claude-desktop"))
                && allows_claude_subscription(&v)
                && allows_claude_subscription(u);
            responses.push((v["requestRef"].as_str().unwrap_or("").to_owned(), direct,
                json!({"session":sid,"request":id,"model":model,"ts":ts.timestamp_millis(),"i":i,"o":o,"r":r,"w":w})));
        }
    }
    for (reference, direct, row) in &mut responses {
        let key = (
            row["session"].as_str().unwrap().to_owned(),
            std::mem::take(reference),
        );
        row["subscribed"] = json!(*direct && requests.get(&key) == Some(&true));
    }
    Ok(responses.into_iter().map(|(_, _, row)| row).collect())
}

fn codex_rows(path: &Path, cursor: i64) -> Result<(Vec<Value>, i64)> {
    // Extract only structured trace attributes from the feedback target, never return log bodies.
    let sql = format!(
        r#"SELECT id, ts*1000 AS ts, thread_id AS session,
        substr(feedback_log_body,instr(feedback_log_body,'turn.id=')+8,38) AS turn,
        instr(feedback_log_body,': endpoint="/responses" auth_header_attached=true auth_header_name="authorization" auth_mode="Chatgpt"')>0 AS subscribed
        FROM logs WHERE id>{cursor} AND target='feedback_tags' AND thread_id IS NOT NULL
        AND instr(feedback_log_body,': endpoint="/responses"')>0 AND instr(feedback_log_body,'turn.id=')>0 ORDER BY id"#
    );
    let result = Command::new("sqlite3")
        .args(["-readonly", "-json"])
        .arg(path)
        .arg(sql)
        .output()?;
    anyhow::ensure!(result.status.success(), "Codex diagnostics unavailable");
    let mut last = cursor;
    let mut rows = Vec::new();
    if !result.stdout.is_empty() {
        for mut row in serde_json::from_slice::<Vec<Value>>(&result.stdout)? {
            last = row["id"].as_i64().unwrap_or(last);
            let Some(turn) = row["turn"]
                .as_str()
                .and_then(|s| s.trim_start_matches('"').get(..36))
                .and_then(|s| uuid::Uuid::parse_str(s).ok())
            else {
                continue;
            };
            row["turn"] = json!(turn.to_string());
            row["subscribed"] = json!(row["subscribed"] == 1);
            rows.push(row);
        }
    }
    Ok((rows, last))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_requires_request_evidence_and_rejects_conflicts() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let mut db = kikimimi_sink::store::Store::open(&dir.path().join("test.db"))?;
        let transcript = [
            json!({"type":"api-request","sessionId":"s","id":"ref","params":{"betas":["oauth-2025-04-20"]}}),
            json!({"type":"assistant","entrypoint":"cli","sessionId":"s","requestId":"req_test","requestRef":"ref","timestamp":"2026-11-15T00:00:00Z","message":{"model":"m","usage":{"input_tokens":4,"output_tokens":2,"cache_read_input_tokens":0,"cache_creation_input_tokens":1}}}),
        ];
        let rows = claude_rows(std::io::Cursor::new(
            transcript
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        ))?;
        assert_eq!(rows[0]["subscribed"], true);
        db.record_request_auth("host", true, &serde_json::to_string(&rows)?)?;
        let ts = rows[0]["ts"].as_i64().unwrap();
        let mut codex = json!([{"session":"s","turn":"t","ts":ts,"subscribed":true}]);
        db.record_request_auth("host", false, &codex.to_string())?;
        let query = format!("SELECT agent, provider FROM (SELECT * FROM (VALUES
            ('host','s','t','claude-code','otel','anthropic','m',{ts},4,2,0,1,NULL),
            ('host','s','t','codex','log','openai','m',{ts},4,2,0,1,NULL),
            ('host','s','t','codex','log','kikimimi','m',{ts},4,2,0,1,NULL),
            ('other','s','t','codex','log','openai','m',{ts},4,2,0,1,NULL),
            ('host','s','t','claude-code','otel','anthropic','m',{ts},4,2,0,1,'api_key'),
            ('host','s','t','codex','otel','openai','m',{ts},4,2,0,1,'router')
            ) v(host_id,session_id,turn_id,agent,source,provider,model,ts,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,billing_route)) u
            WHERE billing_route IS NULL AND ({MATCH}) ORDER BY agent");
        assert_eq!(db.query(&query)?.len(), 2);
        codex[0]["subscribed"] = json!(false);
        db.record_request_auth("host", false, &codex.to_string())?;
        assert_eq!(db.query(&query)?.len(), 1);
        let mut foreign = rows;
        foreign[0]["request"] = json!("req_other");
        db.record_request_auth("host", true, &serde_json::to_string(&foreign)?)?;
        assert!(db.query(&query)?.is_empty());
        Ok(())
    }
}
