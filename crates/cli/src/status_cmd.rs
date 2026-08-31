//! `kikimimi status` — architecture.md §4「状態表示」。
//!
//! 収集対象 (hooks / env)、デーモンの生死、state.json の集計、spool 滞留、
//! data dir のサイズ、既知の健全性警告 (Windows #46204 の類似チェック含む) を表示する。

use std::path::Path;

use serde_json::Value;

use crate::claude_settings as cs;
use crate::state::AgentState;

/// spool backlog がこれ以上なら "growing" 警告を出す (Stage 0 の目安値)。
const SPOOL_BACKLOG_WARN_THRESHOLD: usize = 20;

pub fn run() -> anyhow::Result<()> {
    println!("kikimimi status");
    println!();

    print_collection_targets();
    println!();

    let daemon_alive = kikimimi_spool::send_control(b'n');
    println!(
        "daemon: {}",
        if daemon_alive {
            "running"
        } else {
            "NOT running"
        }
    );

    let state = crate::state::load_opt(&kikimimi_schema::paths::state_path());
    print_state(state.as_ref());
    println!();

    print_web_url(daemon_alive, state.as_ref());

    let backlog = kikimimi_spool::backlog();
    println!("spool backlog: {backlog} file(s)");

    let data_dir = kikimimi_schema::paths::data_dir();
    let (files, bytes) = dir_stats(&data_dir);
    println!(
        "data dir: {} ({} file(s), {})",
        data_dir.display(),
        files,
        human_bytes(bytes)
    );
    println!();

    print_warnings(daemon_alive, backlog, state.as_ref());

    Ok(())
}

fn print_collection_targets() {
    print_claude_collection_target();
    println!();
    print_codex_collection_target();
}

fn print_claude_collection_target() {
    let path = cs::settings_path();
    if !path.exists() {
        println!(
            "claude settings: {} not found (run `kikimimi init`)",
            path.display()
        );
        return;
    }
    println!("claude settings: {}", path.display());
    let value = match cs::load_settings(&path) {
        Ok(v) => v,
        Err(e) => {
            println!("  failed to parse: {e:#}");
            return;
        }
    };

    for (event, _timeout) in cs::HOOK_EVENTS {
        let ok = cs::has_kikimimi_hook(&value, event);
        let legacy = !ok && cs::has_legacy_guru_hook(&value, event);
        println!(
            "  hooks.{:<20} {}",
            event,
            if ok {
                "OK"
            } else if legacy {
                "OK (legacy \"guru hook\" -- re-run `kikimimi init` to upgrade)"
            } else {
                "missing"
            }
        );
    }

    let port = crate::config::resolve_otlp_port();
    for (key, expected) in cs::expected_env(port) {
        let current = value
            .pointer(&format!("/env/{key}"))
            .and_then(Value::as_str);
        let status = match current {
            Some(v) if v == expected => "OK".to_string(),
            Some(v) => format!("mismatch (has {v:?}, expected {expected:?})"),
            None => "missing".to_string(),
        };
        println!("  env.{key:<30} {status}");
    }
}

/// architecture.md §4.1 Codex 行。Stage 0 では Codex 側の hooks/[otel] 設定ファイルへの
/// 書き込みは行わない (`init_cmd.rs` の doc 参照: インストール済みバージョンで
/// hooks の TOML スキーマを `--help`/`codex doctor` から確証できなかったため、
/// 誤った設定を書き込むより「rollout tailer だけに頼る」ことを明示するほうが安全と判断した)。
/// ここではその判断が今この機体でどう効いているかだけを見せる (収集自体は
/// `kikimimi agent` 側の Codex rollout tailer が hooks 設定と無関係に動く)。
fn print_codex_collection_target() {
    let codex_home = kikimimi_schema::paths::codex_home_dir();
    if !codex_home.exists() {
        println!("codex: {} not found (Codex CLI not detected)", codex_home.display());
        return;
    }
    println!("codex: {} found", codex_home.display());
    println!(
        "  rollout tailer: {} (kikimimi agent tails this; see `codex (rollout tailer)` below for live counts)",
        kikimimi_schema::paths::codex_sessions_dir().display()
    );
    println!(
        "  hooks/[otel] config: not written by `kikimimi init` (Stage 0 — could not verify \
         the exact config.toml schema from this machine's `codex --help`/`codex doctor`; \
         relying on the rollout tailer only, see docs/design/architecture.md §4.1)"
    );
}

