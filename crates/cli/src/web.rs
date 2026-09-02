//! `kikimimi agent`'s local web UI (architecture.md §8 「個人ビュー/ローカル」, v0):
//! embeds the built SPA (`web/dist`, see `build.rs`) and serves it plus the
//! `/web/*` API (contract: `web/src/api/types.ts`, reference impl:
//! `web/mock/server.mjs`) on `127.0.0.1:<web_port>` from inside the daemon.
//!
//! Auth is a single local secret, not a login flow: `kikimimi agent` mints a
//! random 32-hex token on every start (never persisted across restarts —
//! `state.web.token`), and `kikimimi status` / `kikimimi web` print it baked into a
//! URL (`http://127.0.0.1:<port>/?t=<token>`). Opening that URL sets an
//! HttpOnly `kikimimi_local` cookie and 302s to `/`; every other `/web/*`
//! endpoint requires that cookie (constant-time compare — this machine's own
//! processes are the only realistic caller, but it costs nothing to not leak
//! timing). `/web/login` is the one exception: it always 404s (no login flow
//! exists locally; see its handler for why).
//!
//! The actual `/web/q/*` query handlers (DuckDB shell-out) live in
//! `web_query.rs`; this module owns routing, auth, and static asset serving.

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Context;
use axum::extract::{Query, Request, State};
use axum::http::{header, HeaderMap, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use rand::RngExt as _;
use rust_embed::RustEmbed;
use serde::Deserialize;

#[derive(RustEmbed)]
#[folder = "../../web/dist"]
struct WebAssets;

/// Name of the local-auth cookie (architecture.md §8). Distinct from
/// `kikimimi_session` (the cloud contract's cookie, `web/mock/server.mjs`) so the
/// two auth schemes can never be confused with each other even if a future
/// build somehow serves both.
const COOKIE_NAME: &str = "kikimimi_local";
/// How long the cookie lasts once set. Local-only, low-stakes secret (this
/// machine's own loopback interface); 30 days trades a little exposure
/// window for not making the user re-open the tokened URL constantly.
const COOKIE_MAX_AGE_SECS: u64 = 60 * 60 * 24 * 30;

/// Generates a fresh local-auth token: 16 random bytes, hex-encoded (32 hex
/// chars). Regenerated on every `kikimimi agent` start (see module docs) — never
/// read back from a previous `state.json`.
pub fn generate_local_token() -> String {
    let mut bytes = [0u8; 16];
    rand::rng().fill(&mut bytes);
    hex::encode(bytes)
}

/// Shared handler state. `data_dir` is injectable (not always
/// `kikimimi_schema::paths::data_dir()`) so tests can point it at a tempdir.
#[derive(Clone)]
pub struct WebAppState {
    pub token: String,
    pub data_dir: PathBuf,
}

/// Builds the full router (static SPA + `/web/*` API). Split out from
/// [`serve`] so tests can drive it directly with `tower::ServiceExt::oneshot`
/// instead of binding a real socket.
pub fn router(state: WebAppState) -> Router {
    // Everything under /web/* except /web/login requires the kikimimi_local
    // cookie (module docs). `route_layer` (not `layer`) so paths that don't
    // match any route in `protected` -- i.e. everything else, including "/"
    // and the SPA's static assets -- never go through this check at all.
    let protected = Router::new()
        .route("/web/me", get(handle_me))
        .route("/web/logout", post(handle_logout))
        .route("/web/q/overview", get(crate::web_query::overview))
        .route("/web/q/machines", get(crate::web_query::machines))
        .route("/web/q/tools", get(crate::web_query::tools))
        .route("/web/q/mcp", get(crate::web_query::mcp))
        .route("/web/q/skills", get(crate::web_query::skills))
        .route("/web/q/unused-mcp", get(crate::web_query::unused_mcp))
        .route("/web/q/sessions", get(crate::web_query::sessions))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_local_auth,
        ));

    Router::new()
        .route("/", get(handle_root))
        // Always 404, cookie or not: local mode has no login flow (the SPA
        // only reaches /web/login from its login page, which a tokened-URL
        // user should never see -- task spec, "acceptable v0"). Registered
        // outside `protected` so it stays reachable/testable regardless of
        // auth state instead of being masked by a 401 first.
        .route("/web/login", post(login_not_available))
        .merge(protected)
        .fallback(get(serve_spa))
        .with_state(state)
}

/// Runs the server until `shutdown` completes (mirrors `kikimimi_otlp::serve`'s
/// bind-then-graceful-shutdown shape, `crates/otlp/src/lib.rs`).
pub async fn serve(
    addr: SocketAddr,
    state: WebAppState,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("kikimimi web: failed to bind {addr}"))?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
        .context("kikimimi web: server error")?;
    Ok(())
}

// --- Auth ---

