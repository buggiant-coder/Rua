use rua_server::services::conversation_history_readiness::{
    build_conversation_history_requirement, ensure_conversation_history_ready,
    verify_conversation_history_requirement, ConversationHistoryReadinessError,
    ConversationHistoryReadinessRequest,
};
use rua_server::services::conversation_live_activity::{
    history_refresh_required_payload, history_watermark_reached_payload,
};
use rua_server::services::conversation_manifest::{
    initial_conversation_manifest_hash, load_conversation_manifest, ConversationManifest,
    ConversationManifestMismatch,
};
use rua_session::{
    ChatMetadata, ChatSessionHeader, NewConversationMessage, NewConversationTurn, TurnLifecycle,
    WorkspaceSessionStore,
};
use tempfile::TempDir;

const CHAT_ID: &str = "00000000-0000-0000-0000-000000009962";

fn address() -> rua_core::ChatAddress {
    rua_core::ChatAddress::agent("default", CHAT_ID)
}

#[test]
fn manifest_serialization_carries_typed_address_and_preserves_unknown_fields() {
    let manifest = ConversationManifest {
        address: address(),
        latest_turn: 0,
        latest_sequence: 0,
        turn_count: 0,
        message_count: 0,
        latest_turn_id: None,
        latest_message_id: None,
        latest_turn_status: None,
        extra: Default::default(),
    };
    let mut value = serde_json::to_value(manifest).expect("serialize manifest");
    assert_eq!(value["address"]["scope"]["kind"], "agent");
    assert_eq!(value["address"]["scope"]["agent_id"], "default");
    assert_eq!(value["address"]["chat_id"], CHAT_ID);
    value["future_manifest_field"] = serde_json::json!({"enabled": true});
    let decoded: ConversationManifest =
        serde_json::from_value(value).expect("deserialize future field");
    assert_eq!(
        decoded.extra["future_manifest_field"],
        serde_json::json!({"enabled": true})
    );
}

#[test]
fn agent_initial_manifest_hash_preserves_pre_cutover_value() {
    use sha2::{Digest, Sha256};

    #[derive(serde::Serialize)]
    struct LegacyManifest<'a> {
        agent_id: &'a str,
        chat_id: &'a str,
        latest_turn: u64,
        latest_sequence: u64,
        turn_count: u64,
        message_count: u64,
    }
    let legacy = LegacyManifest {
        agent_id: "default",
        chat_id: CHAT_ID,
        latest_turn: 0,
        latest_sequence: 0,
        turn_count: 0,
        message_count: 0,
    };
    let expected = format!(
        "sha256:{}",
        hex::encode(Sha256::digest(
            serde_json::to_vec(&legacy).expect("legacy manifest")
        ))
    );
    assert_eq!(
        initial_conversation_manifest_hash(&address()).expect("typed manifest hash"),
        expected
    );
}

#[test]
fn manifest_comparison_distinguishes_equal_chat_ids_across_scopes() {
    let agent = ConversationManifest {
        address: address(),
        latest_turn: 0,
        latest_sequence: 0,
        turn_count: 0,
        message_count: 0,
        latest_turn_id: None,
        latest_message_id: None,
        latest_turn_status: None,
        extra: Default::default(),
    };
    let mut project = agent.clone();
    project.address = rua_core::ChatAddress::project("project-a", CHAT_ID);
    let comparison = agent.compare(&project);
    assert!(!comparison.matches);
    assert_eq!(
        comparison.mismatches,
        vec![ConversationManifestMismatch::Address]
    );
}

#[tokio::test]
async fn manifest_matches_identical_stable_conversation_state() {
    let left = TempDir::new().expect("left");
    let right = TempDir::new().expect("right");
    seed_chat(left.path(), false).await;
    seed_chat(right.path(), false).await;

    let left_manifest = load_conversation_manifest(left.path(), &address())
        .await
        .expect("left manifest")
        .expect("left exists");
    let right_manifest = load_conversation_manifest(right.path(), &address())
        .await
        .expect("right manifest")
        .expect("right exists");

    let comparison = left_manifest.compare(&right_manifest);
    assert!(comparison.matches);
    assert!(comparison.mismatches.is_empty());
}

