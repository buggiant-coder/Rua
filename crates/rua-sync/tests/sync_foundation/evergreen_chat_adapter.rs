use std::sync::Arc;

use rua_core::ChatAddress;
use rua_session::{
    ChatMetadata, ChatSessionHeader, CheckpointModelIdentity, CheckpointSequenceRange,
    CheckpointTurnRange, HistoryReplacementReadyDraft, HistoryReplacementRecord,
    HistoryReplacementRejectedDraft, HistoryReplacementRejectionReason, NewConversationMessage,
    NewConversationTurn, RuntimeNote, RuntimeNotePayload, SummaryCheckpointDraft,
    TaskAttemptTerminalRuntimeNote, TurnLifecycle, TurnWriter, WorkspaceSessionStore,
};
use rua_sync::evergreen_chat_adapter::{
    collect_evergreen_chat_file_snapshots, file_record_id, EvergreenChatSyncAdapter,
    EVERGREEN_CHAT_DOMAIN, EVERGREEN_CHAT_KIND_FILE_SNAPSHOT,
};
use rua_sync::hlc::HlcTimestamp;
use rua_sync::merge_store::MergeStore;
use rua_sync::sidecar::RecordState;
use rua_sync::sync_registry::{DomainSyncAdapter, RegistryApplyStatus, SyncRegistry};

fn agent_chat(agent_id: &str, chat_id: &str) -> ChatAddress {
    ChatAddress::agent(agent_id, chat_id)
}

#[test]
fn evergreen_record_ids_encode_scope_and_keep_equal_chat_ids_distinct() {
    let agent = ChatAddress::agent("agent-a", "shared-chat");
    let project = ChatAddress::project("project-a", "shared-chat");

    assert_eq!(
        file_record_id(&agent, "session"),
        "agent/agent-a/shared-chat/session"
    );
    assert_eq!(
        file_record_id(&project, "session"),
        "project/project-a/shared-chat/session"
    );
    assert_ne!(
        file_record_id(&agent, "session"),
        file_record_id(&project, "session")
    );
}