fn print_state(state: Option<&AgentState>) {
    match state {
        Some(s) => {
            println!("state.json:");
            println!("  pid: {}", s.pid);
            println!("  started_at: {}", fmt_ms(s.started_at_ms));
            println!(
                "  events: hook={} otel={} log={}",
                s.events_by_source.hook, s.events_by_source.otel, s.events_by_source.log
            );
            println!("  skipped: {}", s.skipped);
            print_skipped_by_reason(&s.skipped_by_reason);
            println!(
                "  last_event_ts: {}",
                s.last_event_ts
                    .map(fmt_ms)
                    .unwrap_or_else(|| "-".to_string())
            );
            match &s.last_flush {
                Some(lf) => println!(
                    "  last_flush: {} ({} file(s))",
                    fmt_ms(lf.at_ms),
                    lf.files.len()
                ),
                None => println!("  last_flush: -"),
            }
            println!("  otlp_port: {}", s.otlp_port);
            if let Some(err) = &s.otlp_error {
                println!("  otlp_error: {err}");
            }
            println!("  web_port: {}", s.web.port);
            if let Some(err) = &s.web_error {
                println!("  web_error: {err}");
            }
            if let Some(err) = &s.last_flush_error {
                println!("  last_flush_error: {err}");
            }
            print_cloud_state(s.cloud.as_ref());
            print_s3_state(s.s3.as_ref());
            print_codex_state(&s.codex);
        }
        None => println!("state.json: not found or unreadable (daemon may never have run)"),
    }
}

/// architecture.md §8 task spec: `kikimimi status` prints the web UI URL when the daemon is
/// running. Silent (no line at all) when the daemon isn't running, the port hasn't been
/// recorded yet (agent still starting up), or the web server failed to bind (its error
/// already shows up via `print_state`'s `web_error` line, no need to also print a
/// now-useless URL for it).
fn print_web_url(daemon_alive: bool, state: Option<&AgentState>) {
    if !daemon_alive {
        return;
    }
    let Some(s) = state else { return };
    if s.web.port == 0 || s.web_error.is_some() {
        return;
    }
    println!("web UI: http://127.0.0.1:{}/?t={}", s.web.port, s.web.token);
    println!();
}

fn print_cloud_state(cloud: Option<&crate::state::CloudState>) {
    match cloud {
        Some(c) => {
            println!("  cloud:");
            println!("    endpoint: {}", c.endpoint);
            println!("    pending: {}", c.pending);
            println!(
                "    last_push_at: {}",
                c.last_push_at
                    .map(fmt_ms)
                    .unwrap_or_else(|| "-".to_string())
            );
            if let Some(err) = &c.last_error {
                println!("    last_error: {err}");
            }
        }
        None => println!("  cloud: not logged in (run `kikimimi login`)"),
    }
}

/// architecture.md §6「BYO sink (任意)」: `url` は秘密情報ではない (アップロードは
/// `aws` CLI に委譲し、kikimimi は認証情報を一切保持しない) ので redact せずそのまま出す。
fn print_s3_state(s3: Option<&crate::state::S3State>) {
    match s3 {
        Some(s) => {
            println!("  s3:");
            println!("    url: {}", s.url);
            println!("    pending: {}", s.pending);
            println!(
                "    last_push_at: {}",
                s.last_push_at
                    .map(fmt_ms)
                    .unwrap_or_else(|| "-".to_string())
            );
            if let Some(err) = &s.last_error {
                println!("    last_error: {err}");
            }
        }
        None => println!("  s3: not configured (run `kikimimi sink add s3 <s3://bucket/prefix>`)"),
    }
}

