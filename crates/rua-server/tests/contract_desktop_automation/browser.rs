use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use rua_server::{AppState, ServerProfile};
use serde_json::{json, Value};
use tower::ServiceExt;

// These HTTP boundaries resolve the bundled executable from process environment.
static ENVIRONMENT: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct BrowserDriverEnvironment(Option<std::ffi::OsString>);
impl BrowserDriverEnvironment {
    fn set(value: Option<std::ffi::OsString>) -> Self {
        let previous = std::env::var_os("RUA_BUNDLED_AGENT_BROWSER");
        match value {
            Some(value) => std::env::set_var("RUA_BUNDLED_AGENT_BROWSER", value),
            None => std::env::remove_var("RUA_BUNDLED_AGENT_BROWSER"),
        }
        Self(previous)
    }
}
impl Drop for BrowserDriverEnvironment {
    fn drop(&mut self) {
        match self.0.take() {
            Some(previous) => std::env::set_var("RUA_BUNDLED_AGENT_BROWSER", previous),
            None => std::env::remove_var("RUA_BUNDLED_AGENT_BROWSER"),
        }
    }
}

fn app(root: &std::path::Path) -> (Router, AppState) {
    let mut state = AppState::from_tempdir(root.to_path_buf(), ServerProfile::Test);
    // HTTP tests suppress background startup; the route still runs on a Desktop authority.
    state.profile = ServerProfile::Desktop;
    let router = Router::new()
        .nest("/api", rua_server::endpoints::desktop_automation::router())
        .with_state(state.clone());
    (router, state)
}

async fn call(app: &Router, method: &str, path: &str, value: Value) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(value.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}
async fn body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

#[tokio::test]
async fn invalid_browser_selection_preserves_local_preferences_and_reports_detail() {
    let _environment = ENVIRONMENT.lock().await;
    let temp = tempfile::tempdir().unwrap();
    let (app, state) = app(temp.path());
    let path = rua_config::desktop_automation::preferences_path(&state.cache_dir);
    let previous = json!({"browser_path":"/previous/browser", "future": {"keep":true}});
    rua_config::atomic_write_json(&path, &previous).unwrap();
    let response = call(
        &app,
        "PUT",
        "/api/platform/browser",
        json!({"path":"/nonexistent/browser"}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(body(response).await["detail"].is_string());
    let stored: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(stored, previous);
}

#[tokio::test]
async fn missing_desktop_driver_has_device_status_without_failing_the_route() {
    let _environment = ENVIRONMENT.lock().await;
    let _driver = BrowserDriverEnvironment::set(None);
    let temp = tempfile::tempdir().unwrap();
    let (app, _) = app(temp.path());
    let response = call(&app, "GET", "/api/platform/browser", Value::Null).await;
    assert_eq!(response.status(), StatusCode::OK);
    let result = body(response).await;
    assert_eq!(result["available"], false);
    assert!(!result["diagnostics"].as_array().unwrap().is_empty());
    assert!(result["device_id"].is_string());
}

#[tokio::test]
#[ignore = "requires RUA_TEST_AGENT_BROWSER and RUA_TEST_CHROMIUM"]
async fn real_browser_selection_is_persisted_only_after_protocol_probe() {
    let _environment = ENVIRONMENT.lock().await;
    let driver = std::env::var_os("RUA_TEST_AGENT_BROWSER").expect("native driver required");
    let chrome = std::env::var_os("RUA_TEST_CHROMIUM").expect("system Chromium required");
    let _driver = BrowserDriverEnvironment::set(Some(driver));
    let temp = tempfile::tempdir().unwrap();
    let (app, state) = app(temp.path());
    #[cfg(target_os = "macos")]
    let chrome = {
        let original = std::path::PathBuf::from(chrome);
        let bundle = original.ancestors().nth(3).unwrap();
        let selected_bundle = temp.path().join("自选 Chromium 浏览器.app");
        // Chromium resolves its frameworks relative to the executable's bundle layout.
        std::os::unix::fs::symlink(bundle, &selected_bundle).unwrap();
        selected_bundle
            .join(original.strip_prefix(bundle).unwrap())
            .into_os_string()
    };
    let selected = chrome.to_string_lossy().into_owned();
    let response = call(
        &app,
        "PUT",
        "/api/platform/browser",
        json!({"path":selected,"future":{"keep":true}}),
    )
    .await;
    let status = response.status();
    let result = body(response).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["available"], true);
    assert_eq!(result["resolved_path"], selected);
    let path = rua_config::desktop_automation::preferences_path(&state.cache_dir);
    let saved = rua_config::desktop_automation::DesktopAutomationPreferences::load(&path).unwrap();
    assert_eq!(saved.browser_path, Some(selected.into()));
    assert_eq!(saved.extra["future"]["keep"], true);
    let response = call(&app, "POST", "/api/platform/browser/recheck", json!({})).await;
    assert_eq!(body(response).await["available"], true);
}

#[tokio::test]
async fn missing_computer_host_has_device_diagnostics_without_breaking_settings() {
    let _environment = ENVIRONMENT.lock().await;
    let temp = tempfile::tempdir().unwrap();
    let mut state = AppState::from_tempdir(temp.path().join("data"), ServerProfile::Test);
    state.desktop_automation =
        std::sync::Arc::new(rua_server::desktop_automation::DesktopAutomationRuntime::new(None));
    let app = Router::new()
        .nest("/api", rua_server::endpoints::desktop_automation::router())
        .with_state(state);
    for method in ["GET", "POST"] {
        let path = if method == "GET" {
            "/api/platform/computer"
        } else {
            "/api/platform/computer/recheck"
        };
        let response = call(&app, method, path, json!({})).await;
        assert_eq!(response.status(), StatusCode::OK);
        let result = body(response).await;
        assert_eq!(result["available"], false);
        assert_eq!(result["state"], "unavailable");
        assert!(result["device_id"].is_string());
        assert!(!result["diagnostics"].as_array().unwrap().is_empty());
    }
}
