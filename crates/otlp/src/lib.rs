//! kikimimi-otlp — localhost OTLP/HTTP レシーバ (docs/design/architecture.md §4 「OTLP レシーバ」)。
//!
//! Claude Code は `OTEL_EXPORTER_OTLP_PROTOCOL` に応じて `http/protobuf` または
//! `http/json` で OTLP を export する (§4.2)。本クレートはその両方を受ける最小実装。
//!
//! Stage 0 の割り切り (ドキュメント化):
//! - レスポンスは常に `200 {}` (JSON) を返す。OTLP の `partialSuccess` はプロトコル上
//!   protobuf/JSON いずれのボディでも返せるが、Stage 0 では常に空の成功レスポンスのみを
//!   返し、protobuf ボディは生成しない。デコードに成功した時点で cloud への到達可否に
//!   関わらず 200 を返す (エージェントを絶対に止めない: 設計原則 2)。
//! - デコードに失敗した場合のみ `400` + 短いメッセージを返す。
//! - 判定・正規化は一切行わない。デコード結果をそのまま [`OtlpPayload`] として
//!   `tx` に渡すだけ (正規化は adapter 層の責務)。
//! - 送信チャンネル (`tx`) が詰まっている場合は最大 100ms 待って諦め、破棄する
//!   (レスポンスをブロックし続けない)。
//!
//! ## 認証 (per-install bearer token)
//! `127.0.0.1:<port>` は同一マシン上の *どのプロセスからも* (ブラウザの `no-cors` POST
//! も含め) 到達できてしまうため、認証が無いと偽の OTLP イベントを本物のセッションの
//! ものとして記録させられる (architecture.md §11 はここで集めるメタデータの信頼性に
//! 依存している)。`serve` は [`Arc<RwLock<Option<String>>>`] の形で「今この瞬間の期待
//! トークン」を受け取る (`RwLock` にしてあるのは `kikimimi init` 後の `'r'` コントロール
//! バイトでデーモンを再起動せずに書き換えられるようにするため — `kikimimi agent`
//! 側の実装を参照)。中身が `Some(token)` なら `Authorization: Bearer <token>` の完全一致
//! を要求し、`None` なら (未 `init` のバイナリ更新直後など) fail-open で誰でも通す
//! (設計原則 2)。拒否した回数は `Arc<AtomicU64>` に積み、`kikimimi status` が読む。

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::Context;
use axum::{
    body::Bytes,
    extract::{Request, State},
    http::{header, HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::post,
    Router,
};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use tokio::sync::mpsc;

/// 送信チャンネルが詰まっている場合にレスポンスをブロックしてよい上限。
const SEND_TIMEOUT: Duration = Duration::from_millis(100);

/// デコード済みの OTLP ペイロード。種別ごとに素通しする (正規化はしない)。
#[derive(Debug, Clone, PartialEq)]
pub enum OtlpPayload {
    Logs(ExportLogsServiceRequest),
    Metrics(ExportMetricsServiceRequest),
    Traces(ExportTraceServiceRequest),
}

/// 既定 `127.0.0.1:4318`。`KIKIMIMI_OTLP_PORT` でポートのみ上書き可能
/// (`kikimimi init` がポート衝突を検知した場合に使う。architecture.md §4)。
pub fn default_addr() -> SocketAddr {
    let port =
        kikimimi_schema::env_compat::env_u16_with_legacy("KIKIMIMI_OTLP_PORT", "GURU_OTLP_PORT")
            .unwrap_or(4318);
    SocketAddr::from(([127, 0, 0, 1], port))
}

/// `127.0.0.1:port` に一瞬だけ bind してみて、実際に取れるかを調べる
/// (architecture.md §4: "`kikimimi init` はポート使用状況を検査し...")。
pub fn is_port_available(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

/// `preferred` が空いていればそれを、埋まっていれば OS に空きポートを選ばせて返す
/// (`kikimimi init` の衝突検知・自動切替 — architecture.md §4)。
pub fn pick_port(preferred: u16) -> u16 {
    if is_port_available(preferred) {
        return preferred;
    }
    match std::net::TcpListener::bind(("127.0.0.1", 0)) {
        Ok(listener) => listener.local_addr().map(|a| a.port()).unwrap_or(preferred),
        Err(_) => preferred,
    }
}

#[derive(Clone)]
struct AppState {
    tx: mpsc::Sender<OtlpPayload>,
    /// 今この瞬間の期待トークン。`None` は「未認証で通す」(fail-open)。モジュール doc の
    /// 「認証」節を参照。
    auth_token: Arc<RwLock<Option<String>>>,
    /// トークン不一致/欠落で拒否したリクエスト数の累計。`kikimimi agent` が
    /// `state.otlp_rejected` としてサンプリングする (このプロセスの生存期間中のみ有効な
    /// カウンタで、state.json 自体には永続加算しない)。
    rejected: Arc<AtomicU64>,
}

/// axum サーバーを起動して待ち受ける。`shutdown` が完了するとグレースフルに停止する。
///
/// `auth_token`/`rejected` はどちらも呼び出し側 (`kikimimi agent`) が保持する `Arc` の
/// clone を渡す — `auth_token` は `'r'` コントロールバイトでの生きたまま差し替えに、
/// `rejected` はデーモンの `state.json` サンプリングに使うため (モジュール doc 参照)。
pub async fn serve(
    addr: SocketAddr,
    tx: mpsc::Sender<OtlpPayload>,
    auth_token: Arc<RwLock<Option<String>>>,
    rejected: Arc<AtomicU64>,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let state = AppState {
        tx,
        auth_token,
        rejected,
    };
    let app = Router::new()
        .route("/v1/logs", post(handle_logs))
        .route("/v1/metrics", post(handle_metrics))
        .route("/v1/traces", post(handle_traces))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_bearer_token,
        ))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("kikimimi-otlp: failed to bind {addr}"))?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
        .context("kikimimi-otlp: server error")?;
    Ok(())
}

