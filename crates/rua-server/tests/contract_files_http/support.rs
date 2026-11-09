use axum::body::Body;
use axum::extract::Request;
use axum::http::Method;
use axum::response::Response;
use axum::Router;
use http_body_util::BodyExt;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::{
    config::file_preview_router, files::agent_scoped_router as agent_files_router,
};
use rua_server::{AppState, ServerProfile};

pub struct TestApp {
    pub app: Router,
    pub tmp: TempDir,
}

pub fn build_app() -> TestApp {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("default workspace");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    let app = Router::new()
        .nest("/api", file_preview_router().merge(agent_files_router()))
        .with_state(state);
    TestApp { app, tmp }
}

pub fn preview_uri(path: &std::path::Path) -> String {
    let raw = path.to_string_lossy();
    format!("/api/files/preview/{}", raw.trim_start_matches('/'))
}

pub async fn request(app: &Router, method: Method, uri: &str) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("oneshot")
}

pub async fn read_body(resp: Response) -> Vec<u8> {
    resp.into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes()
        .to_vec()
}

pub async fn read_json(resp: Response) -> serde_json::Value {
    let bytes = read_body(resp).await;
    serde_json::from_slice(&bytes).expect("response json")
}