#[tokio::test]
async fn manifest_detects_stale_turn_state() {
    let left = TempDir::new().expect("left");
    let right = TempDir::new().expect("right");
    seed_chat(left.path(), false).await;
    seed_chat(right.path(), false).await;
    append_turn(
        &WorkspaceSessionStore::new(right.path()),
        2,
        "second user",
        "second assistant",
        false,
    )
    .await;

    let left_manifest = load_conversation_manifest(left.path(), &address())
        .await
        .expect("left manifest")
        .expect("left exists");
    let right_manifest = load_conversation_manifest(right.path(), &address())
        .await
        .expect("right manifest")
        .expect("right exists");

    let comparison = left_manifest.compare(&right_manifest);
    assert!(!comparison.matches);
    assert!(comparison
        .mismatches
        .contains(&ConversationManifestMismatch::LatestTurn));
    assert!(comparison
        .mismatches
        .contains(&ConversationManifestMismatch::LatestSequence));
    assert!(comparison
        .mismatches
        .contains(&ConversationManifestMismatch::TurnCount));
    assert!(comparison
        .mismatches
        .contains(&ConversationManifestMismatch::MessageCount));
    assert!(comparison
        .mismatches
        .contains(&ConversationManifestMismatch::LatestTurnId));
    assert!(comparison
        .mismatches
        .contains(&ConversationManifestMismatch::LatestMessageId));
}

#[tokio::test]
async fn manifest_ignores_non_conversation_header_changes() {
    let left = TempDir::new().expect("left");
    let right = TempDir::new().expect("right");
    seed_chat(left.path(), false).await;
    seed_chat(right.path(), false).await;
    let store = WorkspaceSessionStore::new(right.path());
    let mut header = store
        .load_chat_header(CHAT_ID)
        .await
        .expect("load header")
        .expect("header");
    header.metadata.title = "Different local title".to_string();
    header.metadata.last_activity_at = "2026-07-21T00:00:00Z".to_string();
    header.metadata.pinned = true;
    header.metadata.preview = Some(rua_session::ChatPreview {
        last_message_sequence: Some(2),
        text: "Different local preview".to_string(),
    });
    header.extra.insert(
        "device_activity".to_string(),
        serde_json::json!({"last_active_device_id": "device-b"}),
    );
    header.extra.insert(
        "memory_signal_state".to_string(),
        serde_json::json!({"frustration_hits": 3}),
    );
    store.save_chat_header(&header).await.expect("save header");

    let left_manifest = load_conversation_manifest(left.path(), &address())
        .await
        .expect("left manifest")
        .expect("left exists");
    let right_manifest = load_conversation_manifest(right.path(), &address())
        .await
        .expect("right manifest")
        .expect("right exists");

    assert!(left_manifest.compare(&right_manifest).matches);
    assert_eq!(
        left_manifest.hash().expect("left hash"),
        right_manifest.hash().expect("right hash"),
        "Base Manifest hash must ignore synchronized Header metadata"
    );
}

#[tokio::test]
async fn manifest_ignores_message_content_and_media_reference_changes() {
    let left = TempDir::new().expect("left");
    let right = TempDir::new().expect("right");
    seed_chat(left.path(), false).await;
    seed_chat(right.path(), true).await;

    let left_manifest = load_conversation_manifest(left.path(), &address())
        .await
        .expect("left manifest")
        .expect("left exists");
    let right_manifest = load_conversation_manifest(right.path(), &address())
        .await
        .expect("right manifest")
        .expect("right exists");

    assert!(left_manifest.compare(&right_manifest).matches);
    assert_eq!(
        left_manifest.hash().expect("left hash"),
        right_manifest.hash().expect("right hash"),
        "Base Manifest hash must represent the Turn/Message boundary, not exact content"
    );
}

