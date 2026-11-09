use std::sync::Arc;

use rua_channel::core::console::ConsoleChatRequest;
use rua_server::services::conversation_authority::ConversationInputAcceptanceMode;
use rua_server::services::device_command_bus::{
    RemoteConversationInputAck, RemoteConversationInputRequest,
};
use rua_server::services::remote_conversation_input::{
    RemoteConversationInputDelivery, RemoteConversationInputParams,
};
use rua_session::{
    ChatMetadata, ChatSessionHeader, NewConversationMessage, NewConversationTurn, TurnLifecycle,
    WorkspaceSessionStore,
};
use tempfile::TempDir;

const CHAT_ID: &str = "00000000-0000-0000-0000-000000009902";

fn handoff(data_dir: &std::path::Path) -> RemoteConversationInputDelivery {
    RemoteConversationInputDelivery::new(
        Arc::new(tokio::sync::Mutex::new(None)),
        Arc::new(rua_server::services::device_session_access::DeviceNetworkAccess::new(None, Arc::new(rua_server::services::device_network_availability::DeviceNetworkAvailability::new()))),
        data_dir.to_path_buf(),
    )
}

fn request(message: &str) -> ConsoleChatRequest {
    ConsoleChatRequest {
        message: message.to_string(),
        chat_id: Some(CHAT_ID.to_string()),
        model: None,
        is_reconnect: false,
        attachments: Vec::new(),
        input: None,
        messages: None,
        extra: serde_json::Map::new(),
        channel_kind: None,
    }
}

fn input_params<'a>(
    request: &'a ConsoleChatRequest,
    workspace_dir: &'a std::path::Path,
    address: &'a rua_core::ChatAddress,
) -> RemoteConversationInputParams<'a> {
    RemoteConversationInputParams {
        address,
        execution_agent_id: "default",
        request,
        source_device_id: "source-device",
        target_device_id: "target-device",
        authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
        base_manifest_hash:
            "sha256:1111111111111111111111111111111111111111111111111111111111111111",
        workspace_dir: Some(workspace_dir),
        materialize_conversation: false,
    }
}

fn ack_for(request: &RemoteConversationInputRequest) -> RemoteConversationInputAck {
    RemoteConversationInputAck {
        submission_id: request.submission_id.clone(),
        input_id: "input-remote".to_string(),
        mode: ConversationInputAcceptanceMode::Started,
        device_delivery_id: request.device_delivery_id.clone(),
        target_device_id: request.target_device_id.clone(),
        accepted_at: "2026-07-07T00:00:00Z".to_string(),
        control: None,
    }
}

#[tokio::test]
async fn remote_managed_media_requirement_is_derived_only_from_media_ref() {
    let temp = TempDir::new().expect("tempdir");
    let workspace_dir = temp.path().join("workspaces/default");
    std::fs::create_dir_all(&workspace_dir).expect("workspace");
    let handoff = handoff(temp.path());
    let address = rua_core::ChatAddress::agent("default", CHAT_ID);
    let mut console_request = request("look at this");
    console_request.input = Some(vec![serde_json::json!({
        "role": "user",
        "content": [
            { "type": "text", "text": "look at this" },
            { "type": "image", "media_ref": "media/uuid-image.png" }
        ]
    })]);

    let remote_request = handoff
        .build_remote_conversation_input_request(&input_params(
            &console_request,
            &workspace_dir,
            &address,
        ))
        .await
        .expect("remote request");

    assert_eq!(remote_request.media_requirements.len(), 1);
    let requirement = &remote_request.media_requirements[0];
    assert_eq!(requirement.media_ref, "media/uuid-image.png");
    assert_eq!(requirement.filename, "uuid-image.png");
    assert_eq!(remote_request.input, console_request.input);
}

