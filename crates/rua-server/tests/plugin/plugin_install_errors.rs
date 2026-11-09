use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::plugins::router as plugins_router;
use rua_server::AppState;

async fn build_app() -> (Router, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).unwrap();
    let data_dir = tmp.path().to_path_buf();
    let state = tokio::task::spawn_blocking(move || {
        AppState::from_tempdir(data_dir, rua_server::ServerProfile::Test)
    })
    .await
    .expect("AppState construction");
    let app = Router::new()
        .nest("/api", plugins_router())
        .with_state(state);
    (app, tmp)
}

fn create_valid_plugin(tmp: &TempDir, id: &str) -> String {
    let plugin_dir = tmp.path().join(id);
    std::fs::create_dir_all(&plugin_dir).unwrap();
    let manifest = serde_json::json!({
        "id": id,
        "name": format!("{} Plugin", id),
        "version": "1.0.0",
        "description": "test plugin",
        "author": "tester",
        "entry_point": "plugin.wasm",
        "min_rua_version": "0.2.0",
        "meta": {}
    });
    std::fs::write(
        plugin_dir.join("plugin.json"),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();
    let minimal_wasm: Vec<u8> = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
    std::fs::write(plugin_dir.join("plugin.wasm"), &minimal_wasm).unwrap();
    plugin_dir.display().to_string()
}

fn create_plugin_with_bad_manifest(tmp: &TempDir) -> String {
    let plugin_dir = tmp.path().join("bad-manifest-plugin");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(plugin_dir.join("plugin.json"), "{ not valid json !!!").unwrap();
    let minimal_wasm: Vec<u8> = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
    std::fs::write(plugin_dir.join("plugin.wasm"), &minimal_wasm).unwrap();
    plugin_dir.display().to_string()
}

fn create_plugin_with_bad_tool(tmp: &TempDir) -> String {
    let plugin_dir = tmp.path().join("bad-tool-plugin");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    let manifest = serde_json::json!({
        "id": "bad-tool-plugin",
        "name": "Bad Tool Plugin",
        "version": "1.0.0",
        "description": "plugin with an invalid tool spec in meta",
        "author": "tester",
        "entry_point": "plugin.wasm",
        "min_rua_version": "0.2.0",
        "meta": {
            "tools": [
                {"not_a_name": "missing-name-field"}
            ]
        }
    });
    std::fs::write(
        plugin_dir.join("plugin.json"),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();
    let minimal_wasm: Vec<u8> = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
    std::fs::write(plugin_dir.join("plugin.wasm"), &minimal_wasm).unwrap();
    plugin_dir.display().to_string()
}

async fn read_json(resp: axum::response::Response) -> Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("response is JSON")
}

async fn install_request(app: &Router, body: Value) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/plugins/install")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_bad_manifest_returns_422_with_error_detail() {
    let (app, tmp) = build_app().await;
    let source = create_plugin_with_bad_manifest(&tmp);

    let resp = install_request(&app, serde_json::json!({ "source": source })).await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let body = read_json(resp).await;
    assert_eq!(body["status"], "failed");
    let errors = body["errors"].as_array().expect("errors array");
    assert!(!errors.is_empty());
    assert_eq!(errors[0]["kind"], "manifest_parse_failed");
    assert!(errors[0]["detail"].as_str().unwrap().contains("parse"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_nonexistent_source_returns_422() {
    let (app, _tmp) = build_app().await;

    let resp = install_request(
        &app,
        serde_json::json!({ "source": "/nonexistent/path/to/plugin" }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let body = read_json(resp).await;
    assert_eq!(body["status"], "failed");
    let errors = body["errors"].as_array().expect("errors array");
    assert!(!errors.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_with_bad_tool_returns_207_partial() {
    let (app, tmp) = build_app().await;
    let source = create_plugin_with_bad_tool(&tmp);

    let resp = install_request(&app, serde_json::json!({ "source": source })).await;
    assert_eq!(resp.status(), StatusCode::MULTI_STATUS);

    let body = read_json(resp).await;
    assert_eq!(body["status"], "partial");
    assert_eq!(body["plugin_id"], "bad-tool-plugin");
    assert_eq!(body["plugin_name"], "Bad Tool Plugin");

    let errors = body["errors"].as_array().expect("errors array");
    assert!(!errors.is_empty());
    assert_eq!(errors[0]["kind"], "tool_register_failed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_success_returns_200_installed() {
    let (app, tmp) = build_app().await;
    let source = create_valid_plugin(&tmp, "success-plugin");

    let resp = install_request(&app, serde_json::json!({ "source": source })).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let body = read_json(resp).await;
    assert_eq!(body["status"], "installed");
    assert_eq!(body["plugin_id"], "success-plugin");
    assert_eq!(body["plugin_name"], "success-plugin Plugin");
    assert_eq!(body["source_path"], source);
    assert!(body.get("errors").is_none() || body["errors"].as_array().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn force_reinstall_returns_200() {
    let (app, tmp) = build_app().await;
    let source = create_valid_plugin(&tmp, "reinstall-plugin");

    let resp = install_request(
        &app,
        serde_json::json!({ "source": source, "force": false }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = install_request(&app, serde_json::json!({ "source": source, "force": true })).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let body = read_json(resp).await;
    assert_eq!(body["status"], "installed");
    assert_eq!(body["plugin_id"], "reinstall-plugin");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_install_without_force_returns_422() {
    let (app, tmp) = build_app().await;
    let source = create_valid_plugin(&tmp, "dup-plugin");

    let resp = install_request(&app, serde_json::json!({ "source": source })).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = install_request(&app, serde_json::json!({ "source": source })).await;
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let body = read_json(resp).await;
    assert_eq!(body["status"], "failed");
    let errors = body["errors"].as_array().expect("errors array");
    assert_eq!(errors[0]["kind"], "registry_add_failed");
}
