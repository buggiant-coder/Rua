use axum::body::Body;
use axum::http::{HeaderName, HeaderValue, Method, Request, StatusCode};
use axum::response::Response;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

pub struct TestApp {
    pub app: Router,
    _tmp: TempDir,
    _auth_env: EnvGuard,
}

struct EnvGuard {
    previous: Option<String>,
}

impl EnvGuard {
    fn enable_auth() -> Self {
        let previous = std::env::var("RUA_AUTH_ENABLED").ok();
        std::env::set_var("RUA_AUTH_ENABLED", "true");
        Self { previous }
    }

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
    let auth_env = EnvGuard::enable_auth();
    let tmp = TempDir::new().expect("tempdir");
    let state = rua_server::AppState::from_tempdir(
        tmp.path().to_path_buf(),
        rua_server::ServerProfile::Test,
    );
    let app = rua_server::router::build_app(state, None);
    TestApp {
        app,
        _tmp: tmp,
        _auth_env: auth_env,
    }
}

pub fn build_inactive_auth_app() -> TestApp {
    let auth_env = EnvGuard::disable_auth();
    let tmp = TempDir::new().expect("tempdir");
    let mut state = rua_server::AppState::from_tempdir(
        tmp.path().to_path_buf(),
        rua_server::ServerProfile::Test,
    );
    state.auth = Arc::new(rua_server::auth::AuthService::new_disabled());
    let app = Router::new()
        .nest("/api/auth", rua_server::endpoints::auth::router())
        .with_state(state);
    TestApp {
        app,
        _tmp: tmp,
        _auth_env: auth_env,
    }
}

pub fn build_full_no_auth_app() -> TestApp {
    let auth_env = EnvGuard::disable_auth();
    let tmp = TempDir::new().expect("tempdir");
    let mut state = rua_server::AppState::from_tempdir(
        tmp.path().to_path_buf(),
        rua_server::ServerProfile::Test,
    );
    state.auth = Arc::new(rua_server::auth::AuthService::new_disabled());
    let app = rua_server::router::build_app(state, None);
    TestApp {
        app,
        _tmp: tmp,
        _auth_env: auth_env,
    }
}

pub async fn request(app: &Router, method: Method, uri: &str, body: Option<Value>) -> Response {
    request_with_token(app, method, uri, body, None).await
}

pub async fn request_with_token(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
    token: Option<&str>,
) -> Response {
    request_with_token_and_headers(app, method, uri, body, token, &[]).await
}

pub async fn request_with_token_and_headers(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
    token: Option<&str>,
    headers: &[(&str, &str)],
) -> Response {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    for (name, value) in headers {
        builder = builder.header(
            HeaderName::from_bytes(name.as_bytes()).expect("header name"),
            HeaderValue::from_str(value).expect("header value"),
        );
    }
    let body = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&value).expect("serialize request body"))
        }
        None => Body::empty(),
    };

    app.clone()
        .oneshot(builder.body(body).expect("request"))
        .await
        .expect("oneshot")
}

pub async fn authed_get(app: &Router, uri: &str, token: &str) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
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

pub async fn login(app: &Router, username: &str, password: &str) -> Value {
    let resp = request(
        app,
        Method::POST,
        "/api/auth/login",
        Some(json!({
            "username": username,
            "password": password,
        })),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    read_json(resp).await
}
