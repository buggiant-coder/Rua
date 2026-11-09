use rua_server::services::chat_index::{self, ChatIndexEntry};
use rua_server::services::console_chat::{
    append_remote_submitted_user_turn, materialize_targeted_chat,
};
use rua_server::services::conversation_authority_placement::{
    ConversationAuthorityEpoch, ConversationAuthorityPlacement, CHAT_AUTHORITY_PLACEMENT_META_KEY,
};
use rua_server::services::device_command_bus::RemoteConversationInputRequest;
use rua_session::{ChatMetadata, ChatSessionHeader, WorkspaceSessionStore};
use serde_json::json;
use tempfile::TempDir;

const CHAT_ID: &str = "018f8f75-4eec-7b93-9a65-5e75f8217001";
const TARGET_DEVICE: &str = "0102030405060708";
const MANIFEST_HASH: &str =
    "sha256:1111111111111111111111111111111111111111111111111111111111111111";

fn epoch(wall_ms: u64) -> ConversationAuthorityEpoch {
    ConversationAuthorityEpoch::from_parts(wall_ms, 0, [0; 8])
}

fn placement(
    device_id: &str,
    authority_epoch: ConversationAuthorityEpoch,
) -> ConversationAuthorityPlacement {
    ConversationAuthorityPlacement {
        device_id: device_id.to_string(),
        authority_epoch,
        base_manifest_hash: MANIFEST_HASH.to_string(),
    }
}

#[tokio::test]
async fn target_materializes_session_authority_placement() {
    let tmp = TempDir::new().expect("tempdir");
    let workspace = tmp.path().join("workspaces/default");

    let chat = materialize_targeted_chat(
        &workspace,
        "default",
        CHAT_ID,
        "Handle Mac files",
        &placement(TARGET_DEVICE, epoch(1)),
    )
    .await
    .expect("materialize targeted chat");

    assert_eq!(chat["id"], CHAT_ID);
    assert!(chat.get("session_id").is_none());
    assert_eq!(chat["name"], "Handle Mac files");
    assert_eq!(
        chat["meta"][CHAT_AUTHORITY_PLACEMENT_META_KEY],
        json!({
            "device_id": TARGET_DEVICE,
            "authority_epoch": "0000000000000001000000000000000000000000",
            "base_manifest_hash": MANIFEST_HASH,
        })
    );

    let indexed = chat_index::get(&workspace, CHAT_ID)
        .await
        .expect("read index")
        .expect("indexed chat");
    assert_eq!(
        indexed.meta[CHAT_AUTHORITY_PLACEMENT_META_KEY],
        json!({
            "device_id": TARGET_DEVICE,
            "authority_epoch": "0000000000000001000000000000000000000000",
            "base_manifest_hash": MANIFEST_HASH,
        })
    );

    let header = WorkspaceSessionStore::new(&workspace)
        .load_chat_header(CHAT_ID)
        .await
        .expect("load header")
        .expect("header exists");
    assert_eq!(header.chat_id, CHAT_ID);
    assert_eq!(header.metadata.title, "Handle Mac files");
    assert_eq!(
        header.extra[CHAT_AUTHORITY_PLACEMENT_META_KEY],
        json!({
            "device_id": TARGET_DEVICE,
            "authority_epoch": "0000000000000001000000000000000000000000",
            "base_manifest_hash": MANIFEST_HASH,
        })
    );

    let index = rua_server::services::chat_index::get(&workspace, CHAT_ID)
        .await
        .expect("read index")
        .expect("indexed chat");
    assert_eq!(
        index.meta[CHAT_AUTHORITY_PLACEMENT_META_KEY],
        json!({
            "device_id": TARGET_DEVICE,
            "authority_epoch": "0000000000000001000000000000000000000000",
            "base_manifest_hash": MANIFEST_HASH,
        })
    );
}

#[tokio::test]
async fn target_materialization_is_idempotent_for_same_target() {
    let tmp = TempDir::new().expect("tempdir");
    let workspace = tmp.path().join("workspaces/default");

    materialize_targeted_chat(
        &workspace,
        "default",
        CHAT_ID,
        "Handle Mac files",
        &placement(TARGET_DEVICE, epoch(1)),
    )
    .await
    .expect("first materialize");
    materialize_targeted_chat(
        &workspace,
        "default",
        CHAT_ID,
        "Handle Mac files",
        &placement(TARGET_DEVICE, epoch(1)),
    )
    .await
    .expect("second materialize");

    assert_eq!(
        chat_index::list(&workspace, chat_index::ChatIndexFilter::default(), 0, 10)
            .await
            .expect("list chat index")
            .entries
            .len(),
        1,
        "duplicate request must not duplicate official Chat metadata"
    );
}

