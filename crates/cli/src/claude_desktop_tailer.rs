//! Claude mode (the current desktop UI), not the Code/Cowork transcript paths.
//! Reads live conversation trees and allowlisted React Query conversation metadata.
//! No browser credentials, HTTP calls, raw cache copies, or persisted bodies.
use crate::chromium_store::{self, Cursor, MAX_BYTES};
use crate::chromium_v8::{deserialize_blink_with_limits, V8Limits, V8Value};
use anyhow::{bail, ensure, Context, Result};
use kikimimi_schema::Event;
use leveldb_core::Record;
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub const CLIENT: &str = "claude_desktop";
pub const FOLDER_ATTACHED: &str = "conversation.folder.attached";
const ORIGIN: &str = "https_claude.ai_0.indexeddb";

pub struct ClaudeDesktopTailer {
    root: PathBuf,
    data_dir: PathBuf,
    plugin_inbox: PathBuf,
    host_id: String,
    seen: Option<HashSet<String>>,
    last_scan: Option<Instant>,
}

impl ClaudeDesktopTailer {
    pub fn new(data_dir: PathBuf, host_id: String) -> Self {
        Self {
            root: crate::client_source::claude_support_dir().join("IndexedDB"),
            data_dir,
            plugin_inbox: kikimimi_schema::paths::kikimimi_dir().join("claude-plugin-inbox"),
            host_id,
            seen: None,
            last_scan: None,
        }
    }

    pub fn scan_and_drain(&mut self) -> Result<Option<Vec<Event>>> {
        if self
            .last_scan
            .is_some_and(|at| at.elapsed() < Duration::from_secs(10))
        {
            return Ok(None);
        }
        self.last_scan = Some(Instant::now());
        let dir = self.root.join(format!("{ORIGIN}.leveldb"));
        if self.seen.is_none() {
            self.seen = Some(kikimimi_sink::client_event_ids(&self.data_dir, CLIENT)?);
        }
        let mut events = crate::claude_plugin::collect(&self.plugin_inbox, &self.host_id)?;
        // A transient browser database failure must not block durable plugin uploads.
        if dir.try_exists()? {
            match collect_with_key(
                &self.root,
                &self.host_id,
                crate::claude_plugin::matching_key().as_deref(),
            ) {
                Ok(snapshot) => events.extend(snapshot),
                Err(error) if events.is_empty() => return Err(error),
                Err(_) => {
                    eprintln!("Claude Desktop snapshot unavailable; plugin collection continues")
                }
            }
        }
        let links =
            crate::claude_plugin::collect_links(&self.plugin_inbox, &events, &self.host_id)?;
        events.retain(|e| e.event_type != "message.fingerprint");
        events.extend(links);
        // Only mark IDs after the entire snapshot has decoded successfully. A
        // concurrent compaction/missing blob cannot consume unpersisted events.
        let seen = self.seen.as_mut().unwrap();
        Ok(Some(
            events
                .into_iter()
                .filter(|event| seen.insert(event.event_id.clone()))
                .collect(),
        ))
    }
}

fn utf16(bytes: &[u8]) -> Result<String> {
    ensure!(bytes.len() % 2 == 0, "Invalid IndexedDB string");
    Ok(String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|b| u16::from_be_bytes([b[0], b[1]]))
            .collect::<Vec<_>>(),
    )?)
}
fn string(c: &mut Cursor<'_>) -> Result<String> {
    let len = usize::try_from(c.varint()?)?
        .checked_mul(2)
        .context("IndexedDB string too long")?;
    utf16(c.take(len)?)
}

#[cfg(test)]
fn collect(root: &Path, host_id: &str) -> Result<Vec<Event>> {
    collect_with_key(root, host_id, None)
}

