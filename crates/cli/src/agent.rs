//! `kikimimi agent` — 常駐デーモン本体 (architecture.md §4)。
//!
//! - control socket (unix): `n` = spool をすぐ drain、`f` = 今すぐ flush
//! - OTLP レシーバ (`kikimimi_otlp::serve`): ポート衝突時はエラーを state に記録して続行
//! - メインループ: 2 秒ごと + `n` 受信時に spool を drain → 正規化 → sink へ push
//!   OTLP チャンネルからも同様に正規化 → push。sink は 2 秒ごとに `maybe_flush`、
//!   `f` 受信 / SIGTERM / SIGINT で強制 flush する

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use serde_json::Value;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;

use kikimimi_adapter_claude::Normalizer;
use kikimimi_adapter_codex::CodexNormalizer;
use kikimimi_otlp::OtlpPayload;
use kikimimi_sink::{CloudSink, EventSink, FileSink, S3Config, S3Sink};
use kikimimi_spool::SpoolReader;

use crate::codex_tailer::CodexTailer;
use crate::state::{AgentState, CloudState, CodexTailerState, LastFlush, S3State};

const TICK: Duration = Duration::from_secs(2);
const STATE_SAVE_INTERVAL: Duration = Duration::from_secs(2);
/// OTLP の bind に失敗した後、再挑戦する間隔。起動直後の一過性の衝突
/// (相手プロセスがそのうちポートを手放す等) で、デーモンの残り全生存期間ぶん
/// テレメトリを失わないための救済措置。
const OTLP_RETRY_INTERVAL: Duration = Duration::from_secs(30);

