//! Application attribution from observed session metadata, never from version guesses.
use kikimimi_schema::Event;
use serde_json::Value;
use std::{
    collections::HashMap,
    fs,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

pub const COWORK: &str = "claude_desktop_cowork";

pub fn claude_support_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join("Library/Application Support/Claude")
}

#[derive(Default)]
struct Registry(HashMap<String, String>);

impl Registry {
    fn transcript(&mut self, path: &Path, cowork: bool) {
        visit_records(path, |raw| {
            let Some(sid) = raw.get("sessionId").and_then(Value::as_str) else {
                return false;
            };
            let client = if cowork {
                Some(COWORK)
            } else {
                raw.get("entrypoint")
                    .and_then(Value::as_str)
                    .and_then(kikimimi_schema::claude_client)
            };
            if let Some(client) = client {
                self.0.insert(sid.to_owned(), client.to_owned());
                true
            } else {
                false
            }
        });
    }

    fn discover(claude: &Path, codex: &Path, support: &Path, cowork: &Path) -> Self {
        let mut registry = Self::default();
        for path in crate::claude_backfill::discover_files(claude) {
            registry.transcript(&path, false);
        }
        // Desktop's explicit CLI-session link also identifies old hook/OTel-only history.
        walk(&support.join("claude-code-sessions"), 4, &mut |path| {
            if path.extension().and_then(|x| x.to_str()) != Some("json") {
                return;
            }
            if !fs::metadata(path).is_ok_and(|m| m.len() <= 256 * 1024) {
                return;
            }
            let Ok(bytes) = fs::read(path) else { return };
            let Ok(raw) = serde_json::from_slice::<Value>(&bytes) else {
                return;
            };
            if let Some(id) = raw.get("cliSessionId").and_then(Value::as_str) {
                registry
                    .0
                    .entry(id.into())
                    .or_insert_with(|| "claude_desktop_code".into());
            }
        });
        walk(codex, 5, &mut |path| {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if !name.starts_with("rollout-")
                || !(name.ends_with(".jsonl") || name.ends_with(".jsonl.zst"))
            {
                return;
            }
            visit_records(path, |raw| {
                if raw.get("type").and_then(Value::as_str) != Some("session_meta") {
                    return false;
                }
                let p = &raw["payload"];
                let sid = p
                    .get("session_id")
                    .or_else(|| p.get("id"))
                    .and_then(Value::as_str);
                let client = p
                    .get("originator")
                    .and_then(Value::as_str)
                    .and_then(kikimimi_schema::codex_client);
                if let (Some(sid), Some(client)) = (sid, client) {
                    registry.0.insert(sid.into(), client.into());
                }
                true
            });
        });
        for projects in crate::cowork_tailer::projects_dirs(cowork) {
            for path in crate::claude_backfill::discover_files(&projects) {
                registry.transcript(&path, true);
            }
        }
        registry
    }
}

// Never follow symlinks out of the known metadata directories.
fn walk(root: &Path, depth: usize, visit: &mut impl FnMut(&Path)) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            walk(&entry.path(), depth - 1, visit)
        } else if kind.is_file() {
            visit(&entry.path())
        }
    }
}

fn visit_records(path: &Path, mut visit: impl FnMut(&Value) -> bool) {
    let Ok(file) = fs::File::open(path) else {
        return;
    };
    let reader: Box<dyn Read> = if path.extension().and_then(|x| x.to_str()) == Some("zst") {
        let Ok(reader) = zstd::stream::read::Decoder::new(file) else {
            return;
        };
        Box::new(reader)
    } else {
        Box::new(file)
    };
    for line in BufReader::new(reader.take(256 * 1024)).lines().take(256) {
        let Ok(line) = line else { break };
        if let Ok(raw) = serde_json::from_str::<Value>(&line) {
            if visit(&raw) {
                break;
            }
        }
    }
}

pub fn for_session(sid: &str) -> Option<String> {
    struct Cached {
        roots: Vec<PathBuf>,
        at: Instant,
        registry: Registry,
    }
    static CACHE: OnceLock<Mutex<Option<Cached>>> = OnceLock::new();
    let roots = vec![
        kikimimi_schema::paths::claude_projects_dir(),
        kikimimi_schema::paths::codex_sessions_dir(),
        claude_support_dir(),
        crate::cowork_tailer::sessions_dir(),
    ];
    let mut cache = CACHE.get_or_init(|| Mutex::new(None)).lock().ok()?;
    if cache
        .as_ref()
        .is_none_or(|c| c.roots != roots || c.at.elapsed() >= Duration::from_secs(10))
    {
        let registry = Registry::discover(&roots[0], &roots[1], &roots[2], &roots[3]);
        *cache = Some(Cached {
            roots,
            at: Instant::now(),
            registry,
        });
    }
    cache.as_ref()?.registry.0.get(sid).cloned()
}

pub fn enrich(event: &mut Event) {
    if event.client.is_none() {
        event.client = event.session_id.as_deref().and_then(for_session);
    }
}

/// Old local history can be identified without rewriting Parquet. Never label another
/// machine's S3 events from this machine's metadata; absence stays unknown.
pub fn historical(sid: &str, host: &str) -> Option<String> {
    let own_host =
        fs::read_to_string(kikimimi_schema::paths::kikimimi_dir().join("host_id")).ok()?;
    if own_host.trim() != host {
        return None;
    }
    for_session(sid)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uses_observed_entrypoints_and_canonical_codex_session_not_clone_id() {
        let d = tempfile::tempdir().unwrap();
        let claude = d.path().join("claude/proj");
        fs::create_dir_all(&claude).unwrap();
        fs::write(
            claude.join("cli.jsonl"),
            "{\"sessionId\":\"cli\",\"entrypoint\":\"cli\"}\n",
        )
        .unwrap();
        fs::write(
            claude.join("desktop.jsonl"),
            "{\"sessionId\":\"desktop\",\"entrypoint\":\"claude-desktop\"}\n",
        )
        .unwrap();
        fs::write(
            claude.join("unknown.jsonl"),
            "{\"sessionId\":\"unknown\",\"version\":\"2.1\"}\n",
        )
        .unwrap();
        let codex = d.path().join("codex");
        fs::create_dir(&codex).unwrap();
        fs::write(codex.join("rollout-test.jsonl"), "{\"type\":\"session_meta\",\"payload\":{\"session_id\":\"canonical\",\"id\":\"clone\",\"originator\":\"Codex Desktop\"}}\n").unwrap();
        let r = Registry::discover(
            &d.path().join("claude"),
            &codex,
            &d.path().join("support"),
            &d.path().join("cowork"),
        );
        assert_eq!(r.0.get("cli").map(String::as_str), Some("claude_code_cli"));
        assert_eq!(
            r.0.get("desktop").map(String::as_str),
            Some("claude_desktop_code")
        );
        assert_eq!(
            r.0.get("canonical").map(String::as_str),
            Some("codex_desktop")
        );
        assert!(!r.0.contains_key("clone"));
        assert!(!r.0.contains_key("unknown"));
    }
}
