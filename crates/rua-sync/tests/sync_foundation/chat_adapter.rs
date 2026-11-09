use std::sync::Arc;

use rua_core::{ChatAddress, ChatScopeStore};
use rua_sync::chat_adapter::{
    index_record_id, ChatIndexSyncAdapter, CHAT_DOMAIN, CHAT_KIND_INDEX_DELETE,
    CHAT_KIND_INDEX_UPSERT,
};
use rua_sync::hlc::HlcTimestamp;
use rua_sync::merge_store::MergeStore;
use rua_sync::opaque_replica_log::SyncChange;
use rua_sync::sidecar::RecordState;
use rua_sync::sync_registry::{DomainSyncAdapter, RegistryApplyStatus, SyncRegistry};

fn agent_chat(agent_id: &str, chat_id: &str) -> ChatAddress {
    ChatAddress::agent(agent_id, chat_id)
}

fn hlc(wall_ms: u64) -> HlcTimestamp {
    HlcTimestamp {
        wall_ms,
        counter: 0,
        device_id: [0; 8],
    }
}

fn registry_with_chat_adapter(
    data_dir: &std::path::Path,
    device_id: &str,
) -> (SyncRegistry, Arc<ChatIndexSyncAdapter>) {
    let adapter = Arc::new(ChatIndexSyncAdapter::new(
        data_dir.to_path_buf(),
        device_id.to_string(),
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter.clone()).unwrap();
    (registry, adapter)
}

fn read_index(
    data_dir: &std::path::Path,
    address: &ChatAddress,
) -> Option<rua_session::chat_index::ChatIndexRecord> {
    let root = ChatScopeStore::resolve(data_dir, address.clone());
    rua_session::chat_index::get_sync(root.root(), address.chat_id().as_str()).unwrap()
}

fn seed_index(root: &std::path::Path, chat_id: &str, name: &str) {
    rua_session::chat_index::upsert_sync(
        root,
        &rua_session::chat_index::ChatIndexRecord {
            id: chat_id.to_string(),
            name: name.to_string(),
            user_id: "default".to_string(),
            channel: "console".to_string(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            meta: serde_json::json!({}),
            status: "idle".to_string(),
            pinned: false,
            pin_order: 0,
            tag: rua_session::ChatTag::None,
            archived: false,
        },
    )
    .unwrap();
}

#[test]
fn chat_record_ids_encode_scope_and_keep_equal_chat_ids_distinct() {
    let agent = ChatAddress::agent("agent-a", "shared-chat");
    let project = ChatAddress::project("project-a", "shared-chat");

    assert_eq!(index_record_id(&agent), "agent/agent-a/chats/shared-chat");
    assert_eq!(
        index_record_id(&project),
        "project/project-a/chats/shared-chat"
    );
    assert_ne!(index_record_id(&agent), index_record_id(&project));
}

#[tokio::test]
async fn legacy_record_ids_and_session_records_are_rejected() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let (registry, _) = registry_with_chat_adapter(data.path(), "device-b");

    for record_id in [
        "agent-a/chats/chat-a",
        "agent/agent-a/sessions/chat-a",
        "project/project-a/sessions/chat-a",
    ] {
        let change = SyncChange::new(
            format!("op-{record_id}"),
            CHAT_DOMAIN,
            record_id,
            CHAT_KIND_INDEX_UPSERT,
            "device-a",
            hlc(1),
            serde_json::json!({ "id": "chat-a" }),
        );
        let error = registry
            .apply_change(&change, &merge_store)
            .await
            .expect_err("legacy record id must not be accepted");
        assert!(error.to_string().contains("invalid Chat Index record id"));
    }
}

#[tokio::test]
async fn inbound_chat_index_uses_lww_without_a_second_identity() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let (registry, _) = registry_with_chat_adapter(data.path(), "device-b");
    let address = agent_chat("default", "c1");

    let local = ChatIndexSyncAdapter::index_change(
        "op-local-index",
        address.clone(),
        serde_json::json!({
            "id": "c1",
            "name": "local",
            "channel": "local-channel"
        }),
        "device-b",
        hlc(50),
    );
    registry.apply_change(&local, &merge_store).await.unwrap();
    let remote = ChatIndexSyncAdapter::index_change(
        "op-index",
        address.clone(),
        serde_json::json!({
            "id": "c1",
            "name": "remote",
            "user_id": "remote-user"
        }),
        "device-a",
        hlc(100),
    );

    let result = registry.apply_change(&remote, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    let chat = read_index(data.path(), &address).unwrap();
    assert_eq!(chat.id, "c1");
    assert_eq!(chat.name, "remote");
    assert_eq!(chat.channel, "console");
    assert_eq!(chat.user_id, "remote-user");
    assert_eq!(
        merge_store
            .get_record_cursor("chat:agent/default/chats/c1")
            .unwrap()
            .unwrap()
            .base_blob_hash,
        None
    );
}

#[tokio::test]
async fn agent_and_project_apply_and_tombstones_are_isolated() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let (registry, _) = registry_with_chat_adapter(data.path(), "device-b");
    let agent = ChatAddress::agent("owner-a", "shared-chat");
    let project = ChatAddress::project("owner-p", "shared-chat");

    for (address, name) in [(&agent, "Agent Chat"), (&project, "Project Chat")] {
        let change = ChatIndexSyncAdapter::index_change(
            format!("op-upsert-{}", address.scope().kind()),
            address.clone(),
            serde_json::json!({ "id": "shared-chat", "name": name }),
            "device-a",
            hlc(50),
        );
        assert_eq!(
            registry
                .apply_change(&change, &merge_store)
                .await
                .unwrap()
                .status,
            RegistryApplyStatus::Applied
        );
    }

    assert_eq!(read_index(data.path(), &agent).unwrap().name, "Agent Chat");
    assert_eq!(
        read_index(data.path(), &project).unwrap().name,
        "Project Chat"
    );

    let delete = SyncChange::new(
        "op-project-delete",
        CHAT_DOMAIN,
        index_record_id(&project),
        CHAT_KIND_INDEX_DELETE,
        "device-a",
        hlc(100),
        serde_json::json!({}),
    );
    assert_eq!(
        registry
            .apply_change(&delete, &merge_store)
            .await
            .unwrap()
            .status,
        RegistryApplyStatus::Applied
    );
    assert!(read_index(data.path(), &project).is_none());
    assert_eq!(read_index(data.path(), &agent).unwrap().name, "Agent Chat");
    assert_eq!(
        merge_store
            .get_record_cursor("chat:project/owner-p/chats/shared-chat")
            .unwrap()
            .unwrap()
            .state,
        RecordState::Tombstoned
    );
}