pub async fn run() -> anyhow::Result<()> {
    ensure_dirs()?;

    // Background "a newer kikimimi is out" check (update.rs's module docs). Detached and
    // fire-and-forget like the daemon's other startup-time tasks below (control socket,
    // OTLP, web UI) -- see spawn_notifier's own docs for exactly why this can never affect
    // ingestion. `kikimimi status` is the only thing that ever reads what it writes.
    crate::update::spawn_notifier();

    let host_id = kikimimi_schema::paths::host_id().context("loading/creating host_id")?;

    let sock_path = kikimimi_schema::paths::socket_path();
    if let Some(parent) = sock_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating socket dir {}", parent.display()))?;
    }
    // Cheap liveness probe reusing kikimimi-spool's own 50ms-timeout connect. A positive reply
    // means a real daemon is already listening on this control socket.
    if kikimimi_spool::send_control(b'n') {
        anyhow::bail!(
            "kikimimi agent: another instance appears to already be listening on {}",
            sock_path.display()
        );
    }
    let _ = fs::remove_file(&sock_path); // drop a stale socket file, if any

    let std_listener = std::os::unix::net::UnixListener::bind(&sock_path)
        .with_context(|| format!("binding control socket {}", sock_path.display()))?;
    std_listener
        .set_nonblocking(true)
        .context("setting control socket non-blocking")?;
    let listener = tokio::net::UnixListener::from_std(std_listener)
        .context("wrapping control socket for tokio")?;

    let (ctrl_tx, mut ctrl_rx) = mpsc::channel::<u8>(64);
    tokio::spawn(accept_control_loop(listener, ctrl_tx));

    // architecture.md §4「OTLP レシーバ」認証: `kikimimi init` が発行したトークンを
    // `Arc<RwLock<..>>` に載せて `kikimimi_otlp::serve` に渡す。`'r'` コントロールバイト
    // (config reload) がこの中身だけ書き換えるので、`kikimimi init` はデーモンを再起動
    // させずにトークンを有効化できる (init_cmd.rs が init 完了後に送る)。`otlp_token` が
    // `None` (未 `init`) の間、レシーバは fail-open で誰でも受け付ける
    // (crates/otlp/src/lib.rs のモジュール doc 参照)。
    let otlp_auth: std::sync::Arc<std::sync::RwLock<Option<String>>> = std::sync::Arc::new(
        std::sync::RwLock::new(crate::config::KikimimiConfig::load().otlp_token),
    );
    let otlp_rejected = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));

    let (otlp_tx, mut otlp_rx) = mpsc::channel::<OtlpPayload>(256);
    let otlp_addr =
        std::net::SocketAddr::from(([127, 0, 0, 1], crate::config::resolve_otlp_port()));
    let (otlp_handle, otlp_shutdown_tx, otlp_error) = start_otlp(
        otlp_addr,
        otlp_tx.clone(),
        otlp_auth.clone(),
        otlp_rejected.clone(),
    )
    .await;
    let mut otlp_handle = otlp_handle;
    let mut otlp_shutdown_tx = otlp_shutdown_tx;
    // If the initial bind failed (port taken by something else at startup), keep
    // periodically retrying instead of permanently disabling OTel/token/cost telemetry
    // for the rest of this (long-lived) run — a transient conflict at boot shouldn't be
    // fatal for the process's whole lifetime.
    let mut otlp_retry = tokio::time::interval(OTLP_RETRY_INTERVAL);
    otlp_retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    // architecture.md §8 (個人ビュー/ローカル): the local web UI. Token is fresh every
    // start (never read back from a previous state.json -- see web.rs's docs); port
    // follows the same "pick a free one if the preferred is taken, persist it" shape as
    // kikimimi init's OTLP port, just done here instead since there's no separate `kikimimi
    // init`-equivalent step for the web UI.
    let web_token = crate::web::generate_local_token();
    let web_port = pick_and_persist_web_port();
    let web_addr = std::net::SocketAddr::from(([127, 0, 0, 1], web_port));
    let web_state = crate::web::WebAppState {
        token: web_token.clone(),
        data_dir: kikimimi_schema::paths::data_dir(),
    };
    let (web_handle, web_shutdown_tx, web_error) = start_web(web_addr, web_state).await;

    let data_dir = kikimimi_schema::paths::data_dir();
    let mut normalizer = Normalizer::new(host_id.clone());
    // issue #4: Claude Code hook events populate Event.repo daemon-side, from the hook
    // payload's "cwd" via the git remote URL, with a small per-cwd cache (repo_resolve.rs)
    // so we don't re-read `.git/config` on every single hook event.
    let mut repo_resolver = crate::repo_resolve::RepoResolver::default();
    let mut sink = FileSink::new(
        data_dir,
        host_id.clone(),
        FileSink::DEFAULT_MAX_ROWS,
        FileSink::DEFAULT_MAX_AGE,
    );
    let spool_reader = SpoolReader::new();

    // architecture.md §4「ログ tailer」, §4.1 Codex 行: ~/.codex/sessions/**/rollout-*.jsonl
    // を再帰的にテールする。~/.codex が無い (Codex 未インストール) マシンでも
    // files_watched=0 のまま安全に動く (エラーにしない — codex_tailer.rs 参照)。
    let mut codex_normalizer = CodexNormalizer::new(host_id.clone());
    let mut codex_tailer = CodexTailer::new();

    // §6/§8: when `kikimimi login` has stashed a cloud token in config.json, push every
    // event to the cloud sink too, alongside (never instead of) the local FileSink — the
    // local Parquet stays the offline-safe source of truth (§4), cloud is best-effort.
    let cloud_cfg = crate::config::KikimimiConfig::load().cloud;
    let mut cloud_sink: Option<CloudSink> = cloud_cfg
        .as_ref()
        .map(|c| CloudSink::new(c.endpoint.clone(), c.token.clone(), host_id.clone()));

    // §6.1: team-org repo allowlist — only ever restricts what the *cloud* sink above
    // receives (FileSink/BYO sinks are untouched, see repo_filter.rs's module docs). Built
    // from the same `cloud_cfg` snapshot as `cloud_sink` above so both agree on org_kind at
    // startup; `kikimimi repos allow/remove` refreshes this live via the `b'r'` control byte
    // below, same as the s3 sink's reload.
    let mut repo_filter = crate::repo_filter::RepoFilter::from_cloud_config(cloud_cfg.as_ref());
    if let Some(c) = &cloud_cfg {
        if let Some(warning) = repo_filter.unconfigured_warning(&c.org_slug) {
            eprintln!("{warning}");
        }
    }

    // §6 「BYO sink (任意)」: when `kikimimi sink add s3` has stashed an s3 sink config in
    // config.json, push every event (full body — BYO sinks are not masked, §5.2) to it
    // too, alongside (never instead of) FileSink/CloudSink.
    let mut s3_sink: Option<S3Sink> = build_s3_sink(&host_id);

    let mut state = AgentState::new(std::process::id(), now_ms(), otlp_addr.port());
    state.otlp_error = otlp_error;
    state.web = crate::state::WebState {
        port: web_port,
        token: web_token,
    };
    state.web_error = web_error;
    let mut malformed: u64 = 0;
    save_state_now(
        &state,
        &mut malformed,
        &normalizer,
        cloud_sink.as_ref(),
        s3_sink.as_ref(),
        &codex_tailer,
        &codex_normalizer,
        &otlp_auth,
        &otlp_rejected,
    );

    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_state_save = tokio::time::Instant::now();

    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("installing SIGTERM handler")?;

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                // block_in_place: drain_spool/maybe_flush do synchronous fs + Arrow/Parquet
                // I/O. Running them inline in this select! arm would stall the whole
                // single-threaded-looking event loop (control socket accepts, OTLP
                // channel drains, SIGTERM) for as long as a large backlog or a slow disk
                // takes; block_in_place tells the (multi-thread) runtime it may move other
                // tasks to other worker threads meanwhile instead of starving them.
                tokio::task::block_in_place(|| {
                    drain_spool(&spool_reader, &mut normalizer, &mut sink, cloud_sink.as_mut(), s3_sink.as_mut(), &repo_filter, &mut repo_resolver, &mut state, &mut malformed);
                    drain_codex(&mut codex_tailer, &mut codex_normalizer, &mut sink, cloud_sink.as_mut(), s3_sink.as_mut(), &repo_filter, &mut state);
                });
                apply_flush_result(&mut state, tokio::task::block_in_place(|| sink.maybe_flush()));
                // Each sink's flush is isolated: one sink's error (network blip, bad
                // credentials, S3 outage, ...) must never block or skip the others.
                if let Some(cs) = cloud_sink.as_mut() {
                    apply_cloud_flush_result(tokio::task::block_in_place(|| cs.maybe_flush()));
                }
                if let Some(s3) = s3_sink.as_mut() {
                    apply_s3_flush_result(tokio::task::block_in_place(|| s3.maybe_flush()));
                }
            }
            Some(byte) = ctrl_rx.recv() => {
                match byte {
                    b'n' => {
                        tokio::task::block_in_place(|| {
                            drain_spool(&spool_reader, &mut normalizer, &mut sink, cloud_sink.as_mut(), s3_sink.as_mut(), &repo_filter, &mut repo_resolver, &mut state, &mut malformed);
                            drain_codex(&mut codex_tailer, &mut codex_normalizer, &mut sink, cloud_sink.as_mut(), s3_sink.as_mut(), &repo_filter, &mut state);
                        });
                    }
                    b'f' => {
                        tokio::task::block_in_place(|| {
                            drain_spool(&spool_reader, &mut normalizer, &mut sink, cloud_sink.as_mut(), s3_sink.as_mut(), &repo_filter, &mut repo_resolver, &mut state, &mut malformed);
                            drain_codex(&mut codex_tailer, &mut codex_normalizer, &mut sink, cloud_sink.as_mut(), s3_sink.as_mut(), &repo_filter, &mut state);
                        });
                        apply_flush_result(&mut state, tokio::task::block_in_place(|| EventSink::flush(&mut sink)));
                        if let Some(cs) = cloud_sink.as_mut() {
                            apply_cloud_flush_result(tokio::task::block_in_place(|| EventSink::flush(cs)));
                        }
                        if let Some(s3) = s3_sink.as_mut() {
                            apply_s3_flush_result(tokio::task::block_in_place(|| EventSink::flush(s3)));
                        }
                        sync_skipped(&mut state, &normalizer, malformed);
                        tokio::task::block_in_place(|| {
                            sync_cloud_state(&mut state, cloud_sink.as_ref());
                            sync_s3_state(&mut state, s3_sink.as_ref());
                            sync_codex_state(&mut state, &codex_tailer, &codex_normalizer);
                            sync_otlp_auth_state(&mut state, &otlp_auth, &otlp_rejected);
                            let _ = state.save();
                        });
                        last_state_save = tokio::time::Instant::now();
                    }
                    b'r' => {
                        // architecture.md §6/§6.1: reload BYO sink config and the team-org
                        // repo filter without restarting the daemon (used by `kikimimi sink
                        // add s3` / `kikimimi sink remove s3` / `kikimimi repos
                        // allow`/`remove`). Also reloads the OTLP bearer token (§4「認証」) so
                        // `kikimimi init` can activate/rotate it without a daemon restart.
                        tokio::task::block_in_place(|| {
                            reload_s3_sink(&mut s3_sink, &host_id);
                            let reloaded_cfg = crate::config::KikimimiConfig::load();
                            repo_filter =
                                crate::repo_filter::RepoFilter::from_cloud_config(reloaded_cfg.cloud.as_ref());
                            reload_otlp_auth(&otlp_auth, reloaded_cfg.otlp_token);
                            sync_s3_state(&mut state, s3_sink.as_ref());
                            sync_otlp_auth_state(&mut state, &otlp_auth, &otlp_rejected);
                            let _ = state.save();
                        });
                        last_state_save = tokio::time::Instant::now();
                    }
                    _ => {}
                }
            }
            Some(payload) = otlp_rx.recv() => {
                ingest_otlp(payload, &mut normalizer, &mut sink, cloud_sink.as_mut(), s3_sink.as_mut(), &repo_filter, &mut state, malformed);
            }
            // Retry a failed OTLP bind periodically instead of leaving telemetry
            // permanently disabled for a possibly-transient startup conflict (§4).
            _ = otlp_retry.tick(), if state.otlp_error.is_some() => {
                let (handle, shutdown_tx, error) = start_otlp(
                    otlp_addr,
                    otlp_tx.clone(),
                    otlp_auth.clone(),
                    otlp_rejected.clone(),
                )
                .await;
                if error.is_none() {
                    eprintln!("kikimimi agent: otlp receiver recovered, now listening on {otlp_addr}");
                }
                otlp_handle = handle;
                otlp_shutdown_tx = shutdown_tx;
                state.otlp_error = error;
            }
            _ = sigterm.recv() => {
                break;
            }
            _ = tokio::signal::ctrl_c() => {
                break;
            }
        }

        if last_state_save.elapsed() >= STATE_SAVE_INTERVAL {
            tokio::task::block_in_place(|| {
                save_state_now(
                    &state,
                    &mut malformed,
                    &normalizer,
                    cloud_sink.as_ref(),
                    s3_sink.as_ref(),
                    &codex_tailer,
                    &codex_normalizer,
                    &otlp_auth,
                    &otlp_rejected,
                )
            });
            last_state_save = tokio::time::Instant::now();
        }
    }

    // Final drain + forced flush before exiting.
    tokio::task::block_in_place(|| {
        drain_spool(
            &spool_reader,
            &mut normalizer,
            &mut sink,
            cloud_sink.as_mut(),
            s3_sink.as_mut(),
            &repo_filter,
            &mut repo_resolver,
            &mut state,
            &mut malformed,
        );
        drain_codex(
            &mut codex_tailer,
            &mut codex_normalizer,
            &mut sink,
            cloud_sink.as_mut(),
            s3_sink.as_mut(),
            &repo_filter,
            &mut state,
        );
    });
    apply_flush_result(
        &mut state,
        tokio::task::block_in_place(|| EventSink::flush(&mut sink)),
    );
    if let Some(cs) = cloud_sink.as_mut() {
        apply_cloud_flush_result(tokio::task::block_in_place(|| EventSink::flush(cs)));
    }
    if let Some(s3) = s3_sink.as_mut() {
        apply_s3_flush_result(tokio::task::block_in_place(|| EventSink::flush(s3)));
    }
    tokio::task::block_in_place(|| {
        save_state_now(
            &state,
            &mut malformed,
            &normalizer,
            cloud_sink.as_ref(),
            s3_sink.as_ref(),
            &codex_tailer,
            &codex_normalizer,
            &otlp_auth,
            &otlp_rejected,
        )
    });

    if let Some(tx) = otlp_shutdown_tx {
        let _ = tx.send(());
    }
    if let Some(handle) = otlp_handle {
        let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
    }
    if let Some(tx) = web_shutdown_tx {
        let _ = tx.send(());
    }
    if let Some(handle) = web_handle {
        let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
    }
    let _ = fs::remove_file(&sock_path);

    Ok(())
}

