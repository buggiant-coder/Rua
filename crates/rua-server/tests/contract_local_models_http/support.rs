use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use axum::http::Method;
use axum::response::Response;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::{local_models, local_models_download_stream};
use rua_server::{AppState, ServerProfile};

pub struct TestApp {
    pub app: Router,
    pub state: AppState,
    pub local_model_dir: PathBuf,
    _tmp: TempDir,
}

pub fn build_app() -> TestApp {
    let tmp = TempDir::new().expect("tempdir");
    let mut state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let local_model_dir = tmp.path().join("local-models");
    state.local_model = Arc::new(
        rua_local_model::LocalModelManager::new_for_test(local_model_dir.clone())
            .with_scan_enabled(false)
            .with_llama_binary("sleep")
            .with_skip_health_check(true),
    );
    let app = Router::new()
        .nest("/api", local_models::router())
        .nest("/api", local_models_download_stream::router())
        .with_state(state.clone());

    TestApp {
        app,
        state,
        local_model_dir,
        _tmp: tmp,
    }
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
