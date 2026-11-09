use axum::{
    body::Body,
    http::{Request, StatusCode},
    Extension, Router,
};
use http_body_util::BodyExt;
use rua_core::desktop_control::{DesktopControlCommand as Command, DesktopControlEndpoint};
use rua_server::endpoints::desktop_control::DesktopControlService;
use rua_server::{AppState, ServerProfile};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

fn app(root: &std::path::Path, endpoint: Option<DesktopControlEndpoint>) -> Router {
    let state = AppState::from_tempdir(root.join("data"), ServerProfile::Test);
    app_with_state(root, endpoint, state)
}

fn app_with_state(
    root: &std::path::Path,
    endpoint: Option<DesktopControlEndpoint>,
    state: AppState,
) -> Router {
    let source = root.join("unified rua");
    std::fs::write(&source, b"packaged-cli-bytes").unwrap();
    Router::new()
        .nest(
            "/api",
            Router::new()
                .merge(rua_server::endpoints::desktop_control::router())
                .merge(rua_server::endpoints::desktop_automation::router()),
        )
        .layer(Extension(DesktopControlService::new(
            endpoint,
            source,
            root.join("test-user"),
        )))
        .layer(axum::middleware::from_fn(
            rua_server::errors::json_error_middleware,
        ))
        .with_state(state)
}

async fn call(app: &Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .header("origin", "http://127.0.0.1:5188")
                .header("user-agent", "Mozilla/5.0 Chrome/140.0.0.0")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    (status, body)
}

#[tokio::test]
async fn ordinary_browser_installs_cli_into_execution_user_directory_without_tauri() {
    let temp = tempfile::tempdir().unwrap();
    let app = app(temp.path(), None);
    let (status, before) = call(&app, "GET", "/api/platform/desktop/install", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(before["cliInstalled"], false);
    let (status, after) = call(&app, "POST", "/api/platform/desktop/cli", json!({})).await;
    if cfg!(windows) {
        assert_eq!(status, StatusCode::CONFLICT);
        return;
    }
    assert_eq!(status, StatusCode::OK, "{after}");
    assert_eq!(after["cliInstalled"], true);
    let installed = std::path::Path::new(after["cliPath"].as_str().unwrap());
    assert!(installed.starts_with(temp.path().join("test-user")));
    assert_eq!(std::fs::read(installed).unwrap(), b"packaged-cli-bytes");
    let (_, refreshed) = call(&app, "GET", "/api/platform/desktop/install", Value::Null).await;
    assert_eq!(refreshed["cliInstalled"], true);
}

#[tokio::test]
async fn browser_http_requests_can_manage_the_installed_app_through_its_private_updater() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = DesktopControlEndpoint::new(listener.local_addr().unwrap()).unwrap();
    let cancel = CancellationToken::new();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let received = calls.clone();
    let server = tokio::spawn(
        endpoint
            .clone()
            .serve(listener, cancel.clone(), move |command| {
                let received = received.clone();
                async move {
                    received.lock().await.push(command.clone());
                    Ok(match command {
                        Command::LegacyStatus => {
                            json!({"platform":"macos","legacyRuntime":false,"legacyPort":null})
                        }
                        Command::UpdateStatus | Command::CheckUpdate => {
                            json!({"phase":"up_to_date","currentVersion":"0.1.0"})
                        }
                        Command::InstallUpdate | Command::MigrateLegacy { .. } => {
                            json!({"status":"accepted"})
                        }
                    })
                }
            }),
    );
    let temp = tempfile::tempdir().unwrap();
    let app = app(temp.path(), Some(endpoint));
    for (method, path, body, expected) in [
        (
            "GET",
            "/api/platform/desktop/install",
            Value::Null,
            Command::LegacyStatus,
        ),
        (
            "GET",
            "/api/platform/desktop/update",
            Value::Null,
            Command::UpdateStatus,
        ),
        (
            "POST",
            "/api/platform/desktop/update/check",
            json!({}),
            Command::CheckUpdate,
        ),
        (
            "POST",
            "/api/platform/desktop/update/install",
            json!({}),
            Command::InstallUpdate,
        ),
        (
            "POST",
            "/api/platform/desktop/legacy/migrate",
            json!({"confirmed":true}),
            Command::MigrateLegacy { confirmed: true },
        ),
    ] {
        let (status, result) = call(&app, method, path, body).await;
        assert!(status.is_success(), "{path}: {status} {result}");
        assert_eq!(calls.lock().await.last(), Some(&expected));
    }
    let previous = calls.lock().await.len();
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/platform/desktop/legacy/migrate",
            json!({"confirmed":false})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/platform/computer/permission-settings",
            json!({"setting":"arbitrary-command"})
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(calls.lock().await.len(), previous);
    cancel.cancel();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn missing_driver_is_a_device_diagnostic_and_does_not_disable_cli_installation() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = AppState::from_tempdir(temp.path().join("data"), ServerProfile::Test);
    state.desktop_automation =
        Arc::new(rua_server::desktop_automation::DesktopAutomationRuntime::new(None));
    let app = app_with_state(temp.path(), None, state);
    let (_, status) = call(&app, "GET", "/api/platform/desktop/update", Value::Null).await;
    assert_eq!(status["phase"], "unavailable");
    let (status, error) = call(
        &app,
        "POST",
        "/api/platform/computer/permissions",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(error["detail"]
        .as_str()
        .unwrap()
        .contains("execution device"));
    assert!(!error.to_string().contains("Tauri"));
    assert_eq!(
        call(&app, "GET", "/api/platform/desktop/install", Value::Null)
            .await
            .0,
        StatusCode::OK
    );
}

#[tokio::test]
#[ignore = "requires RUA_TEST_CUA_DRIVER; no Tauri, OS permission prompts or desktop input"]
async fn ordinary_browser_controls_the_runtime_owned_driver_without_a_shell() {
    use rua_mcp::bundled_computer::BundledComputer;
    use rua_server::desktop_automation::DesktopAutomationRuntime;
    let binary =
        std::path::PathBuf::from(std::env::var_os("RUA_TEST_CUA_DRIVER").expect("staged driver"));
    let temp = tempfile::tempdir().unwrap();
    let mut state = AppState::from_tempdir(temp.path().join("data"), ServerProfile::Test);
    state.profile = ServerProfile::Desktop;
    let descriptor = state
        .cache_dir
        .join("desktop-automation/computer/cua-connection.json");
    state.desktop_automation = Arc::new(DesktopAutomationRuntime::new(Some(BundledComputer {
        driver: binary,
        connection_file: descriptor.clone(),
    })));
    let runtime = state.desktop_automation.clone();
    let app = app_with_state(temp.path(), None, state);
    let (status, result) = call(&app, "POST", "/api/platform/computer/recheck", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert!(result["permissions"].is_object(), "{result}");
    let before: Value = serde_json::from_slice(&std::fs::read(&descriptor).unwrap()).unwrap();
    assert!(before["pid"].is_number(), "{before}");
    let (status, result) = call(&app, "POST", "/api/platform/computer/restart", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert!(result["permissions"].is_object(), "{result}");
    let after: Value = serde_json::from_slice(&std::fs::read(&descriptor).unwrap()).unwrap();
    assert_ne!(before["generation"], after["generation"]);
    runtime.stop().await;
    let stopped: Value = serde_json::from_slice(&std::fs::read(&descriptor).unwrap()).unwrap();
    assert!(stopped["error"].is_string());
    let (_, result) = call(&app, "POST", "/api/platform/computer/recheck", json!({})).await;
    assert_eq!(result["state"], "unavailable");
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(&descriptor).unwrap()).unwrap(),
        stopped
    );
}