/// architecture.md §8: like the OTLP port (`kikimimi init`'s `pick_port`), resolve the
/// preferred web UI port (`KIKIMIMI_WEB_PORT` env > config.json > 4319 default) and, unless
/// the env var was set explicitly, swap in a free port if the preferred one is taken.
/// Persists the chosen port to config.json (mirrors `init_cmd.rs`'s OTLP port
/// persistence) so a future `kikimimi agent` binds the same one — there's no separate
/// `kikimimi init`-equivalent step for the web UI, so this happens here instead, at daemon
/// startup, rather than in a one-time setup command.
fn pick_and_persist_web_port() -> u16 {
    let preferred = crate::config::resolve_web_port_preferred();
    let port = if crate::config::web_port_env_override().is_some() {
        preferred
    } else {
        kikimimi_otlp::pick_port(preferred)
    };
    if port != preferred {
        eprintln!(
            "kikimimi agent: web UI port {preferred} is already in use; selected alternate port {port} instead"
        );
    }
    let mut cfg = crate::config::KikimimiConfig::load();
    if cfg.web_port != Some(port) {
        cfg.web_port = Some(port);
        if let Err(e) = cfg.save() {
            eprintln!("kikimimi agent: failed to persist web_port to config.json: {e:#}");
        }
    }
    port
}

/// `start_otlp`と同じ形 (150ms 経過時点でまだ finished していなければ起動成功とみなし、
/// タスクと shutdown 用の oneshot sender を返す。即座に終了していれば bind 失敗として
/// エラーを state に記録するだけで daemon 自体は続行する)。web UI は OTLP と違って
/// 起動時に `pick_and_persist_web_port` で事前に空きポートを選んでいるため、OTLP の
/// ような定期リトライは無し — 初期 bind に失敗するのは基本的に別プロセスとの
/// レース (§4 の TOCTOU) のみで、その場合は次回 `kikimimi agent` 起動時に改めて拾い直す。
async fn start_web(
    addr: std::net::SocketAddr,
    state: crate::web::WebAppState,
) -> (
    Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
    Option<tokio::sync::oneshot::Sender<()>>,
    Option<String>,
) {
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let handle = tokio::spawn(async move {
        let shutdown = async move {
            let _ = shutdown_rx.await;
        };
        crate::web::serve(addr, state, shutdown).await
    });

    tokio::time::sleep(Duration::from_millis(150)).await;

    if handle.is_finished() {
        match handle.await {
            Ok(Ok(())) => (None, None, None),
            Ok(Err(e)) => (
                None,
                None,
                Some(format!("web UI failed to start on {addr}: {e:#}")),
            ),
            Err(e) => (None, None, Some(format!("web UI task panicked: {e}"))),
        }
    } else {
        (Some(handle), Some(shutdown_tx), None)
    }
}

fn ensure_dirs() -> anyhow::Result<()> {
    fs::create_dir_all(kikimimi_schema::paths::kikimimi_dir()).context("creating kikimimi dir")?;
    fs::create_dir_all(kikimimi_schema::paths::data_dir()).context("creating data dir")?;
    fs::create_dir_all(kikimimi_schema::paths::spool_dir()).context("creating spool dir")?;
    Ok(())
}

/// architecture.md §6「BYO sink (任意)」: `kikimimi sink add s3` が `config.json` に書いた
/// `s3` セクションがあれば `S3Sink` を組み立てる。無ければ `None` (BYO sink は完全に
/// オプトイン)。staging ディレクトリは `~/.kikimimi/s3-staging` (`KIKIMIMI_DIR` があればそちら) —
/// アップロード成功後は空になる一時領域で、`~/.kikimimi/data/events` (`FileSink` の恒久
/// 保存先) とは別。`uploader` は常に `None` (既定の `"aws"` を `PATH` から解決する) —
/// テスト用のバイナリ差し替えは `kikimimi-sink` のクレート内単体テストの領分で、
/// ここではテスト目的の smoke.sh も「`PATH` に `aws` という名前のフェイクを置く」形で
/// 検証する (crate 側の差し替え経路を本番コードに引き回さない)。
///
/// `url` は `kikimimi sink add s3` 時点で `sink_cmd::validate_s3_url` を通っているはずだが、
/// `config.json` は手編集され得るファイルなので、ここでも同じ検証を再適用する
/// (defense in depth) — 通らなければ、壊れた `S3Sink` を組み立てて後段のアップロード
/// やログ出力を汚すより、BYO sink 無しとして起動し理由を stderr に残す方が安全。
fn build_s3_sink(host_id: &str) -> Option<S3Sink> {
    let cfg = crate::config::KikimimiConfig::load().s3?;
    if let Err(e) = crate::sink_cmd::validate_s3_url(&cfg.url) {
        eprintln!(
            "kikimimi agent: ignoring s3 sink config.json entry, invalid url: {e:#} \
             (fix with `kikimimi sink add s3 <s3://bucket/prefix>`)"
        );
        return None;
    }
    let staging_dir = kikimimi_schema::paths::kikimimi_dir().join("s3-staging");
    Some(S3Sink::new(
        S3Config {
            url: cfg.url,
            profile: cfg.profile,
            endpoint_url: cfg.endpoint_url,
            uploader: None,
        },
        host_id.to_string(),
        staging_dir,
    ))
}

/// 制御バイト `b'r'` (reload) の実体。既存の `s3_sink` があれば、破棄する前に
/// best-effort で flush する — バッファ済み (まだ staging Parquet に書かれていない)
/// イベントを、`config.json` を読み直して新しい `S3Sink` に差し替える前に永続化
/// しておく (staging ディレクトリ自体はリトライキューなので、これで設定変更をまたいで
/// もイベントを失わない)。
fn reload_s3_sink(s3_sink: &mut Option<S3Sink>, host_id: &str) {
    if let Some(old) = s3_sink.as_mut() {
        if let Err(e) = EventSink::flush(old) {
            eprintln!(
                "kikimimi agent: s3 sink pre-reload flush failed (buffered events stay in \
                 memory and are dropped on reload — a restart, not just `kikimimi sink add`/\
                 `remove`, is needed to fully recover): {e:#}"
            );
        }
    }
    *s3_sink = build_s3_sink(host_id);
    eprintln!(
        "kikimimi agent: reloaded s3 sink from config.json ({})",
        if s3_sink.is_some() {
            "configured"
        } else {
            "not configured"
        }
    );
}