#[tokio::test]
async fn first_input_media_requirements_fingerprint_staged_bytes() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("workspaces/default");
    std::fs::create_dir_all(root.join("media")).unwrap();
    let media_id = "00000000-0000-4000-8000-000000000111";
    std::fs::write(root.join("media").join(media_id), b"abc").unwrap();
    let mut request = request("first staged media");
    request.input = Some(vec![
        serde_json::json!({"role":"user","content":[{"type":"file","media_ref":format!("media/{media_id}")} ]}),
    ]);
    let address = rua_core::ChatAddress::agent("default", CHAT_ID);
    let mut params = input_params(&request, &root, &address);
    params.materialize_conversation = true;
    let prepared = handoff(tmp.path())
        .build_remote_conversation_input_request(&params)
        .await
        .unwrap();
    let media = &prepared.media_requirements[0];
    assert_eq!(media.byte_size, 3);
    assert_eq!(
        media.sha256,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    std::fs::remove_file(root.join("media").join(media_id)).unwrap();
    assert!(handoff(tmp.path())
        .build_remote_conversation_input_request(&params)
        .await
        .is_err());
}

#[tokio::test]
async fn build_remote_conversation_input_request_uses_pending_marker_for_next_sequence() {
    let temp = TempDir::new().expect("tempdir");
    let workspace_dir = temp.path().join("workspaces/default");
    std::fs::create_dir_all(&workspace_dir).expect("workspace");
    let handoff = handoff(temp.path());
    let address = rua_core::ChatAddress::agent("default", CHAT_ID);
    let mut first_request = request("first");
    first_request.extra.insert(
        "submission_id".to_string(),
        serde_json::json!("submission-first"),
    );

    let first = handoff
        .build_remote_conversation_input_request(&input_params(
            &first_request,
            &workspace_dir,
            &address,
        ))
        .await
        .expect("first remote request");
    assert_eq!(first.device_delivery_sequence, 1);
    assert_eq!(first.submission_id, "submission-first");
    assert!(first.device_delivery_id.starts_with("device-delivery-"));
    assert_eq!(first.text, "first");
    assert_eq!(
        first.input.as_ref().unwrap()[0]["content"][0]["text"],
        "first"
    );

    handoff
        .record_source_pending(&first, &ack_for(&first))
        .await
        .expect("record pending");
    let second_request = request("second");
    let second = handoff
        .build_remote_conversation_input_request(&input_params(
            &second_request,
            &workspace_dir,
            &address,
        ))
        .await
        .expect("second remote request");
    assert_eq!(second.device_delivery_sequence, 2);
}

#[tokio::test]
async fn build_remote_conversation_input_request_does_not_carry_chat_model_slot() {
    let temp = TempDir::new().expect("tempdir");
    let workspace_dir = temp.path().join("workspaces/default");
    std::fs::create_dir_all(&workspace_dir).expect("workspace");
    let store = WorkspaceSessionStore::new(&workspace_dir);
    let mut header = ChatSessionHeader::new(ChatMetadata {
        chat_id: CHAT_ID.to_string(),
        agent_id: "default".to_string(),
        title: "Remote model slot".to_string(),
        created_at: "2026-07-19T00:00:00Z".to_string(),
        last_activity_at: "2026-07-19T00:00:00Z".to_string(),
        pinned: false,
        archived: false,
        preview: None,
    });
    header.extra.insert(
        "model".to_string(),
        serde_json::json!({
            "provider_id": "dashscope",
            "model": "qwen-plus",
            "thinking_strength": "high"
        }),
    );
    store.save_chat_header(&header).await.expect("save header");

    let handoff = handoff(temp.path());
    let address = rua_core::ChatAddress::agent("default", CHAT_ID);
    let mut console_request = request("hello remote model");
    console_request.extra.insert(
        "chat_model_slot".to_string(),
        serde_json::json!({
            "provider_id": "source-provider",
            "model": "source-model"
        }),
    );
    let remote_request = handoff
        .build_remote_conversation_input_request(&input_params(
            &console_request,
            &workspace_dir,
            &address,
        ))
        .await
        .expect("remote request");

    assert!(
        remote_request.metadata.get("chat_model_slot").is_none(),
        "ordinary submit transport must not carry source-side model state"
    );
}

