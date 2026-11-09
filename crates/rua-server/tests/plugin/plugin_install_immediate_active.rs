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
    let plugin_dir = tmp.path().join("test-active-plugin");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    let manifest = serde_json::json!({
        "id": "test-active-plugin",
        "name": "Test Active Plugin",
        "version": "2.0.0",
        "description": "Plugin that should be immediately active after install",
        "author": "test-author",
        "entry_point": "plugin.wasm"
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
async fn install_then_list_shows_active_plugin() {
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
    assert_eq!(body["plugin_id"], "test-active-plugin");
    assert_eq!(body["installed"], true);

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
    let list = body.as_array().expect("response is array");

    let plugin = list
        .iter()
        .find(|p| p["id"] == "test-active-plugin")
        .expect("installed plugin should appear in list");

    assert_eq!(plugin["enabled"], true, "plugin should be enabled (active)");
    assert_eq!(plugin["version"], "2.0.0");
}