fn collect_with_key(root: &Path, host_id: &str, matching_key: Option<&str>) -> Result<Vec<Event>> {
    ensure!(
        fs::symlink_metadata(root)?.file_type().is_dir(),
        "Expected an IndexedDB directory"
    );
    let records = chromium_store::read_live(&root.join(format!("{ORIGIN}.leveldb")))?;
    // Resolve IDs from metadata, rather than assuming this profile's DB/store
    // numbering. Model metadata is restricted to the persisted conversation-list query.
    let mut databases = HashSet::new();
    let mut query_databases = HashSet::new();
    let mut response_databases = HashSet::new();
    for record in records.values() {
        let (db, store, index, tail) = chromium_store::prefix(&record.key)?;
        if (db, store, index) == (0, 0, 0) && tail.first() == Some(&201) {
            let mut c = Cursor(&tail[1..]);
            let origin = string(&mut c)?;
            let name = string(&mut c)?;
            if origin == "https_claude.ai_0@1"
                && matches!(
                    name.as_str(),
                    "claude-conversation-store" | "keyval-store" | "kikimimi-response-models"
                )
            {
                ensure!(record.value.len() <= 8, "Invalid IndexedDB database ID");
                let mut id = [0; 8];
                id[..record.value.len()].copy_from_slice(&record.value);
                if name == "keyval-store" {
                    query_databases.insert(u64::from_le_bytes(id));
                } else if name == "kikimimi-response-models" {
                    response_databases.insert(u64::from_le_bytes(id));
                } else {
                    databases.insert(u64::from_le_bytes(id));
                }
            }
        }
    }
    let mut stores = HashSet::new();
    let mut query_stores = HashSet::new();
    let mut response_stores = HashSet::new();
    for record in records.values() {
        let (db, store, index, tail) = chromium_store::prefix(&record.key)?;
        if (databases.contains(&db)
            || query_databases.contains(&db)
            || response_databases.contains(&db))
            && store == 0
            && index == 0
            && tail.first() == Some(&50)
        {
            let mut c = Cursor(&tail[1..]);
            let id = c.varint()?;
            if c.take(1)? == [0] {
                let name = utf16(&record.value)?;
                if databases.contains(&db) && name == "trees" {
                    stores.insert((db, id));
                }
                if response_databases.contains(&db) && name == "observations" {
                    response_stores.insert((db, id));
                }
                if query_databases.contains(&db) && name == "keyval" {
                    query_stores.insert((db, id));
                }
            }
        }
    }
    let mut events = vec![];
    let mut conversations = HashMap::new();
    for record in records.values() {
        let (db, store, index, _) = chromium_store::prefix(&record.key)?;
        if index != 1 || !stores.contains(&(db, store)) {
            continue;
        }
        let bytes = unwrap_record(root, record, &records)?;
        let value = deserialize_blink_with_limits(
            &bytes,
            V8Limits {
                max_depth: 96,
                max_nodes: 500_000,
            },
        )
        .context("Unsupported Claude conversation cache encoding")?;
        let messages = normalize(&value, host_id);
        if let Some(key) = &matching_key {
            events.extend(fingerprint_messages(&value, &messages, key));
        }
        if !messages.is_empty() {
            if let (Some(sid), Some(org)) =
                (text(&value, "conversationUuid"), text(&value, "orgUuid"))
            {
                conversations.insert(sid.to_owned(), org.to_owned());
            }
        }
        events.extend(messages);
    }
    if let Some(support) = root.parent() {
        let grants = folder_grants(&support.join("claude_desktop_config.json"));
        add_granted_folders(&mut events, &grants, host_id);
    }
    for record in records.values() {
        let (db, store, index, tail) = chromium_store::prefix(&record.key)?;
        if index != 1 || !query_stores.contains(&(db, store)) || tail.first() != Some(&1) {
            continue;
        }
        let mut key = Cursor(&tail[1..]);
        if string(&mut key).ok().as_deref() != Some("react-query-cache") || !key.0.is_empty() {
            continue;
        }
        // An unavailable optional model cache must not stop message collection.
        let result = unwrap_record(root, record, &records).and_then(|bytes| {
            Ok(deserialize_blink_with_limits(
                &bytes,
                V8Limits {
                    max_depth: 96,
                    max_nodes: 500_000,
                },
            )?)
        });
        match result {
            Ok(value) => events.extend(normalize_models(&value, &conversations, host_id)),
            Err(_) => {
                eprintln!("Claude Desktop conversation model cache could not be decoded")
            }
        }
    }
    let responses: HashMap<_, _> = events
        .iter()
        .filter(|e| e.event_type == "message.received")
        .filter_map(|e| {
            Some((
                (e.session_id.clone()?, e.correlation_key.clone()?),
                e.clone(),
            ))
        })
        .collect();
    for record in records.values() {
        let (db, store, index, _) = chromium_store::prefix(&record.key)?;
        if index != 1 || !response_stores.contains(&(db, store)) {
            continue;
        }
        let value = unwrap_record(root, record, &records).and_then(|b| {
            Ok(deserialize_blink_with_limits(
                &b,
                V8Limits {
                    max_depth: 8,
                    max_nodes: 100,
                },
            )?)
        });
        if let Ok(value) = value {
            if let Some(event) =
                normalize_response_model(&value, &conversations, &responses, host_id)
            {
                events.push(event);
            }
        }
    }
    events.sort_by(|a, b| (a.ts, &a.event_id).cmp(&(b.ts, &b.event_id)));
    Ok(events)
}

/// Only metadata tied to an existing assistant response is accepted. Historical
/// conversation settings never become response evidence. Source remains explicit.
fn normalize_response_model(
    value: &V8Value,
    conversations: &HashMap<String, String>,
    responses: &HashMap<(String, String), Event>,
    host: &str,
) -> Option<Event> {
    if get(value, "version") != Some(&V8Value::Int(1)) {
        return None;
    }
    let sid = text(value, "conversationUuid")?;
    let mid = text(value, "messageUuid")?;
    if conversations.get(sid)?.as_str() != text(value, "orgUuid")? {
        return None;
    }
    let model = text(value, "model").filter(|m| {
        m.starts_with("claude-")
            && m.len() <= 128
            && m.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.' || b == b'_')
    })?;
    let source = text(value, "modelSource")?;
    if !matches!(
        source,
        "active_turn_selection"
            | "server_model_change"
            | "server_model_fallback"
            | "server_turn_fallback"
    ) {
        return None;
    }
    let mut event = responses.get(&(sid.into(), mid.into()))?.clone();
    event.event_type = format!("message.model.{source}");
    event.event_id = kikimimi_schema::event_id(
        host,
        "log",
        &event.event_type,
        &format!("{sid}:{mid}:{model}"),
    );
    event.model = Some(model.into());
    Some(event)
}