#[tokio::test]
async fn scoped_round_trip_preserves_unified_chat_fields_and_compact_assignment() {
    let source = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let addresses = [
        ChatAddress::agent("agent-a", "chat-unified-agent"),
        ChatAddress::project("project-a", "chat-unified-project"),
    ];
    for address in &addresses {
        let scope = rua_core::ChatScopeStore::resolve(source.path(), address.clone());
        let store = WorkspaceSessionStore::new(scope.root());
        let mut header = ChatSessionHeader::new(ChatMetadata {
            chat_id: address.chat_id().to_string(),
            agent_id: "agent-a".to_string(),
            title: "Unified Chat".to_string(),
            created_at: "2026-08-20T00:00:00Z".to_string(),
            last_activity_at: "2026-08-20T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        });
        header.extra.insert(
            "chat_policy".to_string(),
            serde_json::to_value(rua_chat::ChatPolicy::for_scope(address.scope())).unwrap(),
        );
        header.extra.insert(
            "selected_agent_id".to_string(),
            serde_json::json!("agent-a"),
        );
        header.extra.insert(
            "future_field".to_string(),
            serde_json::json!({"preserve": true}),
        );
        store.save_chat_header(&header).await.unwrap();
        let running_config = rua_config::ChatRunningConfig {
            max_iters: if address.project_id().is_some() {
                41
            } else {
                31
            },
            ..Default::default()
        };
        header.extra.insert(
            "hook_configuration".into(),
            serde_json::json!({"epoch":"config-one", "running_config":running_config}),
        );
        store.save_chat_header(&header).await.unwrap();
        let assignment = rua_chat::TurnAgentAssignment {
            agent_id: "agent-a".to_string(),
            agent_name: "Agent A".to_string(),
            assigned_at: "2026-08-20T00:00:00Z".to_string(),
            system_prompt: Some("Act carefully".to_string()),
            system_prompt_files: vec!["SYSTEM.md".to_string()],
            disabled_tools: vec!["Bash".to_string()],
        };
        let mut metadata = serde_json::Map::new();
        metadata.insert(
            "turn_agent_assignment".to_string(),
            serde_json::to_value(assignment).unwrap(),
        );
        store
            .append_conversation_turn(
                address.chat_id().as_str(),
                NewConversationTurn {
                    turn_id: "turn-1".to_string(),
                    lifecycle: TurnLifecycle {
                        user_submitted_at: "2026-08-20T00:00:00Z".to_string(),
                        assistant_started_at: Some("2026-08-20T00:00:01Z".to_string()),
                        assistant_completed_at: Some("2026-08-20T00:00:02Z".to_string()),
                        status: "completed".to_string(),
                    },
                    selected_variant_id: Some("assistant-1".to_string()),
                    messages: vec![
                        NewConversationMessage {
                            message_id: "user-1".to_string(),
                            role: "user".to_string(),
                            created_at: "2026-08-20T00:00:00Z".to_string(),
                            status: "completed".to_string(),
                            content: serde_json::json!([{"type": "text", "text": "sync"}]),
                            provider: None,
                            tool: None,
                            metadata,
                        },
                        conversation_message(
                            "assistant-1",
                            "assistant",
                            serde_json::json!([{"type": "text", "text": "synced"}]),
                        ),
                    ],
                },
            )
            .await
            .unwrap();
    }

    let source_adapter = EvergreenChatSyncAdapter::new(source.path().to_path_buf(), "device-a");
    let changes = source_adapter.export_records().await.unwrap();
    for address in &addresses {
        assert!(changes
            .iter()
            .any(|change| change.record_id == file_record_id(address, "session")));
        assert!(changes
            .iter()
            .any(|change| change.record_id == file_record_id(address, "configuration_head")));
    }

    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let mut registry = SyncRegistry::new();
    registry
        .register(Arc::new(EvergreenChatSyncAdapter::new(
            target.path().to_path_buf(),
            "device-b",
        )))
        .unwrap();
    for change in &changes {
        registry.apply_change(change, &merge_store).await.unwrap();
    }

    for address in &addresses {
        let scope = rua_core::ChatScopeStore::resolve(target.path(), address.clone());
        let store = WorkspaceSessionStore::new(scope.root());
        let header = store
            .load_chat_header(address.chat_id().as_str())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(header.extra["selected_agent_id"], "agent-a");
        assert_eq!(
            header.extra["chat_policy"]["authority_placement"],
            match address.scope() {
                rua_core::ChatScope::Agent { .. } => "selectable",
                rua_core::ChatScope::Project { .. } => "resource_bound",
            }
        );
        assert_eq!(header.extra["future_field"]["preserve"], true);
        assert_eq!(
            store
                .load_chat_running_config(address.chat_id().as_str())
                .await
                .unwrap()
                .unwrap()
                .max_iters,
            if address.project_id().is_some() {
                41
            } else {
                31
            }
        );
        let messages = store
            .read_turn_messages(address.chat_id().as_str(), "turn-1")
            .await
            .unwrap();
        let assignment = &messages[0].metadata["turn_agent_assignment"];
        assert_eq!(assignment["agent_id"], "agent-a");
        assert_eq!(assignment["disabled_tools"], serde_json::json!(["Bash"]));
        assert!(assignment.get("configuration").is_none());
        assert!(assignment.get("default_model").is_none());
    }
}

#[tokio::test]
async fn complete_chat_configuration_roundtrip_preserves_head_and_snapshot() {
    let source = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let address = ChatAddress::agent("snapshot-agent", "snapshot-chat");
    let scope = rua_core::ChatScopeStore::resolve(source.path(), address.clone());
    let store = WorkspaceSessionStore::new(scope.root());
    let mut header = ChatSessionHeader::new(ChatMetadata {
        chat_id: address.chat_id().to_string(),
        agent_id: "snapshot-agent".into(),
        title: "Snapshot".into(),
        created_at: "2026-08-01T00:00:00Z".into(),
        last_activity_at: "2026-08-01T00:00:00Z".into(),
        pinned: false,
        archived: false,
        preview: None,
    });
    let configuration = serde_json::json!({
        "epoch": "config-one",
        "running_config": rua_config::ChatRunningConfig { max_iters: 17, ..Default::default() },
        "future_section": {"value": true},
    });
    header
        .extra
        .insert("hook_configuration".into(), configuration.clone());
    store.save_chat_header(&header).await.unwrap();
    let changes = EvergreenChatSyncAdapter::new(source.path().to_path_buf(), "source")
        .export_records()
        .await
        .unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let mut registry = SyncRegistry::new();
    registry
        .register(Arc::new(EvergreenChatSyncAdapter::new(
            target.path().to_path_buf(),
            "target",
        )))
        .unwrap();
    for change in &changes {
        registry.apply_change(change, &merge_store).await.unwrap();
    }
    let scope = rua_core::ChatScopeStore::resolve(target.path(), address.clone());
    let restored = WorkspaceSessionStore::new(scope.root());
    assert_eq!(
        restored
            .load_configuration_snapshot(address.chat_id().as_str(), "config-one")
            .await
            .unwrap(),
        Some(configuration.clone())
    );
    let mut conflicting = changes
        .iter()
        .find(|change| change.record_id.ends_with("configurations/config-one.json"))
        .unwrap()
        .clone();
    conflicting.hlc.wall_ms += 1;
    conflicting.payload["future_section"] = serde_json::json!({"value": false});
    assert!(registry
        .apply_change(&conflicting, &merge_store)
        .await
        .is_err());
    assert_eq!(
        restored
            .load_chat_header(address.chat_id().as_str())
            .await
            .unwrap()
            .unwrap()
            .extra["hook_configuration"],
        configuration
    );
    assert_eq!(
        restored
            .load_chat_running_config(address.chat_id().as_str())
            .await
            .unwrap()
            .unwrap()
            .max_iters,
        17
    );
}

fn hlc(wall_ms: u64) -> HlcTimestamp {
    HlcTimestamp {
        wall_ms,
        counter: 0,
        device_id: [0; 8],
    }
}

fn workspace_dir(data_dir: &std::path::Path, agent_id: &str) -> std::path::PathBuf {
    data_dir.join("workspaces").join(agent_id)
}

fn conversation_message(
    message_id: &str,
    role: &str,
    content: serde_json::Value,
) -> NewConversationMessage {
    NewConversationMessage {
        message_id: message_id.to_string(),
        role: role.to_string(),
        created_at: "2026-07-24T00:00:00Z".to_string(),
        status: "completed".to_string(),
        content,
        provider: None,
        tool: None,
        metadata: serde_json::Map::new(),
    }
}

async fn create_history_replacement_record(
    workspace: &std::path::Path,
    chat_id: &str,
    rejected: bool,
) -> HistoryReplacementRecord {
    create_history_replacement_record_for_address(
        workspace,
        &agent_chat("default", chat_id),
        rejected,
    )
    .await
}

async fn create_history_replacement_record_for_address(
    workspace: &std::path::Path,
    address: &ChatAddress,
    rejected: bool,
) -> HistoryReplacementRecord {
    let chat_id = address.chat_id().as_str();
    let store = WorkspaceSessionStore::new(workspace);
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "default".to_string(),
            title: "Synced replacement".to_string(),
            created_at: "2026-07-26T00:00:00Z".to_string(),
            last_activity_at: "2026-07-26T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .unwrap();
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-07-26T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-07-26T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-07-26T00:00:03Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("variant-1".to_string()),
                messages: vec![
                    conversation_message(
                        "user-1",
                        "user",
                        serde_json::json!([{"type": "text", "text": "Inspect sync state"}]),
                    ),
                    conversation_message(
                        "process-1",
                        "assistant",
                        serde_json::json!([{
                            "type": "tool_use",
                            "id": "call-1",
                            "name": "read",
                            "input": {"path": "sync.rs"}
                        }]),
                    ),
                    conversation_message(
                        "result-1",
                        "tool",
                        serde_json::json!([{
                            "type": "tool_result",
                            "id": "call-1",
                            "name": "read",
                            "output": [{
                                "type": "text",
                                "text": format!("sync-record-marker{}", "x".repeat(70_000))
                            }]
                        }]),
                    ),
                    conversation_message(
                        "final-1",
                        "assistant",
                        serde_json::json!([{"type": "text", "text": "Sync state inspected."}]),
                    ),
                ],
            },
        )
        .await
        .unwrap();
    let block = store
        .detect_history_replacement_blocks(chat_id, 4.0)
        .await
        .unwrap()
        .remove(0);
    let model = CheckpointModelIdentity {
        provider: "provider-a".to_string(),
        model: "model-a".to_string(),
    };
    if rejected {
        store
            .publish_rejected_history_replacement(
                address,
                HistoryReplacementRejectedDraft {
                    record_id: block.record_id,
                    source: block.source,
                    model,
                    reason: HistoryReplacementRejectionReason::InsufficientSavings,
                    replacement_token_estimate: 10_000,
                    saved_token_estimate: 7_000,
                    rejected_at: "2026-07-26T00:01:00Z".to_string(),
                },
            )
            .await
            .unwrap()
    } else {
        store
            .publish_ready_history_replacement(
                address,
                HistoryReplacementReadyDraft {
                    record_id: block.record_id,
                    source: block.source,
                    digest: "Synced execution trace digest.".to_string(),
                    replacement_token_estimate: 32,
                    model,
                    generated_at: "2026-07-26T00:01:00Z".to_string(),
                },
            )
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn export_reads_directory_backed_evergreen_chat_files() {
    let data = tempfile::tempdir().unwrap();
    let workspace = workspace_dir(data.path(), "default");
    let chat_dir = workspace.join("sessions").join("chat-a");
    std::fs::create_dir_all(chat_dir.join("messages")).unwrap();
    std::fs::create_dir_all(chat_dir.join("checkpoints")).unwrap();
    std::fs::create_dir_all(chat_dir.join("projection_cache")).unwrap();
    std::fs::create_dir_all(
        chat_dir
            .join("tasks")
            .join("task-1")
            .join("attempts")
            .join("1"),
    )
    .unwrap();
    rua_config::atomic_write_json(
        &chat_dir.join("session.json"),
        &serde_json::json!({"id": "chat-a", "title": "Evergreen"}),
    )
    .unwrap();
    rua_config::atomic_write_json(
        &chat_dir.join("turn_index.json"),
        &serde_json::json!({"turns": []}),
    )
    .unwrap();
    rua_config::atomic_write_json(
        &chat_dir.join("configuration_head.json"),
        &serde_json::json!({"epoch":"config-one"}),
    )
    .unwrap();
    rua_config::atomic_write_json(
        &chat_dir.join("projection_exclusions.json"),
        &serde_json::json!({
            "format_version": 1,
            "chat_id": "chat-a",
            "message_ids": ["assistant-hallucination"],
            "updated_at": "2026-08-03T00:00:00Z"
        }),
    )
    .unwrap();
    rua_config::atomic_write(
        &chat_dir.join("messages").join("000000000000.jsonl"),
        br#"{"message_id":"m1","role":"user"}"#,
    )
    .unwrap();
    rua_config::atomic_write_json(
        &chat_dir.join("checkpoints").join("cp1.json"),
        &serde_json::json!({"checkpoint_id": "cp1"}),
    )
    .unwrap();
    rua_config::atomic_write_json(
        &chat_dir.join("projection_cache").join("pc1.json"),
        &serde_json::json!({"cache_id": "pc1"}),
    )
    .unwrap();
    rua_config::atomic_write_json(
        &chat_dir
            .join("tasks")
            .join("task-1")
            .join("attempts")
            .join("1")
            .join("raw-output.json"),
        &serde_json::json!({"private": "device-local task output"}),
    )
    .unwrap();
    rua_config::atomic_write_json(
        &workspace.join("prompt-state").join("chat-a.json"),
        &serde_json::json!({"mode": "agent", "files": [{"id": "p1"}]}),
    )
    .unwrap();

    let adapter = EvergreenChatSyncAdapter::new(data.path().to_path_buf(), "device-a");
    let records = adapter.export_records().await.unwrap();
    let ids: Vec<_> = records
        .iter()
        .map(|record| record.record_id.as_str())
        .collect();

    assert!(ids.contains(&"agent/default/chat-a/session"));
    assert!(ids.contains(&"agent/default/chat-a/configuration_head"));
    assert!(ids.contains(&"agent/default/chat-a/projection_exclusions"));
    assert!(ids.contains(&"agent/default/chat-a/turn_index"));
    assert!(ids.contains(&"agent/default/chat-a/messages/000000000000.jsonl"));
    assert!(ids.contains(&"agent/default/chat-a/checkpoints/cp1.json"));
    assert!(
        !ids.contains(&"agent/default/chat-a/projection_cache/pc1.json"),
        "derived projection cache records must not be exported"
    );
    assert!(
        ids.iter().all(|record_id| !record_id.contains("/tasks/")),
        "Device-local Task records and Attempt data must not be exported"
    );
    assert!(ids.contains(&"agent/default/chat-a/prompt_state"));
    assert!(records
        .iter()
        .all(|record| record.domain == EVERGREEN_CHAT_DOMAIN));
}

#[tokio::test]
async fn export_includes_durable_runtime_note_but_not_pending_queue() {
    let data = tempfile::tempdir().unwrap();
    let workspace = workspace_dir(data.path(), "default");
    let store = WorkspaceSessionStore::new(&workspace);
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: "chat-note".to_string(),
            agent_id: "default".to_string(),
            title: "Runtime Note sync".to_string(),
            created_at: "2026-08-03T00:00:00Z".to_string(),
            last_activity_at: "2026-08-03T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .unwrap();
    store
        .append_conversation_turn(
            "chat-note",
            NewConversationTurn {
                turn_id: "turn-note".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-08-03T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-08-03T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-08-03T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: None,
                messages: vec![conversation_message(
                    "user-note",
                    "user",
                    serde_json::json!([{"type": "text", "text": "start"}]),
                )],
            },
        )
        .await
        .unwrap();
    store
        .append_runtime_note(
            "chat-note",
            "turn-note",
            RuntimeNote {
                runtime_note_id: "task:sync:attempt:1".to_string(),
                created_at: "2026-08-03T00:00:03Z".to_string(),
                payload: RuntimeNotePayload::TaskAttemptTerminal(TaskAttemptTerminalRuntimeNote {
                    task_id: "sync".to_string(),
                    attempt_no: 1,
                    terminal_status: "completed".to_string(),
                    output: "synced result".to_string(),
                    runner_name: "Fixture".to_string(),
                    tool_call_id: None,
                    full_result_ref: None,
                    artifacts: serde_json::json!([]),
                    device: serde_json::json!({}),
                }),
            },
        )
        .await
        .unwrap();

    let snapshots = collect_evergreen_chat_file_snapshots(&workspace, "chat-note", "device-a");
    let message_snapshot = snapshots
        .iter()
        .find(|snapshot| snapshot.relative.starts_with("messages/"))
        .expect("Message Log snapshot");
    assert!(message_snapshot
        .payload
        .as_str()
        .unwrap()
        .contains("task:sync:attempt:1"));
    assert!(snapshots
        .iter()
        .all(|snapshot| !snapshot.relative.contains("pending_runtime_notes")));
}

#[tokio::test]
async fn export_and_inbound_apply_roundtrip_ready_and_rejected_history_replacements() {
    let source_data = tempfile::tempdir().unwrap();
    let source_workspace = workspace_dir(source_data.path(), "default");
    let ready = create_history_replacement_record(&source_workspace, "chat-ready", false).await;
    let rejected =
        create_history_replacement_record(&source_workspace, "chat-rejected", true).await;
    std::fs::write(
        source_workspace
            .join("sessions")
            .join("chat-ready")
            .join("history_replacements")
            .join("in-flight.tmp"),
        b"local candidate",
    )
    .unwrap();

    let source_adapter =
        EvergreenChatSyncAdapter::new(source_data.path().to_path_buf(), "device-a");
    let exported = source_adapter.export_records().await.unwrap();
    let ready_record_id = format!(
        "agent/default/chat-ready/history_replacements/{}.json",
        ready.record_id
    );
    let rejected_record_id = format!(
        "agent/default/chat-rejected/history_replacements/{}.json",
        rejected.record_id
    );
    assert!(exported
        .iter()
        .any(|change| change.record_id == ready_record_id));
    assert!(exported
        .iter()
        .any(|change| change.record_id == rejected_record_id));
    assert!(
        exported
            .iter()
            .all(|change| !change.record_id.contains("in-flight")),
        "local candidates and leases must not be exported"
    );

    let target_data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let mut registry = SyncRegistry::new();
    registry
        .register(Arc::new(EvergreenChatSyncAdapter::new(
            target_data.path().to_path_buf(),
            "device-b",
        )))
        .unwrap();
    for change in &exported {
        registry.apply_change(change, &merge_store).await.unwrap();
    }

    let target_store = WorkspaceSessionStore::new(workspace_dir(target_data.path(), "default"));
    assert_eq!(
        target_store
            .load_history_replacement("chat-ready", &ready.record_id)
            .await
            .unwrap(),
        Some(ready)
    );
    assert_eq!(
        target_store
            .load_history_replacement("chat-rejected", &rejected.record_id)
            .await
            .unwrap(),
        Some(rejected)
    );
}

#[tokio::test]
async fn project_history_replacement_roundtrips_with_project_scope() {
    let source_data = tempfile::tempdir().unwrap();
    let project_address = ChatAddress::project("project-a", "chat-project");
    let source_project = source_data.path().join("projects").join("project-a");
    let record =
        create_history_replacement_record_for_address(&source_project, &project_address, false)
            .await;
    let publication_error = EvergreenChatSyncAdapter::snapshot_changes_for_chat(
        &source_project,
        &agent_chat("default", "chat-project"),
        "device-a",
        hlc(1),
    )
    .expect_err("Project history replacement must not publish under an Agent namespace");
    assert!(
        matches!(publication_error, rua_sync::SyncError::InvalidPayload(_)),
        "namespace mismatch must be a permanent invalid payload: {publication_error}"
    );
    assert!(publication_error.is_permanent());
    let relative = format!("history_replacements/{}.json", record.record_id);
    let source_adapter =
        EvergreenChatSyncAdapter::new(source_data.path().to_path_buf(), "device-a");
    let exported = source_adapter.export_records().await.unwrap();
    let record_id = file_record_id(&project_address, &relative);
    let change = exported
        .iter()
        .find(|change| change.record_id == record_id)
        .expect("Project-scoped history replacement snapshot");
    assert_eq!(
        change.payload["chat_address"],
        serde_json::to_value(&project_address).unwrap()
    );
    assert!(exported
        .iter()
        .all(|change| !change.record_id.starts_with("agent/default/chat-project/")));

    let target_data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let mut registry = SyncRegistry::new();
    registry
        .register(Arc::new(EvergreenChatSyncAdapter::new(
            target_data.path().to_path_buf(),
            "device-b",
        )))
        .unwrap();

    let result = registry.apply_change(change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    let target_project = target_data.path().join("projects").join("project-a");
    let target_record_path = WorkspaceSessionStore::new(&target_project)
        .history_replacement_path("chat-project", &record.record_id);
    assert_eq!(
        serde_json::from_slice::<HistoryReplacementRecord>(
            &std::fs::read(target_record_path).unwrap()
        )
        .unwrap(),
        record
    );
    assert!(
        !workspace_dir(target_data.path(), "default")
            .join("sessions")
            .join("chat-project")
            .join(&relative)
            .exists(),
        "Project-scoped replacement must not materialize under the Agent workspace"
    );
}

#[tokio::test]
async fn inbound_rejects_tampered_or_noncanonical_history_replacement_records() {
    let source_data = tempfile::tempdir().unwrap();
    let source_workspace = workspace_dir(source_data.path(), "default");
    let record = create_history_replacement_record(&source_workspace, "chat-tampered", false).await;
    let source_adapter =
        EvergreenChatSyncAdapter::new(source_data.path().to_path_buf(), "device-a");
    let mut change = source_adapter
        .export_records()
        .await
        .unwrap()
        .into_iter()
        .find(|change| {
            change.record_id
                == format!(
                    "agent/default/chat-tampered/history_replacements/{}.json",
                    record.record_id
                )
        })
        .expect("history replacement snapshot");
    change.payload["state"]["digest"] = serde_json::json!("tampered digest");

    let target_data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let mut registry = SyncRegistry::new();
    registry
        .register(Arc::new(EvergreenChatSyncAdapter::new(
            target_data.path().to_path_buf(),
            "device-b",
        )))
        .unwrap();
    let error = registry
        .apply_change(&change, &merge_store)
        .await
        .expect_err("tampered replacement must be rejected");
    assert!(
        matches!(error, rua_sync::SyncError::InvalidPayload(_)),
        "tampered replacement must be classified as a permanent invalid payload: {error}"
    );
    assert!(!workspace_dir(target_data.path(), "default")
        .join("sessions")
        .join("chat-tampered")
        .join("history_replacements")
        .join(format!("{}.json", record.record_id))
        .exists());

    let invalid_path = EvergreenChatSyncAdapter::file_change(
        "op-invalid-history-replacement-path",
        &agent_chat("default", "chat-tampered"),
        "history_replacements/not-a-record.json",
        serde_json::to_value(&record).unwrap(),
        "device-a",
        hlc(200),
    );
    assert!(registry
        .apply_change(&invalid_path, &merge_store)
        .await
        .is_err());
}

#[tokio::test]
async fn inbound_consumes_superseded_history_replacement_format_without_materializing_it() {
    let source_data = tempfile::tempdir().unwrap();
    let source_workspace = workspace_dir(source_data.path(), "default");
    let record = create_history_replacement_record(&source_workspace, "chat-legacy", false).await;
    let mut payload = serde_json::to_value(&record).unwrap();
    payload["format_version"] = serde_json::json!(1);
    payload["agent_id"] = serde_json::json!("default");
    payload["chat_id"] = serde_json::json!("chat-legacy");
    payload
        .as_object_mut()
        .expect("history replacement object")
        .remove("chat_address");
    let change = EvergreenChatSyncAdapter::file_change(
        "op-legacy-history-replacement",
        &agent_chat("default", "chat-legacy"),
        format!("history_replacements/{}.json", record.record_id),
        payload,
        "device-a",
        hlc(250),
    );

    let target_data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let mut registry = SyncRegistry::new();
    registry
        .register(Arc::new(EvergreenChatSyncAdapter::new(
            target_data.path().to_path_buf(),
            "device-b",
        )))
        .unwrap();

    let first = registry.apply_change(&change, &merge_store).await.unwrap();
    assert_eq!(first.status, RegistryApplyStatus::Applied);
    let second = registry.apply_change(&change, &merge_store).await.unwrap();
    assert_eq!(second.status, RegistryApplyStatus::AlreadyApplied);
    assert!(!workspace_dir(target_data.path(), "default")
        .join("sessions")
        .join("chat-legacy")
        .join("history_replacements")
        .join(format!("{}.json", record.record_id))
        .exists());
}

#[tokio::test]
async fn outbound_skips_superseded_history_replacement_versions() {
    let source_data = tempfile::tempdir().unwrap();
    let source_workspace = workspace_dir(source_data.path(), "default");
    let record =
        create_history_replacement_record(&source_workspace, "chat-old-prompt", false).await;
    let record_path = WorkspaceSessionStore::new(&source_workspace)
        .history_replacement_path("chat-old-prompt", &record.record_id);
    let mut payload = serde_json::to_value(&record).unwrap();
    payload["prompt_version"] = serde_json::json!(1);
    std::fs::write(&record_path, serde_json::to_vec_pretty(&payload).unwrap()).unwrap();

    let exported = EvergreenChatSyncAdapter::new(source_data.path().to_path_buf(), "device-a")
        .export_records()
        .await
        .unwrap();
    let record_id = format!(
        "agent/default/chat-old-prompt/history_replacements/{}.json",
        record.record_id
    );
    assert!(
        exported.iter().all(|change| change.record_id != record_id),
        "superseded local replacement records must not block or re-enter sync"
    );
}

#[tokio::test]
async fn history_replacement_tombstone_exports_and_deletes_remote_record() {
    let source_data = tempfile::tempdir().unwrap();
    let source_workspace = workspace_dir(source_data.path(), "default");
    let record = create_history_replacement_record(&source_workspace, "chat-deleted", false).await;
    let source_store = WorkspaceSessionStore::new(&source_workspace);
    let record_path = source_store.history_replacement_path("chat-deleted", &record.record_id);
    rua_config::atomic_write_json(
        &source_store.history_replacement_tombstone_path("chat-deleted", &record.record_id),
        &serde_json::json!({
            "record_id": record.record_id,
            "deleted_at": "2026-07-26T00:02:00Z"
        }),
    )
    .unwrap();
    std::fs::remove_file(&record_path).unwrap();

    let source_adapter =
        EvergreenChatSyncAdapter::new(source_data.path().to_path_buf(), "device-a");
    let delete = source_adapter
        .export_records()
        .await
        .unwrap()
        .into_iter()
        .find(|change| {
            change.record_id
                == format!(
                    "agent/default/chat-deleted/history_replacements/{}.json",
                    record.record_id
                )
        })
        .expect("history replacement tombstone");
    assert_eq!(
        delete.kind,
        rua_sync::evergreen_chat_adapter::EVERGREEN_CHAT_KIND_FILE_DELETE
    );

    let target_data = tempfile::tempdir().unwrap();
    let target_workspace = workspace_dir(target_data.path(), "default");
    let target_record =
        create_history_replacement_record(&target_workspace, "chat-deleted", false).await;
    assert_eq!(target_record.record_id, record.record_id);
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let mut registry = SyncRegistry::new();
    registry
        .register(Arc::new(EvergreenChatSyncAdapter::new(
            target_data.path().to_path_buf(),
            "device-b",
        )))
        .unwrap();
    registry.apply_change(&delete, &merge_store).await.unwrap();
    assert!(!WorkspaceSessionStore::new(&target_workspace)
        .history_replacement_path("chat-deleted", &record.record_id)
        .exists());
}

#[tokio::test]
async fn inbound_writes_evergreen_files_without_legacy_flat_session() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = Arc::new(EvergreenChatSyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();

    let change = EvergreenChatSyncAdapter::file_change(
        "op-evergreen-session",
        &agent_chat("default", "chat-a"),
        "session",
        serde_json::json!({"id": "chat-a", "title": "Remote"}),
        "device-a",
        hlc(100),
    );

    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    let header_path = workspace_dir(data.path(), "default")
        .join("sessions")
        .join("chat-a")
        .join("session.json");
    let header: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(header_path).unwrap()).unwrap();
    assert_eq!(header["title"], "Remote");
    assert!(
        !workspace_dir(data.path(), "default")
            .join("sessions")
            .join("default_chat-a.json")
            .exists(),
        "Evergreen v4 apply must not create legacy flat session files"
    );
}

#[tokio::test]
async fn project_tombstone_rejects_later_evergreen_snapshot() {
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
    let adapter = Arc::new(EvergreenChatSyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();
    let address = ChatAddress::project("deleted-project", "chat-a");
    let change = EvergreenChatSyncAdapter::file_change(
        "late-project-session",
        &address,
        "session",
        serde_json::json!({"chat_id": "chat-a"}),
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
    assert!(!data
        .path()
        .join("projects/deleted-project/sessions/chat-a/session.json")
        .exists());
    assert_eq!(
        merge_store
            .get_record_cursor("evergreen_chat:project/deleted-project/chat-a/session")
            .unwrap()
            .unwrap()
            .state,
        RecordState::Tombstoned
    );
}

#[tokio::test]
async fn inbound_writes_prompt_state_as_evergreen_chat_file() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = Arc::new(EvergreenChatSyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();

    let change = EvergreenChatSyncAdapter::file_change(
        "op-evergreen-prompt-state",
        &agent_chat("default", "chat-a"),
        "prompt_state",
        serde_json::json!({"mode": "agent", "files": [{"id": "prompt-1"}]}),
        "device-a",
        hlc(100),
    );

    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    let prompt_state_path = workspace_dir(data.path(), "default")
        .join("prompt-state")
        .join("chat-a.json");
    let prompt_state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(prompt_state_path).unwrap()).unwrap();
    assert_eq!(prompt_state["mode"], "agent");
    assert_eq!(prompt_state["files"][0]["id"], "prompt-1");
}

#[tokio::test]
async fn inbound_message_log_unions_local_and_remote_entries() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let message_path = workspace_dir(data.path(), "default")
        .join("sessions")
        .join("chat-a")
        .join("messages")
        .join("000000000000.jsonl");
    rua_config::atomic_write(
        &message_path,
        br#"{"message_id":"local","sequence":1,"role":"user"}
"#,
    )
    .unwrap();
    let adapter = Arc::new(EvergreenChatSyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();

    let change = EvergreenChatSyncAdapter::file_change(
        "op-evergreen-messages",
        &agent_chat("default", "chat-a"),
        "messages/000000000000.jsonl",
        serde_json::Value::String(
            r#"{"message_id":"remote","sequence":2,"role":"assistant"}
"#
            .to_string(),
        ),
        "device-a",
        hlc(200),
    );

    registry.apply_change(&change, &merge_store).await.unwrap();

    let content = std::fs::read_to_string(message_path).unwrap();
    assert!(content.contains(r#""message_id":"local""#));
    assert!(content.contains(r#""message_id":"remote""#));
}

#[tokio::test]
async fn inbound_file_delete_removes_evergreen_file() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let header_path = workspace_dir(data.path(), "default")
        .join("sessions")
        .join("chat-a")
        .join("session.json");
    rua_config::atomic_write_json(
        &header_path,
        &serde_json::json!({"id": "chat-a", "title": "Local"}),
    )
    .unwrap();
    let adapter = Arc::new(EvergreenChatSyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();

    let change = EvergreenChatSyncAdapter::delete_change(
        "op-evergreen-delete",
        &agent_chat("default", "chat-a"),
        "session",
        "device-a",
        hlc(200),
    );

    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    assert!(!header_path.exists());
}

#[tokio::test]
async fn stale_file_delete_does_not_remove_newer_evergreen_file() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = Arc::new(EvergreenChatSyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();

    let newer = EvergreenChatSyncAdapter::file_change(
        "op-evergreen-newer-session",
        &agent_chat("default", "chat-a"),
        "session",
        serde_json::json!({"id": "chat-a", "title": "Fresh"}),
        "device-a",
        hlc(300),
    );
    registry.apply_change(&newer, &merge_store).await.unwrap();

    let stale_delete = EvergreenChatSyncAdapter::delete_change(
        "op-evergreen-stale-delete",
        &agent_chat("default", "chat-a"),
        "session",
        "device-a",
        hlc(200),
    );
    let result = registry
        .apply_change(&stale_delete, &merge_store)
        .await
        .unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Skipped);
    let header_path = workspace_dir(data.path(), "default")
        .join("sessions")
        .join("chat-a")
        .join("session.json");
    let header: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(header_path).unwrap()).unwrap();
    assert_eq!(header["title"], "Fresh");
}

#[tokio::test]
async fn stale_session_snapshot_does_not_rollback_newer_header() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = Arc::new(EvergreenChatSyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();

    let newer = EvergreenChatSyncAdapter::file_change(
        "op-evergreen-session-newer",
        &agent_chat("default", "chat-a"),
        "session",
        serde_json::json!({"id": "chat-a", "title": "Fresh"}),
        "device-a",
        hlc(200),
    );
    registry.apply_change(&newer, &merge_store).await.unwrap();

    let stale = EvergreenChatSyncAdapter::file_change(
        "op-evergreen-session-stale",
        &agent_chat("default", "chat-a"),
        "session",
        serde_json::json!({"id": "chat-a", "title": "Old"}),
        "device-a",
        hlc(100),
    );
    let result = registry.apply_change(&stale, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Skipped);
    let header_path = workspace_dir(data.path(), "default")
        .join("sessions")
        .join("chat-a")
        .join("session.json");
    let header: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(header_path).unwrap()).unwrap();
    assert_eq!(header["title"], "Fresh");
}

#[tokio::test]
async fn delayed_session_publication_preserves_confirmed_authority_epoch() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = EvergreenChatSyncAdapter::new(data.path().into(), "desktop");
    let address = agent_chat("default", "handoff");
    let workspace = workspace_dir(data.path(), "default");
    let store = WorkspaceSessionStore::new(&workspace);
    let confirmed = serde_json::json!({"device_id":"desktop", "authority_epoch":hlc(200).to_sortable_string(), "base_manifest_hash":"confirmed"});
    let mut header = ChatSessionHeader::new(ChatMetadata {
        chat_id: "handoff".into(),
        agent_id: "default".into(),
        title: "Before sync".into(),
        created_at: "2026-09-30T00:00:00Z".into(),
        last_activity_at: "2026-09-30T00:00:00Z".into(),
        pinned: false,
        archived: false,
        preview: None,
    });
    header
        .extra
        .insert("authority_placement".into(), confirmed.clone());
    store.save_chat_header(&header).await.unwrap();
    // A later publication timestamp does not make its captured placement newer.
    let mut incoming = serde_json::to_value(&header).unwrap();
    incoming["metadata"]["title"] = serde_json::json!("Synced title");
    incoming["authority_placement"] = serde_json::json!({"device_id":"phone", "authority_epoch":hlc(100).to_sortable_string(), "base_manifest_hash":"old"});
    let change = EvergreenChatSyncAdapter::file_change(
        "delayed",
        &address,
        "session",
        incoming,
        "phone",
        hlc(300),
    );
    adapter.apply_change(&change, &merge).await.unwrap();
    let actual = store.load_chat_header("handoff").await.unwrap().unwrap();
    assert_eq!(actual.extra["authority_placement"], confirmed);
    assert_eq!(actual.metadata.title, "Synced title");
    for (index, placement) in [
        None,
        Some(serde_json::json!({"device_id":"phone", "authority_epoch":hlc(200).to_sortable_string(), "base_manifest_hash":"conflicting"})),
    ].into_iter().enumerate() {
        let mut payload = serde_json::to_value(&actual).unwrap();
        match placement {
            Some(value) => { payload["authority_placement"] = value; }
            None => { payload.as_object_mut().unwrap().remove("authority_placement"); }
        }
        let change = EvergreenChatSyncAdapter::file_change(
            format!("delayed-{index}"), &address, "session", payload, "phone", hlc(400 + index as u64),
        );
        adapter.apply_change(&change, &merge).await.unwrap();
        assert_eq!(store.load_chat_header("handoff").await.unwrap().unwrap().extra["authority_placement"], confirmed);
    }
    let next = serde_json::json!({"device_id":"phone", "authority_epoch":hlc(500).to_sortable_string(), "base_manifest_hash":"next"});
    let mut payload = serde_json::to_value(&actual).unwrap();
    payload["authority_placement"] = next.clone();
    adapter
        .apply_change(
            &EvergreenChatSyncAdapter::file_change(
                "next",
                &address,
                "session",
                payload,
                "phone",
                hlc(600),
            ),
            &merge,
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .load_chat_header("handoff")
            .await
            .unwrap()
            .unwrap()
            .extra["authority_placement"],
        next
    );
}

#[tokio::test]
async fn older_message_log_still_unions_new_entries() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let message_path = workspace_dir(data.path(), "default")
        .join("sessions")
        .join("chat-a")
        .join("messages")
        .join("000000000000.jsonl");
    let adapter = Arc::new(EvergreenChatSyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();

    let newer = EvergreenChatSyncAdapter::file_change(
        "op-evergreen-message-newer",
        &agent_chat("default", "chat-a"),
        "messages/000000000000.jsonl",
        serde_json::Value::String(
            r#"{"message_id":"newer","sequence":2,"role":"assistant"}
"#
            .to_string(),
        ),
        "device-a",
        hlc(200),
    );
    registry.apply_change(&newer, &merge_store).await.unwrap();

    let older = EvergreenChatSyncAdapter::file_change(
        "op-evergreen-message-older",
        &agent_chat("default", "chat-a"),
        "messages/000000000000.jsonl",
        serde_json::Value::String(
            r#"{"message_id":"older","sequence":1,"role":"user"}
"#
            .to_string(),
        ),
        "device-a",
        hlc(100),
    );
    let result = registry.apply_change(&older, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    let content = std::fs::read_to_string(message_path).unwrap();
    assert!(content.contains(r#""message_id":"newer""#));
    assert!(content.contains(r#""message_id":"older""#));
}

#[tokio::test]
async fn inbound_projection_cache_record_is_ignored_and_does_not_poison_inbox() {
    let data = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let adapter = Arc::new(EvergreenChatSyncAdapter::new(
        data.path().to_path_buf(),
        "device-b",
    ));
    let mut registry = SyncRegistry::new();
    registry.register(adapter).unwrap();

    let change = EvergreenChatSyncAdapter::file_change(
        "op-evergreen-projection-cache",
        &agent_chat("default", "chat-a"),
        "projection_cache/cache.json",
        serde_json::json!({"estimated_tokens": 1234}),
        "device-a",
        hlc(300),
    );

    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    assert!(merge_store.has_applied_op(&change.op_id).unwrap());
    assert!(
        !workspace_dir(data.path(), "default")
            .join("sessions")
            .join("chat-a")
            .join("projection_cache")
            .join("cache.json")
            .exists(),
        "remote projection cache must not be materialized locally"
    );
}

#[test]
fn snapshot_changes_use_v4_domain_and_stable_record_ids() {
    let data = tempfile::tempdir().unwrap();
    let workspace = workspace_dir(data.path(), "default");
    let chat_dir = workspace.join("sessions").join("chat-a");
    std::fs::create_dir_all(&chat_dir).unwrap();
    rua_config::atomic_write_json(
        &chat_dir.join("session.json"),
        &serde_json::json!({"id": "chat-a"}),
    )
    .unwrap();

    let snapshots = collect_evergreen_chat_file_snapshots(&workspace, "chat-a", "device-a");
    assert_eq!(snapshots.len(), 1);

    let changes = EvergreenChatSyncAdapter::snapshot_changes_for_chat(
        &workspace,
        &agent_chat("default", "chat-a"),
        "device-a",
        hlc(1),
    )
    .unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].domain, EVERGREEN_CHAT_DOMAIN);
    assert_eq!(changes[0].record_id, "agent/default/chat-a/session");
    assert_eq!(changes[0].kind, EVERGREEN_CHAT_KIND_FILE_SNAPSHOT);
}

#[test]
fn snapshot_changes_include_prompt_state_without_session_directory() {
    let data = tempfile::tempdir().unwrap();
    let workspace = workspace_dir(data.path(), "default");
    rua_config::atomic_write_json(
        &workspace.join("prompt-state").join("chat-a.json"),
        &serde_json::json!({"mode": "code"}),
    )
    .unwrap();

    let snapshots = collect_evergreen_chat_file_snapshots(&workspace, "chat-a", "device-a");
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].relative, "prompt_state");

    let changes = EvergreenChatSyncAdapter::snapshot_changes_for_chat(
        &workspace,
        &agent_chat("default", "chat-a"),
        "device-a",
        hlc(1),
    )
    .unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].record_id, "agent/default/chat-a/prompt_state");
    assert_eq!(changes[0].payload["mode"], "code");
}

#[tokio::test]
async fn mid_turn_checkpoint_snapshot_contains_pointer_and_source_history() {
    let data = tempfile::tempdir().unwrap();
    let workspace = workspace_dir(data.path(), "default");
    let chat_id = "chat-mid-turn";
    let store = WorkspaceSessionStore::new(&workspace);
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "default".to_string(),
            title: "Mid-turn checkpoint".to_string(),
            created_at: "2026-07-24T00:00:00Z".to_string(),
            last_activity_at: "2026-07-24T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .unwrap();
    let user = conversation_message(
        "user-active",
        "user",
        serde_json::json!([{"type": "text", "text": "run the tool"}]),
    );
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: "turn-active".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-07-24T00:00:00Z".to_string(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![user.clone()],
            },
        )
        .await
        .unwrap();
    TurnWriter::new(&store, chat_id, "turn-active")
        .append_in_progress_output(
            vec![
                conversation_message(
                    "assistant-tool",
                    "assistant",
                    serde_json::json!([{
                        "type": "tool_use",
                        "id": "call-1",
                        "name": "bash",
                        "arguments": {"command": "pwd"}
                    }]),
                ),
                conversation_message(
                    "tool-result",
                    "system",
                    serde_json::json!([{
                        "type": "tool_result",
                        "call_id": "call-1",
                        "content": [{"type": "text", "text": "/workspace"}]
                    }]),
                ),
            ],
            Some("2026-07-24T00:00:01Z"),
        )
        .await
        .unwrap();
    let projection_state = store.checkpoint_projection_state(chat_id, 3).await.unwrap();
    store
        .publish_summary_checkpoint(
            chat_id,
            None,
            SummaryCheckpointDraft {
                checkpoint_id: "checkpoint-mid-turn".to_string(),
                previous_checkpoint_id: None,
                covered_turn_range: CheckpointTurnRange {
                    start_turn_ordinal: 1,
                    end_turn_ordinal: 1,
                },
                covered_sequence_range: CheckpointSequenceRange {
                    start_sequence: 1,
                    end_sequence: 3,
                },
                source_sequence_range: CheckpointSequenceRange {
                    start_sequence: 1,
                    end_sequence: 3,
                },
                frontier: projection_state.frontier,
                retained_sequence_ranges: projection_state.retained_sequence_ranges,
                projection_lineage: projection_state.projection_lineage,
                summary: "tool step completed".to_string(),
                token_estimate: 4,
                model: CheckpointModelIdentity {
                    provider: "test".to_string(),
                    model: "test-model".to_string(),
                },
                created_at: "2026-07-24T00:01:00Z".to_string(),
            },
        )
        .await
        .unwrap();

    let snapshots = collect_evergreen_chat_file_snapshots(&workspace, chat_id, "device-a");
    let session = snapshots
        .iter()
        .find(|snapshot| snapshot.relative == "session")
        .expect("session snapshot");
    assert_eq!(
        session.payload["active_checkpoint_id"],
        "checkpoint-mid-turn"
    );
    let turn_index = snapshots
        .iter()
        .find(|snapshot| snapshot.relative == "turn_index")
        .expect("turn index snapshot");
    assert_eq!(
        turn_index.payload["turns"][0]["lifecycle"]["status"],
        "in_progress"
    );
    assert!(turn_index.payload["turns"][0]["metadata"]
        .get("model_call_boundaries")
        .is_none());
    let message_snapshot = snapshots
        .iter()
        .find(|snapshot| snapshot.relative.starts_with("messages/"))
        .expect("message log snapshot");
    let message_log = message_snapshot.payload.as_str().expect("jsonl payload");
    assert!(message_log.contains("user-active"));
    assert!(message_log.contains("assistant-tool"));
    assert!(message_log.contains("tool-result"));
    let checkpoint = snapshots
        .iter()
        .find(|snapshot| snapshot.relative == "checkpoints/checkpoint-mid-turn.json")
        .expect("checkpoint snapshot");
    assert_eq!(checkpoint.payload["frontier"]["end_sequence"], 3);
}