#[tokio::test]
async fn chat_tombstone_rejects_newer_index_upsert() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let (registry, _) = registry_with_chat_adapter(data.path(), "device-b");
    let address = ChatAddress::agent("default", "terminal-chat");

    let delete =
        ChatIndexSyncAdapter::delete_change("delete-terminal-chat", &address, "device-a", hlc(100));
    assert_eq!(
        registry
            .apply_change(&delete, &merge_store)
            .await
            .unwrap()
            .status,
        RegistryApplyStatus::Applied
    );

    let newer = ChatIndexSyncAdapter::index_change(
        "resurrect-terminal-chat",
        address.clone(),
        serde_json::json!({"id": "terminal-chat", "name": "Must not return"}),
        "device-c",
        hlc(500),
    );
    assert_eq!(
        registry
            .apply_change(&newer, &merge_store)
            .await
            .unwrap()
            .status,
        RegistryApplyStatus::Skipped
    );
    assert!(read_index(data.path(), &address).is_none());
    assert_eq!(
        merge_store
            .get_record_cursor("chat:agent/default/chats/terminal-chat")
            .unwrap()
            .unwrap()
            .state,
        RecordState::Tombstoned
    );
}

#[tokio::test]
async fn project_tombstone_rejects_later_chat_index_upsert() {
    let data = tempfile::tempdir().unwrap();
    let projects = rua_project::ProjectStores::open(data.path().join("projects")).unwrap();
    projects
        .catalog
        .apply_tombstone(
            rua_core::ProjectId::new("deleted-project"),
            chrono::Utc::now(),
        )
        .unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let (registry, _) = registry_with_chat_adapter(data.path(), "device-b");
    let address = ChatAddress::project("deleted-project", "chat-1");
    let change = ChatIndexSyncAdapter::index_change(
        "late-chat",
        address.clone(),
        serde_json::json!({"id": "chat-1", "name": "Must not return"}),
        "device-a",
        hlc(500),
    );

    assert_eq!(
        registry
            .apply_change(&change, &merge_store)
            .await
            .unwrap()
            .status,
        RegistryApplyStatus::Applied
    );
    assert!(read_index(data.path(), &address).is_none());
    assert_eq!(
        merge_store
            .get_record_cursor("chat:project/deleted-project/chats/chat-1")
            .unwrap()
            .unwrap()
            .state,
        RecordState::Tombstoned
    );
}

