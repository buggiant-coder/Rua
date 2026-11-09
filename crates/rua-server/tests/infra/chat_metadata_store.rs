use rua_server::services::chat_index;
use rua_server::services::chat_metadata_store::{
    apply_confirmed_authority_placement, clear_acp_binding, persist_acp_binding,
    update_authority_placement, AcpBindingMetadata,
};
use rua_server::services::conversation_authority_placement::ConversationAuthorityPlacement;
use rua_server::services::conversation_authority_placement::{
    CHAT_ACP_BINDING_META_KEY, CHAT_AUTHORITY_PLACEMENT_META_KEY,
};
use rua_session::{ChatMetadata, ChatSessionHeader, WorkspaceSessionStore};
use serde_json::json;
use tempfile::TempDir;

const CHAT_ID: &str = "00000000-0000-0000-0000-000000009951";

fn address() -> rua_core::ChatAddress {
    rua_core::ChatAddress::agent("default", CHAT_ID)
}

#[tokio::test]
async fn authority_placement_update_materializes_and_writes_only_session_file() {
    let tmp = TempDir::new().expect("tempdir");
    let workspace = tmp.path().join("workspace");

    let record = update_authority_placement(
        None,
        &workspace,
        &address(),
        ConversationAuthorityPlacement {
            device_id: "device-a".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            base_manifest_hash: format!("sha256:{}", "1".repeat(64)),
        },
    )
    .await
    .expect("update target");

    assert_eq!(
        record.meta[CHAT_AUTHORITY_PLACEMENT_META_KEY],
        json!({
            "device_id": "device-a",
            "authority_epoch": "0000000000000001000000000000000000000000",
            "base_manifest_hash": format!("sha256:{}", "1".repeat(64))
        })
    );

    assert!(
        chat_index::get(&workspace, CHAT_ID)
            .await
            .expect("read chat index")
            .is_none(),
        "Authority-only materialization must not create a Chat Index row"
    );

    let header = rua_session::WorkspaceSessionStore::new(&workspace)
        .load_chat_header(CHAT_ID)
        .await
        .expect("load header")
        .expect("materialized Session File");
    assert_eq!(
        header.extra[CHAT_AUTHORITY_PLACEMENT_META_KEY],
        record.meta[CHAT_AUTHORITY_PLACEMENT_META_KEY]
    );
}

