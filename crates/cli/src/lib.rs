//! `kikimimi` — Stage 0 CLI (architecture.md §4, §12).
//!
//! Ships as three binaries sharing this crate's [`run`] entry point: `kikimimi` (primary),
//! plus the short alias `kkmm` (`src/bin/*.rs`, each a one-line `fn main`).
//!
//! [`run`] stays synchronous. Only `kikimimi agent` spins up a tokio runtime; every other
//! subcommand (most importantly `kikimimi hook`, which runs once per tool call) must not pay
//! for one.

mod agent;
mod claude_settings;
mod codex_tailer;
mod config;
mod daemonize;
mod export_cmd;
mod hook_cmd;
mod init_cmd;
mod login_cmd;
mod query_cmd;
mod self_update_cmd;
mod sink_cmd;
mod state;
mod status_cmd;
mod update;
mod web;
mod web_cmd;
mod web_query;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "kikimimi",
    version,
    about = "Collects Claude Code hooks/OTel locally and detects MCP-bypass patterns (Stage 0)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Hook shim invoked by Claude Code's settings.json (`kikimimi hook <EVENT>`). Always exits 0.
    Hook {
        /// Hook event name, e.g. PreToolUse, PostToolUse, SessionStart.
        event: String,
    },
    /// Run the resident daemon: spool drain, OTLP receiver, local Parquet sink.
    Agent {
        /// Stay attached to the current terminal instead of daemonizing.
        #[arg(long)]
        foreground: bool,
    },
    /// Write kikimimi's hooks/env into ~/.claude/settings.json (idempotent).
    Init {
        /// Print what would change without writing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove exactly what `kikimimi init` added from ~/.claude/settings.json.
    Uninstall {
        /// Also delete ~/.kikimimi (local data + spool + state).
        #[arg(long)]
        purge_data: bool,
    },
    /// Show collection targets, daemon health, spool backlog, and data dir size.
    Status,
    /// Run a query against the local Parquet files via the `duckdb` CLI, or (with
    /// `--cloud`) against kikimimi cloud's `GET /v1/query/<name>` (architecture.md §8).
    Query {
        /// A built-in query name: today, tools, mcp, bypass, reach, unused-mcp, schema-tax.
        name: Option<String>,
        /// Run this raw SQL instead of a named query. Local (DuckDB) only.
        #[arg(long)]
        sql: Option<String>,
        /// Print the SQL that will be run before running it. Local (DuckDB) only.
        #[arg(long)]
        show_sql: bool,
        /// Query kikimimi cloud instead of local Parquet (requires `kikimimi login`).
        #[arg(long)]
        cloud: bool,
        /// Inclusive start date (YYYY-MM-DD). Cloud only; ignored locally.
        /// Omitting this (and --to) for the `today` named query still scopes
        /// to today's date, matching the local DuckDB `today` query.
        #[arg(long = "from")]
        dt_from: Option<String>,
        /// Inclusive end date (YYYY-MM-DD). Cloud only; ignored locally.
        #[arg(long = "to")]
        dt_to: Option<String>,
    },
    /// Ask the daemon to flush its buffered events to Parquet right now.
    Flush,
    /// Authenticate this host with kikimimi cloud (device-code flow, architecture.md §6/§8).
    Login {
        /// Cloud base URL. Defaults to the endpoint from a previous `kikimimi login` (if
        /// any), then the KIKIMIMI_ENDPOINT env var (dev override), then kikimimi cloud's
        /// hosted instance at https://kikimimi.dev.
        #[arg(long)]
        endpoint: Option<String>,
        /// Accepted for forward-compat; this CLI never opens a browser itself (Stage 0).
        #[arg(long)]
        no_browser: bool,
    },
    /// Forget the saved cloud token (`~/.kikimimi/config.json`'s `cloud` section).
    Logout,
    /// Configure BYO sinks (architecture.md §4/§6). kikimimi never stores credentials for
    /// these -- uploads are shelled out to the vendor's own CLI (e.g. `aws`).
    Sink {
        #[command(subcommand)]
        action: SinkAction,
    },
    /// Print the local web UI URL (architecture.md §8) and best-effort open it in a
    /// browser ($BROWSER / xdg-open / open).
    Web,
    /// Download the full `kikimimi.v1` Parquet export from kikimimi cloud (`GET /v1/export`).
    Export {
        /// Inclusive start date (YYYY-MM-DD). Omit for no lower bound.
        #[arg(long = "from")]
        dt_from: Option<String>,
        /// Inclusive end date (YYYY-MM-DD). Omit for no upper bound.
        #[arg(long = "to")]
        dt_to: Option<String>,
        /// Output file path. Defaults to ./kikimimi-export.parquet.
        #[arg(short = 'o', long = "output")]
        output: Option<PathBuf>,
    },
    /// Upgrades this install to the latest GitHub release, via the same cargo-dist install
    /// receipt the shell installer (`kikimimi-installer.sh`) writes -- read through
    /// `axoupdater` (axodotdev's own updater library, same vendor as cargo-dist) to find
    /// what was installed where, then re-run at the latest tag. Only an install that
    /// actually has a receipt can be updated this way; a Homebrew or `cargo install`
    /// install has none, so this instead prints the right command for *that* install and
    /// exits 0 -- nothing was updated, but nothing failed either. If a daemon
    /// (`kikimimi agent`) is running under the old binary, a successful update restarts it.
    SelfUpdate {
        /// Only report whether an update is available -- never downloads or installs
        /// anything, and never restarts a running daemon.
        #[arg(long)]
        check: bool,
    },
}