/// Conversation settings are observed metadata, not evidence for each historical
/// inference. Keep them in separate events; never attach them to past API usage.
fn normalize_models(
    value: &V8Value,
    conversations: &HashMap<String, String>,
    host_id: &str,
) -> Vec<Event> {
    let Some(V8Value::Array(queries)) = get(value, "clientState").and_then(|v| get(v, "queries"))
    else {
        return vec![];
    };
    let mut events = HashMap::new();
    for query in queries {
        let Some(V8Value::Array(key)) = get(query, "queryKey") else {
            continue;
        };
        if !matches!(key.first(),Some(V8Value::String(s)) if s=="chat_conversation_list") {
            continue;
        }
        let Some(org) = key
            .get(1)
            .and_then(|v| text(v, "orgUuid"))
            .filter(|s| id(s))
        else {
            continue;
        };
        let Some(V8Value::Array(pages)) = get(query, "state")
            .and_then(|v| get(v, "data"))
            .and_then(|v| get(v, "pages"))
        else {
            continue;
        };
        for page in pages {
            let Some(V8Value::Array(rows)) = get(page, "data") else {
                continue;
            };
            for row in rows {
                let Some(sid) =
                    text(row, "uuid").filter(|s| conversations.get(*s).is_some_and(|o| o == org))
                else {
                    continue;
                };
                let Some(model) = text(row, "model").filter(|m| {
                    m.starts_with("claude-")
                        && m.len() <= 128
                        && m.bytes().all(|b| {
                            b.is_ascii_alphanumeric() || b == b'-' || b == b'.' || b == b'_'
                        })
                }) else {
                    continue;
                };
                let Some(ts) = text(row, "updated_at")
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|d| d.timestamp_millis())
                else {
                    continue;
                };
                let kind = "conversation.model.observed";
                let event_id = kikimimi_schema::event_id(
                    host_id,
                    "log",
                    kind,
                    &format!("{org}:{sid}:{ts}:{model}"),
                );
                events.entry(event_id.clone()).or_insert_with(|| Event {
                    event_id,
                    ts,
                    dt: kikimimi_schema::dt_of(ts),
                    host_id: host_id.into(),
                    agent: "claude".into(),
                    client: Some(CLIENT.into()),
                    source: "log".into(),
                    event_type: kind.into(),
                    session_id: Some(sid.into()),
                    correlation_key: Some(sid.into()),
                    correlation_confidence: Some("exact".into()),
                    provider: Some("anthropic".into()),
                    model: Some(model.into()),
                    redaction_applied: Some(true),
                    os: Some(std::env::consts::OS.into()),
                    ..Event::default()
                });
            }
        }
    }
    events.into_values().collect()
}

fn unwrap_record(
    root: &Path,
    record: &Record,
    records: &HashMap<Vec<u8>, Record>,
) -> Result<Vec<u8>> {
    let mut c = Cursor(&record.value);
    c.varint()?; // IndexedDB record version, not the Blink/V8 version.
    let mut bytes = c.0.to_vec();
    if bytes.starts_with(&[255, 17, 1]) {
        let mut wrapper = Cursor(&bytes[3..]);
        let size = wrapper.varint()?;
        let blob_index = wrapper.varint()?;
        ensure!(
            wrapper.0.is_empty() && size <= MAX_BYTES as u64,
            "Invalid IndexedDB blob wrapper"
        );
        let (db, _, _, tail) = chromium_store::prefix(&record.key)?;
        let prefix_len = record.key.len() - tail.len();
        let mut key = record.key.clone();
        // The index ID has the same encoded width; change 1 (data) to 3 (blobs).
        let width = (key[0] & 3) as usize + 1;
        key[prefix_len - width] = 3;
        let meta = records
            .get(&key)
            .context("Claude conversation blob is not committed yet")?;
        let mut blobs = Cursor(&meta.value);
        let mut selected = None;
        for i in 0..=blob_index {
            let kind = blobs.take(1)?[0];
            match kind {
                0 | 1 => {
                    let number = blobs.varint()?;
                    let mime = string(&mut blobs)?;
                    let blob_size = blobs.varint()?;
                    if kind == 1 {
                        string(&mut blobs)?;
                        blobs.varint()?;
                    }
                    if i == blob_index {
                        ensure!(
                            kind == 0
                                && mime == "application/vnd.blink-idb-value-wrapper"
                                && blob_size == size,
                            "Unexpected Claude conversation blob"
                        );
                        selected = Some(number);
                    }
                }
                2 => {
                    let len = usize::try_from(blobs.varint()?)?;
                    blobs.take(len)?;
                }
                _ => bail!("Unknown IndexedDB external object"),
            }
        }
        let number = selected.context("Missing Claude conversation blob")?;
        // Chromium uses hexadecimal database ID / high-byte bucket / blob ID.
        let mut path = root.to_path_buf();
        for component in [
            format!("{ORIGIN}.blob"),
            format!("{db:x}"),
            format!("{:02x}", (number >> 8) & 255),
        ] {
            path.push(component);
            ensure!(
                fs::symlink_metadata(&path)?.file_type().is_dir(),
                "Expected a blob directory"
            );
        }
        path.push(format!("{number:x}"));
        bytes = chromium_store::read_bounded(&path)?;
        ensure!(
            bytes.len() as u64 == size,
            "Claude conversation blob changed during scan"
        );
    }
    if bytes.starts_with(&[255, 17, 2]) {
        ensure!(
            snap::raw::decompress_len(&bytes[3..])? <= MAX_BYTES,
            "Conversation exceeds decompression limit"
        );
        bytes = snap::raw::Decoder::new().decompress_vec(&bytes[3..])?;
    }
    Ok(bytes)
}

fn get<'a>(value: &'a V8Value, name: &str) -> Option<&'a V8Value> {
    let V8Value::Object(fields) = value else {
        return None;
    };
    fields
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
}
fn text<'a>(value: &'a V8Value, name: &str) -> Option<&'a str> {
    match get(value, name)? {
        V8Value::String(s) => Some(s),
        _ => None,
    }
}
fn id(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok()
}

/// Coarse, local text-only estimate: ASCII characters / 4, other Unicode
/// scalars / 1, rounded up per message. Not an Anthropic tokenizer. Never
/// persist text or claim hidden context, thinking, attachments or cache usage.
fn estimate_visible_text(message: &V8Value) -> Option<i64> {
    let V8Value::Array(blocks) = get(message, "content")? else {
        return None;
    };
    let mut units = 0i64;
    let mut found = false;
    for block in blocks {
        if text(block, "type") != Some("text") {
            continue;
        }
        if let Some(body) = text(block, "text") {
            found = true;
            units += body
                .chars()
                .map(|c| if c.is_ascii() { 1 } else { 4 })
                .sum::<i64>();
        }
    }
    found.then_some((units + 3) / 4)
}

