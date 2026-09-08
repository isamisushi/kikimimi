//! Public GET/HEAD requests are replayed by Fly Proxy to the independently
//! deployed static website. API bodies and authentication routes stay here.
use axum::{
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Redirect, Response},
};

pub async fn forward() -> Response {
    match std::env::var("KIKIMIMI_SITE_APP") {
        Ok(app) => replay(&app),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "Public website is served separately. Open /overview for the dashboard.",
        )
            .into_response(),
    }
}

fn replay(app: &str) -> Response {
    // A configured app name must not inject additional Fly replay directives.
    if app.is_empty() || !app.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let mut response = StatusCode::OK.into_response();
    response.headers_mut().insert(
        "fly-replay",
        HeaderValue::from_str(&format!("app={app}")).unwrap(),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

pub async fn home() -> Redirect {
    Redirect::permanent("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_configured_app_can_be_a_replay_target() {
        let response = replay("kikimimi-site");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["fly-replay"], "app=kikimimi-site");
        for invalid in [
            "",
            "site;region=iad",
            "site\r\nx-bad: yes",
            "https://example.com",
        ] {
            assert_eq!(replay(invalid).status(), StatusCode::SERVICE_UNAVAILABLE);
        }
    }

    #[tokio::test]
    async fn old_website_home_redirects_to_landing() {
        let response = home().await.into_response();
        assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
        assert_eq!(response.headers()[header::LOCATION], "/");
    }
}
