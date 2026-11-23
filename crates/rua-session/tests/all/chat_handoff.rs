use rua_session::chat_handoff::{ChatHandoffPhase, ChatHandoffRecord};
use rua_session::{ChatMetadata, ChatSessionHeader, WorkspaceSessionStore};
use serde_json::{json, Value};

struct Fixture {
    root: tempfile::TempDir,
    store: WorkspaceSessionStore,
    record: ChatHandoffRecord,
    candidate: Value,
}

impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let store = WorkspaceSessionStore::new(root.path());
        let old = json!({"device_id":"device-a", "authority_epoch":"000001-a", "base_manifest_hash":"sha256:original"});
        let next = json!({"device_id":"device-c", "authority_epoch":"000002-c", "base_manifest_hash":"sha256:original"});
        let mut header = ChatSessionHeader::new(ChatMetadata {
            chat_id: "handoff-chat".into(),
            agent_id: "agent-a".into(),
            title: "handoff".into(),
            created_at: "2026-11-22T00:00:00Z".into(),
            last_activity_at: "2026-11-22T00:00:00Z".into(),
            pinned: false,
            archived: false,
            preview: None,
        });
        header
            .extra
            .insert("authority_placement".into(), old.clone());
        header.extra.insert("hook_configuration".into(), json!({
            "epoch":"config-old", "selected_agent_id":"agent-a", "approval_mode":"ask_first",
            "capability_configuration":{"execution_directory":{"device_id":"device-a", "path":"/a/photos", "managed":false}},
        }));
        store.save_chat_header(&header).await.unwrap();
        Self {
            root,
            store,
            record: ChatHandoffRecord {
                handoff_id: "handoff-1".into(),
                chat_id: "handoff-chat".into(),
                old_placement: old,
                new_placement: next,
                expected_configuration_epoch: "config-old".into(),
                candidate_configuration_epoch: "config-next".into(),
                requirements: json!({"manifest":"original", "permissions":[]}),
                phase: ChatHandoffPhase::Prepared,
                extra: Default::default(),
            },
            candidate: json!({
                "epoch":"config-next", "selected_agent_id":"agent-a", "approval_mode":"ask_first",
                "capability_configuration":{"execution_directory":{"device_id":"device-c", "path":"/c/work", "managed":false}},
            }),
        }
    }

    async fn release(&self) {
        self.store
            .prepare_chat_handoff(&self.record, Some(&self.candidate))
            .await
            .unwrap();
        self.store
            .release_chat_handoff("handoff-chat", "handoff-1")
            .unwrap();
    }
}

#[tokio::test]
async fn preparation_freezes_one_identity_before_directory_resolution_without_publishing_candidate()
{
    let f = Fixture::new().await;
    f.store.prepare_chat_handoff(&f.record, None).await.unwrap();
    assert!(f
        .store
        .release_chat_handoff("handoff-chat", "handoff-1")
        .is_err());
    f.store
        .prepare_chat_handoff(&f.record, Some(&f.candidate))
        .await
        .unwrap();
    assert_eq!(
        f.store
            .load_chat_header("handoff-chat")
            .await
            .unwrap()
            .unwrap()
            .extra["hook_configuration"]["epoch"],
        "config-old"
    );
    assert_eq!(
        f.store
            .load_configuration_snapshot("handoff-chat", "config-next")
            .await
            .unwrap(),
        Some(f.candidate.clone())
    );
    let mut changed = f.candidate.clone();
    changed["capability_configuration"]["execution_directory"]["path"] = json!("/another");
    assert!(f
        .store
        .prepare_chat_handoff(&f.record, Some(&changed))
        .await
        .is_err());
    assert_eq!(
        f.store
            .prepare_chat_handoff(&f.record, Some(&f.candidate))
            .await
            .unwrap(),
        f.record
    );
}

