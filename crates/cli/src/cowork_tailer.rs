//! Cowork's VM transcripts are outside ~/.claude. Read only its known projects
//! layout; never the duplicate audit stream, prompts, or arbitrary VM files.
use kikimimi_adapter_claude::TranscriptNormalizer;
use kikimimi_schema::Event;
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

const MAX_LINE: u64 = 8 * 1024 * 1024;
const SCAN_BYTES: u64 = 8 * 1024 * 1024;

pub fn sessions_dir() -> PathBuf {
    std::env::var_os("KIKIMIMI_COWORK_SESSIONS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            crate::client_source::claude_support_dir().join("local-agent-mode-sessions")
        })
}

pub fn projects_dirs(root: &Path) -> Vec<PathBuf> {
    fn directories(path: &Path) -> Vec<PathBuf> {
        fs::read_dir(path)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.path())
            .collect()
    }
    let mut out = vec![];
    for account in directories(root) {
        for org in directories(&account) {
            for session in directories(&org) {
                if !session
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .starts_with("local_")
                {
                    continue;
                }
                let claude = session.join(".claude");
                let projects = claude.join("projects");
                if fs::symlink_metadata(&claude).is_ok_and(|m| m.file_type().is_dir())
                    && fs::symlink_metadata(&projects).is_ok_and(|m| m.file_type().is_dir())
                {
                    out.push(projects);
                }
            }
        }
    }
    out
}

struct FileState {
    identity: (u64, u64),
    offset: u64,
    normalizer: TranscriptNormalizer,
    discarding: bool,
}

pub struct CoworkTailer {
    root: PathBuf,
    data_dir: PathBuf,
    host_id: String,
    files: HashMap<PathBuf, FileState>,
    seen: Option<HashSet<String>>,
}

impl CoworkTailer {
    pub fn new(data_dir: PathBuf, host_id: String) -> Self {
        Self {
            root: sessions_dir(),
            data_dir,
            host_id,
            files: HashMap::new(),
            seen: None,
        }
    }

    /// Replay normalizer context after restart, but emit only IDs not already
    /// durable in Parquet. Cursors remain in memory: a crash before flush cannot
    /// permanently consume an event. FileSink retains buffered events on failure.
    pub fn scan_and_drain(&mut self) -> anyhow::Result<Vec<Event>> {
        if self.seen.is_none() {
            self.seen = Some(kikimimi_sink::client_event_ids(
                &self.data_dir,
                crate::client_source::COWORK,
            )?);
        }
        let seen = self.seen.as_mut().unwrap();
        let mut out = vec![];
        let mut paths = vec![];
        for projects in projects_dirs(&self.root) {
            paths.extend(crate::claude_backfill::discover_files(&projects));
        }
        paths.sort();
        self.files
            .retain(|path, _| paths.binary_search(path).is_ok());
        for path in paths {
            if !fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file()) {
                continue;
            }
            let Ok(mut file) = fs::File::open(&path) else {
                continue;
            };
            let Ok(metadata) = file.metadata() else {
                continue;
            };
            let state = self.files.entry(path).or_insert_with(|| FileState {
                identity: (metadata.dev(), metadata.ino()),
                offset: 0,
                normalizer: TranscriptNormalizer::new(self.host_id.clone()),
                discarding: false,
            });
            if metadata.len() < state.offset || state.identity != (metadata.dev(), metadata.ino()) {
                *state = FileState {
                    identity: (metadata.dev(), metadata.ino()),
                    offset: 0,
                    normalizer: TranscriptNormalizer::new(self.host_id.clone()),
                    discarding: false,
                };
            }
            if file.seek(SeekFrom::Start(state.offset)).is_err() {
                continue;
            }
            let mut reader = BufReader::new(file);
            let start = state.offset;
            while state.offset - start < SCAN_BYTES {
                let mut bytes = Vec::new();
                let Ok(n) = Read::take(&mut reader, MAX_LINE + 1).read_until(b'\n', &mut bytes)
                else {
                    break;
                };
                if n == 0 {
                    break;
                }
                let complete = bytes.last() == Some(&b'\n');
                if !complete && n as u64 <= MAX_LINE && !state.discarding {
                    // Writer hasn't finished this JSON line. Retry it next scan.
                    break;
                }
                state.offset += n as u64;
                if state.discarding || n as u64 > MAX_LINE {
                    state.discarding = !complete;
                    continue;
                }
                let Ok(raw) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                    continue;
                };
                for mut event in state.normalizer.line(&raw) {
                    event.client = Some(crate::client_source::COWORK.into());
                    if seen.insert(event.event_id.clone()) {
                        out.push(event)
                    }
                }
            }
            // EOF is not a session.end: an active Cowork VM can append later.
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kikimimi_sink::{EventSink, FileSink};
    use std::{io::Write, time::Duration};

