use rua_channel::core::console::ConsoleChatRequest;
use rua_core::ChatAddress;
use rua_server::services::chat_index::{self, ChatIndexEntry};
use rua_server::services::conversation_authority_placement::{
    parse_conversation_authority_placement, ConversationAuthorityEpoch,
    CHAT_AUTHORITY_PLACEMENT_META_KEY,
};
use rua_server::services::conversation_authority_router::{
    ConversationAuthorityRouter, ConversationLiveActivityRoute,
    ConversationLiveActivityRouteRequest,
};
use rua_server::services::conversation_authority_target::{
    load_optional_chat_authority_placement, resolve_acp_route, resolve_submit_target,
    route_for_local_device, PlacementTarget, PlacementTargetSource, RemotePlacementRoute,
};
use rua_server::services::conversation_history_readiness::required_media_set;
use rua_session::{ChatMetadata, ChatSessionHeader, WorkspaceSessionStore};
use serde_json::json;
use tempfile::TempDir;

const CHAT_ID: &str = "00000000-0000-0000-0000-000000009901";
const MANIFEST_HASH: &str =
    "sha256:1111111111111111111111111111111111111111111111111111111111111111";

fn epoch(wall_ms: u64) -> ConversationAuthorityEpoch {
    ConversationAuthorityEpoch::from_parts(wall_ms, 0, [0; 8])
}

