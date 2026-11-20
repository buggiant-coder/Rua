use axum::http::{Method, StatusCode};
use rua_session::{ChatMetadata, ChatSessionHeader, WorkspaceSessionStore};
use serde_json::{json, Map};

use super::support;

#[tokio::test]
async fn captures_are_isolated_by_chat_id_across_project_membership() {
    let test = support::build_app();
    let ordinary_id = uuid::Uuid::new_v4().to_string();
    let project_chat_id = uuid::Uuid::new_v4().to_string();
    let store = WorkspaceSessionStore::new(test.state.rua_root.join("chats"));
    for (chat_id, project_id) in [
        (&ordinary_id, None),
        (&project_chat_id, Some("project-one")),
    ] {
        let now = chrono::Utc::now().to_rfc3339();
        let mut header = ChatSessionHeader::new(ChatMetadata {
            chat_id: chat_id.clone(),
            agent_id: "default".into(),
            title: "Capture fixture".into(),
            created_at: now.clone(),
            last_activity_at: now,
            pinned: false,
            archived: false,
            preview: None,
        });
        header.project_id = project_id.map(str::to_owned);
        store.save_chat_header(&header).await.unwrap();
    }
    let ordinary_uri = format!("/api/chats/{ordinary_id}/llm-debug-capture");
    let project_uri = format!("/api/chats/{project_chat_id}/llm-debug-capture");

    let ordinary_response = support::request(&test.app, Method::POST, &ordinary_uri, None).await;
    assert_eq!(ordinary_response.status(), StatusCode::OK);
    let ordinary = support::json(ordinary_response).await;

    let project_response = support::request(&test.app, Method::POST, &project_uri, None).await;
    assert_eq!(project_response.status(), StatusCode::OK);
    let project = support::json(project_response).await;

    assert_eq!(ordinary["active"], true);
    assert_eq!(project["active"], true);
    assert_ne!(ordinary["capture_id"], project["capture_id"]);
    assert_ne!(ordinary["path"], project["path"]);
    let project_path = project["path"].as_str().expect("project capture path");
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(std::path::Path::new(project_path).join("manifest.json"))
            .expect("project capture manifest"),
    )
    .expect("project capture manifest JSON");
    assert_eq!(manifest["capture_id"], project["capture_id"]);
    assert_eq!(
        manifest["chat_address"],
        json!({"chat_id": project_chat_id})
    );
    let same_capture = support::request(
        &test.app,
        Method::POST,
        &format!("{project_uri}?agent_id=another&project_id=unrelated"),
        None,
    )
    .await;
    assert_eq!(same_capture.status(), StatusCode::OK);
    assert_eq!(support::json(same_capture).await, project);

    let project_address = rua_core::ChatAddress::new(&project_chat_id);
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

    assert!(!std::path::Path::new(ordinary["path"].as_str().unwrap())
        .join("calls/project-call-1/logical-request.json")
        .exists());

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

    let ordinary_still_active = support::request(&test.app, Method::GET, &ordinary_uri, None).await;
    assert_eq!(support::json(ordinary_still_active).await, ordinary);
    let stopped = support::request(&test.app, Method::DELETE, &ordinary_uri, None).await;
    assert_eq!(stopped.status(), StatusCode::OK);
}