/// `Authorization: Bearer <token>` を要求するミドルウェア (モジュール doc の「認証」節)。
/// `auth_token` が `None` の間は fail-open で誰でも通す。
async fn require_bearer_token(State(state): State<AppState>, req: Request, next: Next) -> Response {
    // ロックを保持したまま `.await` へ入らないよう、期待値はここで clone して手放す。
    let expected: Option<String> = {
        let guard = state
            .auth_token
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.clone()
    };
    let Some(expected) = expected else {
        // 未 `init` (or `init` 前のバイナリ更新直後) はトークン未設定 -- fail-open (設計原則 2)。
        return next.run(req).await;
    };
    let provided = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if provided.is_some_and(|p| constant_time_eq(p, &expected)) {
        next.run(req).await
    } else {
        state.rejected.fetch_add(1, Ordering::Relaxed);
        unauthorized_response()
    }
}

/// 長さ一致チェック + XOR 畳み込みの定数時間比較 (`crates/cli/src/web.rs` の
/// `constant_time_eq` と同じ手法 — ローカル web UI トークンの比較で使っているもの)。
/// 両クレートとも `kikimimi-schema` に依存しているが、本体が数行なので共有モジュール化
/// するよりここに複製する方が単純。長さの早期 return は無害 (トークンは常に 32 文字の
/// hex という公開済みの固定長なので、長さの一致/不一致自体は攻撃者にとって新しい情報
/// にならない)。
fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn unauthorized_response() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::CONTENT_TYPE, "application/json")],
        r#"{"error":"unauthorized"}"#,
    )
        .into_response()
}

/// `Content-Type` が protobuf を指しているか。欠落 / それ以外 (`application/json` 含む)
/// は JSON として扱う (架空の Content-Type でも JSON デコードにフォールバックし、
/// 失敗すれば 400 を返す)。
fn is_protobuf(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| {
            let s = s.to_ascii_lowercase();
            s.contains("application/x-protobuf") || s.contains("application/protobuf")
        })
        .unwrap_or(false)
}

fn bad_request(msg: String) -> Response {
    (StatusCode::BAD_REQUEST, msg).into_response()
}

fn ok_response() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        "{}",
    )
        .into_response()
}

/// `tx` へ渡す。詰まっている場合は [`SEND_TIMEOUT`] だけ待ち、それでも詰まっていれば
/// 諦めて破棄する (レスポンスを長時間ブロックしない — fail-open, 設計原則 2)。
async fn dispatch(state: &AppState, payload: OtlpPayload) {
    match tokio::time::timeout(SEND_TIMEOUT, state.tx.send(payload)).await {
        Ok(Ok(())) => {}
        Ok(Err(_)) => {
            eprintln!("kikimimi-otlp: receiver channel closed; dropping payload");
        }
        Err(_) => {
            eprintln!("kikimimi-otlp: send channel full after {SEND_TIMEOUT:?}; dropping payload");
        }
    }
}