/// axum サーバーを spawn し、bind が一瞬 (150ms) で失敗しなかったかを見て
/// 「起動成功でタスクを継続監視」か「エラーを state に記録して hooks のみで続行」かを分ける。
/// 呼び出し側が保持する `shutdown_tx` を送信すると `serve` がグレースフルに終了する。
async fn start_otlp(
    addr: std::net::SocketAddr,
    tx: mpsc::Sender<OtlpPayload>,
    auth: std::sync::Arc<std::sync::RwLock<Option<String>>>,
    rejected: std::sync::Arc<std::sync::atomic::AtomicU64>,
) -> (
    Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
    Option<tokio::sync::oneshot::Sender<()>>,
    Option<String>,
) {
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let handle = tokio::spawn(async move {
        let shutdown = async move {
            let _ = shutdown_rx.await;
        };
        kikimimi_otlp::serve(addr, tx, auth, rejected, shutdown).await
    });

    tokio::time::sleep(Duration::from_millis(150)).await;

    if handle.is_finished() {
        match handle.await {
            Ok(Ok(())) => (None, None, None),
            Ok(Err(e)) => (
                None,
                None,
                Some(format!("otlp receiver failed to start on {addr}: {e:#}")),
            ),
            Err(e) => (
                None,
                None,
                Some(format!("otlp receiver task panicked: {e}")),
            ),
        }
    } else {
        (Some(handle), Some(shutdown_tx), None)
    }
}

/// `sink.maybe_flush()` / `EventSink::flush()` の結果を state に反映する。
/// 失敗時は `state.last_flush_error` にメッセージを残す (buffered events はイベント側で
/// 保持されたまま — sink 側の flush() は失敗時にイベントを buf に戻す。sink/src/lib.rs 参照)。
/// 成功時 (0 件含む) は前回のエラーをクリアする。
fn apply_flush_result(state: &mut AgentState, result: anyhow::Result<Vec<PathBuf>>) {
    match result {
        Ok(files) => {
            record_flush(state, files);
            state.last_flush_error = None;
        }
        Err(e) => {
            eprintln!("kikimimi agent: sink flush failed, buffered events kept for retry: {e:#}");
            state.last_flush_error = Some(format!("{e:#}"));
        }
    }
}

/// cloud sink の flush 結果をログに残すだけの薄いラッパー。state への反映は
/// [`sync_cloud_state`] が `CloudSink` 自身の getter (`pending`/`last_error`/
/// `last_push_at_ms`) から都度作り直すので、ここでは失敗を握りつぶさず見えるようにする。
fn apply_cloud_flush_result(result: anyhow::Result<Vec<PathBuf>>) {
    if let Err(e) = result {
        eprintln!("kikimimi agent: cloud sink flush failed, buffered events kept for retry: {e:#}");
    }
}

/// `state.cloud` を `CloudSink` の現在値から作り直す。cloud が未設定 (`kikimimi login`
/// 前) なら `None` のまま — state.json の後方互換 (`#[serde(default)]`) と対称に、
/// cloud 未使用のデーモンは以前と同じ state.json を書く。
fn sync_cloud_state(state: &mut AgentState, cloud_sink: Option<&CloudSink>) {
    state.cloud = cloud_sink.map(|cs| CloudState {
        endpoint: cs.endpoint().to_string(),
        pending: cs.pending(),
        last_push_at: cs.last_push_at_ms(),
        last_error: cs.last_error().map(|s| s.to_string()),
    });
}

/// s3 sink の flush 結果をログに残すだけの薄いラッパー ([`apply_cloud_flush_result`]
/// と同じ形)。state への反映は [`sync_s3_state`] が `S3Sink` 自身の getter から都度
/// 作り直す。
fn apply_s3_flush_result(result: anyhow::Result<Vec<PathBuf>>) {
    if let Err(e) = result {
        eprintln!("kikimimi agent: s3 sink flush failed, buffered events kept for retry: {e:#}");
    }
}

/// `state.s3` を `S3Sink` の現在値から作り直す ([`sync_cloud_state`] と同じ形)。
/// s3 sink が未設定 (`kikimimi sink add s3` 前) なら `None` のまま。
fn sync_s3_state(state: &mut AgentState, s3_sink: Option<&S3Sink>) {
    state.s3 = s3_sink.map(|s| S3State {
        url: s.url().to_string(),
        pending: s.pending(),
        last_push_at: s.last_push_at_ms(),
        last_error: s.last_error().map(|s| s.to_string()),
    });
}

/// 同時に処理中の control socket 接続数の上限。ローカルのみが信頼できる相手として
/// 繋いでくる socket なので深刻度は低いが、何かが接続を張っては読み込まずに詰まらせる
/// (あるいは単に極端な速さで繋ぎ続ける) 場合でも、spawn するタスク数に上限を設けておく。
const MAX_CONCURRENT_CONTROL_CONNECTIONS: usize = 64;