async fn require_local_auth(
    State(state): State<WebAppState>,
    req: Request,
    next: Next,
) -> Response {
    let authed = cookie_value(req.headers(), COOKIE_NAME)
        .is_some_and(|v| constant_time_eq(&v, &state.token));
    if authed {
        next.run(req).await
    } else {
        unauthorized()
    }
}

fn unauthorized() -> Response {
    json_error(StatusCode::UNAUTHORIZED, "unauthorized")
}

fn json_error(status: StatusCode, msg: &str) -> Response {
    axum::response::Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(
            serde_json::json!({ "error": msg }).to_string(),
        ))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Finds `name`'s value in the `Cookie` header (`k=v; k2=v2` pairs). No
/// percent-decoding: our own cookie value is a plain hex string, never
/// encoded.
fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';').find_map(|part| {
        part.trim()
            .strip_prefix(name)
            .and_then(|rest| rest.strip_prefix('='))
            .map(str::to_string)
    })
}

/// Length-then-XOR compare. The length check is a (harmless) early exit: our
/// tokens are always a fixed, publicly-known length (32 hex chars), so
/// leaking "did the length match" leaks nothing an attacker doesn't already
/// know.
fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// --- Handlers ---

async fn handle_me() -> Response {
    axum::Json(serde_json::json!({ "email": "local", "org_id": "local" })).into_response()
}