/// protobuf なら `prost::Message::decode`、それ以外は `serde_json` でデコードする
/// 共通ハンドラ本体。成功時は `wrap` で [`OtlpPayload`] にして `tx` に渡し 200 を返す。
/// 失敗時は 400 を返す。
async fn decode_and_dispatch<M, F>(
    state: AppState,
    headers: HeaderMap,
    body: Bytes,
    wrap: F,
) -> Response
where
    M: prost::Message + Default + serde::de::DeserializeOwned,
    F: FnOnce(M) -> OtlpPayload,
{
    let decoded = if is_protobuf(&headers) {
        M::decode(body).map_err(|e| format!("invalid protobuf body: {e}"))
    } else {
        serde_json::from_slice::<M>(&body).map_err(|e| format!("invalid json body: {e}"))
    };
    match decoded {
        Ok(msg) => {
            dispatch(&state, wrap(msg)).await;
            ok_response()
        }
        Err(msg) => bad_request(msg),
    }
}

async fn handle_logs(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    decode_and_dispatch::<ExportLogsServiceRequest, _>(state, headers, body, OtlpPayload::Logs)
        .await
}

async fn handle_metrics(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    decode_and_dispatch::<ExportMetricsServiceRequest, _>(
        state,
        headers,
        body,
        OtlpPayload::Metrics,
    )
    .await
}

