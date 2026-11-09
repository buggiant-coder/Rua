use std::sync::Arc;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::response::Response;
use axum::Router;
use http_body_util::BodyExt;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_channel::core::console::ConsoleChatFacade;
use rua_server::endpoints;
use rua_server::middleware::agent_context_middleware;
use rua_server::{AppState, ServerProfile};

pub struct TestApp {
    pub app: Router,
    pub state: AppState,
    _tmp: TempDir,
}

pub fn build_app() -> TestApp {
    build_app_internal(None)
}

pub fn build_app_with_console_facade(facade: Arc<dyn ConsoleChatFacade>) -> TestApp {
    build_app_internal(Some(facade))
}

fn build_app_internal(facade: Option<Arc<dyn ConsoleChatFacade>>) -> TestApp {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("workspace dir");
    let mut state = AppState::from_tempdir(tmp.path().to_path_buf(), ServerProfile::Test);
    if let Some(facade) = facade {
        state.console_facade = facade;
    }
    let app = Router::new()
        .merge(endpoints::console::agent_scoped_router())
        .nest("/api", endpoints::chats::agent_scoped_router())
        .nest("/api", endpoints::chat_media::router())
        .nest("/api", endpoints::projects::router())
        .nest("/api", endpoints::sync::router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .layer(axum::extract::DefaultBodyLimit::disable())
        .with_state(state.clone());

    TestApp {
        app,
        state,
        _tmp: tmp,
    }
}

pub async fn request(app: &Router, method: Method, uri: &str) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("X-Agent-Id", "default")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("oneshot")
}

pub async fn request_json(
    app: &Router,
    method: Method,
    uri: &str,
    body: serde_json::Value,
) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("X-Agent-Id", "default")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).expect("request json")))
                .expect("request"),
        )
        .await
        .expect("oneshot")
}

pub async fn multipart_upload(
    app: &Router,
    uri: &str,
    field_name: &str,
    file_name: &str,
    data: Vec<u8>,
) -> Response {
    multipart_upload_with_fields(app, uri, &[], field_name, file_name, data).await
}

pub async fn multipart_upload_with_fields(
    app: &Router,
    uri: &str,
    fields: &[(&str, &str)],
    field_name: &str,
    file_name: &str,
    data: Vec<u8>,
) -> Response {
    let boundary = "rua-console-contract-boundary";
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(value.as_bytes());
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"{field_name}\"; filename=\"{file_name}\"\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    body.extend_from_slice(&data);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    app.clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(uri)
                .header("X-Agent-Id", "default")
                .header(
                    "content-type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .expect("request"),
        )
        .await
        .expect("oneshot")
}

pub async fn read_json(resp: Response) -> serde_json::Value {
    assert_ne!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "expected JSON response, got 204"
    );
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("response json")
}