/// Only observed metadata enters Event. Cached assistant messages are responses,
/// not evidence of an exact API-request count, cost, model, or token usage.
fn fingerprint_messages(value: &V8Value, events: &[Event], key: &str) -> Vec<Event> {
    let Some(V8Value::Array(messages)) = get(value, "tree").and_then(|v| get(v, "messages")) else {
        return vec![];
    };
    events
        .iter()
        .filter(|e| e.event_type == "message.received")
        .filter_map(|e| {
            let m = messages
                .iter()
                .find(|m| text(m, "uuid") == e.correlation_key.as_deref())?;
            let V8Value::Array(blocks) = get(m, "content")? else {
                return None;
            };
            let body = blocks
                .iter()
                .filter(|b| text(b, "type") == Some("text"))
                .filter_map(|b| text(b, "text"))
                .collect::<Vec<_>>()
                .join("");
            if body.is_empty() {
                return None;
            }
            let mut candidate = e.clone();
            candidate.event_type = "message.fingerprint".into();
            candidate.turn_id = e.correlation_key.clone();
            candidate.correlation_key =
                Some(crate::claude_plugin::response_fingerprint(&body, key));
            Some(candidate)
        })
        .collect()
}

fn normalize(value: &V8Value, host_id: &str) -> Vec<Event> {
    if text(value, "product") != Some("hub") {
        return vec![];
    }
    let Some(sid) = text(value, "conversationUuid").filter(|s| id(s)) else {
        return vec![];
    };
    let Some(tree) = get(value, "tree") else {
        return vec![];
    };
    if text(tree, "kind") != Some("hub_transcript") || text(tree, "conversationUuid") != Some(sid) {
        return vec![];
    }
    let Some(V8Value::Array(messages)) = get(tree, "messages") else {
        return vec![];
    };
    let mut events = vec![];
    for message in messages {
        let Some(mid) = text(message, "uuid").filter(|s| id(s)) else {
            continue;
        };
        let kind = match text(message, "sender") {
            Some("human") => "turn",
            Some("assistant") if text(message, "stop_reason").is_some() => "message.received",
            _ => continue,
        };
        let Some(ts) = text(message, "created_at")
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.timestamp_millis())
        else {
            continue;
        };
        let key = format!("claude-desktop:{sid}:{mid}");
        events.push(Event {
            event_id: kikimimi_schema::event_id(host_id, "log", kind, &key),
            ts,
            dt: kikimimi_schema::dt_of(ts),
            host_id: host_id.into(),
            agent: "claude".into(),
            client: Some(CLIENT.into()),
            source: "log".into(),
            event_type: kind.into(),
            session_id: Some(sid.into()),
            turn_id: if kind == "turn" {
                Some(mid.into())
            } else {
                text(message, "parent_message_uuid")
                    .filter(|s| id(s))
                    .map(str::to_owned)
            },
            correlation_key: Some(mid.into()),
            correlation_confidence: Some("exact".into()),
            provider: Some("anthropic".into()),
            os: Some(std::env::consts::OS.into()),
            query_source: Some("main".into()),
            redaction_applied: Some(true),
            ..Event::default()
        });
        let base = events.last().unwrap().clone();
        if kind == "turn"
            && matches!(get(message, "attachedFolders"), Some(V8Value::Array(f)) if !f.is_empty())
            && !events.iter().any(|e| e.event_type == FOLDER_ATTACHED)
        {
            events.push(folder_event(&base, host_id));
        }
        // A separate deterministic event backfills estimates for messages that
        // were already collected without rewriting or duplicating their turns.
        if let Some(tokens) = estimate_visible_text(message) {
            let mut estimate = base;
            estimate.event_type = "message.tokens.estimated".into();
            estimate.event_id =
                kikimimi_schema::event_id(host_id, "log", "message.tokens.estimated", &key);
            estimate.token_estimation_method = Some("visible_text_chars_v1".into());
            estimate.usage_source = Some("estimate".into());
            if kind == "turn" {
                estimate.estimated_input_tokens = Some(tokens);
            } else {
                estimate.estimated_output_tokens = Some(tokens);
            }
            events.push(estimate);
        }
    }
    events
}

/// Chat and Cowork are one conversation type now; a granted local folder is an
/// attribute of the conversation, not a separate app. One event per conversation,
/// dated at the turn it was first seen on. Paths and folder names are never kept.
fn folder_event(turn: &Event, host_id: &str) -> Event {
    let sid = turn.session_id.as_deref().unwrap_or_default();
    let mut folder = turn.clone();
    folder.event_type = FOLDER_ATTACHED.into();
    folder.event_id = kikimimi_schema::event_id(
        host_id,
        "log",
        FOLDER_ATTACHED,
        &format!("claude-desktop:{sid}:folder"),
    );
    folder
}

/// Conversation IDs the desktop app granted a local folder to. A grant can exist
/// without any message carrying `attachedFolders`. Only the keys are read.
fn folder_grants(config: &Path) -> HashSet<String> {
    let Ok(bytes) = fs::read(config) else {
        return HashSet::new();
    };
    let Ok(raw) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return HashSet::new();
    };
    raw.pointer("/preferences/remoteChatFolderGrants")
        .and_then(serde_json::Value::as_object)
        .map(|grants| grants.keys().filter(|k| id(k)).cloned().collect())
        .unwrap_or_default()
}