#[tokio::test]
async fn background_checkpoint_rechecks_handoff_after_waiting_for_the_storage_lock() {
    let f = Fixture::new().await;
    f.store
        .append_conversation_turn(
            "handoff-chat",
            rua_session::NewConversationTurn {
                turn_id: "turn-1".into(),
                lifecycle: rua_session::TurnLifecycle {
                    user_submitted_at: "2026-11-22T00:00:00Z".into(),
                    assistant_started_at: Some("2026-11-22T00:00:00Z".into()),
                    assistant_completed_at: Some("2026-11-22T00:00:01Z".into()),
                    status: "completed".into(),
                },
                selected_variant_id: Some("assistant-1".into()),
                messages: vec![rua_session::NewConversationMessage {
                    message_id: "assistant-1".into(),
                    role: "assistant".into(),
                    created_at: "2026-11-22T00:00:01Z".into(),
                    status: "completed".into(),
                    content: json!([{"type":"text","text":"history"}]),
                    provider: None,
                    tool: None,
                    metadata: Default::default(),
                }],
            },
        )
        .await
        .unwrap();
    let projection = f
        .store
        .checkpoint_projection_state("handoff-chat", 1)
        .await
        .unwrap();
    let draft = rua_session::SummaryCheckpointDraft {
        checkpoint_id: "late-background".into(),
        previous_checkpoint_id: None,
        covered_turn_range: rua_session::CheckpointTurnRange {
            start_turn_ordinal: 1,
            end_turn_ordinal: 1,
        },
        covered_sequence_range: rua_session::CheckpointSequenceRange {
            start_sequence: 1,
            end_sequence: 1,
        },
        source_sequence_range: rua_session::CheckpointSequenceRange {
            start_sequence: 1,
            end_sequence: 1,
        },
        frontier: projection.frontier,
        retained_sequence_ranges: projection.retained_sequence_ranges,
        projection_lineage: projection.projection_lineage,
        summary: "late summary".into(),
        token_estimate: 3,
        model: rua_session::CheckpointModelIdentity {
            provider: "test".into(),
            model: "test".into(),
        },
        created_at: "2026-11-22T00:00:02Z".into(),
    };
    let guard = f.store.lock_chat_storage("handoff-chat").await;
    let mut prepare = Box::pin(f.store.prepare_chat_handoff(&f.record, Some(&f.candidate)));
    assert!(futures::poll!(prepare.as_mut()).is_pending());
    let mut publish =
        Box::pin(
            f.store
                .publish_summary_checkpoint_if("handoff-chat", None, draft, || {
                    f.store
                        .chat_handoff_allows_history_write("handoff-chat", "device-a")
                        .unwrap()
                }),
        );
    assert!(futures::poll!(publish.as_mut()).is_pending());
    drop(guard);
    prepare.await.unwrap();
    assert!(publish.await.unwrap().is_none());
    assert!(f
        .store
        .read_summary_checkpoint("handoff-chat", "late-background")
        .await
        .unwrap()
        .is_none());
    assert!(f
        .store
        .load_chat_header("handoff-chat")
        .await
        .unwrap()
        .unwrap()
        .active_checkpoint_id
        .is_none());
    f.store
        .abort_chat_handoff("handoff-chat", "handoff-1")
        .await
        .unwrap();
    assert!(f
        .store
        .chat_handoff_allows_history_write("handoff-chat", "device-a")
        .unwrap());
    let next = ChatHandoffRecord {
        handoff_id: "handoff-2".into(),
        ..f.record.clone()
    };
    f.store
        .prepare_chat_handoff(&next, Some(&f.candidate))
        .await
        .unwrap();
    f.store
        .release_chat_handoff("handoff-chat", "handoff-2")
        .unwrap();
    assert!(!f
        .store
        .chat_handoff_allows_history_write("handoff-chat", "device-a")
        .unwrap());
    f.store
        .activate_chat_handoff("handoff-chat", "handoff-2")
        .await
        .unwrap();
    assert!(!f
        .store
        .chat_handoff_allows_history_write("handoff-chat", "device-a")
        .unwrap());
    assert!(f
        .store
        .chat_handoff_allows_history_write("handoff-chat", "device-c")
        .unwrap());
}