#[tokio::test]
async fn target_materialization_preserves_header_content_and_current_placement() {
    let tmp = TempDir::new().expect("tempdir");
    let workspace = tmp.path().join("workspaces/default");
    let store = WorkspaceSessionStore::new(&workspace);
    let placement = placement(TARGET_DEVICE, epoch(2));
    let mut header = ChatSessionHeader::new(ChatMetadata {
        chat_id: CHAT_ID.to_string(),
        agent_id: "default".to_string(),
        title: "Title written on device A".to_string(),
        created_at: "2026-07-19T01:02:03Z".to_string(),
        last_activity_at: "2026-07-20T04:05:06Z".to_string(),
        pinned: true,
        archived: false,
        preview: None,
    });
    header.extra.insert(
        CHAT_AUTHORITY_PLACEMENT_META_KEY.to_string(),
        serde_json::to_value(&placement).expect("placement json"),
    );
    header.extra.insert(
        "model".to_string(),
        json!({"provider_id": "dashscope", "model": "qwen-plus"}),
    );
    header
        .extra
        .insert("authority_note".to_string(), json!("written-on-device-a"));
    store.save_chat_header(&header).await.expect("seed header");
    materialize_targeted_chat(
        &workspace,
        "default",
        CHAT_ID,
        "Message submitted from device C",
        &placement,
    )
    .await
    .expect("materialize synchronized chat");

    let after = store
        .load_chat_header(CHAT_ID)
        .await
        .expect("read resulting header")
        .expect("header");
    assert_eq!(after.metadata, header.metadata);
    assert_eq!(after.extra.get("model"), header.extra.get("model"));
    assert_eq!(
        after.extra.get("authority_note"),
        header.extra.get("authority_note")
    );
    assert_eq!(
        after.extra.get(CHAT_AUTHORITY_PLACEMENT_META_KEY),
        header.extra.get(CHAT_AUTHORITY_PLACEMENT_META_KEY)
    );
    let indexed = chat_index::get(&workspace, CHAT_ID)
        .await
        .expect("read index")
        .expect("indexed chat");
    assert_eq!(indexed.name, "Title written on device A");
    assert_eq!(indexed.updated_at.to_rfc3339(), "2026-07-20T04:05:06+00:00");
}

#[tokio::test]
async fn target_materialization_rejects_existing_chat_without_placement() {
    let tmp = TempDir::new().expect("tempdir");
    let workspace = tmp.path().join("workspaces/default");
    let now = chrono::Utc::now();
    chat_index::upsert(
        &workspace,
        ChatIndexEntry {
            id: CHAT_ID.to_string(),
            name: "Existing local chat".to_string(),
            user_id: "default".to_string(),
            channel: "console".to_string(),
            created_at: now,
            updated_at: now,
            meta: json!({}),
            status: "idle".to_string(),
            pinned: false,
            pin_order: 0,
            tag: rua_session::ChatTag::None,
            archived: false,
        },
    )
    .await
    .expect("seed chat index");

    let error = materialize_targeted_chat(
        &workspace,
        "default",
        CHAT_ID,
        "Handle Mac files",
        &placement(TARGET_DEVICE, epoch(1)),
    )
    .await
    .expect_err("Index-only legacy Chat must be rejected");

    assert!(error.to_string().contains("Session File"));
}

#[tokio::test]
async fn target_materialization_rejects_changing_existing_target() {
    let tmp = TempDir::new().expect("tempdir");
    let workspace = tmp.path().join("workspaces/default");

    materialize_targeted_chat(
        &workspace,
        "default",
        CHAT_ID,
        "Handle Mac files",
        &placement(TARGET_DEVICE, epoch(1)),
    )
    .await
    .expect("first materialize");
    let error = materialize_targeted_chat(
        &workspace,
        "default",
        CHAT_ID,
        "Handle Mac files",
        &placement("1112131415161718", epoch(2)),
    )
    .await
    .expect_err("target materialization cannot switch authority");
    assert!(error.to_string().contains("chat authority is Device"));
}

#[tokio::test]
async fn device_delivery_user_turn_carries_matching_metadata() {
    let tmp = TempDir::new().expect("tempdir");
    let workspace = tmp.path().join("workspaces/default");
    let request = remote_request();

    let appended = append_remote_submitted_user_turn(&workspace, &request)
        .await
        .expect("append remote user turn");
    let turn_id = appended.turn_id;

    let store = WorkspaceSessionStore::new(&workspace);
    let turns = store
        .read_turn_range(CHAT_ID, 1, usize::MAX)
        .await
        .expect("read turns");
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].turn_id, turn_id);
    assert_eq!(turns[0].lifecycle.status, "in_progress");

    let messages = store
        .read_turn_messages(CHAT_ID, &turn_id)
        .await
        .expect("read turn messages");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, "user");
    assert_eq!(appended.user_message["id"], messages[0].message_id);
    assert_eq!(appended.user_message["turn_id"], turn_id);
    assert_eq!(appended.user_message["role"], "user");
    assert_eq!(
        messages[0].metadata["device_delivery_id"],
        request.device_delivery_id
    );
    assert_eq!(
        messages[0].metadata["device_delivery_sequence"],
        request.device_delivery_sequence
    );
    assert_eq!(messages[0].metadata["source_device_id"], "source-device");
    assert_eq!(messages[0].metadata["executed_on_device_id"], TARGET_DEVICE);
}

fn remote_request() -> RemoteConversationInputRequest {
    RemoteConversationInputRequest {
        address: rua_core::ChatAddress::agent("default".to_string(), CHAT_ID.to_string()),
        agent_id: "default".to_string(),
        source_device_id: "source-device".to_string(),
        target_device_id: TARGET_DEVICE.to_string(),
        authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
        base_manifest_hash:
            "sha256:1111111111111111111111111111111111111111111111111111111111111111".to_string(),
        submission_id: "submission-1".to_string(),
        device_delivery_id: "device-delivery-1".to_string(),
        device_delivery_sequence: 1,
        text: "Handle Mac files".to_string(),
        input: None,
        media_requirements: Vec::new(),
        created_at: "2026-06-27T00:00:00Z".to_string(),
        delivery_deadline_at: "2026-06-27T00:00:30Z".to_string(),
        metadata: serde_json::Map::new(),
    }
}
