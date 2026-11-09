use rua_session::{
    ChatMetadata, ChatSessionHeader, CheckpointModelIdentity, CheckpointSequenceRange,
    CheckpointTurnRange, ContextTargetBand, EarlyBackgroundCompactionCoordinator,
    EarlyBackgroundCompactionDecision, EarlyBackgroundCompactionInput,
    EarlyBackgroundCompactionPolicy, EarlyBackgroundCompactionPriority, NewConversationMessage,
    NewConversationTurn, ProjectionEntryMode, ProjectionFitStatus, ProjectionMessageGroupEstimate,
    ProjectionPlanner, ProjectionPlanningInput, SummaryCheckpointDraft, TurnLifecycle,
    WorkspaceSessionStore,
};
use serde_json::json;

fn band() -> ContextTargetBand {
    ContextTargetBand {
        upper_target_tokens: 700,
        emergency_limit_tokens: 1_000,
        tail_reserve_tokens: 100,
    }
}

fn turn(ordinal: u64, tokens: u64) -> ProjectionMessageGroupEstimate {
    ProjectionMessageGroupEstimate {
        turn_id: format!("turn-{ordinal}"),
        turn_ordinal: ordinal,
        first_sequence: ordinal * 2 - 1,
        last_sequence: ordinal * 2,
        token_estimate: tokens,
        foldable: true,
        retained_sequence_ranges: Vec::new(),
    }
}

fn nonfoldable_turn(ordinal: u64, tokens: u64) -> ProjectionMessageGroupEstimate {
    ProjectionMessageGroupEstimate {
        foldable: false,
        ..turn(ordinal, tokens)
    }
}

fn plan(current_projection_tokens: u64) -> rua_session::ProjectionPlan {
    ProjectionPlanner::new(band()).plan(ProjectionPlanningInput {
        entry_mode: ProjectionEntryMode::EarlyBackgroundCompaction,
        current_projection_tokens,
        checkpoint_tokens: 120,
        system_context_tokens: 80,
        pending_user_input_tokens: 0,
        message_groups: vec![turn(1, 300), turn(2, 280), turn(3, 260)],
    })
}

#[test]
fn early_background_compaction_policy_schedules_only_after_target_band_pressure() {
    let policy = EarlyBackgroundCompactionPolicy;

    assert_eq!(
        policy.evaluate(EarlyBackgroundCompactionInput { plan: plan(650) }),
        EarlyBackgroundCompactionDecision::DoNothing
    );

    assert_eq!(
        plan(800).status,
        ProjectionFitStatus::FitsEmergencyButNeedsMaintenance
    );
    assert_eq!(
        policy.evaluate(EarlyBackgroundCompactionInput { plan: plan(800) }),
        EarlyBackgroundCompactionDecision::ScheduleCheckpoint {
            priority: EarlyBackgroundCompactionPriority::Low,
            reason: "target_band_exceeded".to_string(),
        }
    );

    assert_eq!(plan(1_100).status, ProjectionFitStatus::RequiresCheckpoint);
    assert_eq!(
        policy.evaluate(EarlyBackgroundCompactionInput { plan: plan(1_100) }),
        EarlyBackgroundCompactionDecision::ScheduleCheckpoint {
            priority: EarlyBackgroundCompactionPriority::High,
            reason: "checkpoint_required".to_string(),
        }
    );

    assert_eq!(
        policy.evaluate(EarlyBackgroundCompactionInput {
            plan: ProjectionPlanner::new(band()).plan(ProjectionPlanningInput {
                entry_mode: ProjectionEntryMode::EarlyBackgroundCompaction,
                current_projection_tokens: 1_600,
                checkpoint_tokens: 120,
                system_context_tokens: 80,
                pending_user_input_tokens: 0,
                message_groups: vec![turn(1, 100)],
            }),
        }),
        EarlyBackgroundCompactionDecision::SignalEmergencyFallbackRequired
    );

    assert_eq!(
        policy.evaluate(EarlyBackgroundCompactionInput {
            plan: ProjectionPlanner::new(band()).plan(ProjectionPlanningInput {
                entry_mode: ProjectionEntryMode::EarlyBackgroundCompaction,
                current_projection_tokens: 1_600,
                checkpoint_tokens: 120,
                system_context_tokens: 80,
                pending_user_input_tokens: 0,
                message_groups: vec![nonfoldable_turn(1, 100)],
            }),
        }),
        EarlyBackgroundCompactionDecision::SignalEmergencyFallbackRequired
    );
}

