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

fn create_fixture_plugin(tmp: &TempDir) -> String {
    let plugin_dir = tmp.path().join("fixture-plugin");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    let manifest = serde_json::json!({
        "id": "fixture-plugin",
        "name": "Fixture Plugin",
        "version": "1.0.0",
        "description": "A test fixture plugin",
        "author": "tester",
        "entry_point": "plugin.wasm",
        "min_rua_version": "0.2.0",
        "meta": {"key": "value"}
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_plugins_empty() {
    let (app, _tmp) = build_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/plugins")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body, serde_json::json!([]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_list_get_delete_lifecycle() {
    let (app, tmp) = build_app().await;
    let source_path = create_fixture_plugin(&tmp);

    let install_body = serde_json::json!({ "source": source_path });
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/plugins/install")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&install_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["plugin_id"], "fixture-plugin");
    assert_eq!(body["plugin_name"], "Fixture Plugin");
    assert_eq!(body["installed"], true);
    assert_eq!(body["source_path"], source_path);

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/plugins")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let list = body.as_array().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["id"], "fixture-plugin");
    assert_eq!(list[0]["version"], "1.0.0");
    assert_eq!(list[0]["enabled"], true);

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/plugins/fixture-plugin")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["id"], "fixture-plugin");
    assert_eq!(body["entry_point"], "plugin.wasm");
    assert_eq!(body["source_path"], source_path);
    assert_eq!(body["min_rua_version"], "0.2.0");
    assert_eq!(body["author"], "tester");

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/plugins/fixture-plugin")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body["plugin_id"], "fixture-plugin");
    assert_eq!(body["removed"], true);

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/plugins")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(body, serde_json::json!([]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_bad_source_returns_400() {
    let (app, _tmp) = build_app().await;
    let install_body = serde_json::json!({ "source": "/nonexistent/path" });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/plugins/install")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&install_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = read_json(resp).await;
    assert_eq!(body["status"], "failed");
    assert_eq!(body["installed"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_nonexistent_returns_404() {
    let (app, _tmp) = build_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/plugins/nonexistent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_nonexistent_returns_404() {
    let (app, _tmp) = build_app().await;
    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/plugins/nonexistent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
