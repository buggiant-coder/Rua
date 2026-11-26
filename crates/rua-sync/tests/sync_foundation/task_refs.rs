use std::path::{Path, PathBuf};
use std::sync::Arc;

use rua_core::{ChatAddress, ChatStore};
use rua_session::task_refs::ChatTaskRef;
use rua_sync::evergreen_chat_adapter::{
    file_record_id, EvergreenChatSyncAdapter, EVERGREEN_CHAT_DOMAIN,
};
use rua_sync::hlc::HlcTimestamp;
use rua_sync::merge_store::MergeStore;
use rua_sync::opaque_replica_log::SyncChange;
use rua_sync::sync_registry::{sync_cursor_record_id, RegistryApplyStatus, SyncRegistry};
use serde_json::json;

fn hlc(wall_ms: u64) -> HlcTimestamp {
    HlcTimestamp {
        wall_ms,
        counter: 0,
        device_id: [0; 8],
    }
}

fn reference(attempt: u32, revision: u64, state: &str) -> ChatTaskRef {
    let owner = json!({"chat_address":{"chat_id":"chat-one"}});
    let origin = json!({"turn_id":"original-turn","tool_call_id":"original-call"});
    serde_json::from_value(json!({
        "task_id":"task-one","owner":owner,
        "execution":{"device_id":"phone","public_cwd":"/photos"},
        "origin":{"turn_id":"original-turn","tool_call_id":"original-call","agent_id":"original-agent"},
        "last_target_evidence": if revision == 0 { serde_json::Value::Null } else { json!({
            "revision":revision,"record":{
                "task_id":"task-one","spec":{"owner":owner,"instruction":"Photos",
                    "runner":{"type":"tool","tool_name":"bash","arguments":{"command":"pwd"}},
                    "context_refs":[],"depends_on":[],"origin":origin},
                "state":state,"attempt_no":attempt,"created_at":"2026-11-21T00:00:00Z","updated_at":"2026-11-21T00:00:00Z",
                "attempts":[{"attempt_no":attempt,"state":state,"authorization_agent_id":format!("agent-{attempt}"),
                    "device":{"device_id":"phone","public_cwd":"/photos"},
                    "created_at":"2026-11-21T00:00:00Z","updated_at":"2026-11-21T00:00:00Z"}]
            }
        })},"future_ref_field":{"retained":true}
    })).unwrap()
}

fn chat_dir(root: &Path) -> PathBuf {
    ChatStore::resolve(root, ChatAddress::new("chat-one"))
        .root()
        .join("sessions/chat-one")
}

fn registry(root: &Path) -> SyncRegistry {
    let mut registry = SyncRegistry::new();
    registry
        .register(Arc::new(EvergreenChatSyncAdapter::new(
            root.into(),
            "replica",
        )))
        .unwrap();
    registry
}

fn change(op: &str, wall: u64, reference: &ChatTaskRef) -> SyncChange {
    EvergreenChatSyncAdapter::file_change(
        op,
        &ChatAddress::new("chat-one"),
        "task_refs/task-one.json",
        serde_json::to_value(reference).unwrap(),
        "relay",
        hlc(wall),
    )
}