    fn tailer(root: &Path, data: &Path) -> CoworkTailer {
        let mut t = CoworkTailer::new(data.into(), "host".into());
        t.root = root.into();
        t
    }
    #[test]
    fn collects_history_live_appends_and_restart_without_bodies_or_duplicates() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("sessions");
        let data = d.path().join("data");
        let projects = root.join("account/org/local_one/.claude/projects/proj");
        fs::create_dir_all(&projects).unwrap();
        let path = projects.join("session.jsonl");
        let user = serde_json::json!({"type":"user","sessionId":"cowork-session","uuid":"user","timestamp":"2026-10-30T00:00:00Z","message":{"content":"SECRET prompt"}});
        let assistant = serde_json::json!({"type":"assistant","sessionId":"cowork-session","uuid":"assistant","timestamp":"2026-10-30T00:00:01Z","requestId":"request","message":{"id":"msg","model":"claude-test","usage":{"input_tokens":4,"output_tokens":2},"content":[{"type":"tool_use","id":"tool-1","name":"Bash","input":{"command":"SECRET command"}}]}});
        fs::write(&path, format!("{user}\n{assistant}\n")).unwrap();
        let mut t = tailer(&root, &data);
        let events = t.scan_and_drain().unwrap();
        assert!(events.iter().any(|e| e.event_type == "tool.call"));
        assert!(events.iter().any(|e| e.event_type == "api.request"));
        assert!(!events.iter().any(|e| e.event_type == "session.end"));
        assert!(!serde_json::to_string(&events).unwrap().contains("SECRET"));
        assert!(events
            .iter()
            .all(|e| e.client.as_deref() == Some(crate::client_source::COWORK)));
        let count = events.len();
        // A crash before FileSink flush must replay the uncommitted records.
        assert_eq!(tailer(&root, &data).scan_and_drain().unwrap(), events);
        let mut sink = FileSink::new(data.clone(), "host".into(), 500, Duration::from_secs(30));
        for e in events {
            sink.push(e)
        }
        sink.flush().unwrap();
        assert!(t.scan_and_drain().unwrap().is_empty());
        assert!(tailer(&root, &data).scan_and_drain().unwrap().is_empty());
        assert_eq!(
            kikimimi_sink::client_event_ids(&data, crate::client_source::COWORK)
                .unwrap()
                .len(),
            count
        );
        let result = serde_json::json!({"type":"user","sessionId":"cowork-session","timestamp":"2026-10-30T00:00:03Z","message":{"content":[{"type":"tool_result","tool_use_id":"tool-1","content":"SECRET output","is_error":false}]}}).to_string();
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        write!(file, "{}", &result[..result.len() / 2]).unwrap();
        assert!(t.scan_and_drain().unwrap().is_empty());
        writeln!(file, "{}", &result[result.len() / 2..]).unwrap();
        let events = t.scan_and_drain().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "tool.result");
        assert_eq!(events[0].duration_ms, Some(2000));
        assert!(!serde_json::to_string(&events).unwrap().contains("SECRET"));
    }

    #[test]
    fn unreadable_history_blocks_replay_and_audit_is_not_a_transcript() {
        let d = tempfile::tempdir().unwrap();
        let data = d.path().join("data/dt=2026-10-30");
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("broken.parquet"), "broken").unwrap();
        assert!(tailer(d.path(), &d.path().join("data"))
            .scan_and_drain()
            .is_err());
        let root = d.path().join("sessions/account/org/local_one");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("audit.jsonl"), "{\"sessionId\":\"audit\"}\n").unwrap();
        assert!(projects_dirs(&d.path().join("sessions")).is_empty());
    }

    #[test]
    fn replacement_and_truncation_reset_context_but_symlinks_are_not_followed() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("sessions");
        let projects = root.join("account/org/local_one/.claude/projects/proj");
        fs::create_dir_all(&projects).unwrap();
        let path = projects.join("session.jsonl");
        let user = |id: &str| {
            format!("{{\"type\":\"user\",\"sessionId\":\"{id}\",\"uuid\":\"{id}\",\"timestamp\":\"2026-10-30T00:00:00Z\",\"message\":{{\"content\":\"test\"}}}}\n")
        };
        fs::write(&path, user("first-session")).unwrap();
        let mut t = tailer(&root, &d.path().join("data"));
        assert!(!t.scan_and_drain().unwrap().is_empty());
        let replacement = d.path().join("replacement");
        fs::write(&replacement, user("other-session")).unwrap();
        fs::rename(replacement, &path).unwrap();
        let events = t.scan_and_drain().unwrap();
        assert!(!events.is_empty());
        assert!(events
            .iter()
            .all(|e| e.session_id.as_deref() == Some("other-session")));
        fs::write(&path, user("short")).unwrap();
        assert!(t
            .scan_and_drain()
            .unwrap()
            .iter()
            .any(|e| e.session_id.as_deref() == Some("short")));
        fs::remove_file(&path).unwrap();
        let outside = d.path().join("outside.jsonl");
        fs::write(&outside, user("outside")).unwrap();
        std::os::unix::fs::symlink(outside, path).unwrap();
        assert!(t.scan_and_drain().unwrap().is_empty());
    }
}