async fn handle_logout() -> Response {
    let cookie = format!("{COOKIE_NAME}=; HttpOnly; Path=/; SameSite=Lax; Max-Age=0");
    axum::response::Response::builder()
        .status(StatusCode::OK)
        .header(header::SET_COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(r#"{"ok":true}"#))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// `POST /web/login` always 404s in local mode: there's no invite-code login
/// here, only the tokened-URL flow (`handle_root`). Registered ungated (see
/// `router`) so hitting it never depends on cookie state.
async fn login_not_available() -> Response {
    json_error(StatusCode::NOT_FOUND, "not available in local mode")
}

#[derive(Debug, Deserialize)]
struct RootParams {
    t: Option<String>,
}

/// `GET /` — the one path that understands `?t=<token>` (module docs). No
/// `t` at all just serves the SPA shell like any other path (auth for the
/// *data* stays entirely at the `/web/*` layer; serving the static HTML/JS
/// to whoever can reach loopback is not a secrecy boundary this needs).
async fn handle_root(
    State(state): State<WebAppState>,
    Query(params): Query<RootParams>,
) -> Response {
    match params.t {
        Some(t) if constant_time_eq(&t, &state.token) => set_cookie_and_redirect(&state.token),
        Some(_) => unauthorized(),
        None => serve_asset("index.html"),
    }
}

fn set_cookie_and_redirect(token: &str) -> Response {
    let cookie = format!(
        "{COOKIE_NAME}={token}; HttpOnly; Path=/; SameSite=Lax; Max-Age={COOKIE_MAX_AGE_SECS}"
    );
    axum::response::Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, "/")
        .header(header::SET_COOKIE, cookie)
        .body(axum::body::Body::empty())
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Fallback for every path not otherwise routed: real embedded assets
/// (`/assets/x.js`) serve themselves, anything else falls back to
/// `index.html` so the SPA's own client-side router can render it
/// (`web/src/router/Router.tsx`'s history-API routes: `/tools`, `/mcp`, ...).
async fn serve_spa(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    serve_asset(if path.is_empty() { "index.html" } else { path })
}

fn serve_asset(path: &str) -> Response {
    if let Some(file) = WebAssets::get(path) {
        return asset_response(file.metadata.mimetype(), file.data);
    }
    // A path whose last segment has no "." is a client-side route (no real
    // asset could ever match it), not a typo'd asset request -- fall back to
    // index.html. A path that does look like an asset request
    // ("/assets/x.js") and still isn't embedded is a genuine 404, not
    // silently served as HTML.
    let looks_like_asset = path
        .rsplit('/')
        .next()
        .is_some_and(|last| last.contains('.'));
    if !looks_like_asset {
        if let Some(file) = WebAssets::get("index.html") {
            return asset_response("text/html; charset=utf-8", file.data);
        }
    }
    (StatusCode::NOT_FOUND, "not found").into_response()
}

fn asset_response(mime: &str, data: std::borrow::Cow<'static, [u8]>) -> Response {
    axum::response::Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .body(axum::body::Body::from(data.into_owned()))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use tower::ServiceExt as _;

    fn test_state() -> WebAppState {
        WebAppState {
            token: "0123456789abcdef0123456789abcdef".to_string(),
            data_dir: std::env::temp_dir().join("kikimimi-web-test-nonexistent"),
        }
    }

    #[test]
    fn generate_local_token_is_32_lowercase_hex_chars() {
        let t = generate_local_token();
        assert_eq!(t.len(), 32);
        assert!(t
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn generate_local_token_is_not_constant() {
        // Not a proof of randomness, just a smoke check against a
        // regression to a hardcoded/all-zero token.
        assert_ne!(generate_local_token(), generate_local_token());
    }

    #[test]
    fn constant_time_eq_matches_and_rejects() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "abcd"));
        assert!(!constant_time_eq("", "a"));
        assert!(constant_time_eq("", ""));
    }

    #[test]
    fn cookie_value_finds_the_named_cookie_among_several() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            "foo=bar; kikimimi_local=deadbeef; other=1".parse().unwrap(),
        );
        assert_eq!(
            cookie_value(&headers, "kikimimi_local"),
            Some("deadbeef".to_string())
        );
        assert_eq!(cookie_value(&headers, "missing"), None);
    }

    #[test]
    fn cookie_value_does_not_prefix_match_a_longer_cookie_name() {
        let mut headers = HeaderMap::new();
        // "kikimimi_local_extra" must not be mistaken for "kikimimi_local".
        headers.insert(header::COOKIE, "kikimimi_local_extra=x".parse().unwrap());
        assert_eq!(cookie_value(&headers, "kikimimi_local"), None);
    }

    #[test]
    fn cookie_value_none_when_no_cookie_header() {
        assert_eq!(cookie_value(&HeaderMap::new(), "kikimimi_local"), None);
    }

    async fn call(app: Router, req: HttpRequest<Body>) -> Response {
        app.oneshot(req).await.unwrap()
    }

    fn get_req(uri: &str) -> HttpRequest<Body> {
        HttpRequest::builder().uri(uri).body(Body::empty()).unwrap()
    }

    fn get_req_with_cookie(uri: &str, token: &str) -> HttpRequest<Body> {
        HttpRequest::builder()
            .uri(uri)
            .header(header::COOKIE, format!("kikimimi_local={token}"))
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn web_me_without_cookie_is_401() {
        let state = test_state();
        let resp = call(router(state), get_req("/web/me")).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn web_me_with_right_cookie_is_200_local_identity() {
        let state = test_state();
        let token = state.token.clone();
        let resp = call(router(state), get_req_with_cookie("/web/me", &token)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["email"], "local");
        assert_eq!(json["org_id"], "local");
    }

    #[tokio::test]
    async fn web_me_with_wrong_cookie_is_401() {
        let state = test_state();
        let resp = call(router(state), get_req_with_cookie("/web/me", "wrong-token")).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn root_with_right_token_query_sets_cookie_and_redirects() {
        let state = test_state();
        let token = state.token.clone();
        let resp = call(router(state), get_req(&format!("/?t={token}"))).await;
        assert_eq!(resp.status(), StatusCode::FOUND);
        assert_eq!(resp.headers().get(header::LOCATION).unwrap(), "/");
        let cookie = resp
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(cookie.starts_with(&format!("kikimimi_local={token}")));
        assert!(cookie.contains("HttpOnly"));
    }

    #[tokio::test]
    async fn root_with_wrong_token_query_is_401() {
        let state = test_state();
        let resp = call(router(state), get_req("/?t=wrong")).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn root_without_token_query_serves_the_spa_shell_regardless_of_cookie() {
        let state = test_state();
        let resp = call(router(state), get_req("/")).await;
        // No cookie, no ?t=: still 200s with the SPA shell (client-side auth
        // handles the rest) -- see handle_root's doc comment.
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn web_login_always_404s_even_with_a_valid_cookie() {
        let state = test_state();
        let token = state.token.clone();
        let req = HttpRequest::builder()
            .method("POST")
            .uri("/web/login")
            .header(header::COOKIE, format!("kikimimi_local={token}"))
            .body(Body::empty())
            .unwrap();
        let resp = call(router(state), req).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn web_logout_clears_the_cookie() {
        let state = test_state();
        let token = state.token.clone();
        let req = HttpRequest::builder()
            .method("POST")
            .uri("/web/logout")
            .header(header::COOKIE, format!("kikimimi_local={token}"))
            .body(Body::empty())
            .unwrap();
        let resp = call(router(state), req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let cookie = resp
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(cookie.contains("Max-Age=0"));
    }

    #[tokio::test]
    async fn unknown_client_route_falls_back_to_index_html() {
        let state = test_state();
        let token = state.token.clone();
        let resp = call(router(state), get_req_with_cookie("/sessions", &token)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(ct.starts_with("text/html"));
    }

    #[tokio::test]
    async fn missing_asset_with_an_extension_is_a_genuine_404() {
        let state = test_state();
        let resp = call(router(state), get_req("/assets/does-not-exist.js")).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn web_q_endpoints_require_the_cookie_too() {
        for path in [
            "/web/q/overview",
            "/web/q/machines",
            "/web/q/tools",
            "/web/q/mcp",
            "/web/q/sessions",
        ] {
            let resp = call(router(test_state()), get_req(path)).await;
            assert_eq!(
                resp.status(),
                StatusCode::UNAUTHORIZED,
                "{path} without cookie"
            );
        }
    }
}