#[tokio::test]
async fn task_device_snapshot_cannot_roll_back_the_authoritys_active_turn_index() {
    use rua_session::{
        ChatMetadata, ChatSessionHeader, NewConversationMessage, NewConversationTurn,
        TurnLifecycle, WorkspaceSessionStore,
    };
    use rua_sync::sync_registry::DomainSyncAdapter;

    let authority = tempfile::tempdir().unwrap();
    let task_device = tempfile::tempdir().unwrap();
    let address = ChatAddress::new("chat-one");
    let scope = ChatStore::resolve(authority.path(), address.clone());
    let store = WorkspaceSessionStore::new(scope.root());
    let mut header = ChatSessionHeader::new(ChatMetadata {
        chat_id: "chat-one".into(),
        agent_id: "agent".into(),
        title: "Photos".into(),
        created_at: "2026-11-21T00:00:00Z".into(),
        last_activity_at: "2026-11-21T00:00:00Z".into(),
        pinned: false,
        archived: false,
        preview: None,
    });
    header.extra.insert(
        "authority_placement".into(),
        json!({
            "device_id":"computer", "authority_epoch":hlc(1).to_sortable_string(),
            "base_manifest_hash":format!("sha256:{}", "0".repeat(64)),
        }),
    );
    store.save_chat_header(&header).await.unwrap();
    let turn = |id: &str| NewConversationTurn {
        turn_id: id.into(),
        lifecycle: TurnLifecycle {
            user_submitted_at: "2026-11-21T00:00:00Z".into(),
            assistant_started_at: None,
            assistant_completed_at: None,
            status: "in_progress".into(),
        },
        selected_variant_id: None,
        messages: vec![NewConversationMessage {
            message_id: format!("message-{id}"),
            role: "user".into(),
            created_at: "2026-11-21T00:00:00Z".into(),
            status: "completed".into(),
            content: json!([{"type":"text","text":id}]),
            provider: None,
            tool: None,
            metadata: Default::default(),
        }],
    };
    store
        .append_conversation_turn("chat-one", turn("original"))
        .await
        .unwrap();
    let snapshots = EvergreenChatSyncAdapter::snapshot_changes_for_chat(
        scope.root(),
        &address,
        "computer",
        hlc(1),
    )
    .unwrap();
    let destination_merge = MergeStore::open(&task_device.path().join("merge")).unwrap();
    let destination_registry = registry(task_device.path());
    // Placement precedes the history it authorizes, as in ordinary initial hydration.
    for snapshot in snapshots
        .iter()
        .filter(|s| s.record_id.ends_with("/session"))
        .chain(
            snapshots
                .iter()
                .filter(|s| !s.record_id.ends_with("/session")),
        )
    {
        destination_registry
            .apply_change(snapshot, &destination_merge)
            .await
            .unwrap();
    }
    store
        .append_conversation_turn("chat-one", turn("task-result"))
        .await
        .unwrap();
    let read_index = || {
        serde_json::from_slice::<serde_json::Value>(
            &std::fs::read(chat_dir(authority.path()).join("turn_index.json")).unwrap(),
        )
        .unwrap()
    };
    let expected = read_index();
    reference(1, 2, "completed")
        .register(&chat_dir(task_device.path()), "chat-one")
        .unwrap();
    let replica = EvergreenChatSyncAdapter::new(task_device.path().into(), "phone");
    let mut snapshots = replica.export_records().await.unwrap();
    assert!(snapshots
        .iter()
        .any(|c| c.record_id.ends_with("task_refs/task-one.json")));
    let origin_merge = MergeStore::open(&authority.path().join("merge")).unwrap();
    let origin_registry = registry(authority.path());
    for snapshot in &mut snapshots {
        // The replica's later file mtime/export clock must not make it a history writer.
        snapshot.hlc = hlc(u64::MAX - 1);
        origin_registry
            .apply_change(snapshot, &origin_merge)
            .await
            .unwrap();
    }
    assert_eq!(read_index(), expected);
    assert_eq!(
        store
            .load_chat_header("chat-one")
            .await
            .unwrap()
            .unwrap()
            .message_log
            .latest_sequence,
        2
    );
    assert_eq!(
        ChatTaskRef::load(&chat_dir(authority.path()), "chat-one", "task-one").unwrap(),
        Some(reference(1, 2, "completed"))
    );
}