#[tokio::test]
async fn authority_placement_update_rejects_index_only_legacy_chat() {
    let tmp = TempDir::new().expect("tempdir");
    let workspace = tmp.path().join("workspace");
    let now = chrono::Utc::now();
    chat_index::upsert(
        &workspace,
        chat_index::ChatIndexEntry {
            id: CHAT_ID.to_string(),
            name: "Index-only legacy Chat".to_string(),
            user_id: "default".to_string(),
            channel: "console".to_string(),
            created_at: now,
            updated_at: now,
            meta: json!({
                CHAT_AUTHORITY_PLACEMENT_META_KEY: {
                    "device_id": "legacy-device",
                    "authority_epoch": "0000000000000001000000000000000000000000",
                    "base_manifest_hash": format!("sha256:{}", "1".repeat(64))
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
    .expect("seed Index-only Chat");

    let error = update_authority_placement(
        None,
        &workspace,
        &address(),
        ConversationAuthorityPlacement {
            device_id: "device-new".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(2, 0, [0; 8]),
            base_manifest_hash: format!("sha256:{}", "2".repeat(64)),
        },
    )
    .await
    .expect_err("Index-only legacy Chat must be rejected");
    assert!(error.to_string().contains("Session File"));
    assert!(
        WorkspaceSessionStore::new(&workspace)
            .load_chat_header(CHAT_ID)
            .await
            .expect("load Session File")
            .is_none(),
        "rejected Index-only data must not be migrated implicitly"
    );
}

#[tokio::test]
async fn authority_placement_update_preserves_authority_owned_chat_metadata() {
    let tmp = TempDir::new().expect("tempdir");
    let workspace = tmp.path().join("workspace");
    let created_at = chrono::DateTime::parse_from_rfc3339("2026-07-19T01:02:03Z")
        .expect("created at")
        .with_timezone(&chrono::Utc);
    let updated_at = chrono::DateTime::parse_from_rfc3339("2026-07-20T04:05:06Z")
        .expect("updated at")
        .with_timezone(&chrono::Utc);
    chat_index::upsert(
        &workspace,
        chat_index::ChatIndexEntry {
            id: CHAT_ID.to_string(),
            name: "Authority title".to_string(),
            user_id: "default".to_string(),
            channel: "console".to_string(),
            created_at,
            updated_at,
            meta: json!({"authority_note": "written-on-device-a"}),
            status: "idle".to_string(),
            pinned: true,
            pin_order: 7,
            tag: rua_session::ChatTag::None,
            archived: false,
        },
    )
    .await
    .expect("seed index");
    let mut header = ChatSessionHeader::new(ChatMetadata {
        chat_id: CHAT_ID.to_string(),
        agent_id: "default".to_string(),
        title: "Authority title".to_string(),
        created_at: "2026-07-19T01:02:03Z".to_string(),
        last_activity_at: "2026-07-20T04:05:06Z".to_string(),
        pinned: true,
        archived: false,
        preview: None,
    });
    header.extra.insert(
        "model".to_string(),
        json!({"provider_id": "dashscope", "model": "qwen-plus"}),
    );
    header
        .extra
        .insert("authority_note".to_string(), json!("written-on-device-a"));
    header.extra.insert("collaboration".into(), json!({"source":{"scope":{"kind":"agent","agent_id":"default"},"chat_id":"source"},"executor":{"kind":"agent","agent_id":"reviewer"}}));
    let store = WorkspaceSessionStore::new(&workspace);
    store.save_chat_header(&header).await.expect("seed header");

    let record = update_authority_placement(
        None,
        &workspace,
        &address(),
        ConversationAuthorityPlacement {
            device_id: "device-b".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(2, 0, [0; 8]),
            base_manifest_hash: format!("sha256:{}", "3".repeat(64)),
        },
    )
    .await
    .expect("switch authority");

    assert_eq!(record.updated_at, updated_at);
    let indexed = chat_index::get(&workspace, CHAT_ID)
        .await
        .expect("read index")
        .expect("indexed chat");
    assert_eq!(indexed.name, "Authority title");
    assert_eq!(indexed.created_at, created_at);
    assert_eq!(indexed.updated_at, updated_at);
    assert!(indexed.pinned);
    assert_eq!(indexed.pin_order, 7);
    assert_eq!(indexed.meta["collaboration"], header.extra["collaboration"]);

    let after = store
        .load_chat_header(CHAT_ID)
        .await
        .expect("load header")
        .expect("header");
    assert_eq!(after.metadata, header.metadata);
    assert_eq!(after.extra.get("model"), header.extra.get("model"));
    assert_eq!(
        after.extra.get("authority_note"),
        header.extra.get("authority_note")
    );
    assert_eq!(
        after.extra[CHAT_AUTHORITY_PLACEMENT_META_KEY]["device_id"],
        "device-b"
    );
}

#[tokio::test]
async fn confirmed_authority_replacement_bypasses_stale_acp_lock_without_sync_publication() {
    let tmp = TempDir::new().expect("tempdir");
    let workspace = tmp.path().join("workspace");
    let initial = ConversationAuthorityPlacement {
        device_id: "device-old".to_string(),
        authority_epoch:
            rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(
                1,
                0,
                [0; 8],
            ),
        base_manifest_hash: format!("sha256:{}", "4".repeat(64)),
    };
    let replacement = ConversationAuthorityPlacement {
        device_id: "device-new".to_string(),
        authority_epoch:
            rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(
                2,
                0,
                [0; 8],
            ),
        base_manifest_hash: format!("sha256:{}", "5".repeat(64)),
    };

    update_authority_placement(None, &workspace, &address(), initial)
        .await
        .expect("seed placement");
    persist_acp_binding(
        None,
        &workspace,
        &address(),
        "default",
        AcpBindingMetadata {
            target_device_id: Some("device-old".to_string()),
            runner: "codex".to_string(),
            cwd: Some("/tmp".to_string()),
            runner_session_id: Some("runner-session".to_string()),
        },
    )
    .await
    .expect("persist acp binding");
    let indexed_before = chat_index::get(&workspace, CHAT_ID)
        .await
        .expect("read Chat Index before replacement")
        .expect("Chat Index row before replacement");

    let error = update_authority_placement(None, &workspace, &address(), replacement.clone())
        .await
        .expect_err("user-authored switch remains locked by ACP");
    assert!(error.to_string().contains("Exit ACP"));

    let record = apply_confirmed_authority_placement(&workspace, &address(), replacement.clone())
        .await
        .expect("apply confirmed replacement");
    assert_eq!(
        record.meta[CHAT_AUTHORITY_PLACEMENT_META_KEY],
        serde_json::to_value(&replacement).expect("serialize placement")
    );

    let indexed_after = chat_index::get(&workspace, CHAT_ID)
        .await
        .expect("read chat index")
        .expect("indexed chat");
    assert_eq!(indexed_after, indexed_before);
    let header = WorkspaceSessionStore::new(&workspace)
        .load_chat_header(CHAT_ID)
        .await
        .expect("load header")
        .expect("header");
    assert_eq!(
        header.extra[CHAT_AUTHORITY_PLACEMENT_META_KEY],
        serde_json::to_value(&replacement).expect("serialize placement")
    );
}

#[tokio::test]
async fn acp_binding_preserves_explicit_authority_placement() {
    let tmp = TempDir::new().expect("tempdir");
    let workspace = tmp.path().join("workspace");

    update_authority_placement(
        None,
        &workspace,
        &address(),
        ConversationAuthorityPlacement {
            device_id: "device-acp".to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            base_manifest_hash: format!("sha256:{}", "2".repeat(64)),
        },
    )
    .await
    .expect("seed placement");

    let record = persist_acp_binding(
        None,
        &workspace,
        &address(),
        "default",
        AcpBindingMetadata {
            target_device_id: Some("device-acp".to_string()),
            runner: "codex".to_string(),
            cwd: Some("/tmp".to_string()),
            runner_session_id: Some("runner-session".to_string()),
        },
    )
    .await
    .expect("persist acp binding");

    assert_eq!(
        record.meta[CHAT_AUTHORITY_PLACEMENT_META_KEY],
        json!({
            "device_id": "device-acp",
            "authority_epoch": "0000000000000001000000000000000000000000",
            "base_manifest_hash": format!("sha256:{}", "2".repeat(64))
        })
    );
    assert_eq!(record.meta[CHAT_ACP_BINDING_META_KEY]["runner"], "codex");
    assert!(record.meta[CHAT_ACP_BINDING_META_KEY]
        .get("auto_assigned_authority_placement")
        .is_none());

    clear_acp_binding(None, &workspace, &address(), "default")
        .await
        .expect("clear acp binding");

    let indexed = chat_index::get(&workspace, CHAT_ID)
        .await
        .expect("read chat index")
        .expect("indexed chat");
    let meta = &indexed.meta;
    assert!(meta.get(CHAT_ACP_BINDING_META_KEY).is_none());
    assert!(meta.get(CHAT_AUTHORITY_PLACEMENT_META_KEY).is_some());

    let header = rua_session::WorkspaceSessionStore::new(&workspace)
        .load_chat_header(CHAT_ID)
        .await
        .expect("load header")
        .expect("header");
    assert!(header.extra.get(CHAT_ACP_BINDING_META_KEY).is_none());
    assert_eq!(
        header.extra[CHAT_AUTHORITY_PLACEMENT_META_KEY]["device_id"],
        "device-acp"
    );
}

#[tokio::test]
async fn project_metadata_keeps_execution_agent_separate_from_chat_ownership() {
    let tmp = TempDir::new().expect("tempdir");
    let project_root = tmp.path().join("projects/project-a");
    let address = rua_core::ChatAddress::project("project-a", CHAT_ID);
    let placement = ConversationAuthorityPlacement {
        device_id: "resource-device".to_string(),
        authority_epoch:
            rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(
                1,
                0,
                [0; 8],
            ),
        base_manifest_hash: format!("sha256:{}", "6".repeat(64)),
    };

    update_authority_placement(None, &project_root, &address, placement)
        .await
        .expect("Project placement");
    persist_acp_binding(
        None,
        &project_root,
        &address,
        "execution-agent",
        AcpBindingMetadata {
            target_device_id: Some("resource-device".to_string()),
            runner: "codex".to_string(),
            cwd: Some("/repo".to_string()),
            runner_session_id: None,
        },
    )
    .await
    .expect("Project ACP binding");

    let header = WorkspaceSessionStore::new(&project_root)
        .load_chat_header(CHAT_ID)
        .await
        .expect("load header")
        .expect("header");
    assert_eq!(header.agent_id, "execution-agent");
    assert_eq!(
        address.project_id().map(ToString::to_string).as_deref(),
        Some("project-a")
    );
}
