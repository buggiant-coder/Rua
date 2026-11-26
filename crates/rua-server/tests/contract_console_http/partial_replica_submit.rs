use axum::http::{Method, StatusCode};
use rua_session::WorkspaceSessionStore;
use serde_json::json;

use super::support::{build_app, read_json, request_json};

#[tokio::test]
async fn partial_replica_http_submit_rejects_before_model_preparation() {
    for partial in [
        PartialReplica::PlacementWithoutHeader,
        PartialReplica::HeaderlessEpoch,
    ] {
        let test = build_app();
        let chat_id = "00000000-0000-0000-0000-000000009981";
        // The Chat's configuration is one record in its Header, so a Chat has no
        // half-published configuration state to be replicated. What can arrive
        // at different times is the Chat Index placement and the authoritative
        // state, and submitting against the gap must not prepare a model.
        let mut body = json!({"agent_id":"default","chat_id":chat_id,"message":"must not execute"});
        match partial {
            PartialReplica::PlacementWithoutHeader => {
                let local = rua_server::services::device_context::current_sync_device_id(
                    &test.state.sync_device,
                )
                .expect("local device id");
                rua_server::services::chat_index::upsert(
                    test.state.chat_workspace().as_path(),
                    chat_index_record(chat_id, &local),
                )
                .await
                .unwrap();
            }
            PartialReplica::HeaderlessEpoch => {
                body["expected_authority_epoch"] = json!("remote-epoch");
            }
        }
        let response = request_json(
            &test.app,
            Method::POST,
            &format!("/chats/{chat_id}/console/submit"),
            body,
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT, "{partial:?}");
        let body = read_json(response).await;
        let detail = body["detail"].as_str().unwrap();
        assert!(
            detail.contains(partial.expected_detail()),
            "{partial:?}: {body}"
        );
        let store = WorkspaceSessionStore::new(test.state.chat_workspace().as_path());
        assert!(store.load_chat_header(chat_id).await.unwrap().is_none());
    }
}

#[derive(Debug, Clone, Copy)]
enum PartialReplica {
    /// The synchronized Chat Index already names this Device as the authority
    /// while the authoritative Header has not arrived yet.
    PlacementWithoutHeader,
    /// A caller carries an authority epoch for a Chat this Device knows nothing
    /// about.
    HeaderlessEpoch,
}

impl PartialReplica {
    /// Both cases must refuse the submission as "the Chat is not materialized
    /// yet"; the exact wording differs because one gap is a missing Header and
    /// the other is a missing placement.
    fn expected_detail(self) -> &'static str {
        match self {
            Self::PlacementWithoutHeader => "has not arrived",
            Self::HeaderlessEpoch => "not ready",
        }
    }
}

fn chat_index_record(chat_id: &str, device_id: &str) -> rua_session::chat_index::ChatIndexRecord {
    let now = chrono::Utc::now();
    rua_session::chat_index::ChatIndexRecord {
        id: chat_id.to_string(),
        name: "Partial".to_string(),
        user_id: "default".to_string(),
        channel: "console".to_string(),
        created_at: now,
        updated_at: now,
        meta: json!({
            rua_server::services::conversation_authority_placement::CHAT_AUTHORITY_PLACEMENT_META_KEY:
                json!({
                    "device_id": device_id,
                    "authority_epoch": "0000000000000001000000000000000000000000",
                    "base_manifest_hash":
                        "sha256:1111111111111111111111111111111111111111111111111111111111111111",
                }),
        }),
        status: "idle".to_string(),
        pinned: false,
        pin_order: 0,
        tag: rua_session::ChatTag::None,
        archived: false,
    }
}

#[tokio::test]
async fn header_without_configuration_does_not_get_local_defaults() {
    let test = build_app();
    let chat_id = "00000000-0000-0000-0000-000000009982";
    let runtime = test.state.registry.get_or_create("default").await.unwrap();
    let store = WorkspaceSessionStore::new(runtime.chat_workspace_dir());
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
        &format!("/chats/{chat_id}/console/submit"),
        json!({"message":"must not execute"}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        std::fs::read(store.chat_header_path(chat_id)).unwrap(),
        before
    );
    assert!(!store
        .load_chat_header(chat_id)
        .await
        .unwrap()
        .unwrap()
        .extra
        .contains_key("hook_configuration"));
}