async fn accept_control_loop(listener: tokio::net::UnixListener, ctrl_tx: mpsc::Sender<u8>) {
    let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(
        MAX_CONCURRENT_CONTROL_CONNECTIONS,
    ));
    loop {
        match listener.accept().await {
            Ok((mut stream, _addr)) => {
                // acquire_owned never fails here: the semaphore is never closed, and a
                // "no permits left" case just waits rather than erroring — which is the
                // desired backpressure (bounded concurrency instead of unbounded spawn).
                let Ok(permit) = permits.clone().acquire_owned().await else {
                    continue;
                };
                let tx = ctrl_tx.clone();
                tokio::spawn(async move {
                    let _permit = permit; // held until this task finishes, then released
                    let mut buf = [0u8; 1];
                    if stream.read_exact(&mut buf).await.is_ok() {
                        let _ = tx.send(buf[0]).await;
                    }
                });
            }
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

/// 処理できなかった spool エントリを削除ではなく退避する。`SpoolReader::quarantine`
/// のドキュメント通り、`.poisoned/` は `list()` から見えないので無限リトライにはならない。
fn quarantine_entry(reader: &SpoolReader, path: &Path) {
    if let Err(e) = reader.quarantine(path) {
        eprintln!(
            "kikimimi agent: failed to quarantine poisoned spool entry {} ({e:#}); it was dropped instead",
            path.display()
        );
    }
}

/// spool に溜まっている完了済みエントリを全て読み、正規化して sink に積み、消費する。
///
/// 読み込み・JSON パース・正規化のいずれかが失敗したエントリは `.poisoned/` へ退避する
/// (削除ではなく)。以前は読み込み失敗を `continue` で素通りして spool に残しており、
/// そのエントリは次回以降も同じ理由で失敗し続け、`kikimimi status` の spool backlog
/// warning を恒久的に誤発火させながら永遠にリトライされていた。
fn drain_spool(
    reader: &SpoolReader,
    normalizer: &mut Normalizer,
    sink: &mut FileSink,
    mut cloud_sink: Option<&mut CloudSink>,
    mut s3_sink: Option<&mut S3Sink>,
    repo_filter: &crate::repo_filter::RepoFilter,
    repo_resolver: &mut crate::repo_resolve::RepoResolver,
    state: &mut AgentState,
    malformed: &mut u64,
) {
    for path in reader.list() {
        let bytes = match reader.read(&path) {
            Ok(b) => b,
            Err(_) => {
                *malformed += 1;
                quarantine_entry(reader, &path);
                continue;
            }
        };

        let mut raw: Value = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(_) => {
                *malformed += 1;
                quarantine_entry(reader, &path);
                continue;
            }
        };

        // The hook JSON normally carries its own "hook_event_name", but fall back to the
        // kind embedded in the spool filename (`<epoch_ms>-<uuid>.<kind>.json`) if absent.
        if raw.get("hook_event_name").and_then(Value::as_str).is_none() {
            if let (Value::Object(map), Some(kind)) = (&mut raw, kind_from_filename(&path)) {
                map.insert("hook_event_name".to_string(), Value::String(kind));
            }
        }

        match normalizer.hook(&raw) {
            Ok(events) => {
                for mut ev in events {
                    // issue #4: Claude Code hook adapter doesn't know the repo itself (it
                    // only sees `cwd_hash`, §5.2 privacy), so derive `ev.repo` here,
                    // daemon-side, from the raw hook payload's plaintext "cwd" by reading
                    // `.git/config` (no `git` subprocess, cached per cwd). OTel events carry
                    // no "cwd" at all and stay `None`, same as before.
                    if ev.repo.is_none() {
                        if let Some(cwd) = raw.get("cwd").and_then(Value::as_str) {
                            ev.repo = repo_resolver.resolve(cwd);
                        }
                    }
                    bump_source(state, &ev.source);
                    bump_last_event_ts(state, ev.ts);
                    // §6.1: the team-org repo filter only ever gates the cloud sink.
                    if let Some(cs) = cloud_sink.as_deref_mut() {
                        if repo_filter.allows(ev.repo.as_deref()) {
                            cs.push(ev.clone());
                        }
                    }
                    // BYO sinks receive the full, unmasked event (§5.2) — same as FileSink.
                    if let Some(s3) = s3_sink.as_deref_mut() {
                        s3.push(ev.clone());
                    }
                    sink.push(ev);
                }
                let _ = reader.remove(&path);
            }
            Err(_) => {
                *malformed += 1;
                quarantine_entry(reader, &path);
            }
        }
    }
    sync_skipped(state, normalizer, *malformed);
}

fn ingest_otlp(
    payload: OtlpPayload,
    normalizer: &mut Normalizer,
    sink: &mut FileSink,
    mut cloud_sink: Option<&mut CloudSink>,
    mut s3_sink: Option<&mut S3Sink>,
    repo_filter: &crate::repo_filter::RepoFilter,
    state: &mut AgentState,
    malformed: u64,
) {
    match payload {
        OtlpPayload::Logs(req) => {
            if let Ok(events) = normalizer.otlp_logs(&req) {
                for ev in events {
                    bump_source(state, &ev.source);
                    bump_last_event_ts(state, ev.ts);
                    if let Some(cs) = cloud_sink.as_deref_mut() {
                        if repo_filter.allows(ev.repo.as_deref()) {
                            cs.push(ev.clone());
                        }
                    }
                    // BYO sinks receive the full, unmasked event (§5.2) — same as FileSink.
                    if let Some(s3) = s3_sink.as_deref_mut() {
                        s3.push(ev.clone());
                    }
                    sink.push(ev);
                }
            }
        }
        OtlpPayload::Metrics(req) => {
            // Stage 0: kikimimi_adapter_claude::Normalizer::otlp_metrics always returns [] (see its docs).
            if let Ok(events) = normalizer.otlp_metrics(&req) {
                for ev in events {
                    bump_source(state, &ev.source);
                    bump_last_event_ts(state, ev.ts);
                    if let Some(cs) = cloud_sink.as_deref_mut() {
                        if repo_filter.allows(ev.repo.as_deref()) {
                            cs.push(ev.clone());
                        }
                    }
                    // BYO sinks receive the full, unmasked event (§5.2) — same as FileSink.
                    if let Some(s3) = s3_sink.as_deref_mut() {
                        s3.push(ev.clone());
                    }
                    sink.push(ev);
                }
            }
        }
        OtlpPayload::Traces(_) => {
            // Not part of kikimimi.v1 in Stage 0.
        }
    }
    sync_skipped(state, normalizer, malformed);
}

/// `state.skipped` / `state.skipped_by_reason` を、Normalizer が数えている理由別内訳と
/// デーモン側で読めなかった/パースできなかった spool ファイルの件数 (`malformed`,
/// key "malformed_spool") から作り直す。呼び出しごとに正規化された最新の合計を作るので、
/// `state.skipped = normalizer.skipped() + malformed` を毎回別々に書いていた旧実装と違い
/// 内訳と合計が食い違うことがない。
fn sync_skipped(state: &mut AgentState, normalizer: &Normalizer, malformed: u64) {
    let mut by_reason: BTreeMap<String, u64> = normalizer
        .skipped_by_reason()
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    if malformed > 0 {
        by_reason.insert("malformed_spool".to_string(), malformed);
    }
    state.skipped = normalizer.skipped() + malformed;
    state.skipped_by_reason = by_reason;
}

fn bump_source(state: &mut AgentState, source: &str) {
    match source {
        "hook" => state.events_by_source.hook += 1,
        "otel" => state.events_by_source.otel += 1,
        // architecture.md §5.1: Codex rollout tailer events use source="log".
        "log" => state.events_by_source.log += 1,
        _ => {}
    }
}

fn bump_last_event_ts(state: &mut AgentState, ts: i64) {
    state.last_event_ts = Some(state.last_event_ts.map_or(ts, |cur| cur.max(ts)));
}

fn record_flush(state: &mut AgentState, files: Vec<PathBuf>) {
    if files.is_empty() {
        return;
    }
    state.last_flush = Some(LastFlush {
        at_ms: now_ms(),
        files: files
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect(),
    });
}

#[allow(clippy::too_many_arguments)]
fn save_state_now(
    state: &AgentState,
    malformed: &mut u64,
    normalizer: &Normalizer,
    cloud_sink: Option<&CloudSink>,
    s3_sink: Option<&S3Sink>,
    codex_tailer: &CodexTailer,
    codex_normalizer: &CodexNormalizer,
    otlp_auth: &std::sync::Arc<std::sync::RwLock<Option<String>>>,
    otlp_rejected: &std::sync::Arc<std::sync::atomic::AtomicU64>,
) {
    let mut s = state.clone();
    sync_skipped(&mut s, normalizer, *malformed);
    sync_cloud_state(&mut s, cloud_sink);
    sync_s3_state(&mut s, s3_sink);
    sync_codex_state(&mut s, codex_tailer, codex_normalizer);
    sync_otlp_auth_state(&mut s, otlp_auth, otlp_rejected);
    if let Err(e) = s.save() {
        eprintln!("kikimimi agent: failed to save state.json: {e:#}");
    }
}

/// `state.otlp_auth_enabled`/`state.otlp_rejected` を otlp crate 側の生きたハンドルから
/// 作り直す (`sync_cloud_state`/`sync_s3_state` と同じ「都度サンプリング」の形)。
/// `otlp_rejected` はプロセス生存期間中の累計 (`AtomicU64`) をそのまま写すだけで、
/// state.json 側で加算はしない。
fn sync_otlp_auth_state(
    state: &mut AgentState,
    otlp_auth: &std::sync::Arc<std::sync::RwLock<Option<String>>>,
    otlp_rejected: &std::sync::Arc<std::sync::atomic::AtomicU64>,
) {
    state.otlp_auth_enabled = otlp_auth
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .is_some();
    state.otlp_rejected = otlp_rejected.load(std::sync::atomic::Ordering::Relaxed);
}

/// 制御バイト `b'r'` (reload) の一部: `otlp_auth` ハンドルの中身を config.json 最新値に
/// 差し替える。`kikimimi_otlp::serve` はこのハンドルを見ながら動いているので、
/// デーモンを再起動せずに `kikimimi init` が発行した (または削除した) トークンを反映できる。
fn reload_otlp_auth(
    otlp_auth: &std::sync::Arc<std::sync::RwLock<Option<String>>>,
    token: Option<String>,
) {
    match otlp_auth.write() {
        Ok(mut guard) => *guard = token,
        Err(poisoned) => *poisoned.into_inner() = token,
    }
}

/// `state.codex` を `CodexTailer`/`CodexNormalizer` の現在値から作り直す
/// (`sync_cloud_state`/`sync_s3_state` と同じ形)。
fn sync_codex_state(state: &mut AgentState, tailer: &CodexTailer, normalizer: &CodexNormalizer) {
    state.codex = CodexTailerState {
        files_watched: tailer.files_watched(),
        lines_read: tailer.lines_read(),
        malformed_lines: tailer.malformed_lines(),
        skipped: normalizer.skipped(),
        skipped_by_reason: normalizer
            .skipped_by_reason()
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect(),
    };
}

/// Codex rollout tailer を 1 回スキャンし、生まれたイベントを (hook/OTel と同じく)
/// FileSink/CloudSink/S3Sink すべてに push する (`drain_spool`/`ingest_otlp` と同じ形)。
/// スキャン自体が失敗しても (`~/.codex` の権限エラー等) デーモンは止めない。
fn drain_codex(
    tailer: &mut CodexTailer,
    normalizer: &mut CodexNormalizer,
    sink: &mut FileSink,
    mut cloud_sink: Option<&mut CloudSink>,
    mut s3_sink: Option<&mut S3Sink>,
    repo_filter: &crate::repo_filter::RepoFilter,
    state: &mut AgentState,
) {
    match tailer.scan_and_drain(normalizer) {
        Ok(events) => {
            for ev in events {
                bump_source(state, &ev.source);
                bump_last_event_ts(state, ev.ts);
                if let Some(cs) = cloud_sink.as_deref_mut() {
                    if repo_filter.allows(ev.repo.as_deref()) {
                        cs.push(ev.clone());
                    }
                }
                // BYO sinks receive the full, unmasked event (§5.2) — same as FileSink.
                if let Some(s3) = s3_sink.as_deref_mut() {
                    s3.push(ev.clone());
                }
                sink.push(ev);
            }
        }
        Err(e) => {
            eprintln!("kikimimi agent: codex rollout tailer scan failed: {e:#}");
        }
    }
    sync_codex_state(state, tailer, normalizer);
}

/// `<epoch_ms>-<uuid>.<kind>.json` から kind を取り出す。
fn kind_from_filename(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let base = name.strip_suffix(".json")?;
    let (_, kind) = base.rsplit_once('.')?;
    Some(kind.to_string())
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_from_filename_parses_hook_kind() {
        let p = Path::new("/tmp/spool/1700000000000-abcd1234.PreToolUse.json");
        assert_eq!(kind_from_filename(p).as_deref(), Some("PreToolUse"));
    }

    #[test]
    fn kind_from_filename_none_for_malformed_name() {
        assert_eq!(kind_from_filename(Path::new("noext")), None);
    }

    #[test]
    fn bump_last_event_ts_keeps_max() {
        let mut s = AgentState::new(1, 0, 4318);
        bump_last_event_ts(&mut s, 100);
        bump_last_event_ts(&mut s, 50);
        bump_last_event_ts(&mut s, 200);
        assert_eq!(s.last_event_ts, Some(200));
    }

    #[test]
    fn bump_source_counts_hook_and_otel_separately() {
        let mut s = AgentState::new(1, 0, 4318);
        bump_source(&mut s, "hook");
        bump_source(&mut s, "hook");
        bump_source(&mut s, "otel");
        bump_source(&mut s, "log");
        bump_source(&mut s, "unknown-source");
        assert_eq!(s.events_by_source.hook, 2);
        assert_eq!(s.events_by_source.otel, 1);
        assert_eq!(s.events_by_source.log, 1);
    }

    #[test]
    fn apply_flush_result_clears_error_on_success_and_sets_it_on_failure() {
        let mut s = AgentState::new(1, 0, 4318);
        s.last_flush_error = Some("stale error from before".into());

        apply_flush_result(&mut s, Ok(vec![PathBuf::from("a.parquet")]));
        assert_eq!(
            s.last_flush_error, None,
            "a success must clear any prior error"
        );
        assert!(s.last_flush.is_some());

        apply_flush_result(&mut s, Err(anyhow::anyhow!("disk full")));
        assert_eq!(s.last_flush_error.as_deref(), Some("disk full"));
    }

    #[test]
    fn record_flush_ignores_empty_and_records_nonempty() {
        let mut s = AgentState::new(1, 0, 4318);
        record_flush(&mut s, vec![]);
        assert!(s.last_flush.is_none());
        record_flush(&mut s, vec![PathBuf::from("a.parquet")]);
        assert!(s.last_flush.is_some());
        assert_eq!(s.last_flush.unwrap().files, vec!["a.parquet".to_string()]);
    }

    /// architecture.md §4.1: hooks に無い情報 (トークン等) は OTel 側でしか来ない。
    /// drain_spool / ingest_otlp が同じ Normalizer・同じ FileSink を共有しても
    /// カウンタが正しく別れることを、実際の正規化を通して確認する。
    #[test]
    fn drain_spool_updates_state_and_removes_processed_files() {
        let dir = tempfile::tempdir().unwrap();
        let raw = serde_json::json!({
            "hook_event_name": "PreToolUse",
            "session_id": "sess-1",
            "cwd": "/tmp",
            "tool_name": "Bash",
            "tool_use_id": "toolu_1"
        });
        let path =
            kikimimi_spool::write_entry_in(dir.path(), "PreToolUse", raw.to_string().as_bytes())
                .unwrap();
        assert!(path.exists());

        let reader = SpoolReader::new_in(dir.path());
        let mut normalizer = Normalizer::new("host-1".into());
        let sink_dir = tempfile::tempdir().unwrap();
        let mut sink = FileSink::new(
            sink_dir.path().to_path_buf(),
            "host-1".into(),
            500,
            Duration::from_secs(30),
        );
        let mut state = AgentState::new(1, 0, 4318);
        let mut malformed = 0u64;
        let mut repo_resolver = crate::repo_resolve::RepoResolver::default();

        drain_spool(
            &reader,
            &mut normalizer,
            &mut sink,
            None,
            None,
            &crate::repo_filter::RepoFilter::default(),
            &mut repo_resolver,
            &mut state,
            &mut malformed,
        );

        assert!(!path.exists(), "processed spool entry must be removed");
        assert_eq!(state.events_by_source.hook, 1);
        assert_eq!(state.events_by_source.otel, 0);
        assert_eq!(sink.pending(), 1);
        assert_eq!(malformed, 0);
    }

    #[test]
    fn drain_spool_counts_malformed_json_and_still_removes_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = kikimimi_spool::write_entry_in(dir.path(), "PreToolUse", b"not json").unwrap();

        let reader = SpoolReader::new_in(dir.path());
        let mut normalizer = Normalizer::new("host-1".into());
        let sink_dir = tempfile::tempdir().unwrap();
        let mut sink = FileSink::new(
            sink_dir.path().to_path_buf(),
            "host-1".into(),
            500,
            Duration::from_secs(30),
        );
        let mut state = AgentState::new(1, 0, 4318);
        let mut malformed = 0u64;
        let mut repo_resolver = crate::repo_resolve::RepoResolver::default();

        drain_spool(
            &reader,
            &mut normalizer,
            &mut sink,
            None,
            None,
            &crate::repo_filter::RepoFilter::default(),
            &mut repo_resolver,
            &mut state,
            &mut malformed,
        );

        assert!(!path.exists());
        assert_eq!(malformed, 1);
        assert_eq!(state.skipped, 1);
        assert_eq!(
            state.skipped_by_reason.get("malformed_spool"),
            Some(&1),
            "daemon-side unreadable/unparsable spool files must show up under malformed_spool"
        );
        assert_eq!(sink.pending(), 0);
    }

    /// state.skipped_by_reason must combine the Normalizer's per-hook-name reasons with the
    /// daemon-side "malformed_spool" bucket, and the two must never collide/overwrite each
    /// other (sync_skipped rebuilds the whole map from scratch on every call).
    #[test]
    fn drain_spool_records_both_unknown_hook_reason_and_malformed_spool() {
        let dir = tempfile::tempdir().unwrap();
        let unknown = serde_json::json!({
            "hook_event_name": "PreCompact",
            "session_id": "sess-1"
        });
        kikimimi_spool::write_entry_in(dir.path(), "PreCompact", unknown.to_string().as_bytes())
            .unwrap();
        kikimimi_spool::write_entry_in(dir.path(), "PreToolUse", b"not json").unwrap();

        let reader = SpoolReader::new_in(dir.path());
        let mut normalizer = Normalizer::new("host-1".into());
        let sink_dir = tempfile::tempdir().unwrap();
        let mut sink = FileSink::new(
            sink_dir.path().to_path_buf(),
            "host-1".into(),
            500,
            Duration::from_secs(30),
        );
        let mut state = AgentState::new(1, 0, 4318);
        let mut malformed = 0u64;
        let mut repo_resolver = crate::repo_resolve::RepoResolver::default();

        drain_spool(
            &reader,
            &mut normalizer,
            &mut sink,
            None,
            None,
            &crate::repo_filter::RepoFilter::default(),
            &mut repo_resolver,
            &mut state,
            &mut malformed,
        );

        assert_eq!(state.skipped, 2);
        assert_eq!(state.skipped_by_reason.get("PreCompact"), Some(&1));
        assert_eq!(state.skipped_by_reason.get("malformed_spool"), Some(&1));
        assert_eq!(state.skipped_by_reason.len(), 2);
    }

    #[test]
    fn drain_spool_falls_back_to_filename_kind_when_json_lacks_it() {
        let dir = tempfile::tempdir().unwrap();
        // No hook_event_name field at all; must be recovered from the filename.
        let raw = serde_json::json!({ "session_id": "sess-1" });
        kikimimi_spool::write_entry_in(dir.path(), "SessionStart", raw.to_string().as_bytes())
            .unwrap();

        let reader = SpoolReader::new_in(dir.path());
        let mut normalizer = Normalizer::new("host-1".into());
        let sink_dir = tempfile::tempdir().unwrap();
        let mut sink = FileSink::new(
            sink_dir.path().to_path_buf(),
            "host-1".into(),
            500,
            Duration::from_secs(30),
        );
        let mut state = AgentState::new(1, 0, 4318);
        let mut malformed = 0u64;
        let mut repo_resolver = crate::repo_resolve::RepoResolver::default();

        drain_spool(
            &reader,
            &mut normalizer,
            &mut sink,
            None,
            None,
            &crate::repo_filter::RepoFilter::default(),
            &mut repo_resolver,
            &mut state,
            &mut malformed,
        );

        assert_eq!(
            state.events_by_source.hook, 1,
            "SessionStart recovered from filename must normalize"
        );
        assert_eq!(sink.pending(), 1);
    }

    /// Regression test for "poisoned spool entry retried forever": an entry that can't
    /// even be read (here: a directory sitting where a completed spool entry's filename
    /// pattern would be — the reviewer's own repro shape) must be quarantined on the
    /// first pass, not left in place to be re-attempted (and mis-reported as "backlog
    /// growing") on every subsequent tick indefinitely.
    #[test]
    fn drain_spool_quarantines_unreadable_entries_instead_of_retrying_forever() {
        let dir = tempfile::tempdir().unwrap();
        let poisoned = dir.path().join("1700000000000-deadbeef.PreToolUse.json");
        fs::create_dir(&poisoned).unwrap();

        let reader = SpoolReader::new_in(dir.path());
        let mut normalizer = Normalizer::new("host-1".into());
        let sink_dir = tempfile::tempdir().unwrap();
        let mut sink = FileSink::new(
            sink_dir.path().to_path_buf(),
            "host-1".into(),
            500,
            Duration::from_secs(30),
        );
        let mut state = AgentState::new(1, 0, 4318);
        let mut malformed = 0u64;
        let mut repo_resolver = crate::repo_resolve::RepoResolver::default();

        // First pass: the reader's own list() already filters out non-file entries, so
        // this asserts the higher-level, end-to-end behavior stays stable across passes.
        for _ in 0..3 {
            drain_spool(
                &reader,
                &mut normalizer,
                &mut sink,
                None,
                None,
                &crate::repo_filter::RepoFilter::default(),
                &mut repo_resolver,
                &mut state,
                &mut malformed,
            );
        }

        assert_eq!(
            reader.list().len(),
            0,
            "must not still be sitting in the backlog after repeated passes"
        );
        assert_eq!(sink.pending(), 0);
    }

    /// Two things this task cares about: (1) the s3 (BYO) sink receives the same,
    /// full-body event as FileSink (§5.2 — BYO sinks are not masked), and (2) a broken
    /// s3 sink (here: an uploader binary that doesn't exist) never blocks or drops
    /// events for the *other* sinks — `drain_spool` must still hand the event to
    /// FileSink, and FileSink's own flush must still succeed and land on disk.
    #[test]
    fn drain_spool_and_flush_give_s3_sink_full_body_with_failure_isolation() {
        let dir = tempfile::tempdir().unwrap();
        let raw = serde_json::json!({
            "hook_event_name": "PreToolUse",
            "session_id": "sess-1",
            "cwd": "/tmp",
            "tool_name": "Bash",
            "tool_use_id": "toolu_1",
            "tool_input": { "command": "echo hi" }
        });
        let path =
            kikimimi_spool::write_entry_in(dir.path(), "PreToolUse", raw.to_string().as_bytes())
                .unwrap();

        let reader = SpoolReader::new_in(dir.path());
        let mut normalizer = Normalizer::new("host-1".into());
        let sink_dir = tempfile::tempdir().unwrap();
        let mut sink = FileSink::new(
            sink_dir.path().to_path_buf(),
            "host-1".into(),
            500,
            Duration::from_secs(30),
        );
        let staging_dir = dir.path().join("s3-staging");
        let mut s3_sink = S3Sink::new(
            S3Config {
                url: "s3://fake-bucket/team".to_string(),
                profile: None,
                endpoint_url: None,
                // A binary that will never exist -- simulates the "aws CLI not
                // installed"/BYO-sink-misconfigured case without needing a fake script.
                uploader: Some(dir.path().join("no-such-uploader").display().to_string()),
            },
            "host-1".to_string(),
            staging_dir,
        );
        let mut state = AgentState::new(1, 0, 4318);
        let mut malformed = 0u64;
        let mut repo_resolver = crate::repo_resolve::RepoResolver::default();

        drain_spool(
            &reader,
            &mut normalizer,
            &mut sink,
            None,
            Some(&mut s3_sink),
            &crate::repo_filter::RepoFilter::default(),
            &mut repo_resolver,
            &mut state,
            &mut malformed,
        );

        assert!(!path.exists(), "processed spool entry must be removed");
        assert_eq!(sink.pending(), 1, "FileSink must have received the event");
        assert_eq!(
            s3_sink.pending(),
            1,
            "s3 sink must have received the same event (BYO sinks get full body)"
        );

        // The s3 sink's upload will fail (missing uploader binary) -- that must not
        // stop FileSink from flushing successfully (sink isolation, agent.rs's select!
        // loop calls each sink's flush independently and never lets one `?` the other).
        let file_written = EventSink::flush(&mut sink);
        assert!(
            file_written.is_ok(),
            "FileSink flush must succeed even though the s3 sink is broken: {file_written:?}"
        );
        let written = file_written.unwrap();
        assert_eq!(written.len(), 1);
        assert!(
            written[0].exists(),
            "FileSink's parquet must actually land on disk"
        );

        let s3_result = EventSink::flush(&mut s3_sink);
        assert!(
            s3_result.is_err(),
            "the broken s3 sink's flush must surface its own error"
        );
        assert_eq!(s3_sink.last_error(), Some("aws CLI not found"));
    }

    // -----------------------------------------------------------------------
    // §6.1 repo filter integration: cloud sink only, file sink unaffected.
    // -----------------------------------------------------------------------

    /// A hook-sourced event (Claude Code adapter never sets `repo` -- see
    /// `repo_filter.rs`'s module docs) on a team org with a configured, non-matching
    /// allowlist must still land in FileSink (nothing is ever dropped locally) but must be
    /// held back from the cloud sink.
    #[test]
    fn drain_spool_filters_repo_less_event_from_cloud_on_team_org_with_patterns() {
        let dir = tempfile::tempdir().unwrap();
        let raw = serde_json::json!({
            "hook_event_name": "PreToolUse",
            "session_id": "sess-1",
            "cwd": "/tmp",
            "tool_name": "Bash",
            "tool_use_id": "toolu_1"
        });
        kikimimi_spool::write_entry_in(dir.path(), "PreToolUse", raw.to_string().as_bytes())
            .unwrap();

        let reader = SpoolReader::new_in(dir.path());
        let mut normalizer = Normalizer::new("host-1".into());
        let sink_dir = tempfile::tempdir().unwrap();
        let mut sink = FileSink::new(
            sink_dir.path().to_path_buf(),
            "host-1".into(),
            500,
            Duration::from_secs(30),
        );
        let mut cloud_sink = CloudSink::new(
            "http://127.0.0.1:1".into(), // never contacted: push() never sends over the wire
            "tok".into(),
            "host-1".into(),
        );
        let team_cloud_cfg = crate::config::CloudConfig {
            org_kind: "team".to_string(),
            org_slug: "acme".to_string(),
            repo_patterns: vec!["github.com/acme/*".to_string()],
            ..Default::default()
        };
        let filter = crate::repo_filter::RepoFilter::from_cloud_config(Some(&team_cloud_cfg));
        let mut state = AgentState::new(1, 0, 4318);
        let mut malformed = 0u64;
        let mut repo_resolver = crate::repo_resolve::RepoResolver::default();

        drain_spool(
            &reader,
            &mut normalizer,
            &mut sink,
            Some(&mut cloud_sink),
            None,
            &filter,
            &mut repo_resolver,
            &mut state,
            &mut malformed,
        );

        assert_eq!(sink.pending(), 1, "FileSink must still receive every event");
        assert_eq!(
            cloud_sink.pending(),
            0,
            "an event with no repo info must not reach the cloud sink once a team org has a \
             configured allowlist"
        );
    }

    /// The same event, same team org, but with an *empty* allowlist (§6.1: "empty/absent
    /// patterns = send everything") -- must reach both FileSink and the cloud sink,
    /// confirming the filter only blocks once patterns are actually configured, not simply
    /// because the org is a team org.
    #[test]
    fn drain_spool_sends_everything_to_cloud_when_team_org_has_no_patterns_configured() {
        let dir = tempfile::tempdir().unwrap();
        let raw = serde_json::json!({
            "hook_event_name": "PreToolUse",
            "session_id": "sess-1",
            "cwd": "/tmp",
            "tool_name": "Bash",
            "tool_use_id": "toolu_1"
        });
        kikimimi_spool::write_entry_in(dir.path(), "PreToolUse", raw.to_string().as_bytes())
            .unwrap();

        let reader = SpoolReader::new_in(dir.path());
        let mut normalizer = Normalizer::new("host-1".into());
        let sink_dir = tempfile::tempdir().unwrap();
        let mut sink = FileSink::new(
            sink_dir.path().to_path_buf(),
            "host-1".into(),
            500,
            Duration::from_secs(30),
        );
        let mut cloud_sink =
            CloudSink::new("http://127.0.0.1:1".into(), "tok".into(), "host-1".into());
        let team_cloud_cfg = crate::config::CloudConfig {
            org_kind: "team".to_string(),
            org_slug: "acme".to_string(),
            repo_patterns: Vec::new(),
            ..Default::default()
        };
        let filter = crate::repo_filter::RepoFilter::from_cloud_config(Some(&team_cloud_cfg));
        let mut state = AgentState::new(1, 0, 4318);
        let mut malformed = 0u64;
        let mut repo_resolver = crate::repo_resolve::RepoResolver::default();

        drain_spool(
            &reader,
            &mut normalizer,
            &mut sink,
            Some(&mut cloud_sink),
            None,
            &filter,
            &mut repo_resolver,
            &mut state,
            &mut malformed,
        );

        assert_eq!(sink.pending(), 1);
        assert_eq!(
            cloud_sink.pending(),
            1,
            "an unconfigured (empty-patterns) team-org allowlist must not hold anything back"
        );
    }

    fn write_file(dir: &Path, rel: &str, contents: &str) -> PathBuf {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, contents).unwrap();
        path
    }

    /// Same fixture file `codex_tailer.rs`'s own tests use, one JSON value per line
    /// (rollout JSONL shape). Its `git.repository_url` is
    /// `"git@github.com:example-org/example-repo.git"`.
    fn codex_session_meta_fixture() -> String {
        let path = format!(
            "{}/../adapter-codex/tests/fixtures/rollout_line_session_meta.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let raw = fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"));
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        format!("{}\n", serde_json::to_string(&v).unwrap())
    }

    /// End-to-end through the real Codex rollout tailer (the one adapter that actually
    /// populates `Event::repo`, from the session's `git.repository_url` — Claude Code hook
    /// events never do, see the two tests above): a repo that *matches* the team org's
    /// allowlist must reach the cloud sink, proving the filter isn't just a one-way "always
    /// block" switch.
    #[test]
    fn drain_codex_pushes_matching_repo_to_cloud_sink() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = dir.path().join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        write_file(&sessions, "rollout-a.jsonl", &codex_session_meta_fixture());
        let cursors = dir.path().join("cursors.json");

        let mut tailer = CodexTailer::new_in(sessions, cursors);
        let mut codex_normalizer = CodexNormalizer::new("host-1".into());
        let sink_dir = tempfile::tempdir().unwrap();
        let mut sink = FileSink::new(
            sink_dir.path().to_path_buf(),
            "host-1".into(),
            500,
            Duration::from_secs(30),
        );
        let mut cloud_sink =
            CloudSink::new("http://127.0.0.1:1".into(), "tok".into(), "host-1".into());
        let team_cloud_cfg = crate::config::CloudConfig {
            org_kind: "team".to_string(),
            org_slug: "acme".to_string(),
            // Matches the fixture's "git@github.com:example-org/example-repo.git".
            repo_patterns: vec!["*example-org/example-repo*".to_string()],
            ..Default::default()
        };
        let filter = crate::repo_filter::RepoFilter::from_cloud_config(Some(&team_cloud_cfg));
        let mut state = AgentState::new(1, 0, 4318);

        drain_codex(
            &mut tailer,
            &mut codex_normalizer,
            &mut sink,
            Some(&mut cloud_sink),
            None,
            &filter,
            &mut state,
        );

        assert_eq!(sink.pending(), 1);
        assert_eq!(
            cloud_sink.pending(),
            1,
            "a repo matching the team org's allowlist must reach the cloud sink"
        );
    }

    /// Same fixture, but the team org's allowlist doesn't match this repo at all: FileSink
    /// still gets it, the cloud sink does not.
    #[test]
    fn drain_codex_filters_non_matching_repo_from_cloud_sink_but_keeps_file_sink() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = dir.path().join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        write_file(&sessions, "rollout-a.jsonl", &codex_session_meta_fixture());
        let cursors = dir.path().join("cursors.json");

        let mut tailer = CodexTailer::new_in(sessions, cursors);
        let mut codex_normalizer = CodexNormalizer::new("host-1".into());
        let sink_dir = tempfile::tempdir().unwrap();
        let mut sink = FileSink::new(
            sink_dir.path().to_path_buf(),
            "host-1".into(),
            500,
            Duration::from_secs(30),
        );
        let mut cloud_sink =
            CloudSink::new("http://127.0.0.1:1".into(), "tok".into(), "host-1".into());
        let team_cloud_cfg = crate::config::CloudConfig {
            org_kind: "team".to_string(),
            org_slug: "acme".to_string(),
            repo_patterns: vec!["github.com/someone-else/*".to_string()],
            ..Default::default()
        };
        let filter = crate::repo_filter::RepoFilter::from_cloud_config(Some(&team_cloud_cfg));
        let mut state = AgentState::new(1, 0, 4318);

        drain_codex(
            &mut tailer,
            &mut codex_normalizer,
            &mut sink,
            Some(&mut cloud_sink),
            None,
            &filter,
            &mut state,
        );

        assert_eq!(sink.pending(), 1, "FileSink must still receive the event");
        assert_eq!(
            cloud_sink.pending(),
            0,
            "a repo not matching the team org's allowlist must not reach the cloud sink"
        );
    }
}
