use std::path::PathBuf;

use axum::body::Body;
use axum::extract::Request;
use axum::http::Method;
use axum::response::Response;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints::{agents, chats, console};
use rua_server::middleware::agent_context_middleware;
use rua_server::{AppState, ServerProfile};

pub struct TestApp {
    pub app: Router,
    pub state: AppState,
    pub data_dir: PathBuf,
    _tmp: TempDir,
}

pub fn build_app() -> TestApp {
    let tmp = TempDir::new().expect("tempdir");
    let data_dir = tmp.path().to_path_buf();
    let state = AppState::from_tempdir(data_dir.clone(), ServerProfile::Test);
    let api = Router::new()
        .merge(agents::router())
        .merge(chats::root_router())
        .merge(chats::agent_scoped_router())
        .merge(console::agent_scoped_router());
    let app = Router::new()
        .nest("/api", api)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state.clone());

    TestApp {
        app,
        state,
        data_dir,
        _tmp: tmp,
    }
}

pub async fn request(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
    agent_id: Option<&str>,
) -> Response {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(agent_id) = agent_id {
        builder = builder.header("X-Agent-Id", agent_id);
    }
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

pub async fn create_agent(app: &Router, id: &str, name: &str) {
    let resp = request(
        app,
        Method::POST,
        "/api/agents",
        Some(json!({
            "id": id,
            "name": name,
            "description": ""
        })),
        Some("default"),
    )
    .await;
    assert_eq!(resp.status(), axum::http::StatusCode::CREATED);
}

pub async fn delete_agent(app: &Router, id: &str) {
    let resp = request(
        app,
        Method::DELETE,
        &format!("/api/agents/{id}"),
        None,
        Some("default"),
    )
    .await;
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
}

pub async fn create_chat(
    app: &Router,
    uri: &str,
    agent_id: Option<&str>,
    name: &str,
    user_id: &str,
) -> Value {
    let resp = request(
        app,
        Method::POST,
        uri,
        Some(json!({
            "name": name,
            "user_id": user_id,
            "channel": "console",
            "meta": {}
        })),
        agent_id,
    )
    .await;
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    read_json(resp).await
}

pub fn workspace_dir(data_dir: &std::path::Path, agent_id: &str) -> PathBuf {
    data_dir.join("workspaces").join(agent_id)
}

pub async fn accept_first_input(test: &TestApp, agent_id: &str, chat_id: &str, user_id: &str) {
    let response = request(
        &test.app,
        Method::POST,
        &format!("/api/agents/{agent_id}/chats/{chat_id}/console/submit"),
        Some(json!({"message":"integration first input","user_id":user_id})),
        Some(agent_id),
    )
    .await;
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body = read_json(response).await;
    assert_eq!(body["accepted"], true, "{body}");
    let address = rua_core::ChatAddress::agent(agent_id, chat_id);
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while test
            .state
            .conversation_authority
            .active_turn(&address)
            .await
            .is_some()
        {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("accepted first input finishes");
}
