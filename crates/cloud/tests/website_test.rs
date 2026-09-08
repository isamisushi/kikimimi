//! Cloud routing delegates only public website requests to Fly's static app.
mod support;
use support::{SpawnOpts, TestApp};

#[tokio::test]
async fn public_site_dashboard_and_api_coexist() {
    let app = TestApp::spawn(SpawnOpts::default()).await;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    for path in [
        "/",
        "/kikimimi/overview/",
        "/kikimimi/_astro/example.js",
        "/kikimimi/missing.css",
    ] {
        let response = client
            .get(format!("{}{path}", app.base_url))
            .send()
            .await
            .unwrap();
        if let Ok(target) = std::env::var("KIKIMIMI_SITE_APP") {
            assert_eq!(response.status(), 200);
            assert_eq!(response.headers()["fly-replay"], format!("app={target}"));
        } else {
            assert_eq!(response.status(), 503);
        }
    }
    for path in ["/overview", "/login", "/sessions", "/join/test-token"] {
        let response = client
            .get(format!("{}{path}", app.base_url))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert!(!response.headers().contains_key("fly-replay"));
        assert!(response.text().await.unwrap().contains("id=\"root\""));
    }
    let response = client
        .get(format!("{}/kikimimi/", app.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 308);
    assert_eq!(response.headers()[reqwest::header::LOCATION], "/");
    for (path, status) in [
        ("/web/config", 200),
        ("/v1/devices", 401),
        ("/healthz", 200),
    ] {
        let response = client
            .get(format!("{}{path}", app.base_url))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        assert!(!response.headers().contains_key("fly-replay"));
    }
    // The replay mechanism never receives uploads/API POST bodies.
    let response = client
        .post(format!("{}/v1/events", app.base_url))
        .body("invalid")
        .send()
        .await
        .unwrap();
    assert!(!response.headers().contains_key("fly-replay"));
    assert_ne!(response.status(), 200);
    app.teardown().await;
}
