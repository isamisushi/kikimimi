//! `guru` — Stage 0 CLI (architecture.md §4, §12).
//!
//! `main` stays synchronous. Only `guru agent` spins up a tokio runtime; every other
//! subcommand (most importantly `guru hook`, which runs once per tool call) must not pay
//! for one.

mod agent;
mod claude_settings;
mod config;
mod daemonize;
mod export_cmd;
mod hook_cmd;
mod init_cmd;
mod login_cmd;
mod query_cmd;
mod sink_cmd;
mod state;
mod status_cmd;
mod web;
mod web_cmd;
mod web_query;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "guru",
    version,
    about = "Collects Claude Code hooks/OTel locally and detects MCP-bypass patterns (Stage 0)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Hook shim invoked by Claude Code's settings.json (`guru hook <EVENT>`). Always exits 0.
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
    /// Write guru's hooks/env into ~/.claude/settings.json (idempotent).
    Init {
        /// Print what would change without writing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove exactly what `guru init` added from ~/.claude/settings.json.
    Uninstall {
        /// Also delete ~/.guru (local data + spool + state).
        #[arg(long)]
        purge_data: bool,
    },
    /// Show collection targets, daemon health, spool backlog, and data dir size.
    Status,
    /// Run a query against the local Parquet files via the `duckdb` CLI, or (with
    /// `--cloud`) against guru cloud's `GET /v1/query/<name>` (architecture.md §8).
    Query {
        /// A built-in query name: today, tools, mcp, bypass, reach, unused-mcp, schema-tax.
        name: Option<String>,
        /// Run this raw SQL instead of a named query. Local (DuckDB) only.
        #[arg(long)]
        sql: Option<String>,
        /// Print the SQL that will be run before running it. Local (DuckDB) only.
        #[arg(long)]
        show_sql: bool,
        /// Query guru cloud instead of local Parquet (requires `guru login`).
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
    /// Authenticate this host with guru cloud (device-code flow, architecture.md §6/§8).
    Login {
        /// Cloud base URL. Defaults to http://127.0.0.1:8787.
        #[arg(long)]
        endpoint: Option<String>,
        /// Accepted for forward-compat; this CLI never opens a browser itself (Stage 0).
        #[arg(long)]
        no_browser: bool,
    },
    /// Forget the saved cloud token (`~/.guru/config.json`'s `cloud` section).
    Logout,
    /// Configure BYO sinks (architecture.md §4/§6). guru never stores credentials for
    /// these -- uploads are shelled out to the vendor's own CLI (e.g. `aws`).
    Sink {
        #[command(subcommand)]
        action: SinkAction,
    },
    /// Print the local web UI URL (architecture.md §8) and best-effort open it in a
    /// browser ($BROWSER / xdg-open / open).
    Web,
    /// Download the full `guru.v1` Parquet export from guru cloud (`GET /v1/export`).
    Export {
        /// Inclusive start date (YYYY-MM-DD). Omit for no lower bound.
        #[arg(long = "from")]
        dt_from: Option<String>,
        /// Inclusive end date (YYYY-MM-DD). Omit for no upper bound.
        #[arg(long = "to")]
        dt_to: Option<String>,
        /// Output file path. Defaults to ./guru-export.parquet.
        #[arg(short = 'o', long = "output")]
        output: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum SinkAction {
    /// Add (or replace) a BYO sink.
    Add {
        #[command(subcommand)]
        kind: SinkAddKind,
    },
    /// List configured sinks (file/cloud/s3). See `guru status` for live pending/last_push/last_error.
    List,
    /// Remove a configured BYO sink.
    Remove {
        /// Which sink to remove. Currently only "s3".
        kind: String,
    },
}

#[derive(Subcommand)]
enum SinkAddKind {
    /// BYO S3 sink: writes guru.v1 Parquet to your own bucket via the `aws` CLI
    /// (architecture.md §6). guru never touches your AWS credentials.
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

fn main() {
    let cli = Cli::parse();

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
    }
}

fn run_agent(foreground: bool) {
    if !foreground {
        let log_path = guru_schema::paths::guru_dir().join("agent.log");
        if let Err(e) = daemonize::daemonize(&log_path) {
            eprintln!("guru agent: failed to daemonize ({e:#}); continuing in foreground");
        }
    }

    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("guru agent: failed to start tokio runtime: {e:#}");
            std::process::exit(1);
        }
    };

    if let Err(e) = rt.block_on(agent::run()) {
        eprintln!("guru agent: {e:#}");
        std::process::exit(1);
    }
}

fn run_flush() {
    let acked = guru_spool::send_control(b'f');
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