#[tokio::test]
async fn history_readiness_rejects_target_ahead_of_source() {
    let data_dir = TempDir::new().expect("data");
    let workspace_dir = data_dir.path().join("workspaces/default");
    seed_chat(&workspace_dir, false).await;
    let address = address();
    let mut source_requirement = build_conversation_history_requirement(&workspace_dir, &address)
        .await
        .expect("history requirement");
    source_requirement.latest_sequence = source_requirement.latest_sequence.saturating_sub(1);

    let error =
        verify_conversation_history_requirement(data_dir.path(), &address, &source_requirement)
            .await
            .expect_err("target ahead of source must reject");

    assert!(
        error.contains("ahead of source"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn history_readiness_rejects_different_message_boundary() {
    let data_dir = TempDir::new().expect("data");
    let workspace_dir = data_dir.path().join("workspaces/default");
    seed_chat(&workspace_dir, false).await;
    let address = address();
    let mut source_requirement = build_conversation_history_requirement(&workspace_dir, &address)
        .await
        .expect("history requirement");
    source_requirement.latest_message_id = Some("different-message".to_string());

    let error =
        verify_conversation_history_requirement(data_dir.path(), &address, &source_requirement)
            .await
            .expect_err("different message boundary must reject");

    assert!(
        error.contains("LatestMessageId"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn history_readiness_rejects_missing_messages_at_same_watermark() {
    let data_dir = TempDir::new().expect("data");
    let workspace_dir = data_dir.path().join("workspaces/default");
    seed_chat(&workspace_dir, false).await;
    let address = address();
    let mut source_requirement = build_conversation_history_requirement(&workspace_dir, &address)
        .await
        .expect("history requirement");
    source_requirement.message_count += 1;

    let error =
        verify_conversation_history_requirement(data_dir.path(), &address, &source_requirement)
            .await
            .expect_err("same watermark with missing messages must reject");

    assert!(error.contains("MessageCount"), "unexpected error: {error}");
}

#[tokio::test]
async fn history_watermark_readiness_identifies_exact_published_manifest() {
    let workspace = TempDir::new().expect("workspace");
    seed_chat(workspace.path(), false).await;
    let manifest = load_conversation_manifest(workspace.path(), &address())
        .await
        .expect("load manifest")
        .expect("manifest exists");
    let payload =
        history_watermark_reached_payload(&manifest.address, "authority-device", &manifest)
            .expect("readiness payload");

    assert_eq!(payload["type"], "history_watermark_reached");
    assert_eq!(payload["agent_id"], "default");
    assert_eq!(payload["chat_id"], CHAT_ID);
    assert_eq!(payload["authority_device_id"], "authority-device");
    assert_eq!(payload["durable_sequence"], manifest.latest_sequence);
    assert_eq!(payload["manifest_hash"], manifest.hash().unwrap());
}

#[tokio::test]
async fn project_history_watermark_readiness_preserves_full_chat_address() {
    let workspace = TempDir::new().expect("workspace");
    seed_chat(workspace.path(), false).await;
    let project_address = rua_core::ChatAddress::project("project-a", CHAT_ID);
    let manifest = load_conversation_manifest(workspace.path(), &address())
        .await
        .expect("load agent fixture manifest")
        .expect("agent fixture manifest exists");
    let project_manifest = ConversationManifest {
        address: project_address.clone(),
        ..manifest
    };

    let payload =
        history_watermark_reached_payload(&project_address, "authority-device", &project_manifest)
            .expect("Project readiness payload");

    assert_eq!(payload["type"], "history_watermark_reached");
    assert_eq!(
        payload["address"],
        serde_json::to_value(&project_address).unwrap()
    );
    assert!(payload.get("agent_id").is_none());
    assert!(payload.get("chat_id").is_none());
    assert_eq!(payload["manifest_hash"], project_manifest.hash().unwrap());
}

#[test]
fn live_gap_requests_refresh_without_claiming_history_readiness() {
    let payload = history_refresh_required_payload(&address(), 4, 9);

    assert_eq!(payload["type"], "history_refresh_required");
    assert_eq!(payload["reason"], "live_activity_gap");
    assert_eq!(payload["chat_id"], CHAT_ID);
    assert_eq!(payload["requested_after_sequence"], 4);
    assert_eq!(payload["first_available_sequence"], 9);
    assert_ne!(payload["type"], "history_watermark_reached");
}

#[tokio::test]
async fn history_readiness_ignores_header_display_metadata_changes() {
    let data_dir = TempDir::new().expect("data");
    let workspace_dir = data_dir.path().join("workspaces/default");
    seed_chat(&workspace_dir, false).await;
    let address = address();
    let source_requirement = build_conversation_history_requirement(&workspace_dir, &address)
        .await
        .expect("history requirement");
    let store = WorkspaceSessionStore::new(&workspace_dir);
    let mut header = store
        .load_chat_header(CHAT_ID)
        .await
        .expect("load header")
        .expect("header");
    header.metadata.title = "Local display title".to_string();
    header.metadata.last_activity_at = "2026-07-21T00:00:00Z".to_string();
    header.metadata.preview = Some(rua_session::ChatPreview {
        last_message_sequence: Some(header.message_log.latest_sequence),
        text: "Local preview".to_string(),
    });
    header.extra.insert(
        "device_activity".to_string(),
        serde_json::json!({"last_active_device_id": "device-b"}),
    );
    store.save_chat_header(&header).await.expect("save header");

    verify_conversation_history_requirement(data_dir.path(), &address, &source_requirement)
        .await
        .expect("display metadata must not block history readiness");
}

#[tokio::test]
async fn history_readiness_resolves_project_scope_independently_from_execution_agent() {
    let data_dir = TempDir::new().expect("data");
    let project_root = data_dir.path().join("projects/project-a");
    seed_chat_with_agent(&project_root, "execution-agent").await;
    let address = rua_core::ChatAddress::project("project-a", CHAT_ID);
    let requirement = build_conversation_history_requirement(&project_root, &address)
        .await
        .expect("project history requirement");

    verify_conversation_history_requirement(data_dir.path(), &address, &requirement)
        .await
        .expect("Project Scope history must resolve independently from execution Agent");
    assert_eq!(requirement.address, address);
}

#[tokio::test]
async fn history_readiness_reports_sync_unavailable_without_changing_local_prefix() {
    let data_dir = TempDir::new().expect("data");
    let workspace_dir = data_dir.path().join("workspaces/default");
    seed_chat(&workspace_dir, false).await;
    let address = address();
    let before = load_conversation_manifest(&workspace_dir, &address)
        .await
        .expect("load local manifest")
        .expect("local manifest");
    let mut requirement = before.clone();
    requirement.message_count += 1;

    let error = ensure_conversation_history_ready(ConversationHistoryReadinessRequest {
        data_dir: data_dir.path(),
        address: &address,
        source_device_id: "authority-device",
        requirement: &requirement,
        sync_tx: None,
    })
    .await
    .expect_err("missing Sync runtime must fail readiness");

    assert_eq!(error, ConversationHistoryReadinessError::SyncUnavailable);
    let after = load_conversation_manifest(&workspace_dir, &address)
        .await
        .expect("reload local manifest")
        .expect("local manifest remains");
    assert_eq!(
        after, before,
        "failed readiness must preserve local History"
    );
}

async fn seed_chat(root: &std::path::Path, include_media: bool) {
    seed_chat_with_agent_and_media(root, "default", include_media).await;
}

async fn seed_chat_with_agent(root: &std::path::Path, agent_id: &str) {
    seed_chat_with_agent_and_media(root, agent_id, false).await;
}

async fn seed_chat_with_agent_and_media(
    root: &std::path::Path,
    agent_id: &str,
    include_media: bool,
) {
    let store = WorkspaceSessionStore::new(root);
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: CHAT_ID.to_string(),
            agent_id: agent_id.to_string(),
            title: "Manifest".to_string(),
            created_at: "2026-07-20T00:00:00Z".to_string(),
            last_activity_at: "2026-07-20T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .expect("save header");
    append_turn(&store, 1, "hello", "hi", include_media).await;
}

async fn append_turn(
    store: &WorkspaceSessionStore,
    ordinal: u64,
    user_text: &str,
    assistant_text: &str,
    include_media: bool,
) {
    store
        .append_conversation_turn(
            CHAT_ID,
            NewConversationTurn {
                turn_id: format!("turn-{ordinal}"),
                lifecycle: TurnLifecycle {
                    user_submitted_at: format!("2026-07-20T00:00:0{ordinal}Z"),
                    assistant_started_at: Some(format!("2026-07-20T00:00:1{ordinal}Z")),
                    assistant_completed_at: Some(format!("2026-07-20T00:00:2{ordinal}Z")),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some(format!("variant-{ordinal}")),
                messages: vec![
                    NewConversationMessage {
                        message_id: format!("msg-user-{ordinal}"),
                        role: "user".to_string(),
                        created_at: format!("2026-07-20T00:00:0{ordinal}Z"),
                        status: "completed".to_string(),
                        content: user_content(user_text, include_media),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                    NewConversationMessage {
                        message_id: format!("msg-assistant-{ordinal}"),
                        role: "assistant".to_string(),
                        created_at: format!("2026-07-20T00:00:2{ordinal}Z"),
                        status: "completed".to_string(),
                        content: serde_json::json!([{ "type": "text", "text": assistant_text }]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                ],
            },
        )
        .await
        .expect("append turn");
}

fn user_content(text: &str, include_media: bool) -> serde_json::Value {
    let mut content = vec![serde_json::json!({ "type": "text", "text": text })];
    if include_media {
        content.push(serde_json::json!({
            "type": "file",
            "url": "/tmp/rua-media/report.pdf",
            "stored_name": "report.pdf"
        }));
    }
    serde_json::Value::Array(content)
}