/// architecture.md §4「ログ tailer」, §4.1 Codex 行: Codex rollout tailer の現況。
/// `~/.codex` が無い/Codex を使っていないマシンでは `files_watched == 0` のまま
/// (エラーではない — `codex_tailer.rs` 参照)。
fn print_codex_state(codex: &crate::state::CodexTailerState) {
    println!("  codex (rollout tailer):");
    println!("    files_watched: {}", codex.files_watched);
    println!("    lines_read: {}", codex.lines_read);
    println!("    malformed_lines: {}", codex.malformed_lines);
    println!("    skipped: {}", codex.skipped);
    print_skipped_by_reason(&codex.skipped_by_reason);
}

/// `skipped: N` の下に理由別の内訳を件数の多い順 (降順、同数はキー名の昇順で安定ソート) に
/// 表示する。内訳が空 (skipped=0、または旧い state.json で never populated) なら何も出さない。
fn print_skipped_by_reason(by_reason: &std::collections::BTreeMap<String, u64>) {
    if by_reason.is_empty() {
        return;
    }
    let mut entries: Vec<(&String, &u64)> = by_reason.iter().collect();
    entries.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    for (reason, count) in entries {
        println!("    {reason}: {count}");
    }
}

fn print_warnings(daemon_alive: bool, backlog: usize, state: Option<&AgentState>) {
    let mut warnings: Vec<String> = Vec::new();

    if !daemon_alive {
        warnings.push("daemon not running".to_string());
    }
    if let Some(s) = state {
        if s.events_by_source.hook > 0 && s.events_by_source.otel == 0 {
            warnings.push(
                "hooks are arriving but zero OTel events (see the Windows-OTel-silent-failure analog, claude-code#46204)"
                    .to_string(),
            );
        }
        if let Some(err) = &s.last_flush_error {
            warnings.push(format!(
                "last sink flush failed, buffered events are being retried, not lost: {err}"
            ));
        }
        if let Some(err) = s.cloud.as_ref().and_then(|c| c.last_error.as_ref()) {
            warnings.push(format!(
                "last cloud push failed, buffered events are being retried, not lost: {err}"
            ));
        }
        if let Some(err) = s.s3.as_ref().and_then(|c| c.last_error.as_ref()) {
            warnings.push(format!(
                "last s3 sink push failed, buffered events are being retried, not lost: {err}"
            ));
        }
    }
    if backlog >= SPOOL_BACKLOG_WARN_THRESHOLD {
        warnings.push(format!("spool backlog growing ({backlog} files pending)"));
    }
    if !crate::web_query::duckdb_available() {
        warnings.push(
            "duckdb CLI not found: `kikimimi query` and the web UI's /web/q/* endpoints will \
             fail (the latter with 503) until it's installed. See https://duckdb.org"
                .to_string(),
        );
    }
    if kikimimi_schema::paths::using_runtime_dir_fallback() {
        warnings.push(format!(
            "XDG_RUNTIME_DIR is not set: spool/socket fall back to persistent disk ({}) instead of tmpfs, which can make hook writes slower and less crash-safe by design",
            kikimimi_schema::paths::kikimimi_dir().display()
        ));
    }

    if warnings.is_empty() {
        println!("warnings: none");
    } else {
        println!("warnings:");
        for w in &warnings {
            println!("  - {w}");
        }
    }
}

fn fmt_ms(ms: i64) -> String {
    use chrono::TimeZone;
    chrono::Utc
        .timestamp_millis_opt(ms)
        .single()
        .map(|t| t.to_rfc3339())
        .unwrap_or_else(|| ms.to_string())
}

fn dir_stats(dir: &Path) -> (u64, u64) {
    let mut files = 0u64;
    let mut bytes = 0u64;
    walk(dir, &mut files, &mut bytes);
    (files, bytes)
}

fn walk(dir: &Path, files: &mut u64, bytes: &mut u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        match entry.file_type() {
            Ok(ft) if ft.is_dir() => walk(&path, files, bytes),
            Ok(ft) if ft.is_file() => {
                *files += 1;
                if let Ok(meta) = entry.metadata() {
                    *bytes += meta.len();
                }
            }
            _ => {}
        }
    }
}

