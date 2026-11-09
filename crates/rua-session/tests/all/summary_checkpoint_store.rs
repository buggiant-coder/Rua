use rua_session::{
    ChatMetadata, ChatSessionHeader, CheckpointFrontier, CheckpointModelIdentity,
    CheckpointSequenceRange, CheckpointTurnRange, NewConversationMessage, NewConversationTurn,
    SummaryCheckpointDraft, TurnLifecycle, WorkspaceSessionStore,
};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Barrier;

#[test]
fn checkpoint_frontier_reads_legacy_kind_but_new_frontiers_omit_it() {
    let legacy: CheckpointFrontier = serde_json::from_value(json!({
        "kind": "model_call_boundary",
        "turn_id": "turn-1",
        "end_sequence": 3
    }))
    .unwrap();
    assert_eq!(legacy.legacy_kind.as_deref(), Some("model_call_boundary"));
    assert_eq!(
        serde_json::to_value(&legacy).unwrap()["kind"],
        "model_call_boundary"
    );

    let current = CheckpointFrontier {
        legacy_kind: None,
        turn_id: "turn-1".to_string(),
        end_sequence: 3,
    };
    assert!(serde_json::to_value(&current)
        .unwrap()
        .get("kind")
        .is_none());
}

fn header(chat_id: &str) -> ChatSessionHeader {
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

fn message(message_id: &str, role: &str) -> NewConversationMessage {
    NewConversationMessage {
        message_id: message_id.to_string(),
        role: role.to_string(),
        created_at: "2026-06-19T00:00:00Z".to_string(),
        status: "completed".to_string(),
        content: json!([{"type": "text", "text": message_id}]),
        provider: None,
        tool: None,
        metadata: serde_json::Map::new(),
    }
}

async fn prepare_chat(store: &WorkspaceSessionStore, chat_id: &str) {
    store.save_chat_header(&header(chat_id)).await.unwrap();
    for ordinal in 1..=4 {
        store
            .append_conversation_turn(
                chat_id,
                NewConversationTurn {
                    turn_id: format!("turn-{ordinal}"),
                    lifecycle: TurnLifecycle {
                        user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                        assistant_started_at: Some("2026-06-19T00:00:01Z".to_string()),
                        assistant_completed_at: Some("2026-06-19T00:00:02Z".to_string()),
                        status: "completed".to_string(),
                    },
                    selected_variant_id: Some(format!("assistant-{ordinal}")),
                    messages: vec![
                        message(&format!("user-{ordinal}"), "user"),
                        message(&format!("assistant-{ordinal}"), "assistant"),
                    ],
                },
            )
            .await
            .unwrap();
    }
}

async fn draft(
    store: &WorkspaceSessionStore,
    chat_id: &str,
    checkpoint_id: &str,
    previous_checkpoint_id: Option<&str>,
) -> SummaryCheckpointDraft {
    let projection_state = store.checkpoint_projection_state(chat_id, 8).await.unwrap();
    SummaryCheckpointDraft {
        checkpoint_id: checkpoint_id.to_string(),
        previous_checkpoint_id: previous_checkpoint_id.map(str::to_string),
        covered_turn_range: CheckpointTurnRange {
            start_turn_ordinal: 1,
            end_turn_ordinal: 4,
        },
        covered_sequence_range: CheckpointSequenceRange {
            start_sequence: 1,
            end_sequence: 8,
        },
        source_sequence_range: CheckpointSequenceRange {
            start_sequence: 1,
            end_sequence: 8,
        },
        frontier: projection_state.frontier,
        retained_sequence_ranges: projection_state.retained_sequence_ranges,
        projection_lineage: projection_state.projection_lineage,
        summary: "User asked about evergreen chat; assistant proposed checkpoint storage."
            .to_string(),
        token_estimate: 128,
        model: CheckpointModelIdentity {
            provider: "openai".to_string(),
            model: "gpt-5".to_string(),
        },
        created_at: "2026-06-19T00:01:00Z".to_string(),
    }
}

#[tokio::test]
async fn workspace_store_publishes_valid_summary_checkpoint_before_advancing_pointer() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    prepare_chat(&store, "chat-1").await;

    let published = store
        .publish_summary_checkpoint(
            "chat-1",
            None,
            draft(&store, "chat-1", "checkpoint-1", None).await,
        )
        .await
        .unwrap();

    assert_eq!(published.checkpoint_id, "checkpoint-1");
    assert_eq!(published.chat_id, "chat-1");
    assert_eq!(published.agent_id, "agent-1");
    assert_eq!(published.previous_checkpoint_id, None);
    assert_eq!(published.covered_turn_range.start_turn_ordinal, 1);
    assert_eq!(published.covered_turn_range.end_turn_ordinal, 4);
    assert_eq!(published.covered_sequence_range.end_sequence, 8);
    assert_eq!(published.source_sequence_range.start_sequence, 1);
    assert_eq!(published.frontier.turn_id, "turn-4");
    assert_eq!(published.frontier.end_sequence, 8);
    assert!(published.retained_sequence_ranges.is_empty());
    assert!(published.projection_lineage.starts_with("sha256:"));
    assert_eq!(published.token_estimate, 128);
    assert_eq!(published.model.model, "gpt-5");
    assert!(published.checksum.starts_with("sha256:"));
    assert_eq!(published.checksum.len(), "sha256:".len() + 64);

    let raw_path = dir
        .path()
        .join("sessions/chat-1/checkpoints/checkpoint-1.json");
    assert!(raw_path.exists(), "checkpoint file should be durable");
    let raw = std::fs::read_to_string(raw_path).unwrap();
    let raw_json: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(raw_json["checksum"], published.checksum);

    let header = store.load_chat_header("chat-1").await.unwrap().unwrap();
    assert_eq!(header.active_checkpoint_id.as_deref(), Some("checkpoint-1"));

    let active = store
        .load_active_summary_checkpoint("chat-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(active, published);
}

#[tokio::test]
async fn workspace_store_rejects_checkpoint_covering_non_durable_sequence() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    prepare_chat(&store, "chat-1").await;
    let mut invalid = draft(&store, "chat-1", "checkpoint-missing-source", None).await;
    invalid.covered_sequence_range.end_sequence = 9;
    invalid.source_sequence_range.end_sequence = 9;
    invalid.frontier.end_sequence = 9;

    let result = store
        .publish_summary_checkpoint("chat-1", None, invalid)
        .await;

    assert!(result.is_err());
    assert!(
        !dir.path()
            .join("sessions/chat-1/checkpoints/checkpoint-missing-source.json")
            .exists(),
        "source validation must happen before checkpoint publication"
    );
    assert!(store
        .load_chat_header("chat-1")
        .await
        .unwrap()
        .unwrap()
        .active_checkpoint_id
        .is_none());
}

