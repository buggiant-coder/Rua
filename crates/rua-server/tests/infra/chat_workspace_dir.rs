#![allow(deprecated)]

use std::convert::Infallible;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints;
use rua_server::middleware::agent_context_middleware;
use rua_server::AppState;

const AGENT_ID: &str = "default";

fn build_state() -> (AppState, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    (state, tmp)
}

fn build_app(state: AppState) -> Router {
    Router::new()
        .merge(endpoints::chats::root_router())
        .merge(endpoints::chats::agent_scoped_router())
        .merge(endpoints::chat_prompt::root_router())
        .merge(endpoints::chat_prompt::agent_scoped_router())
        .merge(endpoints::chat_search::root_router())
        .merge(endpoints::chat_search::agent_scoped_router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state)
}

async fn read_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("body is json")
    }
}

fn json_post(uri: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header("X-Agent-Id", AGENT_ID)
        .body(Body::from(body.to_string()))
        .expect("request")
}

fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("X-Agent-Id", AGENT_ID)
        .body(Body::empty())
        .expect("request")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_then_list_chat_round_trip_uses_runtime_workspace_dir() {
    let (state, guard) = build_state();
    let app = build_app(state.clone());

    let create_req = json_post(
        "/chats",
        serde_json::json!({
            "name": "feat-604 round-trip chat",
        }),
    );
    let create_resp = app.clone().oneshot(create_req).await.expect("oneshot");
    assert_eq!(
        create_resp.status(),
        StatusCode::OK,
        "POST /chats must succeed via runtime.workspace_dir()"
    );
    let created: serde_json::Value = read_json(create_resp).await;
    let chat_id = created["id"].as_str().expect("created chat id").to_string();
    accept_first_input(&state, &chat_id).await;

    let expected_chat_index = guard
        .path()
        .join("workspaces")
        .join(AGENT_ID)
        .join("chat-index.sqlite");
    assert!(
        expected_chat_index.exists(),
        "chat-index.sqlite should be written under runtime.workspace_dir(), checked at {expected_chat_index:?}"
    );

    let list_req = get("/chats");
    let list_resp = app.clone().oneshot(list_req).await.expect("oneshot");
    assert_eq!(list_resp.status(), StatusCode::OK);
    let listed: serde_json::Value = read_json(list_resp).await;
    let arr = listed.as_array().expect("list returns array");
    assert!(
        arr.iter().any(|c| c["id"] == chat_id),
        "GET /chats must surface the chat we just created"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prompt_state_and_search_handlers_resolve_via_runtime() {
    let (state, _guard) = build_state();
    let app = build_app(state);

    let ps_req = get("/chats/prompt-state?chat_id=session-feat-604");
    let ps_resp = app.clone().oneshot(ps_req).await.expect("oneshot");
    assert_eq!(
        ps_resp.status(),
        StatusCode::OK,
        "prompt-state must succeed via runtime.workspace_dir()"
    );
    let ps_body: serde_json::Value = read_json(ps_resp).await;
    assert!(ps_body.get("mode").is_none());

    let search_req = get("/chats/search?q=anything");
    let search_resp = app.oneshot(search_req).await.expect("oneshot");
    assert_eq!(
        search_resp.status(),
        StatusCode::OK,
        "search must succeed via runtime.workspace_dir()"
    );
    let search_body: serde_json::Value = read_json(search_resp).await;
    let results = search_body["results"]
        .as_array()
        .expect("search returns results array");
    assert!(results.is_empty(), "no chats yet → no search hits");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scoped_route_resolves_runtime_through_registry() {
    let (state, guard) = build_state();
    let app = build_app(state.clone());

    let create_req = Request::builder()
        .method("POST")
        .uri(format!("/agents/{AGENT_ID}/chats"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::json!({"name": "agent-scoped feat-604"}).to_string(),
        ))
        .expect("request");
    let create_resp = app.oneshot(create_req).await.expect("oneshot");
    assert_eq!(create_resp.status(), StatusCode::OK);
    let created: serde_json::Value = read_json(create_resp).await;
    assert!(created["id"].is_string());
    accept_first_input(&state, created["id"].as_str().unwrap()).await;

    let expected_chat_index = guard
        .path()
        .join("workspaces")
        .join(AGENT_ID)
        .join("chat-index.sqlite");
    assert!(
        expected_chat_index.exists(),
        "agent-scoped POST should reuse the same workspace dir as the root handler"
    );
}

#[allow(dead_code)]
fn _assert_infallible() -> Infallible {
    unreachable!()
}

async fn accept_first_input(state: &AppState, chat_id: &str) {
    let outcome = state
        .conversation_input_intake
        .submit_delivery(
            AGENT_ID,
            rua_channel::core::console::ConsoleChatRequest {
                message: "feat-604 round-trip chat".into(),
                chat_id: Some(chat_id.into()),
                model: None,
                is_reconnect: false,
                attachments: vec![],
                input: None,
                messages: None,
                extra: Default::default(),
                channel_kind: None,
            },
        )
        .await;
    assert!(matches!(outcome, rua_server::services::conversation_input_intake::ConversationInputDeliveryOutcome::Delivered { .. }), "{outcome:?}");
}