#[tokio::test]
async fn foreground_compact_preempts_only_matching_early_compaction_job() {
    let coordinator = EarlyBackgroundCompactionCoordinator::new();
    let chat_a = coordinator.start_job("chat-a");
    let chat_b = coordinator.start_job("chat-b");

    assert!(!chat_a.is_cancelled());
    assert!(!chat_b.is_cancelled());
    assert!(coordinator.can_publish(&chat_a));

    coordinator.preempt_for_foreground("chat-a");

    tokio::time::timeout(std::time::Duration::from_millis(100), chat_a.cancelled())
        .await
        .expect("preempted job receives cancellation signal");
    assert!(chat_a.is_cancelled());
    assert!(!coordinator.can_publish(&chat_a));
    assert!(!chat_b.is_cancelled());
    assert!(coordinator.can_publish(&chat_b));

    let new_chat_a = coordinator.start_job("chat-a");
    assert!(!new_chat_a.is_cancelled());
    assert!(coordinator.can_publish(&new_chat_a));
    assert!(
        new_chat_a.generation > chat_a.generation,
        "new job should use a newer generation token"
    );
}

#[test]
fn early_compaction_job_does_not_preempt_existing_job_for_same_chat() {
    let coordinator = EarlyBackgroundCompactionCoordinator::new();
    let first = coordinator
        .try_start_job("chat-a")
        .expect("first job starts");
    assert!(
        coordinator.try_start_job("chat-a").is_none(),
        "new turns must not cancel an in-flight checkpoint job"
    );
    assert!(coordinator.can_publish(&first));

    coordinator.finish_job(&first);
    let second = coordinator
        .try_start_job("chat-a")
        .expect("next job starts after first finishes");
    assert!(!second.is_cancelled());
}

fn chat_header(chat_id: &str) -> ChatSessionHeader {
    ChatSessionHeader::new(ChatMetadata {
        chat_id: chat_id.to_string(),
        agent_id: "agent-1".to_string(),
        title: "Evergreen".to_string(),
        created_at: "2026-06-19T00:00:00Z".to_string(),
        last_activity_at: "2026-06-19T00:00:00Z".to_string(),
        pinned: false,
        archived: false,
        preview: None,
    })
}

async fn checkpoint(
    store: &WorkspaceSessionStore,
    chat_id: &str,
    id: &str,
    previous: Option<&str>,
    summary: &str,
) -> SummaryCheckpointDraft {
    let projection_state = store.checkpoint_projection_state(chat_id, 2).await.unwrap();
    SummaryCheckpointDraft {
        checkpoint_id: id.to_string(),
        previous_checkpoint_id: previous.map(str::to_string),
        covered_turn_range: CheckpointTurnRange {
            start_turn_ordinal: 1,
            end_turn_ordinal: 1,
        },
        covered_sequence_range: CheckpointSequenceRange {
            start_sequence: 1,
            end_sequence: 2,
        },
        source_sequence_range: CheckpointSequenceRange {
            start_sequence: 1,
            end_sequence: 2,
        },
        frontier: projection_state.frontier,
        retained_sequence_ranges: projection_state.retained_sequence_ranges,
        projection_lineage: projection_state.projection_lineage,
        summary: summary.to_string(),
        token_estimate: 16,
        model: CheckpointModelIdentity {
            provider: "manual".to_string(),
            model: "compact".to_string(),
        },
        created_at: "2026-06-19T00:01:00Z".to_string(),
    }
}

#[tokio::test]
async fn stale_early_compaction_checkpoint_publish_is_rejected_by_cas() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&chat_header("chat-1"))
        .await
        .unwrap();
    store
        .append_conversation_turn(
            "chat-1",
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-06-19T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("variant-1".to_string()),
                messages: vec![
                    NewConversationMessage {
                        message_id: "user-1".to_string(),
                        role: "user".to_string(),
                        created_at: "2026-06-19T00:00:00Z".to_string(),
                        status: "completed".to_string(),
                        content: json!([{"type": "text", "text": "hello"}]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                    NewConversationMessage {
                        message_id: "assistant-1".to_string(),
                        role: "assistant".to_string(),
                        created_at: "2026-06-19T00:00:01Z".to_string(),
                        status: "completed".to_string(),
                        content: json!([{"type": "text", "text": "hi"}]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                ],
            },
        )
        .await
        .unwrap();

    let coordinator = EarlyBackgroundCompactionCoordinator::new();
    let background = coordinator.start_job("chat-1");
    let foreground = store
        .publish_summary_checkpoint(
            "chat-1",
            None,
            checkpoint(&store, "chat-1", "foreground", None, "foreground").await,
        )
        .await
        .unwrap();
    coordinator.preempt_for_foreground("chat-1");
    assert!(background.is_cancelled());

    let stale = store
        .publish_summary_checkpoint(
            "chat-1",
            None,
            checkpoint(&store, "chat-1", "background", None, "background").await,
        )
        .await;

    assert!(stale.is_err(), "stale background publish should fail CAS");
    let active = store
        .load_active_summary_checkpoint("chat-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(active.checkpoint_id, foreground.checkpoint_id);
    assert_eq!(active.summary, "foreground");
}