/// `kikimimi export` (export_cmd.rs) も同じ整形を使うので crate 内に公開する。
pub(crate) fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[0])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn print_codex_state_covers_zero_and_populated() {
        // Just make sure neither branch panics; output goes to stdout.
        print_codex_state(&crate::state::CodexTailerState::default());
        print_codex_state(&crate::state::CodexTailerState {
            files_watched: 2,
            lines_read: 40,
            malformed_lines: 1,
            skipped: 3,
            skipped_by_reason: std::collections::BTreeMap::from([(
                "rollout:world_state".to_string(),
                3,
            )]),
        });
    }

    #[test]
    fn print_codex_collection_target_does_not_panic() {
        // Whatever this environment's actual $HOME/.codex state is, this must not panic.
        print_codex_collection_target();
    }

    #[test]
    fn human_bytes_formats_units() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(2048), "2.0 KB");
        assert_eq!(human_bytes(5 * 1024 * 1024), "5.0 MB");
    }

    #[test]
    fn dir_stats_counts_nested_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"12345").unwrap();
        let sub = dir.path().join("dt=2026-08-30");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("b.parquet"), b"1234567890").unwrap();

        let (files, bytes) = dir_stats(dir.path());
        assert_eq!(files, 2);
        assert_eq!(bytes, 15);
    }

    #[test]
    fn dir_stats_on_missing_dir_is_zero() {
        let (files, bytes) = dir_stats(Path::new("/does/not/exist/at/all"));
        assert_eq!(files, 0);
        assert_eq!(bytes, 0);
    }

    #[test]
    fn warns_when_hooks_present_but_no_otel() {
        let mut s = AgentState::new(1, 0, 4318);
        s.events_by_source.hook = 10;
        s.events_by_source.otel = 0;
        // Just make sure this doesn't panic and exercises the branch; output goes to stdout.
        print_warnings(true, 0, Some(&s));
    }

    #[test]
    fn print_cloud_state_covers_logged_in_and_logged_out() {
        // Just make sure neither branch panics; output goes to stdout.
        print_cloud_state(None);
        print_cloud_state(Some(&crate::state::CloudState {
            endpoint: "https://cloud.example".into(),
            pending: 2,
            last_push_at: Some(1_700_000_000_000),
            last_error: Some("connection refused".into()),
        }));
    }

    #[test]
    fn print_s3_state_covers_configured_and_unconfigured() {
        // Just make sure neither branch panics; output goes to stdout.
        print_s3_state(None);
        print_s3_state(Some(&crate::state::S3State {
            url: "s3://my-bucket/team".into(),
            pending: 4,
            last_push_at: Some(1_700_000_000_000),
            last_error: Some("aws CLI not found".into()),
        }));
    }

    #[test]
    fn warns_when_s3_push_failed() {
        let mut s = AgentState::new(1, 0, 4318);
        s.s3 = Some(crate::state::S3State {
            url: "s3://my-bucket/team".into(),
            pending: 5,
            last_push_at: None,
            last_error: Some("aws CLI not found".into()),
        });
        // Just make sure this doesn't panic and exercises the branch; output goes to stdout.
        print_warnings(true, 0, Some(&s));
    }

    #[test]
    fn warns_when_cloud_push_failed() {
        let mut s = AgentState::new(1, 0, 4318);
        s.cloud = Some(crate::state::CloudState {
            endpoint: "https://cloud.example".into(),
            pending: 5,
            last_push_at: None,
            last_error: Some("connection refused".into()),
        });
        // Just make sure this doesn't panic and exercises the branch; output goes to stdout.
        print_warnings(true, 0, Some(&s));
    }

    #[test]
    fn print_skipped_by_reason_is_noop_when_empty() {
        // Just make sure this doesn't panic; output goes to stdout (nothing should print).
        print_skipped_by_reason(&std::collections::BTreeMap::new());
    }

    #[test]
    fn print_skipped_by_reason_does_not_panic_and_covers_the_sort() {
        let mut by_reason = std::collections::BTreeMap::new();
        by_reason.insert("PreCompact".to_string(), 3u64);
        by_reason.insert("no_hook_event_name".to_string(), 5u64);
        by_reason.insert("malformed_spool".to_string(), 5u64);
        // Exercises the descending-count / ascending-key tie-break sort; output goes to
        // stdout so we just assert it runs without panicking.
        print_skipped_by_reason(&by_reason);
    }
}
