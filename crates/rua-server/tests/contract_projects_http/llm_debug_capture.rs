use axum::http::{Method, StatusCode};
use serde_json::{json, Map};

use super::support;

const CHAT_ID: &str = "shared-chat-id";

#[tokio::test]
async fn project_and_agent_scopes_with_the_same_chat_id_are_isolated() {
    let test = support::build_app();
    for address in [
        rua_core::ChatAddress::agent("agent-one", CHAT_ID),
        rua_core::ChatAddress::project("project-one", CHAT_ID),
    ] {
        let scope = rua_core::ChatScopeStore::resolve(&test.state.rua_root, address);
        let header_path =
            rua_session::WorkspaceSessionStore::new(scope.root()).chat_header_path(CHAT_ID);
        std::fs::create_dir_all(header_path.parent().expect("chat directory"))
            .expect("create chat directory");
        std::fs::write(header_path, b"{}").expect("materialize chat header");
    }
    let agent_uri = format!("/api/agents/agent-one/chats/{CHAT_ID}/llm-debug-capture");
    let project_uri = format!("/api/projects/project-one/chats/{CHAT_ID}/llm-debug-capture");

    let agent_response = support::request(&test.app, Method::POST, &agent_uri, None).await;
    assert_eq!(agent_response.status(), StatusCode::OK);
    let agent = support::json(agent_response).await;

    let project_response = support::request(&test.app, Method::POST, &project_uri, None).await;
    assert_eq!(project_response.status(), StatusCode::OK);
    let project = support::json(project_response).await;

    assert_eq!(agent["active"], true);
    assert_eq!(project["active"], true);
    assert_ne!(agent["capture_id"], project["capture_id"]);
    assert_ne!(agent["path"], project["path"]);
    let project_path = project["path"].as_str().expect("project capture path");
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(std::path::Path::new(project_path).join("manifest.json"))
            .expect("project capture manifest"),
    )
    .expect("project capture manifest JSON");
    assert_eq!(manifest["capture_id"], project["capture_id"]);
    assert_eq!(manifest["chat_address"]["scope"]["kind"], "project");
    assert_eq!(
        manifest["chat_address"]["scope"]["project_id"],
        "project-one"
    );
    assert_eq!(manifest["chat_address"]["chat_id"], CHAT_ID);

    let project_address = rua_core::ChatAddress::project("project-one", CHAT_ID);
    let model_call_metadata = Map::from_iter([
        (
            llm_multiplex::debug_capture::CHAT_ADDRESS_METADATA_KEY.to_string(),
            serde_json::to_value(&project_address).expect("chat address serializes"),
        ),
        (
            llm_multiplex::debug_capture::MODEL_CALL_ID_METADATA_KEY.to_string(),
            json!("project-call-1"),
        ),
    ]);
    assert!(llm_multiplex::debug_capture::record_named_json(
        &model_call_metadata,
        "logical-request.json",
        &json!({"messages": ["hello"]}),
    ));
    llm_multiplex::debug_capture::flush_capture(
        std::path::Path::new(project_path),
        std::time::Duration::from_secs(5),
    )
    .unwrap();
    assert!(std::path::Path::new(project_path)
        .join("calls/project-call-1/logical-request.json")
        .is_file());

    let stopped = support::request(&test.app, Method::DELETE, &project_uri, None).await;
    assert_eq!(stopped.status(), StatusCode::OK);
    assert_eq!(support::json(stopped).await["active"], false);
    let mut stopped_call_metadata = model_call_metadata;
    stopped_call_metadata.insert(
        llm_multiplex::debug_capture::MODEL_CALL_ID_METADATA_KEY.to_string(),
        json!("project-call-2"),
    );
    assert!(!llm_multiplex::debug_capture::record_named_json(
        &stopped_call_metadata,
        "logical-request.json",
        &json!({"messages": ["ignored"]}),
    ));

    let agent_still_active = support::request(&test.app, Method::GET, &agent_uri, None).await;
    assert_eq!(support::json(agent_still_active).await, agent);
}