#[derive(Subcommand)]
enum SinkAction {
    /// Add (or replace) a BYO sink.
    Add {
        #[command(subcommand)]
        kind: SinkAddKind,
    },
    /// List configured sinks (file/cloud/s3). See `kikimimi status` for live pending/last_push/last_error.
    List,
    /// Remove a configured BYO sink.
    Remove {
        /// Which sink to remove. Currently only "s3".
        kind: String,
    },
}

#[derive(Subcommand)]
enum SinkAddKind {
    /// BYO S3 sink: writes kikimimi.v1 Parquet to your own bucket via the `aws` CLI
    /// (architecture.md §6). kikimimi never touches your AWS credentials.
    S3 {
        /// s3://bucket/prefix
        url: String,
        /// AWS CLI profile to use (passed as `aws ... --profile <PROFILE>`).
        #[arg(long)]
        profile: Option<String>,
        /// S3-compatible endpoint override (e.g. for R2/MinIO), passed as `--endpoint-url`.
        #[arg(long = "endpoint-url")]
        endpoint_url: Option<String>,
    },
}

pub fn run() {
    // Ships as two binaries (`kikimimi`, `kkmm`) sharing this entry point, and the
    // README promises they "behave identically". clap derives the `Usage:` line's program name
    // from argv[0] by default, which would otherwise make `kkmm --help` differ from `kikimimi
    // --help` only in that line -- pin it to "kikimimi" so help/usage/error text is byte-identical
    // no matter which alias invoked us.
    let mut args = std::env::args_os();
    args.next(); // drop the real argv[0] (kikimimi / kkmm)
    let cli = Cli::parse_from(std::iter::once(std::ffi::OsString::from("kikimimi")).chain(args));

    match cli.command {
        Command::Hook { event } => {
            // Contract: never print on success, never panic, always exit 0.
            hook_cmd::run(&event);
            std::process::exit(0);
        }
        Command::Agent { foreground } => run_agent(foreground),
        Command::Init { dry_run } => exit_on_err(init_cmd::init(dry_run)),
        Command::Uninstall { purge_data } => exit_on_err(init_cmd::uninstall(purge_data)),
        Command::Status => exit_on_err(status_cmd::run()),
        Command::Query {
            name,
            sql,
            show_sql,
            cloud,
            dt_from,
            dt_to,
        } => exit_on_err(query_cmd::run(query_cmd::QueryArgs {
            name,
            sql,
            show_sql,
            cloud,
            dt_from,
            dt_to,
        })),
        Command::Flush => run_flush(),
        Command::Web => exit_on_err(web_cmd::run()),
        Command::Login {
            endpoint,
            no_browser,
        } => exit_on_err(login_cmd::login(endpoint, no_browser)),
        Command::Logout => exit_on_err(login_cmd::logout()),
        Command::Sink { action } => exit_on_err(match action {
            SinkAction::Add {
                kind:
                    SinkAddKind::S3 {
                        url,
                        profile,
                        endpoint_url,
                    },
            } => sink_cmd::add_s3(url, profile, endpoint_url),
            SinkAction::List => sink_cmd::list(),
            SinkAction::Remove { kind } => sink_cmd::remove(&kind),
        }),
        Command::Export {
            dt_from,
            dt_to,
            output,
        } => exit_on_err(export_cmd::run(export_cmd::ExportArgs {
            dt_from,
            dt_to,
            output,
        })),
        Command::SelfUpdate { check } => exit_on_err(self_update_cmd::run(check)),
    }
}

fn run_agent(foreground: bool) {
    if !foreground {
        let log_path = kikimimi_schema::paths::kikimimi_dir().join("agent.log");
        if let Err(e) = daemonize::daemonize(&log_path) {
            eprintln!("kikimimi agent: failed to daemonize ({e:#}); continuing in foreground");
        }
    }

    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("kikimimi agent: failed to start tokio runtime: {e:#}");
            std::process::exit(1);
        }
    };

    if let Err(e) = rt.block_on(agent::run()) {
        eprintln!("kikimimi agent: {e:#}");
        std::process::exit(1);
    }
}

fn run_flush() {
    let acked = kikimimi_spool::send_control(b'f');
    println!("flush acked by daemon: {acked}");
    if !acked {
        std::process::exit(1);
    }
}

fn exit_on_err(result: anyhow::Result<()>) {
    if let Err(e) = result {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