#[tokio::test]
async fn workspace_store_rejects_stale_projection_lineage_before_pointer_advance() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    prepare_chat(&store, "chat-1").await;
    let mut stale = draft(&store, "chat-1", "checkpoint-stale-lineage", None).await;
    stale.projection_lineage = "sha256:stale".to_string();

    let result = store
        .publish_summary_checkpoint("chat-1", None, stale)
        .await;

    assert!(result.is_err());
    assert!(store
        .load_chat_header("chat-1")
        .await
        .unwrap()
        .unwrap()
        .active_checkpoint_id
        .is_none());
}

#[tokio::test]
async fn workspace_store_rejects_checkpoint_publish_when_active_pointer_changed() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    prepare_chat(&store, "chat-1").await;

    let first = store
        .publish_summary_checkpoint(
            "chat-1",
            None,
            draft(&store, "chat-1", "checkpoint-1", None).await,
        )
        .await
        .unwrap();
    let stale_publish = store
        .publish_summary_checkpoint(
            "chat-1",
            None,
            draft(&store, "chat-1", "checkpoint-stale", None).await,
        )
        .await;

    assert!(stale_publish.is_err(), "stale base pointer must fail CAS");

    let active = store
        .load_active_summary_checkpoint("chat-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(active.checkpoint_id, first.checkpoint_id);

    let orphan = store
        .read_summary_checkpoint("chat-1", "checkpoint-stale")
        .await
        .unwrap();
    assert!(
        orphan.is_none(),
        "a stale logical base must be rejected before writing an orphan checkpoint"
    );

    let header = store.load_chat_header("chat-1").await.unwrap().unwrap();
    assert_eq!(header.active_checkpoint_id.as_deref(), Some("checkpoint-1"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_checkpoint_publications_advance_pointer_once() {
    const PUBLISHERS: usize = 16;

    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(WorkspaceSessionStore::new(dir.path()));
    prepare_chat(&store, "chat-1").await;
    let mut drafts = Vec::new();
    for index in 0..PUBLISHERS {
        drafts.push(
            draft(
                &store,
                "chat-1",
                &format!("checkpoint-concurrent-{index}"),
                None,
            )
            .await,
        );
    }

    let barrier = Arc::new(Barrier::new(PUBLISHERS));
    let mut tasks = tokio::task::JoinSet::new();
    for draft in drafts {
        let store = Arc::clone(&store);
        let barrier = Arc::clone(&barrier);
        tasks.spawn(async move {
            barrier.wait().await;
            store
                .publish_summary_checkpoint("chat-1", None, draft)
                .await
        });
    }

    let mut published = Vec::new();
    while let Some(result) = tasks.join_next().await {
        if let Ok(checkpoint) = result.unwrap() {
            published.push(checkpoint);
        }
    }

    assert_eq!(
        published.len(),
        1,
        "the active checkpoint compare-and-swap must have exactly one winner"
    );
    let header = store.load_chat_header("chat-1").await.unwrap().unwrap();
    assert_eq!(
        header.active_checkpoint_id.as_deref(),
        Some(published[0].checkpoint_id.as_str())
    );
}

#[tokio::test]
async fn workspace_store_falls_back_to_previous_valid_checkpoint_when_active_is_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    prepare_chat(&store, "chat-1").await;

    store
        .publish_summary_checkpoint(
            "chat-1",
            None,
            draft(&store, "chat-1", "checkpoint-1", None).await,
        )
        .await
        .unwrap();
    store
        .publish_summary_checkpoint(
            "chat-1",
            Some("checkpoint-1"),
            draft(&store, "chat-1", "checkpoint-2", Some("checkpoint-1")).await,
        )
        .await
        .unwrap();

    let checkpoint_2 = dir
        .path()
        .join("sessions/chat-1/checkpoints/checkpoint-2.json");
    let mut raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&checkpoint_2).unwrap()).unwrap();
    raw["summary"] = serde_json::json!("corrupted after checksum");
    std::fs::write(&checkpoint_2, serde_json::to_string_pretty(&raw).unwrap()).unwrap();

    let active = store
        .load_active_summary_checkpoint("chat-1")
        .await
        .unwrap()
        .unwrap();

    assert_eq!(active.checkpoint_id, "checkpoint-1");
    assert_eq!(
        active.summary,
        "User asked about evergreen chat; assistant proposed checkpoint storage."
    );
}