/// Adds the folder event for granted conversations whose messages did not show one,
/// dated at the conversation's first turn.
fn add_granted_folders(events: &mut Vec<Event>, grants: &HashSet<String>, host_id: &str) {
    let marked: HashSet<_> = events
        .iter()
        .filter(|e| e.event_type == FOLDER_ATTACHED)
        .filter_map(|e| e.session_id.clone())
        .collect();
    let mut first: HashMap<String, &Event> = HashMap::new();
    for e in events.iter().filter(|e| e.event_type == "turn") {
        let Some(sid) = e.session_id.as_ref() else { continue };
        if grants.contains(sid) && !marked.contains(sid) {
            let slot = first.entry(sid.clone()).or_insert(e);
            if e.ts < slot.ts {
                *slot = e;
            }
        }
    }
    let added: Vec<_> = first.values().map(|e| folder_event(e, host_id)).collect();
    events.extend(added);
}

#[cfg(test)]
mod tests {
    use super::*;
    use kikimimi_sink::{EventSink, FileSink};
    use std::io::Write;
    const FIRST: &[u8] = include_bytes!("../tests/fixtures/claude-desktop-hub.ssv");
    const NEXT: &[u8] = include_bytes!("../tests/fixtures/claude-desktop-hub-continued.ssv");

    /// Explicitly opt in to reading a live profile; never modifies that profile.
    #[test]
    #[ignore = "requires KIKIMIMI_CLAUDE_TEST_ROOT and KIKIMIMI_CLAUDE_TEST_SESSION"]
    fn live_cache_metadata() {
        let root = PathBuf::from(
            std::env::var_os("KIKIMIMI_CLAUDE_TEST_ROOT").expect("explicit cache root"),
        );
        let sid = std::env::var("KIKIMIMI_CLAUDE_TEST_SESSION").expect("explicit session ID");
        let events: Vec<_> = collect(&root, "verification")
            .unwrap()
            .into_iter()
            .filter(|e| e.session_id.as_deref() == Some(&sid))
            .collect();
        assert!(events.iter().any(|e| e.event_type == "turn"));
        assert!(events.iter().any(|e| e.event_type == "message.received"));
        for e in events {
            assert!(
                e.prompt_text.is_none()
                    && e.tool_input_json.is_none()
                    && e.tool_output_excerpt.is_none()
            );
            println!(
                "{} {} {} estimated_in={:?} estimated_out={:?} model={:?}",
                e.event_type,
                e.ts,
                sid,
                e.estimated_input_tokens,
                e.estimated_output_tokens,
                e.model
            );
        }
    }

    #[test]
    fn response_models_require_a_known_assistant_and_never_modify_usage() {
        let tree = deserialize_blink_with_limits(FIRST, V8Limits::default()).unwrap();
        let response = normalize(&tree, "host")
            .into_iter()
            .find(|e| e.event_type == "message.received")
            .unwrap();
        let sid = response.session_id.clone().unwrap();
        let mid = response.correlation_key.clone().unwrap();
        let org = text(&tree, "orgUuid").unwrap().to_owned();
        let mut value = V8Value::Object(vec![
            ("version".into(), V8Value::Int(1)),
            ("orgUuid".into(), V8Value::String(org.clone())),
            ("conversationUuid".into(), V8Value::String(sid.clone())),
            ("messageUuid".into(), V8Value::String(mid.clone())),
            ("model".into(), V8Value::String("claude-haiku-test".into())),
            (
                "modelSource".into(),
                V8Value::String("active_turn_selection".into()),
            ),
        ]);
        let conversations = HashMap::from([(sid.clone(), org)]);
        let responses = HashMap::from([((sid, mid), response)]);
        let e = normalize_response_model(&value, &conversations, &responses, "host").unwrap();
        assert_eq!(e.event_type, "message.model.active_turn_selection");
        assert!(
            e.input_tokens.is_none() && e.estimated_input_tokens.is_none() && e.cost_usd.is_none()
        );
        assert!(
            normalize_response_model(&value, &conversations, &HashMap::new(), "host").is_none()
        );
        if let V8Value::Object(f) = &mut value {
            f.iter_mut().find(|(k, _)| k == "modelSource").unwrap().1 =
                V8Value::String("guessed".into());
        }
        assert!(normalize_response_model(&value, &conversations, &responses, "host").is_none());
    }

