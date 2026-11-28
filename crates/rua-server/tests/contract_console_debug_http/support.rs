use axum::body::Body;
use axum::http::{Method, Request};
use axum::response::Response;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::Value;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tower::ServiceExt;

pub struct TestApp {
    pub app: Router,
    data_dir: PathBuf,
    _tmp: TempDir,
}

impl TestApp {
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }
}

pub fn build_app() -> TestApp {
    let tmp = TempDir::new().expect("tempdir");
    let data_dir = tmp.path().to_path_buf();
    let state =
        rua_server::AppState::from_tempdir(data_dir.clone(), rua_server::ServerProfile::Test);
    let app = rua_server::router::build_app(state, None);
    TestApp {
        app,
        data_dir,
        _tmp: tmp,
    }
}

pub async fn get(app: &Router, uri: &str) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
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
    serde_json::from_slice(&bytes).expect("response is JSON")
}

pub fn write_log(data_dir: &Path, content: &str) {
    let logs_dir = data_dir.join("logs");
    std::fs::create_dir_all(&logs_dir).expect("create logs dir");
    std::fs::write(logs_dir.join("rua.log"), content).expect("write rua log");
}