#[tokio::test]
async fn workspace_store_rejects_incomplete_orphan_checkpoint_without_affecting_active() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    prepare_chat(&store, "chat-1").await;
    store
        .publish_summary_checkpoint(
            "chat-1",
            None,
            draft(&store, "chat-1", "checkpoint-1", None).await,
        )
        .await
        .unwrap();

    let checkpoint_dir = dir.path().join("sessions/chat-1/checkpoints");
    std::fs::create_dir_all(&checkpoint_dir).unwrap();
    std::fs::write(
        checkpoint_dir.join("incomplete.json"),
        r#"{"checkpoint_id":"#,
    )
    .unwrap();

    let incomplete = store.read_summary_checkpoint("chat-1", "incomplete").await;
    assert!(
        incomplete.is_err(),
        "incomplete checkpoint should be rejected"
    );

    let active = store
        .load_active_summary_checkpoint("chat-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(active.checkpoint_id, "checkpoint-1");
}

#[tokio::test]
async fn checkpoint_ending_at_active_user_survives_compact_command_completion() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    let chat_id = "compact-command-checkpoint";
    prepare_chat(&store, chat_id).await;
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: "compact-turn".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-09-20T07:00:00Z".to_string(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![message("compact-user", "user")],
            },
        )
        .await
        .unwrap();
    let state = store.checkpoint_projection_state(chat_id, 9).await.unwrap();
    let mut candidate = draft(&store, chat_id, "compact-checkpoint", None).await;
    candidate.covered_turn_range.end_turn_ordinal = 5;
    candidate.covered_sequence_range.end_sequence = 9;
    candidate.source_sequence_range.end_sequence = 9;
    candidate.frontier = state.frontier;
    candidate.retained_sequence_ranges = state.retained_sequence_ranges;
    candidate.projection_lineage = state.projection_lineage;
    let checkpoint = store
        .publish_summary_checkpoint(chat_id, None, candidate)
        .await
        .unwrap();
    assert_eq!(
        checkpoint
            .covered_selected_variants
            .last()
            .unwrap()
            .selected_variant_id,
        None
    );

    let lifecycle = TurnLifecycle {
        user_submitted_at: "2026-09-20T07:00:00Z".to_string(),
        assistant_started_at: Some("2026-09-20T07:00:01Z".to_string()),
        assistant_completed_at: Some("2026-09-20T07:02:00Z".to_string()),
        status: "completed".to_string(),
    };
    store
        .append_messages_to_turn(
            chat_id,
            "compact-turn",
            lifecycle.clone(),
            Some("compact-confirmation".to_string()),
            vec![message("compact-confirmation", "assistant")],
        )
        .await
        .unwrap();
    // Reopen the store as the next Turn does; no in-memory cache may hide invalidation.
    let store = WorkspaceSessionStore::new(dir.path());
    let active = store.load_active_summary_checkpoint(chat_id).await.unwrap();
    assert_eq!(
        active.as_ref().map(|c| c.checkpoint_id.as_str()),
        Some("compact-checkpoint")
    );
    let projection = store
        .build_context_projection(
            chat_id,
            rua_session::ContextProjectionRequest {
                current_user_input: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        projection.summary.as_deref(),
        Some(checkpoint.summary.as_str())
    );
    assert_eq!(
        projection
            .retained_messages
            .iter()
            .map(|m| m.message_id.as_str())
            .collect::<Vec<_>>(),
        vec!["compact-user"]
    );
    assert_eq!(
        projection
            .tail_messages
            .iter()
            .map(|m| m.message_id.as_str())
            .collect::<Vec<_>>(),
        vec!["compact-confirmation"]
    );

    // A response variant entirely after the frontier cannot change its summarized prefix.
    store
        .append_messages_to_turn_as_new_variant(
            chat_id,
            "compact-turn",
            lifecycle.clone(),
            Some("confirmation-retry".to_string()),
            vec![message("confirmation-retry", "assistant")],
        )
        .await
        .unwrap();
    assert!(store
        .load_active_summary_checkpoint(chat_id)
        .await
        .unwrap()
        .is_some());

    // An actual change to summarized output must still invalidate the checkpoint.
    store
        .append_messages_to_turn_as_new_variant(
            chat_id,
            "turn-4",
            lifecycle,
            Some("changed-history".to_string()),
            vec![message("changed-history", "assistant")],
        )
        .await
        .unwrap();
    assert!(store
        .load_active_summary_checkpoint(chat_id)
        .await
        .unwrap()
        .is_none());
}
