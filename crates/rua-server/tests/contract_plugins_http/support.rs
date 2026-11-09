use axum::body::Body;
use axum::extract::Request;
use axum::http::Method;
use axum::response::Response;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::plugins::router as plugins_router;
use rua_server::{AppState, ServerProfile};

pub struct TestApp {
    pub app: Router,
    pub tmp: TempDir,
}

pub async fn build_app() -> TestApp {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("default workspace");
    let data_dir = tmp.path().to_path_buf();
    let state =
        tokio::task::spawn_blocking(move || AppState::from_tempdir(data_dir, ServerProfile::Test))
            .await
            .expect("AppState construction");
    let app = Router::new()
        .nest("/api", plugins_router())
        .with_state(state);
    TestApp { app, tmp }
}

pub fn create_fixture_plugin(tmp: &TempDir, plugin_id: &str) -> String {
    let plugin_dir = tmp.path().join(plugin_id);
    std::fs::create_dir_all(&plugin_dir).expect("plugin dir");
    let manifest = json!({
        "id": plugin_id,
        "name": "Contract Plugin",
        "version": "1.0.0",
        "description": "A contract fixture plugin",
        "author": "rua-test",
        "entry_point": "plugin.wasm",
        "min_rua_version": "0.2.0",
        "meta": {
            "key": "value"
        }
    });
    std::fs::write(
        plugin_dir.join("plugin.json"),
        serde_json::to_string_pretty(&manifest).expect("manifest json"),
    )
    .expect("write plugin manifest");
    let minimal_wasm: Vec<u8> = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
    std::fs::write(plugin_dir.join("plugin.wasm"), &minimal_wasm).expect("write plugin wasm");
    plugin_dir.display().to_string()
}

pub async fn request(app: &Router, method: Method, uri: &str, body: Option<Value>) -> Response {
    let mut builder = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&value).expect("request json"))
        }
        None => Body::empty(),
    };

    app.clone()
        .oneshot(builder.body(body).expect("request"))
        .await
        .expect("oneshot")
}

pub async fn read_json(resp: Response) -> Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    if bytes.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(&bytes).expect("response json")
}
