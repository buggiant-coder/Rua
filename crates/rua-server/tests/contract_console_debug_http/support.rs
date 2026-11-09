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
    _auth_env: EnvGuard,
}

impl TestApp {
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }
}

struct EnvGuard {
    previous: Option<String>,
}

impl EnvGuard {
    fn disable_auth() -> Self {
        let previous = std::env::var("RUA_AUTH_ENABLED").ok();
        std::env::remove_var("RUA_AUTH_ENABLED");
        Self { previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var("RUA_AUTH_ENABLED", value),
            None => std::env::remove_var("RUA_AUTH_ENABLED"),
        }
    }
}

pub fn build_app() -> TestApp {
    let auth_env = EnvGuard::disable_auth();
    let tmp = TempDir::new().expect("tempdir");
    let data_dir = tmp.path().to_path_buf();
    let state =
        rua_server::AppState::from_tempdir(data_dir.clone(), rua_server::ServerProfile::Test);
    let app = rua_server::router::build_app(state, None);
    TestApp {
        app,
        data_dir,
        _tmp: tmp,
        _auth_env: auth_env,
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