#[tokio::test]
async fn chat_snapshot_replicates_unconfirmed_and_accepted_task_refs_without_database_or_artifact_bytes(
) {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let source_chat = chat_dir(source.path());
    let pending = reference(0, 0, "accepted");
    pending.register(&source_chat, "chat-one").unwrap();
    std::fs::write(
        source.path().join("task-runtime.sqlite"),
        b"database must stay local",
    )
    .unwrap();
    std::fs::create_dir_all(source_chat.join("tasks/task-one")).unwrap();
    std::fs::write(
        source_chat.join("tasks/task-one/result.bin"),
        b"large result must stay local",
    )
    .unwrap();
    let scope = ChatStore::resolve(source.path(), ChatAddress::new("chat-one"));
    let snapshots = EvergreenChatSyncAdapter::snapshot_changes_for_chat(
        scope.root(),
        &ChatAddress::new("chat-one"),
        "phone",
        hlc(1),
    )
    .unwrap();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(
        snapshots[0].record_id,
        "chats/chat-one/task_refs/task-one.json"
    );
    let merge = MergeStore::open(&destination.path().join("merge")).unwrap();
    let registry = registry(destination.path());
    registry.apply_change(&snapshots[0], &merge).await.unwrap();
    assert_eq!(
        ChatTaskRef::load(&chat_dir(destination.path()), "chat-one", "task-one").unwrap(),
        Some(pending)
    );
    let accepted = reference(1, 1, "waiting_approval");
    registry
        .apply_change(&change("target-accepted", 2, &accepted), &merge)
        .await
        .unwrap();
    assert_eq!(
        ChatTaskRef::load(&chat_dir(destination.path()), "chat-one", "task-one").unwrap(),
        Some(accepted)
    );
    assert!(!destination.path().join("task-runtime.sqlite").exists());
    assert!(!chat_dir(destination.path()).join("tasks").exists());
}

#[tokio::test]
async fn target_attempt_and_revision_outvote_relay_clock_without_regressing_the_transport_cursor() {
    let root = tempfile::tempdir().unwrap();
    let registry = registry(root.path());
    let merge = MergeStore::open(&root.path().join("merge")).unwrap();
    let running = reference(1, 1, "running");
    registry
        .apply_change(&change("running", 100, &running), &merge)
        .await
        .unwrap();
    let terminal = reference(1, 2, "failed");
    assert_eq!(
        registry
            .apply_change(&change("terminal-earlier-clock", 50, &terminal), &merge)
            .await
            .unwrap()
            .status,
        RegistryApplyStatus::Applied
    );
    assert_eq!(
        registry
            .apply_change(&change("old-newer-relay-clock", 500, &running), &merge)
            .await
            .unwrap()
            .status,
        RegistryApplyStatus::Skipped
    );
    let cursor_id = sync_cursor_record_id(
        EVERGREEN_CHAT_DOMAIN,
        &file_record_id(&ChatAddress::new("chat-one"), "task_refs/task-one.json"),
    );
    assert_eq!(
        merge.get_record_cursor(&cursor_id).unwrap().unwrap().hlc,
        hlc(100)
    );
    let retry = reference(2, 3, "running");
    registry
        .apply_change(&change("retry-older-clock", 20, &retry), &merge)
        .await
        .unwrap();
    let stale_attempt = reference(1, 99, "waiting_approval");
    assert_eq!(
        registry
            .apply_change(&change("old-attempt", 900, &stale_attempt), &merge)
            .await
            .unwrap()
            .status,
        RegistryApplyStatus::Skipped
    );
    assert_eq!(
        registry
            .apply_change(
                &change("delayed-unconfirmed", 1000, &reference(0, 0, "accepted")),
                &merge
            )
            .await
            .unwrap()
            .status,
        RegistryApplyStatus::Skipped
    );
    assert_eq!(
        ChatTaskRef::load(&chat_dir(root.path()), "chat-one", "task-one").unwrap(),
        Some(retry)
    );
}