#[tokio::test]
async fn chat_index_sync_never_regresses_authority_epoch() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let (registry, _) = registry_with_chat_adapter(data.path(), "device-b");
    let address = agent_chat("default", "c1");
    let local = ChatIndexSyncAdapter::index_change(
        "op-authority-local",
        address.clone(),
        serde_json::json!({
            "id": "c1",
            "name": "local",
            "meta": {
                "authority_placement": {
                    "device_id": "device-c",
                    "authority_epoch": "0000000000000003000000000000000000000000",
                    "base_manifest_hash": format!("sha256:{}", "3".repeat(64)),
                }
            }
        }),
        "device-b",
        hlc(50),
    );
    registry.apply_change(&local, &merge_store).await.unwrap();
    let lower_epoch = ChatIndexSyncAdapter::index_change(
        "op-authority-lower",
        address.clone(),
        serde_json::json!({
            "id": "c1",
            "name": "newer display metadata",
            "meta": {
                "authority_placement": {
                    "device_id": "device-a",
                    "authority_epoch": "0000000000000002000000000000000000000000",
                    "base_manifest_hash": format!("sha256:{}", "2".repeat(64)),
                }
            }
        }),
        "device-a",
        hlc(100),
    );
    registry
        .apply_change(&lower_epoch, &merge_store)
        .await
        .unwrap();
    let after_lower = read_index(data.path(), &address).unwrap();
    assert_eq!(after_lower.name, "newer display metadata");
    assert_eq!(
        after_lower.meta["authority_placement"]["device_id"],
        "device-c"
    );

    let higher_epoch = ChatIndexSyncAdapter::index_change(
        "op-authority-higher",
        address.clone(),
        serde_json::json!({
            "id": "c1",
            "name": "authority moved",
            "meta": {
                "authority_placement": {
                    "device_id": "device-d",
                    "authority_epoch": "0000000000000004000000000000000000000000",
                    "base_manifest_hash": format!("sha256:{}", "4".repeat(64)),
                }
            }
        }),
        "device-d",
        hlc(150),
    );
    registry
        .apply_change(&higher_epoch, &merge_store)
        .await
        .unwrap();
    assert_eq!(
        read_index(data.path(), &address).unwrap().meta["authority_placement"]["device_id"],
        "device-d"
    );
}

#[tokio::test]
async fn stale_chat_index_delete_does_not_remove_newer_chat() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let (registry, _) = registry_with_chat_adapter(data.path(), "device-b");
    let address = agent_chat("default", "c1");
    let newer = ChatIndexSyncAdapter::index_change(
        "op-newer-upsert",
        address.clone(),
        serde_json::json!({ "id": "c1", "name": "Newer" }),
        "device-b",
        hlc(200),
    );
    registry.apply_change(&newer, &merge_store).await.unwrap();
    let stale_delete = SyncChange::new(
        "op-stale-delete",
        CHAT_DOMAIN,
        index_record_id(&address),
        CHAT_KIND_INDEX_DELETE,
        "device-a",
        hlc(100),
        serde_json::json!({}),
    );

    let result = registry
        .apply_change(&stale_delete, &merge_store)
        .await
        .unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Skipped);
    assert_eq!(read_index(data.path(), &address).unwrap().name, "Newer");
}

#[tokio::test]
async fn export_contains_agent_and_project_chat_indexes_only() {
    let data = tempfile::tempdir().unwrap();
    let agent_root = data.path().join("workspaces/default");
    let project_root = data.path().join("projects/project-a");
    seed_index(&agent_root, "agent-chat", "Agent Chat");
    seed_index(&project_root, "project-chat", "Project Chat");
    std::fs::create_dir_all(agent_root.join("sessions")).unwrap();
    std::fs::write(
        agent_root.join("sessions/legacy-session.json"),
        serde_json::to_vec(&serde_json::json!({ "session_id": "legacy" })).unwrap(),
    )
    .unwrap();
    let adapter = ChatIndexSyncAdapter::new(data.path().to_path_buf(), "device-a");

    let changes = adapter.export_records().await.unwrap();

    assert_eq!(changes.len(), 2);
    assert!(changes
        .iter()
        .all(|change| change.kind == CHAT_KIND_INDEX_UPSERT));
    assert_eq!(
        changes
            .iter()
            .map(|change| change.record_id.as_str())
            .collect::<Vec<_>>(),
        vec![
            "agent/default/chats/agent-chat",
            "project/project-a/chats/project-chat"
        ]
    );
    assert!(changes
        .iter()
        .all(|change| change.payload.get("session_id").is_none()));
}