async fn handle_traces(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    decode_and_dispatch::<ExportTraceServiceRequest, _>(state, headers, body, OtlpPayload::Traces)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::common::v1::{any_value::Value, AnyValue, KeyValue};
    use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
    use prost::Message as _;
    use std::time::Duration as StdDuration;
    use tokio::sync::oneshot;

    /// OS にポートを選ばせてから即座に解放する。テスト間の衝突をほぼ避けられる
    /// (本番の bind は `serve` 自身が行う)。
    fn free_addr() -> SocketAddr {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local_addr");
        drop(listener);
        addr
    }

    fn sample_logs_request() -> ExportLogsServiceRequest {
        ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                resource: None,
                scope_logs: vec![ScopeLogs {
                    scope: None,
                    log_records: vec![LogRecord {
                        time_unix_nano: 1,
                        body: Some(AnyValue {
                            value: Some(Value::StringValue("hello".into())),
                        }),
                        attributes: vec![KeyValue {
                            key: "session.id".into(),
                            value: Some(AnyValue {
                                value: Some(Value::StringValue("sess-1".into())),
                            }),
                            key_strindex: 0,
                        }],
                        ..Default::default()
                    }],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        }
    }

    /// サーバーを spawn し、(addr, 受信チャンネル, shutdown トリガー, auth ハンドル,
    /// rejected カウンタ) を返す。`auth_token` は `None` なら fail-open (未認証で通す)、
    /// `Some(t)` なら `t` を要求する状態で起動する。返す auth ハンドルは呼び出し側が
    /// `write()` して `'r'` コントロールバイト相当のライブ差し替えもテストできる。
    async fn spawn_server(
        auth_token: Option<&str>,
    ) -> (
        SocketAddr,
        mpsc::Receiver<OtlpPayload>,
        oneshot::Sender<()>,
        std::sync::Arc<std::sync::RwLock<Option<String>>>,
        std::sync::Arc<std::sync::atomic::AtomicU64>,
    ) {
        let addr = free_addr();
        let (tx, rx) = mpsc::channel(8);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let shutdown = async move {
            let _ = shutdown_rx.await;
        };
        let auth = std::sync::Arc::new(std::sync::RwLock::new(auth_token.map(str::to_string)));
        let rejected = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let (auth_for_server, rejected_for_server) = (auth.clone(), rejected.clone());
        tokio::spawn(async move {
            serve(addr, tx, auth_for_server, rejected_for_server, shutdown)
                .await
                .expect("server exited with error");
        });
        // リスナーが bind を終えるまでの短い猶予。
        tokio::time::sleep(StdDuration::from_millis(50)).await;
        (addr, rx, shutdown_tx, auth, rejected)
    }

    #[tokio::test]
    async fn accepts_json_logs_and_forwards_to_channel() {
        let (addr, mut rx, _shutdown, _auth, _rejected) = spawn_server(None).await;
        let body = serde_json::to_vec(&sample_logs_request()).unwrap();

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{addr}/v1/logs"))
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .expect("request failed");

        assert_eq!(resp.status(), 200);
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "application/json"
        );
        assert_eq!(resp.text().await.unwrap(), "{}");

        let payload = tokio::time::timeout(StdDuration::from_secs(1), rx.recv())
            .await
            .expect("timed out waiting for payload")
            .expect("channel closed");
        match payload {
            OtlpPayload::Logs(req) => {
                assert_eq!(req.resource_logs.len(), 1);
                assert_eq!(
                    req.resource_logs[0].scope_logs[0].log_records[0].time_unix_nano,
                    1
                );
            }
            other => panic!("expected Logs payload, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn accepts_protobuf_logs_and_forwards_to_channel() {
        let (addr, mut rx, _shutdown, _auth, _rejected) = spawn_server(None).await;
        let body = sample_logs_request().encode_to_vec();

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{addr}/v1/logs"))
            .header("content-type", "application/x-protobuf")
            .body(body)
            .send()
            .await
            .expect("request failed");

        assert_eq!(resp.status(), 200);

        let payload = tokio::time::timeout(StdDuration::from_secs(1), rx.recv())
            .await
            .expect("timed out waiting for payload")
            .expect("channel closed");
        match payload {
            OtlpPayload::Logs(req) => assert_eq!(req.resource_logs.len(), 1),
            other => panic!("expected Logs payload, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn missing_content_type_falls_back_to_json() {
        let (addr, mut rx, _shutdown, _auth, _rejected) = spawn_server(None).await;
        let body = serde_json::to_vec(&sample_logs_request()).unwrap();

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{addr}/v1/logs"))
            .body(body)
            .send()
            .await
            .expect("request failed");

        assert_eq!(resp.status(), 200);
        let payload = tokio::time::timeout(StdDuration::from_secs(1), rx.recv())
            .await
            .expect("timed out waiting for payload")
            .expect("channel closed");
        assert!(matches!(payload, OtlpPayload::Logs(_)));
    }

    #[tokio::test]
    async fn rejects_garbage_protobuf_with_400() {
        let (addr, _rx, _shutdown, _auth, _rejected) = spawn_server(None).await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{addr}/v1/logs"))
            .header("content-type", "application/x-protobuf")
            .body(vec![0xFF, 0x00, 0x01, 0x02, 0xAB, 0xCD])
            .send()
            .await
            .expect("request failed");

        assert_eq!(resp.status(), 400);
        let text = resp.text().await.unwrap();
        assert!(!text.is_empty());
    }

    #[tokio::test]
    async fn rejects_garbage_json_with_400() {
        let (addr, _rx, _shutdown, _auth, _rejected) = spawn_server(None).await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{addr}/v1/metrics"))
            .header("content-type", "application/json")
            .body("not json at all {{{")
            .send()
            .await
            .expect("request failed");

        assert_eq!(resp.status(), 400);
    }

    #[tokio::test]
    async fn accepts_json_traces() {
        let (addr, mut rx, _shutdown, _auth, _rejected) = spawn_server(None).await;
        let req = ExportTraceServiceRequest {
            resource_spans: vec![],
        };
        let body = serde_json::to_vec(&req).unwrap();

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{addr}/v1/traces"))
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .expect("request failed");

        assert_eq!(resp.status(), 200);
        let payload = tokio::time::timeout(StdDuration::from_secs(1), rx.recv())
            .await
            .expect("timed out waiting for payload")
            .expect("channel closed");
        assert!(matches!(payload, OtlpPayload::Traces(_)));
    }

    // --- 認証 (per-install bearer token) ---

    #[tokio::test]
    async fn no_token_configured_accepts_requests_without_authorization_header() {
        let (addr, mut rx, _shutdown, _auth, rejected) = spawn_server(None).await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{addr}/v1/logs"))
            .header("content-type", "application/json")
            .body(serde_json::to_vec(&sample_logs_request()).unwrap())
            .send()
            .await
            .expect("request failed");

        assert_eq!(resp.status(), 200, "auth_token=None must fail-open");
        assert_eq!(rejected.load(std::sync::atomic::Ordering::Relaxed), 0);
        let payload = tokio::time::timeout(StdDuration::from_secs(1), rx.recv())
            .await
            .expect("timed out waiting for payload")
            .expect("channel closed");
        assert!(matches!(payload, OtlpPayload::Logs(_)));
    }

    #[tokio::test]
    async fn token_configured_missing_authorization_header_is_rejected() {
        let (addr, _rx, _shutdown, _auth, rejected) = spawn_server(Some("secret-token")).await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{addr}/v1/logs"))
            .header("content-type", "application/json")
            .body(serde_json::to_vec(&sample_logs_request()).unwrap())
            .send()
            .await
            .expect("request failed");

        assert_eq!(resp.status(), 401);
        assert_eq!(rejected.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn token_configured_wrong_token_is_rejected() {
        let (addr, _rx, _shutdown, _auth, rejected) = spawn_server(Some("secret-token")).await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{addr}/v1/logs"))
            .header("content-type", "application/json")
            .header("authorization", "Bearer wrong-token")
            .body(serde_json::to_vec(&sample_logs_request()).unwrap())
            .send()
            .await
            .expect("request failed");

        assert_eq!(resp.status(), 401);
        assert_eq!(rejected.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn token_configured_correct_token_is_accepted_and_payload_delivered() {
        let (addr, mut rx, _shutdown, _auth, rejected) = spawn_server(Some("secret-token")).await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{addr}/v1/logs"))
            .header("content-type", "application/json")
            .header("authorization", "Bearer secret-token")
            .body(serde_json::to_vec(&sample_logs_request()).unwrap())
            .send()
            .await
            .expect("request failed");

        assert_eq!(resp.status(), 200);
        assert_eq!(rejected.load(std::sync::atomic::Ordering::Relaxed), 0);
        let payload = tokio::time::timeout(StdDuration::from_secs(1), rx.recv())
            .await
            .expect("timed out waiting for payload")
            .expect("channel closed");
        assert!(matches!(payload, OtlpPayload::Logs(_)));
    }

    /// `kikimimi agent`'s `'r'` control byte reloads `otlp_token` into this same `Arc`
    /// without restarting the server (init_cmd.rs/agent.rs) -- pin that the running
    /// server actually observes a live write to the handle, not just a value baked in
    /// at `serve()` call time.
    #[tokio::test]
    async fn auth_handle_write_takes_effect_without_restarting_the_server() {
        let (addr, _rx, _shutdown, auth, _rejected) = spawn_server(None).await;
        let client = reqwest::Client::new();
        let body = || serde_json::to_vec(&sample_logs_request()).unwrap();

        let resp = client
            .post(format!("http://{addr}/v1/logs"))
            .header("content-type", "application/json")
            .body(body())
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "still fail-open before the write");

        *auth.write().unwrap() = Some("new-token".to_string());

        let resp = client
            .post(format!("http://{addr}/v1/logs"))
            .header("content-type", "application/json")
            .body(body())
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            401,
            "must now require the token just written into the handle"
        );
    }

    #[test]
    fn default_addr_uses_4318_and_respects_env_override() {
        std::env::remove_var("KIKIMIMI_OTLP_PORT");
        assert_eq!(default_addr(), SocketAddr::from(([127, 0, 0, 1], 4318)));

        std::env::set_var("KIKIMIMI_OTLP_PORT", "19999");
        assert_eq!(default_addr(), SocketAddr::from(([127, 0, 0, 1], 19999)));

        std::env::remove_var("KIKIMIMI_OTLP_PORT");
    }

    #[test]
    fn pick_port_keeps_preferred_when_free() {
        let addr = free_addr(); // bound then immediately released above
        assert_eq!(pick_port(addr.port()), addr.port());
    }

    #[test]
    fn pick_port_picks_an_alternate_when_preferred_is_taken() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let taken_port = listener.local_addr().unwrap().port();
        assert!(!is_port_available(taken_port));

        let picked = pick_port(taken_port);
        assert_ne!(
            picked, taken_port,
            "must not return the port that's actually occupied"
        );
        assert!(is_port_available(picked), "the picked port must be free");

        drop(listener);
    }

    #[test]
    fn constant_time_eq_matches_and_rejects() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "abcd"));
        assert!(!constant_time_eq("", "a"));
        assert!(constant_time_eq("", ""));
    }
}
