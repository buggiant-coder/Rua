use axum::http::{Method, StatusCode};
use rua_session::WorkspaceSessionStore;
use serde_json::json;

use super::support::{build_app, read_json, request_json};

#[tokio::test]
async fn partial_replica_http_submit_rejects_before_model_preparation() {
    for partial in [
        Some("configuration_head.json"),
        Some("configurations/remote.json"),
        None,
    ] {
        let test = build_app();
        let chat_id = "00000000-0000-0000-0000-000000009981";
        let runtime = test.state.registry.get_or_create("default").await.unwrap();
        let store = WorkspaceSessionStore::new(runtime.workspace_dir());
        if let Some(relative) = partial {
            let file = store.chat_dir(chat_id).join(relative);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, b"{}\n").unwrap();
        }
        let mut body = json!({"chat_id":chat_id,"message":"must not execute"});
        if partial.is_none() {
            body["expected_authority_epoch"] = json!("remote-epoch");
        }
        let response = request_json(
            &test.app,
            Method::POST,
            &format!("/agents/default/chats/{chat_id}/console/submit"),
            body,
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = read_json(response).await;
        assert!(body["detail"].as_str().unwrap().contains("not ready"));
        assert!(store.load_chat_header(chat_id).await.unwrap().is_none());
    }
}

#[tokio::test]
async fn header_without_configuration_does_not_get_local_defaults() {
    let test = build_app();
    let chat_id = "00000000-0000-0000-0000-000000009982";
    let runtime = test.state.registry.get_or_create("default").await.unwrap();
    let store = WorkspaceSessionStore::new(runtime.workspace_dir());
    let mut header = rua_session::ChatSessionHeader::new(rua_session::ChatMetadata {
        chat_id: chat_id.into(),
        agent_id: "default".into(),
        title: "partial".into(),
        created_at: "2026-10-28T00:00:00Z".into(),
        last_activity_at: "2026-10-28T00:00:00Z".into(),
        pinned: false,
        archived: false,
        preview: None,
    });
    header.extra.insert("authority_placement".into(), json!({
        "device_id":rua_server::services::device_context::current_sync_device_id(&test.state.sync_device).unwrap(),
        "authority_epoch":"0000000000000001000000000000000000000000",
        "base_manifest_hash":"sha256:1111111111111111111111111111111111111111111111111111111111111111"
    }));
    store.save_chat_header(&header).await.unwrap();
    let before = std::fs::read(store.chat_header_path(chat_id)).unwrap();
    let response = request_json(
        &test.app,
        Method::POST,
        &format!("/agents/default/chats/{chat_id}/console/submit"),
        json!({"message":"must not execute"}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        std::fs::read(store.chat_header_path(chat_id)).unwrap(),
        before
    );
    assert!(!store.configuration_head_path(chat_id).exists());
}