#[tokio::test]
async fn release_survives_restart_cannot_abort_and_activation_reuses_captured_epoch() {
    let f = Fixture::new().await;
    f.release().await;
    let reopened = WorkspaceSessionStore::new(f.root.path());
    assert_eq!(
        reopened
            .load_chat_handoff("handoff-chat")
            .await
            .unwrap()
            .unwrap()
            .phase,
        ChatHandoffPhase::Released
    );
    assert!(reopened
        .abort_chat_handoff("handoff-chat", "handoff-1")
        .await
        .is_err());
    let mut replacement = f.record.clone();
    replacement.handoff_id = "handoff-2".into();
    assert!(reopened
        .prepare_chat_handoff(&replacement, None)
        .await
        .is_err());
    let accepted = reopened
        .activate_chat_handoff("handoff-chat", "handoff-1")
        .await
        .unwrap();
    assert_eq!(accepted.phase, ChatHandoffPhase::Activated);
    assert_eq!(
        reopened
            .activate_chat_handoff("handoff-chat", "handoff-1")
            .await
            .unwrap(),
        accepted
    );
    let header = reopened
        .load_chat_header("handoff-chat")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(header.extra["authority_placement"], f.record.new_placement);
    assert_eq!(header.extra["hook_configuration"], f.candidate);
    assert_eq!(header.extra["execution_directory"]["path"], "/c/work");
}

#[tokio::test]
async fn released_commit_recovers_after_each_head_and_placement_disk_boundary() {
    for committed_files in 0..=2 {
        let f = Fixture::new().await;
        f.release().await;
        if committed_files >= 1 {
            let path = f.store.configuration_head_path("handoff-chat");
            let mut head: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            head["epoch"] = json!("config-next");
            head["execution_directory"] =
                f.candidate["capability_configuration"]["execution_directory"].clone();
            rua_config::atomic_write_json(&path, &head).unwrap();
        }
        if committed_files >= 2 {
            let mut header = f
                .store
                .load_chat_header("handoff-chat")
                .await
                .unwrap()
                .unwrap();
            header
                .extra
                .insert("authority_placement".into(), f.record.new_placement.clone());
            rua_config::atomic_write_json(
                &f.store.chat_header_path("handoff-chat"),
                &json!(header),
            )
            .unwrap();
        }
        let reopened = WorkspaceSessionStore::new(f.root.path());
        assert_eq!(
            reopened
                .activate_chat_handoff("handoff-chat", "handoff-1")
                .await
                .unwrap()
                .phase,
            ChatHandoffPhase::Activated
        );
        let header = reopened
            .load_chat_header("handoff-chat")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(header.extra["authority_placement"], f.record.new_placement);
        assert_eq!(header.extra["hook_configuration"], f.candidate);
    }
}

#[tokio::test]
async fn unrelated_configuration_is_not_overwritten_by_recovery() {
    let f = Fixture::new().await;
    f.release().await;
    let mut unrelated = f.candidate.clone();
    unrelated["epoch"] = json!("config-unrelated");
    f.store
        .replace_chat_configuration("handoff-chat", "config-old", unrelated.clone())
        .await
        .unwrap();
    assert!(f
        .store
        .activate_chat_handoff("handoff-chat", "handoff-1")
        .await
        .is_err());
    assert_eq!(
        f.store
            .load_chat_header("handoff-chat")
            .await
            .unwrap()
            .unwrap()
            .extra["hook_configuration"],
        unrelated
    );
    assert_eq!(
        f.store
            .load_chat_handoff("handoff-chat")
            .await
            .unwrap()
            .unwrap()
            .phase,
        ChatHandoffPhase::Released
    );
}

