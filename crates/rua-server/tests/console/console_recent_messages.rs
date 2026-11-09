use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::Router;
use http_body_util::BodyExt;
use tempfile::TempDir;
use tower::ServiceExt;

use rua_server::endpoints;
use rua_server::middleware::agent_context_middleware;
use rua_server::AppState;
use rua_session::{
    ChatMetadata, ChatSessionHeader, NewConversationMessage, NewConversationTurn, TurnLifecycle,
    WorkspaceSessionStore,
};
use serde_json::json;

fn build_state() -> (AppState, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("workspaces/default")).expect("seed workspace dir");
    let state = AppState::from_tempdir(tmp.path().to_path_buf(), rua_server::ServerProfile::Test);
    (state, tmp)
}

fn build_app(state: AppState) -> Router {
    Router::new()
        .merge(endpoints::console::agent_scoped_router())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            agent_context_middleware,
        ))
        .with_state(state)
}

async fn seed_evergreen_session_with_messages(
    ws_dir: &std::path::Path,
    agent_id: &str,
    chat_id: &str,
    count: usize,
) {
    let store = WorkspaceSessionStore::new(ws_dir);
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: agent_id.to_string(),
            title: "Evergreen Recent".to_string(),
            created_at: "2026-06-19T00:00:00Z".to_string(),
            last_activity_at: "2026-06-19T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .expect("save evergreen header");

    for i in 0..count {
        let role = if i % 2 == 0 { "user" } else { "assistant" };
        store
            .append_conversation_turn(
                chat_id,
                NewConversationTurn {
                    turn_id: format!("turn-{i}"),
                    lifecycle: TurnLifecycle {
                        user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                        assistant_started_at: None,
                        assistant_completed_at: None,
                        status: "completed".to_string(),
                    },
                    selected_variant_id: None,
                    messages: vec![NewConversationMessage {
                        message_id: format!("evergreen-msg-{i}"),
                        role: role.to_string(),
                        created_at: format!("2026-06-19T00:00:0{i}Z"),
                        status: "completed".to_string(),
                        content: json!([{"type": "text", "text": format!("evergreen message {i}")}]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    }],
                },
            )
            .await
            .expect("append evergreen message");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recent_messages_returns_three_stored_messages() {
    let (state, tmp) = build_state();
    let ws_dir = tmp.path().join("workspaces/default");
    let session_id = "test-session-001";
    seed_evergreen_session_with_messages(&ws_dir, "default", session_id, 3).await;

    let app = build_app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/agents/default/console/recent_messages?chat_id={session_id}"
                ))
                .header("X-Agent-Id", "default")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("oneshot");

    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    let messages: Vec<serde_json::Value> =
        serde_json::from_slice(&bytes).expect("parse JSON array");

    assert_eq!(
        messages.len(),
        3,
        "expected 3 messages, got {}",
        messages.len()
    );
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(messages[2]["role"], "user");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recent_messages_limit_parameter() {
    let (state, tmp) = build_state();
    let ws_dir = tmp.path().join("workspaces/default");
    let session_id = "test-session-limit";
    seed_evergreen_session_with_messages(&ws_dir, "default", session_id, 10).await;

    let app = build_app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/agents/default/console/recent_messages?chat_id={session_id}&limit=5"
                ))
                .header("X-Agent-Id", "default")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("oneshot");

    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    let messages: Vec<serde_json::Value> =
        serde_json::from_slice(&bytes).expect("parse JSON array");

    assert_eq!(messages.len(), 5, "expected 5 messages (limit=5)");

    assert_eq!(messages[0]["id"], "evergreen-msg-5");
    assert_eq!(messages[4]["id"], "evergreen-msg-9");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recent_messages_reads_evergreen_message_log_without_legacy_session() {
    let (state, tmp) = build_state();
    let ws_dir = tmp.path().join("workspaces/default");
    let session_id = "evergreen-recent-session";
    seed_evergreen_session_with_messages(&ws_dir, "default", session_id, 6).await;

    let app = build_app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/agents/default/console/recent_messages?chat_id={session_id}&limit=3"
                ))
                .header("X-Agent-Id", "default")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("oneshot");

    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    let messages: Vec<serde_json::Value> =
        serde_json::from_slice(&bytes).expect("parse JSON array");

    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0]["id"], "evergreen-msg-3");
    assert_eq!(messages[0]["content"][0]["text"], "evergreen message 3");
    assert_eq!(messages[2]["id"], "evergreen-msg-5");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recent_messages_empty_when_no_session() {
    let (state, _tmp) = build_state();
    let app = build_app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/agents/default/console/recent_messages")
                .header("X-Agent-Id", "default")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("oneshot");

    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    let messages: Vec<serde_json::Value> =
        serde_json::from_slice(&bytes).expect("parse JSON array");

    assert!(
        messages.is_empty(),
        "expected empty array when no session exists"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recent_messages_path_agent_overrides_header() {
    let (state, tmp) = build_state();
    let ws_dir = tmp.path().join("workspaces/default");
    let session_id = "test-root-session";
    seed_evergreen_session_with_messages(&ws_dir, "default", session_id, 3).await;

    let app = build_app(state);

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/agents/default/console/recent_messages?chat_id={session_id}"
                ))
                .header("X-Agent-Id", "other-agent")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("oneshot");

    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    let messages: Vec<serde_json::Value> =
        serde_json::from_slice(&bytes).expect("parse JSON array");

    assert_eq!(
        messages.len(),
        3,
        "path agent must select the stored messages despite the conflicting header"
    );
}
