use axum::body::Body;
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use http_body_util::BodyExt;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::{router, AppState, ServerProfile};

fn build_app() -> (axum::Router, AppState, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let app = router::build_app(state.clone(), None);
    (app, state, tmp)
}

fn build_app_with_static_dir() -> (axum::Router, AppState, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let static_dir = tmp.path().join("console-dist");
    std::fs::create_dir_all(static_dir.join("assets")).expect("create static assets");
    std::fs::write(
        static_dir.join("index.html"),
        "<!doctype html><html><body><main id=\"root\">rua static bundle</main></body></html>",
    )
    .expect("write index");
    std::fs::write(static_dir.join("favicon.ico"), b"icon-bytes").expect("write favicon");
    std::fs::write(
        static_dir.join("assets").join("app.123.js"),
        "window.__RUA_STATIC_TEST__ = true;",
    )
    .expect("write asset");

    let state = AppState::from_tempdir(tmp.path().join("data"), ServerProfile::Test);
    let app = router::build_app(state.clone(), Some(static_dir));
    (app, state, tmp)
}

async fn request(app: &axum::Router, method: Method, uri: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("oneshot")
}

async fn read_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("json body")
}

async fn read_text(resp: axum::response::Response) -> String {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    String::from_utf8(bytes.to_vec()).expect("utf8 body")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_version_is_public_and_matches_cargo_package_version() {
    let (app, _state, _tmp) = build_app();

    let resp = request(&app, Method::GET, "/api/version").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let version = body["version"].as_str().expect("version string");
    assert_eq!(version, env!("CARGO_PKG_VERSION"));
    assert!(
        version.split('.').count() >= 2,
        "version should be semver-like, got {version}"
    );
    assert!(
        body["build"].is_null(),
        "build metadata must stay present as null until release wiring provides it"
    );
    assert!(
        body["commit"].is_null(),
        "commit metadata must stay present as null until release wiring provides it"
    );
    assert!(
        body.get("extra").is_none(),
        "VersionInfo uses flattened extension fields; it must not expose a nested `extra` object"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn healthz_is_public_minimal_startup_signal() {
    let (app, _state, _tmp) = build_app();

    let resp = request(&app, Method::GET, "/healthz").await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn readyz_reports_not_ready_then_ready_with_version() {
    let (app, state, _tmp) = build_app();

    let before = request(&app, Method::GET, "/readyz").await;
    assert_eq!(before.status(), StatusCode::SERVICE_UNAVAILABLE);
    let before_body = read_json(before).await;
    assert_eq!(before_body["ready"], false);
    assert_eq!(before_body["version"], env!("CARGO_PKG_VERSION"));

    state.set_ready();

    let after = request(&app, Method::GET, "/readyz").await;
    assert_eq!(after.status(), StatusCode::OK);
    let after_body = read_json(after).await;
    assert_eq!(after_body["ready"], true);
    assert_eq!(after_body["version"], env!("CARGO_PKG_VERSION"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_entry_returns_html_spa_fallback_when_static_dir_is_absent() {
    let (app, _state, _tmp) = build_app();

    let resp = request(&app, Method::GET, "/console/").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/html; charset=utf-8")
    );

    let body = read_text(resp).await;
    assert!(body.to_lowercase().contains("<!doctype html>"));
    assert!(body.contains("rua WebUI"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn root_and_unknown_non_api_paths_return_spa_fallback_html() {
    let (app, _state, _tmp) = build_app();

    for path in ["/", "/settings/agents"] {
        let resp = request(&app, Method::GET, path).await;
        assert_eq!(resp.status(), StatusCode::OK, "path={path}");
        assert_eq!(
            resp.headers()
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/html; charset=utf-8"),
            "path={path}"
        );
        let body = read_text(resp).await;
        assert!(
            body.to_lowercase().contains("<!doctype html>"),
            "path={path}, body={body}"
        );
        assert!(body.contains("rua WebUI"), "path={path}, body={body}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_static_dir_serves_spa_assets_without_catching_api_paths() {
    let (app, _state, _tmp) = build_app_with_static_dir();

    for path in ["/console/", "/settings/agents"] {
        let resp = request(&app, Method::GET, path).await;
        assert_eq!(resp.status(), StatusCode::OK, "path={path}");
        assert_eq!(
            resp.headers()
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/html; charset=utf-8"),
            "path={path}"
        );
        let body = read_text(resp).await;
        assert!(body.contains("rua static bundle"), "path={path}");
        assert!(
            !body.contains("WebUI static files not found"),
            "path={path}"
        );
    }

    let root_asset = request(&app, Method::GET, "/favicon.ico").await;
    assert_eq!(root_asset.status(), StatusCode::OK);
    assert_eq!(
        root_asset
            .headers()
            .get(axum::http::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("public, max-age=3600")
    );
    assert_eq!(read_text(root_asset).await, "icon-bytes");

    let hashed_asset = request(&app, Method::GET, "/assets/app.123.js").await;
    assert_eq!(hashed_asset.status(), StatusCode::OK);
    assert_eq!(
        hashed_asset
            .headers()
            .get(axum::http::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("public, max-age=31536000, immutable")
    );
    let asset_body = read_text(hashed_asset).await;
    assert!(asset_body.contains("__RUA_STATIC_TEST__"));

    let api_404 = request(&app, Method::GET, "/api/does-not-exist").await;
    assert_eq!(api_404.status(), StatusCode::NOT_FOUND);
    let body = read_json(api_404).await;
    assert_eq!(body["detail"], "Not found");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_voice_paths_return_json_404_instead_of_spa_fallback() {
    let (app, _state, _tmp) = build_app_with_static_dir();

    let resp = request(&app, Method::GET, "/voice/not-a-real-route").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        resp.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
    let body = read_json(resp).await;
    assert_eq!(body["detail"], "Not found");
}