#[tokio::test]
async fn materializing_conversation_carries_initial_chat_model() {
    let temp = TempDir::new().expect("tempdir");
    let workspace_dir = temp.path().join("projects/project-a");
    std::fs::create_dir_all(&workspace_dir).expect("workspace");
    let handoff = handoff(temp.path());
    let address = rua_core::ChatAddress::project("project-a", CHAT_ID);
    let mut console_request = request("hello Project model");
    let model = serde_json::json!({
        "provider_id": "fixture",
        "model": "project-model",
        "thinking_strength": "high"
    });
    console_request
        .extra
        .insert("chat_model_slot".to_string(), model.clone());

    let mut params = input_params(&console_request, &workspace_dir, &address);
    params.materialize_conversation = true;
    let remote_request = handoff
        .build_remote_conversation_input_request(&params)
        .await
        .expect("remote request");

    assert_eq!(remote_request.metadata["chat_model_slot"], model);
}

#[tokio::test]
async fn pending_marker_round_trips_and_clears() {
    let temp = TempDir::new().expect("tempdir");
    let workspace_dir = temp.path().join("workspaces/default");
    std::fs::create_dir_all(&workspace_dir).expect("workspace");
    let handoff = handoff(temp.path());
    let address = rua_core::ChatAddress::agent("default", CHAT_ID);
    let console_request = request("hello");
    let remote_request = handoff
        .build_remote_conversation_input_request(&input_params(
            &console_request,
            &workspace_dir,
            &address,
        ))
        .await
        .expect("remote request");

    handoff
        .record_source_pending(&remote_request, &ack_for(&remote_request))
        .await
        .expect("record pending");
    let pending = handoff
        .read_source_pending(&address)
        .await
        .expect("read pending")
        .expect("pending marker");
    assert_eq!(
        pending.device_delivery_id,
        remote_request.device_delivery_id
    );
    assert_eq!(pending.address, address);
    assert_eq!(pending.target_device_id, "target-device");

    handoff
        .clear_source_pending(&address)
        .await
        .expect("clear pending");
    assert!(handoff
        .read_source_pending(&address)
        .await
        .expect("read cleared")
        .is_none());
}

#[tokio::test]
async fn pending_markers_isolate_equal_chat_ids_across_scopes() {
    let temp = TempDir::new().expect("tempdir");
    let agent_root = temp.path().join("workspaces/default");
    let project_root = temp.path().join("projects/project-a");
    std::fs::create_dir_all(&agent_root).expect("agent root");
    std::fs::create_dir_all(&project_root).expect("project root");
    let handoff = handoff(temp.path());
    let agent_address = rua_core::ChatAddress::agent("default", CHAT_ID);
    let project_address = rua_core::ChatAddress::project("project-a", CHAT_ID);
    let agent_request = handoff
        .build_remote_conversation_input_request(&input_params(
            &request("agent"),
            &agent_root,
            &agent_address,
        ))
        .await
        .expect("agent request");
    let project_request = handoff
        .build_remote_conversation_input_request(&input_params(
            &request("project"),
            &project_root,
            &project_address,
        ))
        .await
        .expect("project request");

    handoff
        .record_source_pending(&agent_request, &ack_for(&agent_request))
        .await
        .expect("agent pending");
    handoff
        .record_source_pending(&project_request, &ack_for(&project_request))
        .await
        .expect("project pending");

    assert_eq!(
        handoff
            .read_source_pending(&agent_address)
            .await
            .expect("agent pending")
            .expect("agent marker")
            .address,
        agent_address
    );
    assert_eq!(
        handoff
            .read_source_pending(&project_address)
            .await
            .expect("project pending")
            .expect("project marker")
            .address,
        project_address
    );
}

#[tokio::test]
async fn finalized_device_delivery_clears_pending_marker() {
    let temp = TempDir::new().expect("tempdir");
    let workspace_dir = temp.path().join("workspaces/default");
    std::fs::create_dir_all(&workspace_dir).expect("workspace");
    let handoff = handoff(temp.path());
    let address = rua_core::ChatAddress::agent("default", CHAT_ID);
    let console_request = request("hello");
    let remote_request = handoff
        .build_remote_conversation_input_request(&input_params(
            &console_request,
            &workspace_dir,
            &address,
        ))
        .await
        .expect("remote request");
    handoff
        .record_source_pending(&remote_request, &ack_for(&remote_request))
        .await
        .expect("record pending");
    write_completed_remote_turn(&workspace_dir, &remote_request.device_delivery_id).await;

    handoff
        .clear_source_pending_if_stable_locally(&workspace_dir, &address)
        .await
        .expect("finalized pending");
    assert!(handoff
        .read_source_pending(&address)
        .await
        .expect("read pending")
        .is_none());
}