fn request() -> ConsoleChatRequest {
    ConsoleChatRequest {
        message: "hello".to_string(),
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

async fn write_chat_placement(
    workspace_dir: &std::path::Path,
    device_id: &str,
    authority_epoch: ConversationAuthorityEpoch,
) {
    let now = chrono::Utc::now();
    let placement = json!({
        "device_id": device_id,
        "authority_epoch": authority_epoch,
        "base_manifest_hash": MANIFEST_HASH,
    });
    chat_index::upsert(
        workspace_dir,
        ChatIndexEntry {
            id: CHAT_ID.to_string(),
            name: "Targeted".to_string(),
            user_id: "default".to_string(),
            channel: "console".to_string(),
            created_at: now,
            updated_at: now,
            meta: json!({
                CHAT_AUTHORITY_PLACEMENT_META_KEY: placement,
            }),
            status: "idle".to_string(),
            pinned: false,
            pin_order: 0,
            tag: rua_session::ChatTag::None,
            archived: false,
        },
    )
    .await
    .expect("write chat index");

    let mut header = ChatSessionHeader::new(ChatMetadata {
        chat_id: CHAT_ID.to_string(),
        agent_id: "default".to_string(),
        title: "Targeted".to_string(),
        created_at: now.to_rfc3339(),
        last_activity_at: now.to_rfc3339(),
        pinned: false,
        archived: false,
        preview: None,
    });
    header
        .extra
        .insert(CHAT_AUTHORITY_PLACEMENT_META_KEY.to_string(), placement);
    WorkspaceSessionStore::new(workspace_dir)
        .save_chat_header(&header)
        .await
        .expect("write Chat Session Header");
}

#[tokio::test]
async fn submit_uses_only_backend_authority_placement() {
    let tmp = TempDir::new().expect("tempdir");
    let workspace = tmp.path().join("workspace");
    write_chat_placement(&workspace, "device-authority", epoch(4)).await;
    let mut request = request();
    request.extra.insert(
        CHAT_AUTHORITY_PLACEMENT_META_KEY.to_string(),
        json!({
            "device_id": "device-forged",
            "authority_epoch": "0000000000000063000000000000000000000000",
            "base_manifest_hash": MANIFEST_HASH,
        }),
    );

    let target = resolve_submit_target(
        Some(&workspace),
        CHAT_ID,
        &request,
        "default",
        Some("device-local"),
        None,
    )
    .await
    .expect("resolve target")
    .expect("placement");

    assert_eq!(
        target,
        PlacementTarget {
            device_id: "device-authority".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(4, 0, [0; 8]),
            base_manifest_hash: MANIFEST_HASH.to_string(),
            source: PlacementTargetSource::ConversationAuthorityPlacement,
        }
    );
}

#[tokio::test]
async fn submit_rejects_media_uploaded_under_a_stale_authority_epoch() {
    let tmp = TempDir::new().expect("tempdir");
    let workspace = tmp.path().join("workspace");
    write_chat_placement(&workspace, "device-authority", epoch(5)).await;
    let mut request = request();
    request.extra.insert(
        "expected_authority_epoch".to_string(),
        serde_json::Value::String(epoch(4).to_string()),
    );

    let error = resolve_submit_target(
        Some(&workspace),
        CHAT_ID,
        &request,
        "default",
        Some("device-local"),
        None,
    )
    .await
    .expect_err("stale Media epoch must fail before routing");

    assert!(error.contains("epoch changed"), "unexpected error: {error}");
}

#[tokio::test]
async fn only_the_authority_requires_referenced_media_to_exist_locally() {
    let tmp = TempDir::new().expect("tempdir");
    let workspace = tmp.path().join("workspace");
    let current_epoch = epoch(6);
    write_chat_placement(&workspace, "device-authority", current_epoch).await;
    let mut request = request();
    request
        .attachments
        .push(rua_channel::core::console::AttachmentInfo {
            url: "/api/media".to_string(),
            name: "photo.png".to_string(),
            mime_type: Some("image/png".to_string()),
            stored_name: Some("00000000-0000-4000-8000-000000000006".to_string()),
            size: Some(12),
        });
    request.extra.insert(
        "expected_authority_epoch".to_string(),
        serde_json::Value::String(current_epoch.to_string()),
    );

    let remote_target = resolve_submit_target(
        Some(&workspace),
        CHAT_ID,
        &request,
        "default",
        Some("device-controller"),
        None,
    )
    .await
    .expect("Controller routes without requiring a local replica")
    .expect("placement target");
    assert_eq!(remote_target.device_id, "device-authority");

    let error = resolve_submit_target(
        Some(&workspace),
        CHAT_ID,
        &request,
        "default",
        Some("device-authority"),
        None,
    )
    .await
    .expect_err("Authority must reject missing required Media");
    assert!(error.contains("required Chat Media is unavailable"));
}

#[test]
fn required_media_set_is_canonical_deduplicated_and_sorted() {
    let tmp = TempDir::new().expect("tempdir");
    let workspace = tmp.path().join("workspace");
    let address = ChatAddress::agent("default", CHAT_ID);
    let chat_dir = workspace.join("sessions").join(CHAT_ID);
    std::fs::create_dir_all(&chat_dir).expect("chat dir");
    std::fs::write(
        chat_dir.join("events.json"),
        serde_json::to_vec(&serde_json::json!({
            "content": [
                {"media_ref": "media/00000000-0000-4000-8000-000000000002"},
                {"media_ref": "media/00000000-0000-4000-8000-000000000001"},
                {"media_ref": "media/00000000-0000-4000-8000-000000000002"},
                {"text": "media/not-a-reference"}
            ]
        }))
        .expect("json"),
    )
    .expect("write stable state");

    let required = required_media_set(&workspace, &address).expect("required Media Set");
    assert_eq!(
        required.iter().map(ToString::to_string).collect::<Vec<_>>(),
        vec![
            "00000000-0000-4000-8000-000000000001",
            "00000000-0000-4000-8000-000000000002"
        ]
    );

    std::fs::write(
        chat_dir.join("malformed.json"),
        br#"{"media_ref":"media/not-a-uuid"}"#,
    )
    .expect("write malformed reference");
    let error = required_media_set(&workspace, &address)
        .expect_err("malformed Media Reference must fail readiness");
    assert!(error.contains("invalid Media Reference"));
}

#[test]
fn required_media_set_reads_canonical_message_log_segments() {
    let tmp = TempDir::new().unwrap();
    let address = ChatAddress::project("project-media", CHAT_ID);
    let messages = tmp.path().join("sessions").join(CHAT_ID).join("messages");
    std::fs::create_dir_all(&messages).unwrap();
    let entry = serde_json::json!({
        "role": "assistant",
        "content": [{"type": "text", "text": "![Preview](media/00000000-0000-4000-8000-000000000002)"}],
        "metadata": {"display_media": [{
            "media_ref": "media/00000000-0000-4000-8000-000000000001",
            "origin": {"device_id": "desktop", "src": "/tmp/preview.png"}
        }]}
    });
    std::fs::write(
        messages.join("000000000000.jsonl"),
        format!("{{\"header\":true}}\n{entry}\n"),
    )
    .unwrap();
    assert_eq!(
        required_media_set(tmp.path(), &address)
            .unwrap()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        vec![
            "00000000-0000-4000-8000-000000000001",
            "00000000-0000-4000-8000-000000000002"
        ]
    );
}

#[tokio::test]
async fn draft_target_preparation_does_not_install_authority_placement() {
    let tmp = TempDir::new().expect("tempdir");
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");

    let target = resolve_submit_target(
        Some(&workspace),
        CHAT_ID,
        &request(),
        "default",
        Some("device-local"),
        None,
    )
    .await
    .expect("valid first input has a draft target")
    .unwrap();
    assert_eq!(target.source, PlacementTargetSource::DraftTarget);
    assert!(load_optional_chat_authority_placement(&workspace, CHAT_ID)
        .await
        .unwrap()
        .is_none());
    let mut existing_request = request();
    existing_request.extra.insert(
        "expected_authority_epoch".into(),
        serde_json::json!(epoch(1).to_string()),
    );
    assert!(resolve_submit_target(
        Some(&workspace),
        CHAT_ID,
        &existing_request,
        "default",
        Some("device-local"),
        None
    )
    .await
    .is_err());
}

#[tokio::test]
async fn index_only_authority_placement_is_not_loaded() {
    let tmp = TempDir::new().expect("tempdir");
    let workspace = tmp.path().join("workspace");
    let now = chrono::Utc::now();
    chat_index::upsert(
        &workspace,
        ChatIndexEntry {
            id: CHAT_ID.to_string(),
            name: "Legacy Index-only Chat".to_string(),
            user_id: "default".to_string(),
            channel: "console".to_string(),
            created_at: now,
            updated_at: now,
            meta: json!({
                CHAT_AUTHORITY_PLACEMENT_META_KEY: {
                    "device_id": "device-index",
                    "authority_epoch": epoch(5),
                    "base_manifest_hash": MANIFEST_HASH,
                }
            }),
            status: "idle".to_string(),
            pinned: false,
            pin_order: 0,
            tag: rua_session::ChatTag::None,
            archived: false,
        },
    )
    .await
    .expect("write legacy Chat Index row");

    assert!(
        load_optional_chat_authority_placement(&workspace, CHAT_ID)
            .await
            .expect("load authority placement")
            .is_none(),
        "Index-only legacy data must not be treated as authoritative"
    );
}

#[tokio::test]
async fn acp_route_cannot_override_conversation_authority() {
    let tmp = TempDir::new().expect("tempdir");
    let workspace = tmp.path().join("workspace");
    write_chat_placement(&workspace, "device-authority", epoch(2)).await;

    let error = resolve_acp_route(
        &workspace,
        CHAT_ID,
        Some("device-other"),
        Some("device-local"),
    )
    .await
    .expect_err("ACP override must fail");

    assert!(error.contains("not the conversation authority"));
}

#[tokio::test]
async fn live_activity_route_carries_explicit_authority_epoch() {
    let tmp = TempDir::new().expect("tempdir");
    let workspace = tmp.path().join("workspace");
    write_chat_placement(&workspace, "device-authority", epoch(7)).await;

    let route = ConversationAuthorityRouter
        .resolve_live_activity_route(ConversationLiveActivityRouteRequest {
            workspace_dir: Some(&workspace),
            address: &rua_core::ChatAddress::agent("default", CHAT_ID),
            local_device_id: Some("device-local"),
        })
        .await;

    assert_eq!(
        route,
        ConversationLiveActivityRoute::RemoteAuthority {
            source_device_id: "device-local".to_string(),
            target_device_id: "device-authority".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(7, 0, [0; 8]),
        }
    );
}

#[test]
fn authority_epoch_is_sortable_hlc_and_rejects_numeric_wire_values() {
    let first = epoch(10);
    let second = ConversationAuthorityEpoch::next_for_device("device-b", Some(first));
    assert!(second > first);

    let encoded = serde_json::to_value(second).expect("serialize authority Epoch");
    assert_eq!(encoded.as_str().map(str::len), Some(40));
    assert!(serde_json::from_value::<ConversationAuthorityEpoch>(json!(1)).is_err());
}

#[test]
fn authority_epoch_advances_after_future_observation_and_breaks_device_ties() {
    let observed = ConversationAuthorityEpoch::from_parts(u64::MAX - 1, 7, [1; 8]);
    let next = ConversationAuthorityEpoch::next_for_device("device-b", Some(observed));
    assert!(next > observed);

    let left = ConversationAuthorityEpoch::from_parts(100, 0, [1; 8]);
    let right = ConversationAuthorityEpoch::from_parts(100, 0, [2; 8]);
    assert!(right > left);
}

#[tokio::test]
async fn session_header_placement_is_authoritative_over_chat_index() {
    let tmp = TempDir::new().expect("tempdir");
    let workspace = tmp.path().join("workspace");
    write_chat_placement(&workspace, "device-index", epoch(5)).await;

    let mut header = ChatSessionHeader::new(ChatMetadata {
        chat_id: CHAT_ID.to_string(),
        agent_id: "default".to_string(),
        title: "Header".to_string(),
        created_at: "2026-07-31T00:00:00Z".to_string(),
        last_activity_at: "2026-07-31T00:00:00Z".to_string(),
        pinned: false,
        archived: false,
        preview: None,
    });
    header.extra.insert(
        CHAT_AUTHORITY_PLACEMENT_META_KEY.to_string(),
        json!({
            "device_id": "device-header",
            "authority_epoch": epoch(99),
            "base_manifest_hash": MANIFEST_HASH,
        }),
    );
    WorkspaceSessionStore::new(&workspace)
        .save_chat_header(&header)
        .await
        .expect("save conflicting header");

    let target = resolve_submit_target(
        Some(&workspace),
        CHAT_ID,
        &request(),
        "default",
        Some("device-local"),
        None,
    )
    .await
    .expect("resolve target")
    .expect("placement");
    assert_eq!(target.device_id, "device-header");
    assert_eq!(target.authority_epoch, epoch(99));

    let index = chat_index::get(&workspace, CHAT_ID)
        .await
        .expect("load index")
        .expect("index row");
    assert_eq!(
        parse_conversation_authority_placement(&index.meta)
            .expect("parse index placement")
            .device_id,
        "device-index"
    );
}

#[test]
fn local_route_retains_authority_identity() {
    let route = route_for_local_device(
        PlacementTarget {
            device_id: "device-local".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(3, 0, [0; 8]),
            base_manifest_hash: MANIFEST_HASH.to_string(),
            source: PlacementTargetSource::ConversationAuthorityPlacement,
        },
        Some("device-local"),
    );

    assert_eq!(
        route,
        RemotePlacementRoute::Local {
            authority_device_id: "device-local".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(3, 0, [0; 8]),
        }
    );
}