#[tokio::test]
async fn task_ref_sync_rejects_identity_version_path_conflicts_and_standalone_deletion() {
    let root = tempfile::tempdir().unwrap();
    let registry = registry(root.path());
    let merge = MergeStore::open(&root.path().join("merge")).unwrap();
    let valid = reference(1, 1, "running");
    registry
        .apply_change(&change("initial", 1, &valid), &merge)
        .await
        .unwrap();
    let mut wrong_location = reference(0, 0, "accepted");
    wrong_location.execution.device_id = "other-device".into();
    assert!(registry
        .apply_change(&change("wrong-device", 2, &wrong_location), &merge)
        .await
        .is_err());
    assert!(registry
        .apply_change(
            &change("same-revision-new-state", 3, &reference(1, 1, "completed")),
            &merge
        )
        .await
        .is_err());
    let mut wrong_owner = reference(0, 0, "accepted");
    wrong_owner.owner.chat_address = rua_core::ChatAddress::new("other-chat");
    assert!(registry
        .apply_change(&change("wrong-owner", 4, &wrong_owner), &merge)
        .await
        .is_err());
    for (index, relative) in [
        "task_refs/../outside.json",
        "task_refs/nested/task-one.json",
        "task_refs/C:drive.json",
        "task_refs/wrong-id.json",
    ]
    .iter()
    .enumerate()
    {
        let invalid = EvergreenChatSyncAdapter::file_change(
            format!("bad-path-{index}"),
            &ChatAddress::new("chat-one"),
            relative,
            serde_json::to_value(&valid).unwrap(),
            "relay",
            hlc(10),
        );
        assert!(registry.apply_change(&invalid, &merge).await.is_err());
    }
    let deletion = EvergreenChatSyncAdapter::delete_change(
        "delete-ref",
        &ChatAddress::new("chat-one"),
        "task_refs/task-one.json",
        "relay",
        hlc(20),
    );
    assert!(registry.apply_change(&deletion, &merge).await.is_err());
    assert_eq!(
        ChatTaskRef::load(&chat_dir(root.path()), "chat-one", "task-one").unwrap(),
        Some(valid)
    );
}

#[cfg(unix)]
#[tokio::test]
async fn task_ref_collection_and_apply_do_not_follow_symlinked_metadata() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let chat = chat_dir(root.path());
    std::fs::create_dir_all(&chat).unwrap();
    let protected = outside.path().join("task-one.json");
    std::fs::write(
        &protected,
        serde_json::to_vec(&reference(1, 1, "running")).unwrap(),
    )
    .unwrap();
    symlink(outside.path(), chat.join("task_refs")).unwrap();
    let scope = ChatStore::resolve(root.path(), ChatAddress::new("chat-one"));
    assert!(EvergreenChatSyncAdapter::snapshot_changes_for_chat(
        scope.root(),
        &ChatAddress::new("chat-one"),
        "source",
        hlc(1)
    )
    .unwrap()
    .is_empty());
    let registry = registry(root.path());
    let merge = MergeStore::open(&root.path().join("merge")).unwrap();
    let original = std::fs::read(&protected).unwrap();
    assert!(registry
        .apply_change(&change("symlink", 2, &reference(1, 2, "completed")), &merge)
        .await
        .is_err());
    assert_eq!(std::fs::read(protected).unwrap(), original);
}

#[tokio::test]
async fn confirmed_non_acceptance_survives_old_pending_sync_and_yields_to_actual_target_evidence() {
    let root = tempfile::tempdir().unwrap();
    let registry = registry(root.path());
    let merge = MergeStore::open(&root.path().join("merge")).unwrap();
    let pending = reference(0, 0, "accepted");
    let mut denied = pending.clone();
    denied.not_accepted = true;
    registry
        .apply_change(&change("pending", 100, &pending), &merge)
        .await
        .unwrap();
    assert_eq!(
        registry
            .apply_change(&change("confirmed-not-accepted", 10, &denied), &merge)
            .await
            .unwrap()
            .status,
        RegistryApplyStatus::Applied
    );
    assert_eq!(
        registry
            .apply_change(&change("delayed-pending", 1000, &pending), &merge)
            .await
            .unwrap()
            .status,
        RegistryApplyStatus::Skipped
    );
    assert_eq!(
        ChatTaskRef::load(&chat_dir(root.path()), "chat-one", "task-one").unwrap(),
        Some(denied.clone())
    );
    let accepted = reference(1, 1, "running");
    registry
        .apply_change(&change("actual-target-acceptance", 5, &accepted), &merge)
        .await
        .unwrap();
    registry
        .apply_change(&change("delayed-negative", 2000, &denied), &merge)
        .await
        .unwrap();
    let retained = ChatTaskRef::load(&chat_dir(root.path()), "chat-one", "task-one")
        .unwrap()
        .unwrap();
    assert!(retained.not_accepted);
    assert_eq!(retained.last_target_evidence, accepted.last_target_evidence);
}
