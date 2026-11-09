use axum::http::{Method, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;

use super::support;

const AGENT_ID: &str = "agent-debug";
const CHAT_ID: &str = "chat-debug";

fn materialize_chat(test: &support::TestApp) -> (rua_core::ChatAddress, std::path::PathBuf) {
    let address = rua_core::ChatAddress::agent(AGENT_ID, CHAT_ID);
    let scope = rua_core::ChatScopeStore::resolve(&test.state.rua_root, address.clone());
    let header_path =
        rua_session::WorkspaceSessionStore::new(scope.root()).chat_header_path(CHAT_ID);
    std::fs::create_dir_all(header_path.parent().expect("chat directory"))
        .expect("create chat directory");
    std::fs::write(&header_path, b"{}").expect("materialize chat header");
    (address, scope.root().to_path_buf())
}

fn capture_uri(chat_id: &str) -> String {
    format!("/agents/{AGENT_ID}/chats/{chat_id}/llm-debug-capture")
}

async fn request_json(app: &axum::Router, method: Method, uri: &str) -> (StatusCode, Value) {
    let response = support::request(app, method, uri).await;
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let body = serde_json::from_slice(&bytes).expect("JSON response");
    (status, body)
}

#[tokio::test]
async fn agent_chat_capture_can_start_query_and_stop() {
    let test = support::build_app();
    let (address, scope_root) = materialize_chat(&test);
    let uri = capture_uri(CHAT_ID);

    let (status, inactive) = request_json(&test.app, Method::GET, &uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(inactive, serde_json::json!({ "active": false }));

    let (status, started) = request_json(&test.app, Method::POST, &uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(started["active"], true);
    let capture_id = started["capture_id"].as_str().expect("capture id");
    let path = started["path"].as_str().expect("capture path");
    assert!(path.contains("debug/llm-captures"));
    assert!(path.ends_with(capture_id));
    assert!(std::path::Path::new(path).is_dir());
    assert!(!path.contains("/sessions/"));
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(std::path::Path::new(path).join("manifest.json")).expect("capture manifest"),
    )
    .expect("capture manifest JSON");
    assert_eq!(manifest["schema_version"], 1);
    assert_eq!(manifest["capture_id"], capture_id);
    assert_eq!(manifest["chat_address"]["scope"]["kind"], "agent");
    assert_eq!(manifest["chat_address"]["scope"]["agent_id"], AGENT_ID);
    assert_eq!(manifest["chat_address"]["chat_id"], CHAT_ID);
    assert!(manifest["created_at"].as_str().is_some());
    let state_path = scope_root
        .join("sessions")
        .join(CHAT_ID)
        .join("llm_debug_capture.json");
    let persisted: Value =
        serde_json::from_slice(&std::fs::read(&state_path).expect("persisted capture state"))
            .expect("persisted capture state JSON");
    assert_eq!(persisted["enabled"], true);
    assert_eq!(persisted["capture_id"], capture_id);

    llm_multiplex::debug_capture::unregister_capture(&address);
    let restored_registry =
        rua_server::services::llm_debug_capture::LlmDebugCaptureRegistry::new(&test.state.rua_root);
    let restored = restored_registry
        .get(&scope_root, &address)
        .expect("restore capture state")
        .expect("capture remains enabled after restart");
    assert_eq!(restored.capture_id, capture_id);
    assert_eq!(restored.path, std::path::Path::new(path));

    let (status, queried) = request_json(&test.app, Method::GET, &uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(queried, started);

    let other_uri = capture_uri("another-chat");
    let (_, other) = request_json(&test.app, Method::GET, &other_uri).await;
    assert_eq!(other, serde_json::json!({ "active": false }));

    let (status, stopped) = request_json(&test.app, Method::DELETE, &uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stopped, serde_json::json!({ "active": false }));

    let (_, queried_after_stop) = request_json(&test.app, Method::GET, &uri).await;
    assert_eq!(queried_after_stop, serde_json::json!({ "active": false }));

    let persisted: Value =
        serde_json::from_slice(&std::fs::read(&state_path).expect("disabled capture state"))
            .expect("disabled capture state JSON");
    assert_eq!(persisted["enabled"], false);
    assert!(persisted.get("capture_id").is_none());

    let restarted_registry =
        rua_server::services::llm_debug_capture::LlmDebugCaptureRegistry::new(&test.state.rua_root);
    assert_eq!(
        restarted_registry
            .get(&scope_root, &address)
            .expect("load disabled capture state"),
        None
    );
}