#[tokio::test]
async fn delayed_activation_receipt_preserves_new_writers_later_configuration_after_reopen() {
    let f = Fixture::new().await;
    f.release().await;
    let released = f
        .store
        .load_chat_handoff("handoff-chat")
        .await
        .unwrap()
        .unwrap();
    f.store
        .activate_chat_handoff("handoff-chat", "handoff-1")
        .await
        .unwrap();
    let mut later = f.candidate.clone();
    later["epoch"] = json!("config-after-activation");
    later["approval_mode"] = json!("hands_off");
    later["capability_configuration"]["execution_directory"]["path"] = json!("/c/another");
    f.store
        .replace_chat_configuration("handoff-chat", "config-next", later.clone())
        .await
        .unwrap();
    // A received C's persisted state while its own journal still awaits C's ACK.
    rua_config::atomic_write_json(&f.store.chat_handoff_path("handoff-chat"), &json!(released))
        .unwrap();
    let reopened = WorkspaceSessionStore::new(f.root.path());
    assert_eq!(
        reopened
            .activate_chat_handoff("handoff-chat", "handoff-1")
            .await
            .unwrap()
            .phase,
        ChatHandoffPhase::Activated
    );
    let header = reopened
        .load_chat_header("handoff-chat")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(header.extra["hook_configuration"], later);
    assert_eq!(header.extra["execution_directory"]["path"], "/c/another");
    assert_eq!(header.extra["authority_placement"], f.record.new_placement);
    let next = ChatHandoffRecord {
        handoff_id: "return-to-a".into(),
        old_placement: f.record.new_placement,
        new_placement: json!({"device_id":"device-a", "authority_epoch":"000003-a", "base_manifest_hash":"sha256:original"}),
        expected_configuration_epoch: "config-after-activation".into(),
        candidate_configuration_epoch: "config-return".into(),
        ..f.record
    };
    reopened.prepare_chat_handoff(&next, None).await.unwrap();
}

#[tokio::test]
async fn activation_rechecks_placement_after_acquiring_chat_storage_lock() {
    let f = Fixture::new().await;
    f.release().await;
    let guard = f.store.lock_chat_storage("handoff-chat").await;
    let task = tokio::spawn({
        let store = f.store.clone();
        async move {
            store
                .activate_chat_handoff("handoff-chat", "handoff-1")
                .await
        }
    });
    tokio::task::yield_now().await;
    assert!(!task.is_finished());
    let mut header = f
        .store
        .load_chat_header("handoff-chat")
        .await
        .unwrap()
        .unwrap();
    header.extra["authority_placement"]["authority_epoch"] = json!("000003-d");
    rua_config::atomic_write_json(&f.store.chat_header_path("handoff-chat"), &json!(header))
        .unwrap();
    drop(guard);
    assert!(task.await.unwrap().is_err());
    assert_eq!(
        f.store
            .load_chat_header("handoff-chat")
            .await
            .unwrap()
            .unwrap()
            .extra["hook_configuration"]["epoch"],
        "config-old"
    );
}

#[tokio::test]
async fn only_prepared_requirements_can_refresh_and_unknown_journal_fields_survive() {
    let mut f = Fixture::new().await;
    f.record
        .extra
        .insert("future_field".into(), json!({"preserve":true}));
    f.store
        .prepare_chat_handoff(&f.record, Some(&f.candidate))
        .await
        .unwrap();
    let refreshed = f
        .store
        .refresh_chat_handoff_requirements(
            "handoff-chat",
            "handoff-1",
            &json!({"permissions":["new-revocation"]}),
        )
        .await
        .unwrap();
    assert_eq!(refreshed.candidate_configuration_epoch, "config-next");
    assert_eq!(refreshed.extra, f.record.extra);
    f.store
        .release_chat_handoff("handoff-chat", "handoff-1")
        .unwrap();
    assert!(f
        .store
        .refresh_chat_handoff_requirements("handoff-chat", "handoff-1", &json!({}))
        .await
        .is_err());
    assert_eq!(
        f.store
            .load_chat_handoff("handoff-chat")
            .await
            .unwrap()
            .unwrap()
            .requirements,
        refreshed.requirements
    );
}