#[tokio::test]
async fn terminal_seen_device_delivery_keeps_pending_marker_for_sequence_until_synced_turn() {
    let temp = TempDir::new().expect("tempdir");
    let workspace_dir = temp.path().join("workspaces/default");
    std::fs::create_dir_all(&workspace_dir).expect("workspace");
    let handoff = handoff(temp.path());
    let address = rua_core::ChatAddress::agent("default", CHAT_ID);
    let console_request = request("remote conversation inputning");
    let remote_request = handoff
        .build_remote_conversation_input_request(&input_params(
            &console_request,
            &workspace_dir,
            &address,
        ))
        .await
        .expect("remote request");

    handoff
        .record_source_pending(&remote_request, &ack_for(&remote_request))
        .await
        .expect("record pending");
    handoff
        .mark_source_pending_terminal_seen(
            &address,
            &remote_request.device_delivery_id,
            "completed",
        )
        .await
        .expect("mark terminal seen");

    assert!(!handoff
        .clear_source_pending_if_stable_locally(&workspace_dir, &address)
        .await
        .expect("terminal-seen pending should not clear without stable transcript"));

    let pending = handoff
        .read_source_pending(&address)
        .await
        .expect("read pending")
        .expect("pending marker");
    assert_eq!(
        pending.device_delivery_id,
        remote_request.device_delivery_id
    );
    assert_eq!(pending.terminal_status.as_deref(), Some("completed"));
}

#[tokio::test]
async fn queue_remote_input_does_not_block_on_previous_source_pending_marker() {
    let temp = TempDir::new().expect("tempdir");
    let workspace_dir = temp.path().join("workspaces/default");
    std::fs::create_dir_all(&workspace_dir).expect("workspace");
    let handoff = handoff(temp.path());
    let address = rua_core::ChatAddress::agent("default", CHAT_ID);
    let first_request = request("first");
    let first = handoff
        .build_remote_conversation_input_request(&input_params(
            &first_request,
            &workspace_dir,
            &address,
        ))
        .await
        .expect("first remote request");
    handoff
        .record_source_pending(&first, &ack_for(&first))
        .await
        .expect("record pending");

    let second_request = request("second");
    let outcome = handoff
        .queue_remote_input(input_params(&second_request, &workspace_dir, &address))
        .await;

    assert!(!outcome.accepted);
    assert!(outcome.device_delivery_id.is_some());
    assert_eq!(outcome.device_delivery_sequence, Some(2));
    assert!(
        outcome
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("Device Network Runtime is suspended"),
        "unexpected detail: {:?}",
        outcome.detail
    );
    let pending = handoff
        .read_source_pending(&address)
        .await
        .expect("read pending")
        .expect("pending marker");
    assert_eq!(pending.device_delivery_id, first.device_delivery_id);
}

async fn write_completed_remote_turn(workspace_dir: &std::path::Path, device_delivery_id: &str) {
    let now = "2026-07-07T00:00:00Z".to_string();
    let store = WorkspaceSessionStore::new(workspace_dir);
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: CHAT_ID.to_string(),
            agent_id: "default".to_string(),
            title: "Remote".to_string(),
            created_at: now.clone(),
            last_activity_at: now.clone(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .expect("save header");
    let mut metadata = serde_json::Map::new();
    metadata.insert(
        "device_delivery_id".to_string(),
        serde_json::Value::String(device_delivery_id.to_string()),
    );
    store
        .append_conversation_turn(
            CHAT_ID,
            NewConversationTurn {
                turn_id: "turn-remote".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: now.clone(),
                    assistant_started_at: Some(now.clone()),
                    assistant_completed_at: Some(now.clone()),
                    status: "completed".to_string(),
                },
                selected_variant_id: None,
                messages: vec![NewConversationMessage {
                    message_id: "msg-user".to_string(),
                    role: "user".to_string(),
                    created_at: now,
                    status: "completed".to_string(),
                    content: serde_json::json!([{ "type": "text", "text": "hello" }]),
                    provider: None,
                    tool: None,
                    metadata,
                }],
            },
        )
        .await
        .expect("append turn");
}