    #[test]
    fn a_granted_folder_marks_the_turn_without_keeping_its_path() {
        fn field<'a>(v: &'a mut V8Value, name: &str) -> &'a mut V8Value {
            let V8Value::Object(fields) = v else { panic!("object") };
            &mut fields.iter_mut().find(|(k, _)| k == name).unwrap().1
        }
        let mut tree = deserialize_blink_with_limits(FIRST, V8Limits::default()).unwrap();
        let before = normalize(&tree, "host");
        assert!(before.iter().all(|e| e.event_type != FOLDER_ATTACHED));
        let V8Value::Array(messages) = field(field(&mut tree, "tree"), "messages") else {
            panic!("messages")
        };
        let human = messages
            .iter_mut()
            .find(|m| text(m, "sender") == Some("human"))
            .unwrap();
        let V8Value::Object(fields) = human else { panic!("object") };
        fields.push((
            "attachedFolders".into(),
            V8Value::Array(vec![V8Value::Object(vec![
                ("path".into(), V8Value::String("/Users/SECRET/work".into())),
                ("displayName".into(), V8Value::String("PRIVATE".into())),
            ])]),
        ));
        let after = normalize(&tree, "host");
        let folders: Vec<_> = after.iter().filter(|e| e.event_type == FOLDER_ATTACHED).collect();
        assert_eq!(folders.len(), 1);
        let turn = after.iter().find(|e| e.event_type == "turn").unwrap();
        assert_eq!(folders[0].turn_id, turn.turn_id);
        assert_eq!(folders[0].client.as_deref(), Some(CLIENT));
        assert_ne!(folders[0].event_id, turn.event_id);
        // The other events are unchanged, so earlier collections are not duplicated.
        let rest: Vec<_> = after.iter().filter(|e| e.event_type != FOLDER_ATTACHED).collect();
        assert_eq!(rest, before.iter().collect::<Vec<_>>());
        let json = serde_json::to_string(&after).unwrap();
        assert!(!json.contains("SECRET") && !json.contains("PRIVATE"));

        // A grant alone marks the conversation once, with the same identity.
        let sid = turn.session_id.clone().unwrap();
        let mut granted = before.clone();
        add_granted_folders(&mut granted, &HashSet::from([sid.clone()]), "host");
        let added: Vec<_> = granted.iter().filter(|e| e.event_type == FOLDER_ATTACHED).collect();
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].event_id, folders[0].event_id);
        let mut both = after.clone();
        add_granted_folders(&mut both, &HashSet::from([sid]), "host");
        assert_eq!(both.len(), after.len());
        let mut other = before.clone();
        add_granted_folders(&mut other, &HashSet::from(["someone-else".into()]), "host");
        assert_eq!(other, before);
    }

    #[test]
    fn folder_grants_read_only_valid_conversation_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let config = tmp.path().join("claude_desktop_config.json");
        assert!(folder_grants(&config).is_empty());
        fs::write(&config, br#"{"preferences":{"remoteChatFolderGrants":{
            "b14f5f23-e922-4baf-8fe4-b43ba7e35a18":{"folders":["/Users/SECRET"]},
            "../bad id":{}}}}"#)
        .unwrap();
        assert_eq!(
            folder_grants(&config),
            HashSet::from(["b14f5f23-e922-4baf-8fe4-b43ba7e35a18".to_owned()])
        );
        fs::write(&config, b"not json").unwrap();
        assert!(folder_grants(&config).is_empty());
    }

    // Synthetic data only; exercises padding before UTF-16 strings and nested values.
    fn encode_json(v: &serde_json::Value, out: &mut Vec<u8>) {
        use serde_json::Value;
        match v {
            Value::String(s) => {
                out.extend([0, b'c']);
                varint((s.encode_utf16().count() * 2) as u64, out);
                for c in s.encode_utf16() {
                    out.extend(c.to_le_bytes());
                }
            }
            Value::Object(o) => {
                out.push(b'o');
                for (k, v) in o {
                    encode_json(&Value::String(k.clone()), out);
                    encode_json(v, out);
                }
                out.extend([0, b'{']);
                varint(o.len() as u64, out);
            }
            Value::Array(a) => {
                out.push(b'A');
                varint(a.len() as u64, out);
                for v in a {
                    encode_json(v, out);
                }
                out.extend([0, b'$']);
                varint(0, out);
                varint(a.len() as u64, out);
            }
            Value::Null => out.push(b'0'),
            _ => panic!("unsupported synthetic value"),
        }
    }

    #[test]
    fn automatically_backfills_scoped_models_without_usage_or_duplicate_turns() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("IndexedDB");
        let data = tmp.path().join("events");
        let mut first = FIRST.to_vec();
        let old = b"\x22\x0bPRIVATE org";
        let at = first.windows(old.len()).position(|w| w == old).unwrap();
        first.splice(
            at..at + old.len(),
            b"\x22\x2422222222-2222-4222-8222-222222222222"
                .iter()
                .copied(),
        );
        let dir = fixture(&root, &first);
        let tree = deserialize_blink_with_limits(&first, V8Limits::default()).unwrap();
        let sid = text(&tree, "conversationUuid").unwrap();
        let org = text(&tree, "orgUuid").unwrap();
        let row = serde_json::json!({"uuid":sid,"model":"claude-haiku-test","updated_at":"2026-11-01T06:51:14Z","name":"PRIVATE テスト"});
        let query = serde_json::json!({"queryKey":["chat_conversation_list",{"orgUuid":org}],"state":{"data":{"pages":[{"data":[row.clone(),row]}]}}});
        let cache = serde_json::json!({"clientState":{"queries":[query]}});
        let mut encoded = vec![1, 255, 21, 255, 16];
        encode_json(&cache, &mut encoded);
        let mut dbkey = vec![0, 0, 0, 0, 201];
        encoded_string("https_claude.ai_0@1", &mut dbkey);
        encoded_string("keyval-store", &mut dbkey);
        let mut cachekey = vec![0, 7, 2, 1, 1];
        encoded_string("react-query-cache", &mut cachekey);
        let storename: Vec<_> = "keyval".encode_utf16().flat_map(u16::to_be_bytes).collect();
        let mut sink = FileSink::new(data.clone(), "host".into(), 500, Duration::from_secs(1));
        for e in collect(&root, "host").unwrap() {
            sink.push(e);
        }
        sink.flush().unwrap();
        write_batch(
            &dir,
            20,
            vec![
                (dbkey, Some(vec![7])),
                (vec![0, 7, 0, 0, 50, 2, 0], Some(storename)),
                (cachekey.clone(), Some(encoded)),
            ],
        );
        let models = tailer(&root, &data).scan_and_drain().unwrap().unwrap();
        assert_eq!(models.len(), 1);
        let e = &models[0];
        assert_eq!(e.model.as_deref(), Some("claude-haiku-test"));
        assert_eq!(e.event_type, "conversation.model.observed");
        assert!(
            e.turn_id.is_none()
                && e.input_tokens.is_none()
                && e.estimated_input_tokens.is_none()
                && e.cost_usd.is_none()
        );
        assert!(!serde_json::to_string(&models).unwrap().contains("PRIVATE"));
        for e in models {
            sink.push(e);
        }
        sink.flush().unwrap();
        assert!(tailer(&root, &data)
            .scan_and_drain()
            .unwrap()
            .unwrap()
            .is_empty());
        // A broken optional cache does not stop collection of messages.
        write_batch(
            &dir,
            30,
            vec![(cachekey, Some(vec![1, 255, 21, 255, 16, 255]))],
        );
        assert_eq!(collect(&root, "host").unwrap().len(), 4);
        let decoded = {
            let mut b = vec![255, 21, 255, 16];
            encode_json(&cache, &mut b);
            deserialize_blink_with_limits(&b, V8Limits::default()).unwrap()
        };
        assert!(normalize_models(
            &decoded,
            &HashMap::from([(sid.into(), "wrong-org".into())]),
            "host"
        )
        .is_empty());
        assert!(normalize_models(&decoded, &HashMap::new(), "host").is_empty());
    }

    fn varint(mut n: u64, out: &mut Vec<u8>) {
        while n >= 128 {
            out.push((n as u8) | 128);
            n >>= 7;
        }
        out.push(n as u8);
    }
    fn encoded_string(s: &str, out: &mut Vec<u8>) {
        varint(s.encode_utf16().count() as u64, out);
        for c in s.encode_utf16() {
            out.extend(c.to_be_bytes());
        }
    }
    fn physical(data: &[u8]) -> Vec<u8> {
        assert!(data.len() + 7 < 32768);
        let crc = crc32c::crc32c_append(crc32c::crc32c(&[1]), data)
            .rotate_left(17)
            .wrapping_add(0xa282_ead8);
        let mut out = crc.to_le_bytes().to_vec();
        out.extend((data.len() as u16).to_le_bytes());
        out.push(1);
        out.extend(data);
        out
    }
    fn key(index: u8) -> Vec<u8> {
        vec![0, 9, 4, index, 1, 1, 0, b'k']
    }
    fn write_batch(dir: &Path, seq: u64, ops: Vec<(Vec<u8>, Option<Vec<u8>>)>) {
        let mut batch = seq.to_le_bytes().to_vec();
        batch.extend((ops.len() as u32).to_le_bytes());
        for (key, value) in ops {
            batch.push(u8::from(value.is_some()));
            varint(key.len() as u64, &mut batch);
            batch.extend(key);
            if let Some(value) = value {
                varint(value.len() as u64, &mut batch);
                batch.extend(value);
            }
        }
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("000004.log"))
            .unwrap()
            .write_all(&physical(&batch))
            .unwrap();
    }
    fn fixture(root: &Path, bytes: &[u8]) -> PathBuf {
        let dir = root.join(format!("{ORIGIN}.leveldb"));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("CURRENT"), "MANIFEST-000001\n").unwrap();
        // Add then delete table 5: its leftover file must never be carved.
        fs::write(
            dir.join("MANIFEST-000001"),
            physical(&[2, 4, 7, 0, 5, 0, 0, 0, 6, 0, 5]),
        )
        .unwrap();
        fs::write(dir.join("000005.ldb"), "obsolete invalid table").unwrap();
        let mut dbkey = vec![0, 0, 0, 0, 201];
        encoded_string("https_claude.ai_0@1", &mut dbkey);
        encoded_string("claude-conversation-store", &mut dbkey);
        let mut store_name = vec![];
        for c in "trees".encode_utf16() {
            store_name.extend(c.to_be_bytes());
        }
        let mut value = vec![1];
        value.extend(bytes);
        write_batch(
            &dir,
            1,
            vec![
                (dbkey, Some(vec![9])),
                (vec![0, 9, 0, 0, 50, 4, 0], Some(store_name)),
                (key(1), Some(value)),
            ],
        );
        dir
    }
    fn tailer(root: &Path, data: &Path) -> ClaudeDesktopTailer {
        let mut tailer = ClaudeDesktopTailer::new(data.into(), "host".into());
        tailer.root = root.into();
        tailer.plugin_inbox = root.join("test-plugin-inbox");
        tailer
    }
    #[test]
    fn fingerprints_are_ephemeral_and_use_assistant_message_identity() {
        let tmp = tempfile::tempdir().unwrap();
        fixture(tmp.path(), FIRST);
        let events = collect_with_key(tmp.path(), "host", Some(&"a".repeat(64))).unwrap();
        let candidates: Vec<_> = events
            .iter()
            .filter(|e| e.event_type == "message.fingerprint")
            .collect();
        assert_eq!(candidates.len(), 1);
        let assistant = events
            .iter()
            .find(|e| e.event_type == "message.received")
            .unwrap();
        assert_eq!(candidates[0].turn_id, assistant.correlation_key);
        assert_eq!(candidates[0].correlation_key.as_ref().unwrap().len(), 64);
        assert!(candidates[0].prompt_text.is_none());
        let mut t = tailer(tmp.path(), &tmp.path().join("events"));
        assert!(t
            .scan_and_drain()
            .unwrap()
            .unwrap()
            .iter()
            .all(|e| e.event_type != "message.fingerprint"));
    }

    #[test]
    fn collects_live_messages_without_bodies_guessed_models_or_restart_duplicates() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("IndexedDB");
        let data = tmp.path().join("events");
        let dir = fixture(&root, FIRST);
        let mut t = tailer(&root, &data);
        let events = t.scan_and_drain().unwrap().unwrap();
        assert_eq!(events.len(), 4);
        let base: Vec<_> = events
            .iter()
            .filter(|e| e.event_type != "message.tokens.estimated")
            .collect();
        assert_eq!(base[0].event_type, "turn");
        assert_eq!(base[1].event_type, "message.received");
        assert_eq!(base[1].turn_id, base[0].correlation_key);
        assert!(events.iter().all(|e| e.client.as_deref() == Some(CLIENT)
            && e.model.is_none()
            && e.input_tokens.is_none()
            && e.output_tokens.is_none()
            && e.cost_usd.is_none()));
        let json = serde_json::to_string(&events).unwrap();
        assert!(!json.contains("SECRET"));
        assert!(!json.contains("PRIVATE"));
        let mut sink = FileSink::new(data.clone(), "host".into(), 500, Duration::from_secs(1));
        for e in events {
            sink.push(e);
        }
        sink.flush().unwrap();
        assert!(t.scan_and_drain().unwrap().is_none());
        let mut t = tailer(&root, &data);
        assert!(t.scan_and_drain().unwrap().unwrap().is_empty());
        let mut value = vec![2];
        value.extend(NEXT);
        write_batch(&dir, 10, vec![(key(1), Some(value))]);
        t.last_scan = None;
        let next = t.scan_and_drain().unwrap().unwrap();
        assert_eq!(next.len(), 4);
        assert!(next.iter().all(|e| e.ts >= 1793515920000));
        write_batch(&dir, 11, vec![(key(1), None)]);
        t.last_scan = None;
        assert!(t.scan_and_drain().unwrap().unwrap().is_empty());
    }
    #[test]
    fn backfills_only_estimates_for_previously_collected_messages() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("IndexedDB");
        let data = tmp.path().join("events");
        fixture(&root, FIRST);
        let mut sink = FileSink::new(data.clone(), "host".into(), 500, Duration::from_secs(1));
        for e in collect(&root, "host")
            .unwrap()
            .into_iter()
            .filter(|e| e.event_type != "message.tokens.estimated")
        {
            sink.push(e);
        }
        sink.flush().unwrap();
        let estimates = tailer(&root, &data).scan_and_drain().unwrap().unwrap();
        assert_eq!(estimates.len(), 2);
        assert!(estimates
            .iter()
            .all(|e| e.event_type == "message.tokens.estimated"
                && e.input_tokens.is_none()
                && e.output_tokens.is_none()));
        assert_eq!(
            estimates
                .iter()
                .filter_map(|e| e.estimated_input_tokens)
                .sum::<i64>(),
            4
        );
        assert_eq!(
            estimates
                .iter()
                .filter_map(|e| e.estimated_output_tokens)
                .sum::<i64>(),
            3
        );
    }

    #[test]
    fn text_estimate_handles_unicode_empty_and_missing_text() {
        let message = |body: &str| {
            V8Value::Object(vec![(
                "content".into(),
                V8Value::Array(vec![V8Value::Object(vec![
                    ("type".into(), V8Value::String("text".into())),
                    ("text".into(), V8Value::String(body.into())),
                ])]),
            )])
        };
        assert_eq!(estimate_visible_text(&message("テスト")), Some(3));
        assert_eq!(estimate_visible_text(&message("OK")), Some(1));
        assert_eq!(estimate_visible_text(&message("")), Some(0));
        assert_eq!(estimate_visible_text(&V8Value::Undefined), None);
    }

    #[test]
    fn reads_snappy_and_only_the_referenced_blob_and_retries_after_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("IndexedDB");
        let data = tmp.path().join("events");
        let mut compressed = vec![255, 17, 2];
        compressed.extend(snap::raw::Encoder::new().compress_vec(FIRST).unwrap());
        let dir = fixture(&root, &compressed);
        assert_eq!(collect(&root, "host").unwrap().len(), 4);
        let blob = root.join(format!("{ORIGIN}.blob/9/01/12c"));
        fs::create_dir_all(blob.parent().unwrap()).unwrap();
        let mut wrapper = vec![1, 255, 17, 1];
        varint(compressed.len() as u64, &mut wrapper);
        varint(0, &mut wrapper);
        let mut meta = vec![0];
        varint(300, &mut meta);
        encoded_string("application/vnd.blink-idb-value-wrapper", &mut meta);
        varint(compressed.len() as u64, &mut meta);
        write_batch(
            &dir,
            20,
            vec![(key(1), Some(wrapper)), (key(3), Some(meta))],
        );
        let mut t = tailer(&root, &data);
        assert!(t.scan_and_drain().is_err());
        fs::write(&blob, &compressed).unwrap();
        fs::write(
            blob.parent().unwrap().join("12d"),
            b"unreferenced SECRET blob",
        )
        .unwrap();
        t.last_scan = None;
        assert_eq!(t.scan_and_drain().unwrap().unwrap().len(), 4);
        let mut bad = vec![255, 17, 2];
        varint(MAX_BYTES as u64 + 1, &mut bad);
        let mut record = vec![1];
        record.extend(bad);
        write_batch(&dir, 30, vec![(key(1), Some(record))]);
        assert!(collect(&root, "host")
            .unwrap_err()
            .to_string()
            .contains("decompression limit"));
    }
    #[test]
    fn excludes_streaming_replies_unknown_products_and_malformed_times() {
        let mut v = deserialize_blink_with_limits(FIRST, V8Limits::default()).unwrap();
        fn set(v: &mut V8Value, k: &str, value: V8Value) {
            let V8Value::Object(fields) = v else { panic!() };
            fields.iter_mut().find(|(key, _)| key == k).unwrap().1 = value;
        }
        let V8Value::Object(fields) = &mut v else {
            panic!()
        };
        let tree = &mut fields.iter_mut().find(|(k, _)| k == "tree").unwrap().1;
        let V8Value::Object(fields) = tree else {
            panic!()
        };
        let V8Value::Array(messages) =
            &mut fields.iter_mut().find(|(k, _)| k == "messages").unwrap().1
        else {
            panic!()
        };
        set(&mut messages[1], "stop_reason", V8Value::Undefined);
        set(
            &mut messages[0],
            "created_at",
            V8Value::String("invalid".into()),
        );
        assert!(normalize(&v, "host").is_empty());
        set(&mut v, "product", V8Value::String("code".into()));
        assert!(normalize(&v, "host").is_empty());
    }
}
