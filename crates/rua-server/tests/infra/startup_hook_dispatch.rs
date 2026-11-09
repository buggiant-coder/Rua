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

fn create_plugin_with_startup_hook(tmp: &TempDir) -> String {
    let plugin_dir = tmp.path().join("startup-plugin");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    let manifest = serde_json::json!({
        "id": "startup-plugin",
        "name": "Startup Plugin",
        "version": "1.0.0",
        "description": "Plugin with startup hook",
        "author": "test",
        "entry_point": "plugin.wasm",
        "meta": {
            "hooks": [
                {
                    "hook_point": "startup",
                    "hook_name": "on_init",
                    "callback": "on_startup",
                    "priority": 100
                }
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_plugin_with_startup_hook_dispatches_and_succeeds() {
    let (app, tmp) = build_app().await;
    let source_path = create_plugin_with_startup_hook(&tmp);

    let install_body = serde_json::json!({ "source": source_path });
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

    let status = resp.status();
    assert!(
        status == StatusCode::OK || status == StatusCode::MULTI_STATUS,
        "unexpected status {status}"
    );
    let body = read_json(resp).await;
    assert_eq!(body["plugin_id"], "startup-plugin");
    assert!(body["installed"].as_bool().unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plugin_detail_shows_startup_hooks() {
    let (app, tmp) = build_app().await;
    let source_path = create_plugin_with_startup_hook(&tmp);

    let install_body = serde_json::json!({ "source": source_path });
    app.clone()
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

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/plugins/startup-plugin")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    let hooks = body["startup_hooks"]
        .as_array()
        .expect("startup_hooks array");
    assert!(!hooks.is_empty(), "startup_hooks should be populated");
    assert_eq!(hooks[0], "on_init");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_start_dispatches_startup_for_persisted_plugins() {
    let tmp = TempDir::new().expect("tempdir");
    let data_dir = tmp.path().to_path_buf();
    std::fs::create_dir_all(data_dir.join("workspaces/default")).unwrap();

    let source_path = create_plugin_with_startup_hook(&tmp);

    let plugins_json = serde_json::json!({
        "startup-plugin": {
            "id": "startup-plugin",
            "name": "Startup Plugin",
            "version": "1.0.0",
            "description": "Plugin with startup hook",
            "enabled": true,
            "installed": true,
            "source_path": source_path,
            "meta": {
                "hooks": [
                    {
                        "hook_point": "startup",
                        "hook_name": "on_init",
                        "callback": "on_startup",
                        "priority": 100
                    }
                ]
            }
        }
    });
    std::fs::write(
        data_dir.join("plugins.json"),
        serde_json::to_string_pretty(&plugins_json).unwrap(),
    )
    .unwrap();

    let dd = data_dir.clone();
    let state = tokio::task::spawn_blocking(move || {
        AppState::from_tempdir(dd, rua_server::ServerProfile::Test)
    })
    .await
    .expect("AppState construction should not panic on startup hook dispatch failure");

    let mut loaded = Vec::new();
    for _ in 0..20 {
        loaded = state.plugin_registry.list().unwrap_or_default();
        if loaded.contains(&"startup-plugin".to_string()) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        loaded.contains(&"startup-plugin".to_string()),
        "persisted plugin should be loaded into runtime registry on cold start"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_failure_does_not_block_startup() {
    let tmp = TempDir::new().expect("tempdir");
    let data_dir = tmp.path().to_path_buf();
    std::fs::create_dir_all(data_dir.join("workspaces/default")).unwrap();

    let plugins_json = serde_json::json!({
        "broken-plugin": {
            "id": "broken-plugin",
            "name": "Broken Plugin",
            "version": "1.0.0",
            "enabled": true,
            "installed": true,
            "source_path": "/nonexistent/path/to/plugin"
        }
    });
    std::fs::write(
        data_dir.join("plugins.json"),
        serde_json::to_string_pretty(&plugins_json).unwrap(),
    )
    .unwrap();

    let dd = data_dir.clone();
    let _state = tokio::task::spawn_blocking(move || {
        AppState::from_tempdir(dd, rua_server::ServerProfile::Test)
    })
    .await
    .expect("AppState construction must not panic even with broken plugins");
}
