use rua_core::ChatAddress;
use rua_session::{
    ChatMetadata, ChatSessionHeader, CheckpointModelIdentity, CheckpointSequenceRange,
    CheckpointTurnRange, ContextProjectionRequest, ConversationRecallRequest,
    NewConversationMessage, NewConversationTurn, SummaryCheckpointDraft, TurnLifecycle,
    WorkspaceSessionStore,
};
use serde_json::json;

fn header(chat_id: &str) -> ChatSessionHeader {
    let mut header = ChatSessionHeader::new(ChatMetadata {
        chat_id: chat_id.to_string(),
        agent_id: "agent-1".to_string(),
        title: "Evergreen".to_string(),
        created_at: "2026-06-19T00:00:00Z".to_string(),
        last_activity_at: "2026-06-19T00:00:00Z".to_string(),
        pinned: false,
        archived: false,
        preview: None,
    });
    header.message_log.chunk_size = 2;
    header
}

fn message(message_id: &str, role: &str, text: &str) -> NewConversationMessage {
    NewConversationMessage {
        message_id: message_id.to_string(),
        role: role.to_string(),
        created_at: "2026-06-19T00:00:00Z".to_string(),
        status: "completed".to_string(),
        content: json!([{"type": "text", "text": text}]),
        provider: None,
        tool: None,
        metadata: serde_json::Map::new(),
    }
}

fn acp_message(message_id: &str, role: &str, text: &str, runner: &str) -> NewConversationMessage {
    let mut message = message(message_id, role, text);
    message.metadata.insert(
        "origin".to_string(),
        json!({ "kind": "acp", "runner": runner }),
    );
    message
}

fn tool_use_message(message_id: &str, call_id: &str) -> NewConversationMessage {
    NewConversationMessage {
        message_id: message_id.to_string(),
        role: "assistant".to_string(),
        created_at: "2026-07-24T00:00:01Z".to_string(),
        status: "completed".to_string(),
        content: json!([{
            "type": "tool_use",
            "id": call_id,
            "name": "bash",
            "arguments": {"command": "pwd"}
        }]),
        provider: None,
        tool: None,
        metadata: serde_json::Map::new(),
    }
}

fn tool_result_message(message_id: &str, call_id: &str) -> NewConversationMessage {
    NewConversationMessage {
        message_id: message_id.to_string(),
        role: "system".to_string(),
        created_at: "2026-07-24T00:00:02Z".to_string(),
        status: "completed".to_string(),
        content: json!([{
            "type": "tool_result",
            "call_id": call_id,
            "content": [{"type": "text", "text": format!("result for {call_id}")}]
        }]),
        provider: None,
        tool: None,
        metadata: serde_json::Map::new(),
    }
}

#[tokio::test]
async fn completed_turn_projection_groups_use_message_and_tool_call_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&header("chat-completed-groups"))
        .await
        .unwrap();
    store
        .append_conversation_turn(
            "chat-completed-groups",
            NewConversationTurn {
                turn_id: "turn-completed".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-07-24T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-07-24T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-07-24T00:00:03Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: None,
                messages: vec![
                    message("user", "user", "do the work"),
                    tool_use_message("tool-use", "call-1"),
                    tool_result_message("tool-result", "call-1"),
                    message("assistant", "assistant", "done"),
                ],
            },
        )
        .await
        .unwrap();

    let groups = store
        .projection_message_group_estimates_effective("chat-completed-groups", false, 4.0)
        .await
        .unwrap();

    assert_eq!(
        groups
            .iter()
            .map(|group| (group.first_sequence, group.last_sequence))
            .collect::<Vec<_>>(),
        vec![(1, 1), (2, 3), (4, 4)]
    );
    assert!(groups.iter().all(|group| group.foldable));
}

#[tokio::test]
async fn projection_exclusions_hide_agent_trace_without_rewriting_message_log() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&header("chat-projection-exclusions"))
        .await
        .unwrap();
    store
        .append_conversation_turn(
            "chat-projection-exclusions",
            NewConversationTurn {
                turn_id: "turn-hallucination".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-07-24T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-07-24T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-07-24T00:00:03Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: None,
                messages: vec![
                    message("user", "user", "continue reviewing ADR-0027"),
                    message("hallucinated-text", "assistant", "inspect task restart"),
                    tool_use_message("hallucinated-tool-use", "call-hallucinated"),
                    tool_result_message("hallucinated-tool-result", "call-hallucinated"),
                ],
            },
        )
        .await
        .unwrap();

    store
        .save_projection_exclusions(
            "chat-projection-exclusions",
            vec![
                "hallucinated-text".to_string(),
                "hallucinated-tool-use".to_string(),
                "hallucinated-tool-result".to_string(),
            ],
        )
        .await
        .unwrap();

    let projection = store
        .build_context_projection(
            "chat-projection-exclusions",
            ContextProjectionRequest {
                current_user_input: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        projection
            .tail_messages
            .iter()
            .map(|message| message.message_id.as_str())
            .collect::<Vec<_>>(),
        vec!["user"]
    );
    assert_eq!(
        store
            .read_message_range("chat-projection-exclusions", 1, 10)
            .await
            .unwrap()
            .len(),
        4,
        "projection editing must not rewrite the append-only message log"
    );

    store
        .save_projection_exclusions("chat-projection-exclusions", Vec::new())
        .await
        .unwrap();
    let restored = store
        .build_context_projection(
            "chat-projection-exclusions",
            ContextProjectionRequest {
                current_user_input: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(restored.tail_messages.len(), 4);
}

#[tokio::test]
async fn active_turn_projection_groups_split_messages_without_splitting_tool_calls() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&header("chat-active-groups"))
        .await
        .unwrap();
    store
        .append_conversation_turn(
            "chat-active-groups",
            NewConversationTurn {
                turn_id: "turn-active".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-07-24T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-07-24T00:00:01Z".to_string()),
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![
                    message("user", "user", "do the work"),
                    message("assistant-1", "assistant", "first step"),
                    tool_use_message("tool-use-1", "call-1"),
                    tool_result_message("tool-result-1", "call-1"),
                    tool_use_message("tool-use-open", "call-open"),
                ],
            },
        )
        .await
        .unwrap();

    let groups = store
        .projection_message_group_estimates_effective("chat-active-groups", false, 4.0)
        .await
        .unwrap();

    assert_eq!(
        groups
            .iter()
            .map(|group| (group.first_sequence, group.last_sequence))
            .collect::<Vec<_>>(),
        vec![(1, 1), (2, 2), (3, 4)]
    );
    assert!(groups.iter().all(|group| group.foldable));
}

#[tokio::test]
async fn failed_turn_projection_groups_do_not_retain_the_failed_user_request() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&header("chat-failed-groups"))
        .await
        .unwrap();
    store
        .append_conversation_turn(
            "chat-failed-groups",
            NewConversationTurn {
                turn_id: "turn-failed".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-07-24T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-07-24T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-07-24T00:00:02Z".to_string()),
                    status: "failed".to_string(),
                },
                selected_variant_id: None,
                messages: vec![
                    message("failed-user", "user", &"x".repeat(20_000)),
                    message(
                        "failed-runtime-note",
                        "system",
                        "conversation failed because the request exceeded the context window",
                    ),
                ],
            },
        )
        .await
        .unwrap();

    let groups = store
        .projection_message_group_estimates_effective("chat-failed-groups", false, 4.0)
        .await
        .unwrap();

    assert_eq!(
        groups
            .iter()
            .map(|group| (group.first_sequence, group.last_sequence))
            .collect::<Vec<_>>(),
        vec![(1, 1), (2, 2)]
    );
    assert!(
        groups
            .iter()
            .all(|group| group.retained_sequence_ranges.is_empty()),
        "a terminal failed turn must be fully foldable"
    );
}

async fn checkpoint(
    store: &WorkspaceSessionStore,
    chat_id: &str,
    covered_end_sequence: u64,
) -> SummaryCheckpointDraft {
    let projection_state = store
        .checkpoint_projection_state(chat_id, covered_end_sequence)
        .await
        .unwrap();
    SummaryCheckpointDraft {
        checkpoint_id: "checkpoint-1".to_string(),
        previous_checkpoint_id: None,
        covered_turn_range: CheckpointTurnRange {
            start_turn_ordinal: 1,
            end_turn_ordinal: covered_end_sequence / 2,
        },
        covered_sequence_range: CheckpointSequenceRange {
            start_sequence: 1,
            end_sequence: covered_end_sequence,
        },
        source_sequence_range: CheckpointSequenceRange {
            start_sequence: 1,
            end_sequence: covered_end_sequence,
        },
        frontier: projection_state.frontier,
        retained_sequence_ranges: projection_state.retained_sequence_ranges,
        projection_lineage: projection_state.projection_lineage,
        summary: "summary through turn 4".to_string(),
        token_estimate: 64,
        model: CheckpointModelIdentity {
            provider: "openai".to_string(),
            model: "gpt-5".to_string(),
        },
        created_at: "2026-06-19T00:01:00Z".to_string(),
    }
}

async fn append_turns(store: &WorkspaceSessionStore, chat_id: &str, count: u64) {
    for turn_number in 1..=count {
        store
            .append_conversation_turn(
                chat_id,
                NewConversationTurn {
                    turn_id: format!("turn-{turn_number}"),
                    lifecycle: TurnLifecycle {
                        user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                        assistant_started_at: Some("2026-06-19T00:00:01Z".to_string()),
                        assistant_completed_at: Some("2026-06-19T00:00:02Z".to_string()),
                        status: "completed".to_string(),
                    },
                    selected_variant_id: Some(format!("assistant-{turn_number}")),
                    messages: vec![
                        message(&format!("user-{turn_number}"), "user", "hello"),
                        message(&format!("assistant-{turn_number}"), "assistant", "hi"),
                    ],
                },
            )
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn projection_image_base64_uses_image_cost_instead_of_text_size() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&header("chat-image-estimate"))
        .await
        .unwrap();
    let image = NewConversationMessage {
        message_id: "image-1".to_string(),
        role: "user".to_string(),
        created_at: "2026-06-19T00:00:00Z".to_string(),
        status: "completed".to_string(),
        content: json!([{
            "type": "image",
            "url": format!("data:image/png;base64,{}", "A".repeat(1_942_716)),
        }]),
        provider: None,
        tool: None,
        metadata: serde_json::Map::new(),
    };
    store
        .append_conversation_turn(
            "chat-image-estimate",
            NewConversationTurn {
                turn_id: "turn-image".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-06-19T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: None,
                messages: vec![image],
            },
        )
        .await
        .unwrap();

    let projection = store
        .build_context_projection(
            "chat-image-estimate",
            ContextProjectionRequest {
                current_user_input: None,
            },
        )
        .await
        .unwrap();

    assert!(
        projection.stable_token_estimate < 2_500,
        "base64 bytes must not be counted as text tokens"
    );
}

#[tokio::test]
async fn workspace_store_projects_mid_turn_checkpoint_with_retained_user_request_once() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&header("chat-mid-turn"))
        .await
        .unwrap();
    let user = message(
        "user-active",
        "user",
        "keep this exact original instruction",
    );
    let first_tool_use = tool_use_message("assistant-tool-1", "call-1");
    let first_tool_result = tool_result_message("tool-result-1", "call-1");
    let second_tool_use = tool_use_message("assistant-tool-2", "call-2");
    let second_tool_result = tool_result_message("tool-result-2", "call-2");
    store
        .append_conversation_turn(
            "chat-mid-turn",
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
    let lifecycle = TurnLifecycle {
        user_submitted_at: "2026-07-24T00:00:00Z".to_string(),
        assistant_started_at: Some("2026-07-24T00:00:01Z".to_string()),
        assistant_completed_at: None,
        status: "in_progress".to_string(),
    };
    store
        .append_messages_to_turn(
            "chat-mid-turn",
            "turn-active",
            lifecycle.clone(),
            Some("assistant-tool-1".to_string()),
            vec![first_tool_use.clone(), first_tool_result.clone()],
        )
        .await
        .unwrap();
    assert!(
        store
            .checkpoint_projection_state("chat-mid-turn", 2)
            .await
            .is_err(),
        "a tool call without its result is not a safe checkpoint position"
    );
    let projection_state = store
        .checkpoint_projection_state("chat-mid-turn", 3)
        .await
        .unwrap();
    let projection_lineage = projection_state.projection_lineage.clone();
    let retained_sequence_ranges = projection_state.retained_sequence_ranges.clone();
    let summary_source = store
        .read_checkpoint_summary_source("chat-mid-turn", 1, 3, &retained_sequence_ranges)
        .await
        .unwrap();
    assert_eq!(
        summary_source
            .iter()
            .map(|message| message.message_id.as_str())
            .collect::<Vec<_>>(),
        vec!["assistant-tool-1", "tool-result-1"],
        "retained user request must not be duplicated into summary input"
    );
    store
        .publish_summary_checkpoint(
            "chat-mid-turn",
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
                retained_sequence_ranges,
                projection_lineage,
                summary: "The assistant completed the first tool step.".to_string(),
                token_estimate: 16,
                model: CheckpointModelIdentity {
                    provider: "test".to_string(),
                    model: "test-model".to_string(),
                },
                created_at: "2026-07-24T00:01:00Z".to_string(),
            },
        )
        .await
        .unwrap();

    store
        .append_messages_to_turn(
            "chat-mid-turn",
            "turn-active",
            lifecycle,
            Some("assistant-tool-2".to_string()),
            vec![second_tool_use, second_tool_result],
        )
        .await
        .unwrap();

    let projection = store
        .build_context_projection(
            "chat-mid-turn",
            ContextProjectionRequest {
                current_user_input: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        projection
            .retained_messages
            .iter()
            .map(|message| message.message_id.as_str())
            .collect::<Vec<_>>(),
        vec!["user-active"]
    );
    assert_eq!(
        projection
            .tail_messages
            .iter()
            .map(|message| message.message_id.as_str())
            .collect::<Vec<_>>(),
        vec!["assistant-tool-2", "tool-result-2"]
    );
    assert_eq!(
        projection
            .retained_messages
            .iter()
            .chain(projection.tail_messages.iter())
            .filter(|message| message.message_id == "user-active")
            .count(),
        1
    );

    let raw = store
        .read_turn_messages("chat-mid-turn", "turn-active")
        .await
        .unwrap();
    assert_eq!(
        raw.len(),
        5,
        "checkpoint publication must not rewrite history"
    );
    let recall = store
        .conversation_recall(
            "chat-mid-turn",
            ConversationRecallRequest {
                query: "exact original instruction".to_string(),
                max_results: 5,
                surrounding_messages: 0,
            },
        )
        .await
        .unwrap();
    assert_eq!(recall.hits.len(), 1);
    assert_eq!(recall.hits[0].message.message_id, "user-active");
}

#[tokio::test]
async fn workspace_store_projection_skips_acp_origin_turns_after_acp_exit() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store.save_chat_header(&header("chat-1")).await.unwrap();

    store
        .append_conversation_turn(
            "chat-1",
            NewConversationTurn {
                turn_id: "turn-normal-before".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-06-19T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("assistant-before".to_string()),
                messages: vec![
                    message("user-before", "user", "normal before"),
                    message("assistant-before", "assistant", "normal answer before"),
                ],
            },
        )
        .await
        .unwrap();
    store
        .append_conversation_turn(
            "chat-1",
            NewConversationTurn {
                turn_id: "turn-acp".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:01:00Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:01:01Z".to_string()),
                    assistant_completed_at: Some("2026-06-19T00:01:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("assistant-acp".to_string()),
                messages: vec![
                    acp_message("user-acp", "user", "acp prompt", "claude"),
                    acp_message("assistant-acp", "assistant", "acp output", "claude"),
                ],
            },
        )
        .await
        .unwrap();
    store
        .append_conversation_turn(
            "chat-1",
            NewConversationTurn {
                turn_id: "turn-normal-after".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:02:00Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:02:01Z".to_string()),
                    assistant_completed_at: Some("2026-06-19T00:02:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("assistant-after".to_string()),
                messages: vec![
                    message("user-after", "user", "normal after"),
                    message("assistant-after", "assistant", "normal answer after"),
                ],
            },
        )
        .await
        .unwrap();

    let raw_messages = store.read_message_range("chat-1", 1, 10).await.unwrap();
    assert_eq!(raw_messages.len(), 6);

    let turns = store.read_turn_range("chat-1", 1, 10).await.unwrap();
    let acp_turn = turns
        .iter()
        .find(|turn| turn.turn_id == "turn-acp")
        .expect("acp turn");
    assert!(rua_session::turn_origin_is_acp(acp_turn));

    let projection = store
        .build_context_projection(
            "chat-1",
            ContextProjectionRequest {
                current_user_input: None,
            },
        )
        .await
        .unwrap();
    let ids: Vec<_> = projection
        .tail_messages
        .iter()
        .map(|message| message.message_id.as_str())
        .collect();
    assert_eq!(
        ids,
        vec![
            "user-before",
            "assistant-before",
            "user-after",
            "assistant-after"
        ]
    );

    let projectable = store
        .read_projectable_message_range("chat-1", 1, 6)
        .await
        .unwrap();
    let projectable_ids: Vec<_> = projectable
        .iter()
        .map(|message| message.message_id.as_str())
        .collect();
    assert_eq!(
        projectable_ids,
        vec![
            "user-before",
            "assistant-before",
            "user-after",
            "assistant-after"
        ]
    );
}

#[tokio::test]
async fn workspace_store_builds_projection_from_active_checkpoint_and_bounded_tail_only() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store.save_chat_header(&header("chat-1")).await.unwrap();
    append_turns(&store, "chat-1", 6).await;
    store
        .publish_summary_checkpoint("chat-1", None, checkpoint(&store, "chat-1", 8).await)
        .await
        .unwrap();

    std::fs::write(
        dir.path()
            .join("sessions/chat-1/messages/000000000000.jsonl"),
        "{older chunk should not be parsed",
    )
    .unwrap();

    let projection = store
        .build_context_projection(
            "chat-1",
            ContextProjectionRequest {
                current_user_input: Some("live user input".to_string()),
            },
        )
        .await
        .unwrap();

    assert_eq!(
        projection.summary.as_deref(),
        Some("summary through turn 4")
    );
    let ids: Vec<_> = projection
        .tail_messages
        .iter()
        .map(|message| message.message_id.as_str())
        .collect();
    assert_eq!(ids, vec!["user-5", "assistant-5", "user-6", "assistant-6"]);
    assert_eq!(projection.tail_start_sequence, Some(9));
    assert_eq!(projection.tail_end_sequence, Some(12));
    assert!(
        !dir.path().join("sessions/chat-1/projection_cache").exists(),
        "context projection must not materialize a derived projection cache"
    );
    assert_eq!(
        projection.current_user_input.as_deref(),
        Some("live user input")
    );
}

#[tokio::test]
async fn selected_variant_change_falls_back_to_compatible_checkpoint_ancestor() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&header("chat-variant"))
        .await
        .unwrap();
    store
        .append_conversation_turn(
            "chat-variant",
            NewConversationTurn {
                turn_id: "turn-stable".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-06-19T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("assistant-stable".to_string()),
                messages: vec![
                    message("user-stable", "user", "stable question"),
                    message("assistant-stable", "assistant", "stable answer"),
                ],
            },
        )
        .await
        .unwrap();
    store
        .publish_summary_checkpoint(
            "chat-variant",
            None,
            SummaryCheckpointDraft {
                checkpoint_id: "checkpoint-stable".to_string(),
                previous_checkpoint_id: None,
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
                frontier: store
                    .checkpoint_projection_state("chat-variant", 2)
                    .await
                    .unwrap()
                    .frontier,
                retained_sequence_ranges: Vec::new(),
                projection_lineage: store
                    .projection_lineage_through_sequence("chat-variant", 2)
                    .await
                    .unwrap(),
                summary: "stable checkpoint summary".to_string(),
                token_estimate: 8,
                model: CheckpointModelIdentity {
                    provider: "test".to_string(),
                    model: "test-model".to_string(),
                },
                created_at: "2026-06-19T00:01:00Z".to_string(),
            },
        )
        .await
        .unwrap();
    store
        .append_conversation_turn(
            "chat-variant",
            NewConversationTurn {
                turn_id: "turn-variant".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:02:00Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:02:01Z".to_string()),
                    assistant_completed_at: Some("2026-06-19T00:02:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("assistant-a".to_string()),
                messages: vec![
                    message("user-variant", "user", "choose a response"),
                    message("assistant-a", "assistant", "variant A"),
                ],
            },
        )
        .await
        .unwrap();
    let descendant_state = store
        .checkpoint_projection_state("chat-variant", 4)
        .await
        .unwrap();
    store
        .publish_summary_checkpoint(
            "chat-variant",
            Some("checkpoint-stable"),
            SummaryCheckpointDraft {
                checkpoint_id: "checkpoint-variant-a".to_string(),
                previous_checkpoint_id: Some("checkpoint-stable".to_string()),
                covered_turn_range: CheckpointTurnRange {
                    start_turn_ordinal: 1,
                    end_turn_ordinal: 2,
                },
                covered_sequence_range: CheckpointSequenceRange {
                    start_sequence: 1,
                    end_sequence: 4,
                },
                source_sequence_range: CheckpointSequenceRange {
                    start_sequence: 3,
                    end_sequence: 4,
                },
                frontier: descendant_state.frontier,
                retained_sequence_ranges: descendant_state.retained_sequence_ranges,
                projection_lineage: descendant_state.projection_lineage,
                summary: "incompatible variant A summary".to_string(),
                token_estimate: 9,
                model: CheckpointModelIdentity {
                    provider: "test".to_string(),
                    model: "test-model".to_string(),
                },
                created_at: "2026-06-19T00:03:00Z".to_string(),
            },
        )
        .await
        .unwrap();
    store
        .append_messages_to_turn_as_new_variant(
            "chat-variant",
            "turn-variant",
            TurnLifecycle {
                user_submitted_at: "2026-06-19T00:02:00Z".to_string(),
                assistant_started_at: Some("2026-06-19T00:02:01Z".to_string()),
                assistant_completed_at: Some("2026-06-19T00:02:03Z".to_string()),
                status: "completed".to_string(),
            },
            Some("assistant-b".to_string()),
            vec![message("assistant-b", "assistant", "variant B")],
        )
        .await
        .unwrap();
    store
        .set_turn_selected_variant("chat-variant", "turn-variant", "assistant-b".to_string())
        .await
        .unwrap();

    let active = store
        .load_active_summary_checkpoint("chat-variant")
        .await
        .unwrap()
        .expect("compatible ancestor");
    assert_eq!(active.checkpoint_id, "checkpoint-stable");

    let projection = store
        .build_context_projection(
            "chat-variant",
            ContextProjectionRequest {
                current_user_input: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        projection.summary.as_deref(),
        Some("stable checkpoint summary")
    );
    assert_eq!(
        projection
            .tail_messages
            .iter()
            .map(|message| message.message_id.as_str())
            .collect::<Vec<_>>(),
        vec!["user-variant", "assistant-b"]
    );
    assert_eq!(
        store
            .read_message_range("chat-variant", 1, 5)
            .await
            .unwrap()
            .iter()
            .map(|message| message.message_id.as_str())
            .collect::<Vec<_>>(),
        vec![
            "user-stable",
            "assistant-stable",
            "user-variant",
            "assistant-a",
            "assistant-b",
        ]
    );

    let replacement_state = store
        .checkpoint_projection_state("chat-variant", 5)
        .await
        .unwrap();
    store
        .publish_summary_checkpoint(
            "chat-variant",
            Some("checkpoint-variant-a"),
            SummaryCheckpointDraft {
                checkpoint_id: "checkpoint-variant-b".to_string(),
                previous_checkpoint_id: Some("checkpoint-stable".to_string()),
                covered_turn_range: CheckpointTurnRange {
                    start_turn_ordinal: 1,
                    end_turn_ordinal: 2,
                },
                covered_sequence_range: CheckpointSequenceRange {
                    start_sequence: 1,
                    end_sequence: 5,
                },
                source_sequence_range: CheckpointSequenceRange {
                    start_sequence: 3,
                    end_sequence: 5,
                },
                frontier: replacement_state.frontier,
                retained_sequence_ranges: replacement_state.retained_sequence_ranges,
                projection_lineage: replacement_state.projection_lineage,
                summary: "compatible variant B summary".to_string(),
                token_estimate: 9,
                model: CheckpointModelIdentity {
                    provider: "test".to_string(),
                    model: "test-model".to_string(),
                },
                created_at: "2026-06-19T00:04:00Z".to_string(),
            },
        )
        .await
        .unwrap();
    let replacement = store
        .load_active_summary_checkpoint("chat-variant")
        .await
        .unwrap()
        .expect("replacement checkpoint");
    assert_eq!(replacement.checkpoint_id, "checkpoint-variant-b");
    assert_eq!(
        replacement.previous_checkpoint_id.as_deref(),
        Some("checkpoint-stable")
    );
}

#[tokio::test]
async fn branch_inherits_newest_checkpoint_ancestor_compatible_with_branch_point() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&header("chat-source"))
        .await
        .unwrap();
    append_turns(&store, "chat-source", 3).await;

    let ancestor_state = store
        .checkpoint_projection_state("chat-source", 2)
        .await
        .unwrap();
    store
        .publish_summary_checkpoint(
            "chat-source",
            None,
            SummaryCheckpointDraft {
                checkpoint_id: "checkpoint-ancestor".to_string(),
                previous_checkpoint_id: None,
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
                frontier: ancestor_state.frontier,
                retained_sequence_ranges: ancestor_state.retained_sequence_ranges,
                projection_lineage: ancestor_state.projection_lineage,
                summary: "summary through source turn 1".to_string(),
                token_estimate: 8,
                model: CheckpointModelIdentity {
                    provider: "test".to_string(),
                    model: "test-model".to_string(),
                },
                created_at: "2026-06-19T00:01:00Z".to_string(),
            },
        )
        .await
        .unwrap();
    let descendant_state = store
        .checkpoint_projection_state("chat-source", 6)
        .await
        .unwrap();
    store
        .publish_summary_checkpoint(
            "chat-source",
            Some("checkpoint-ancestor"),
            SummaryCheckpointDraft {
                checkpoint_id: "checkpoint-descendant".to_string(),
                previous_checkpoint_id: Some("checkpoint-ancestor".to_string()),
                covered_turn_range: CheckpointTurnRange {
                    start_turn_ordinal: 1,
                    end_turn_ordinal: 3,
                },
                covered_sequence_range: CheckpointSequenceRange {
                    start_sequence: 1,
                    end_sequence: 6,
                },
                source_sequence_range: CheckpointSequenceRange {
                    start_sequence: 3,
                    end_sequence: 6,
                },
                frontier: descendant_state.frontier,
                retained_sequence_ranges: descendant_state.retained_sequence_ranges,
                projection_lineage: descendant_state.projection_lineage,
                summary: "summary beyond the branch point".to_string(),
                token_estimate: 12,
                model: CheckpointModelIdentity {
                    provider: "test".to_string(),
                    model: "test-model".to_string(),
                },
                created_at: "2026-06-19T00:02:00Z".to_string(),
            },
        )
        .await
        .unwrap();

    store
        .save_chat_header(&header("chat-branch"))
        .await
        .unwrap();
    append_turns(&store, "chat-branch", 2).await;

    let inherited = store
        .inherit_compatible_summary_checkpoint("chat-source", "chat-branch")
        .await
        .unwrap()
        .expect("compatible checkpoint ancestor");
    assert_eq!(inherited.summary, "summary through source turn 1");
    assert_eq!(inherited.previous_checkpoint_id, None);
    assert_eq!(inherited.frontier.end_sequence, 2);

    let projection = store
        .build_context_projection(
            "chat-branch",
            ContextProjectionRequest {
                current_user_input: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        projection.summary.as_deref(),
        Some("summary through source turn 1")
    );
    assert_eq!(
        projection
            .tail_messages
            .iter()
            .map(|message| message.message_id.as_str())
            .collect::<Vec<_>>(),
        vec!["user-2", "assistant-2"]
    );
}

#[tokio::test]
async fn branch_can_inherit_checkpoint_for_variant_not_selected_in_source_chat() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&header("chat-source-variant"))
        .await
        .unwrap();
    store
        .append_conversation_turn(
            "chat-source-variant",
            NewConversationTurn {
                turn_id: "turn-variant".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-06-19T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("assistant-a".to_string()),
                messages: vec![
                    message("user-variant", "user", "choose"),
                    message("assistant-a", "assistant", "variant A"),
                ],
            },
        )
        .await
        .unwrap();
    let variant_a_state = store
        .checkpoint_projection_state("chat-source-variant", 2)
        .await
        .unwrap();
    store
        .publish_summary_checkpoint(
            "chat-source-variant",
            None,
            SummaryCheckpointDraft {
                checkpoint_id: "checkpoint-variant-a".to_string(),
                previous_checkpoint_id: None,
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
                frontier: variant_a_state.frontier,
                retained_sequence_ranges: variant_a_state.retained_sequence_ranges,
                projection_lineage: variant_a_state.projection_lineage,
                summary: "summary for variant A".to_string(),
                token_estimate: 8,
                model: CheckpointModelIdentity {
                    provider: "test".to_string(),
                    model: "test-model".to_string(),
                },
                created_at: "2026-06-19T00:01:00Z".to_string(),
            },
        )
        .await
        .unwrap();
    store
        .append_messages_to_turn_as_new_variant(
            "chat-source-variant",
            "turn-variant",
            TurnLifecycle {
                user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                assistant_started_at: Some("2026-06-19T00:00:01Z".to_string()),
                assistant_completed_at: Some("2026-06-19T00:00:03Z".to_string()),
                status: "completed".to_string(),
            },
            Some("assistant-b".to_string()),
            vec![message("assistant-b", "assistant", "variant B")],
        )
        .await
        .unwrap();
    store
        .set_turn_selected_variant(
            "chat-source-variant",
            "turn-variant",
            "assistant-b".to_string(),
        )
        .await
        .unwrap();
    assert!(store
        .load_active_summary_checkpoint("chat-source-variant")
        .await
        .unwrap()
        .is_none());

    store
        .save_chat_header(&header("chat-branch-variant"))
        .await
        .unwrap();
    store
        .append_conversation_turn(
            "chat-branch-variant",
            NewConversationTurn {
                turn_id: "turn-variant".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-06-19T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("assistant-a".to_string()),
                messages: vec![
                    message("user-variant", "user", "choose"),
                    message("assistant-a", "assistant", "variant A"),
                ],
            },
        )
        .await
        .unwrap();

    let inherited = store
        .inherit_compatible_summary_checkpoint("chat-source-variant", "chat-branch-variant")
        .await
        .unwrap()
        .expect("variant A checkpoint");
    assert_eq!(inherited.summary, "summary for variant A");
    assert_eq!(inherited.frontier.end_sequence, 2);
}

#[tokio::test]
async fn workspace_store_projection_uses_rollback_active_boundary_without_deleting_raw_log() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store.save_chat_header(&header("chat-1")).await.unwrap();
    append_turns(&store, "chat-1", 3).await;

    let rolled_back = store
        .rollback_active_conversation_to_turn("chat-1", "turn-2", "assistant-2".to_string())
        .await
        .unwrap()
        .expect("active turn");
    assert_eq!(rolled_back.last_sequence, 4);

    let raw_messages = store.read_message_range("chat-1", 1, 6).await.unwrap();
    assert_eq!(
        raw_messages.len(),
        6,
        "rollback changes the active projection boundary, not the raw Message Log"
    );

    let projection = store
        .build_context_projection(
            "chat-1",
            ContextProjectionRequest {
                current_user_input: None,
            },
        )
        .await
        .unwrap();

    let ids: Vec<_> = projection
        .tail_messages
        .iter()
        .map(|message| message.message_id.as_str())
        .collect();
    assert_eq!(ids, vec!["user-1", "assistant-1", "user-2", "assistant-2"]);
    assert_eq!(projection.tail_end_sequence, Some(4));
    assert!(
        !dir.path().join("sessions/chat-1/projection_cache").exists(),
        "projections should not duplicate raw Message Log into cache files"
    );
}

#[tokio::test]
async fn workspace_store_can_rollback_active_projection_to_empty() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store.save_chat_header(&header("chat-1")).await.unwrap();
    append_turns(&store, "chat-1", 2).await;

    assert!(store
        .rollback_active_conversation_to_empty("chat-1")
        .await
        .unwrap());

    let raw_messages = store.read_message_range("chat-1", 1, 4).await.unwrap();
    assert_eq!(raw_messages.len(), 4);

    let projection = store
        .build_context_projection(
            "chat-1",
            ContextProjectionRequest {
                current_user_input: None,
            },
        )
        .await
        .unwrap();
    assert!(projection.tail_messages.is_empty());
    assert_eq!(projection.tail_end_sequence, None);

    store
        .append_conversation_turn(
            "chat-1",
            NewConversationTurn {
                turn_id: "turn-new".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:02:00Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:02:01Z".to_string()),
                    assistant_completed_at: Some("2026-06-19T00:02:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("assistant-new".to_string()),
                messages: vec![
                    message("user-new", "user", "new question"),
                    message("assistant-new", "assistant", "new answer"),
                ],
            },
        )
        .await
        .unwrap();

    let projection = store
        .build_context_projection(
            "chat-1",
            ContextProjectionRequest {
                current_user_input: None,
            },
        )
        .await
        .unwrap();
    let ids = projection
        .tail_messages
        .iter()
        .map(|message| message.message_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["user-new", "assistant-new"]);
}

#[tokio::test]
async fn workspace_store_clear_boundary_invalidates_prior_checkpoint_without_deleting_raw_log() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store.save_chat_header(&header("chat-1")).await.unwrap();
    append_turns(&store, "chat-1", 2).await;
    store
        .publish_summary_checkpoint("chat-1", None, checkpoint(&store, "chat-1", 2).await)
        .await
        .unwrap();

    store
        .record_clear_projection_boundary("chat-1")
        .await
        .unwrap();
    store
        .append_conversation_turn(
            "chat-1",
            NewConversationTurn {
                turn_id: "turn-clear".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:00:03Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:00:04Z".to_string()),
                    assistant_completed_at: Some("2026-06-19T00:00:05Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("assistant-clear".to_string()),
                messages: vec![
                    message("user-clear", "user", "/clear"),
                    message("assistant-clear", "assistant", "cleared"),
                ],
            },
        )
        .await
        .unwrap();
    store
        .append_conversation_turn(
            "chat-1",
            NewConversationTurn {
                turn_id: "turn-fresh".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:00:06Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:00:07Z".to_string()),
                    assistant_completed_at: Some("2026-06-19T00:00:08Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("assistant-fresh".to_string()),
                messages: vec![
                    message("user-fresh", "user", "fresh question"),
                    message("assistant-fresh", "assistant", "fresh answer"),
                ],
            },
        )
        .await
        .unwrap();

    let raw_messages = store.read_message_range("chat-1", 1, 8).await.unwrap();
    assert_eq!(
        raw_messages.len(),
        8,
        "clear is an active projection boundary, not a raw log delete"
    );

    let projection = store
        .build_context_projection(
            "chat-1",
            ContextProjectionRequest {
                current_user_input: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(projection.summary, None);
    assert_eq!(projection.active_checkpoint_id, None);
    let ids: Vec<_> = projection
        .tail_messages
        .iter()
        .map(|message| message.message_id.as_str())
        .collect();
    assert_eq!(
        ids,
        vec![
            "user-clear",
            "assistant-clear",
            "user-fresh",
            "assistant-fresh"
        ]
    );
}

#[tokio::test]
async fn workspace_store_builds_emergency_bounded_projection_without_scanning_full_history() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store.save_chat_header(&header("chat-1")).await.unwrap();
    append_turns(&store, "chat-1", 6).await;

    std::fs::write(
        dir.path()
            .join("sessions/chat-1/messages/000000000000.jsonl"),
        "{older chunk should not be parsed",
    )
    .unwrap();

    let projection = store
        .build_emergency_context_projection(
            &ChatAddress::agent("agent-a", "chat-1"),
            9,
            ContextProjectionRequest {
                current_user_input: Some("must fit".to_string()),
            },
            false,
            4.0,
        )
        .await
        .unwrap();

    let ids: Vec<_> = projection
        .tail_messages
        .iter()
        .map(|message| message.message_id.as_str())
        .collect();
    assert_eq!(ids, vec!["user-5", "assistant-5", "user-6", "assistant-6"]);
    assert_eq!(projection.summary, None);
    assert_eq!(projection.tail_start_sequence, Some(9));
    assert_eq!(projection.tail_end_sequence, Some(12));
}

#[tokio::test]
async fn checkpoint_frontier_cannot_split_one_assistant_response() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    let chat_id = "split-response-boundaries";
    store.save_chat_header(&header(chat_id)).await.unwrap();
    let mut reasoning = message("reasoning", "assistant", "thinking");
    reasoning.content = json!([{"type": "thinking", "thinking": "plan the next tools"}]);
    let mut text = message("text", "assistant", "calling tools");
    let mut tool_a = tool_use_message("tool-a", "call-a");
    let mut tool_b = tool_use_message("tool-b", "call-b");
    for part in [&mut reasoning, &mut text, &mut tool_a, &mut tool_b] {
        part.metadata.insert(
            agent_types::conversation_message_identity::MODEL_DIALOGUE_ITEM_ID_METADATA_KEY
                .to_string(),
            json!("model-call-1"),
        );
    }
    let lifecycle = TurnLifecycle {
        user_submitted_at: "2026-09-20T07:05:00Z".to_string(),
        assistant_started_at: Some("2026-09-20T07:05:01Z".to_string()),
        assistant_completed_at: None,
        status: "in_progress".to_string(),
    };
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: lifecycle.clone(),
                selected_variant_id: None,
                messages: vec![
                    message("user", "user", "continue"),
                    reasoning,
                    text,
                    tool_a,
                    tool_b,
                    tool_result_message("result-a", "call-a"),
                ],
            },
        )
        .await
        .unwrap();
    let pending_groups = store
        .projection_message_group_estimates_effective(chat_id, false, 4.0)
        .await
        .unwrap();
    assert_eq!(
        pending_groups
            .iter()
            .map(|g| (g.first_sequence, g.last_sequence))
            .collect::<Vec<_>>(),
        vec![(1, 1)],
        "neither reasoning nor text is foldable while another tool in this response is pending"
    );
    store
        .append_messages_to_turn(
            chat_id,
            "turn-1",
            lifecycle,
            None,
            vec![
                tool_result_message("result-b", "call-b"),
                message("final", "assistant", "done"),
            ],
        )
        .await
        .unwrap();
    let groups = store
        .projection_message_group_estimates_effective(chat_id, false, 4.0)
        .await
        .unwrap();
    assert_eq!(
        groups
            .iter()
            .map(|g| (g.first_sequence, g.last_sequence))
            .collect::<Vec<_>>(),
        vec![(1, 1), (2, 7), (8, 8)]
    );
    for sequence in 2..7 {
        assert!(
            store
                .checkpoint_projection_state(chat_id, sequence)
                .await
                .is_err(),
            "unsafe frontier {sequence}"
        );
    }
    assert!(store.checkpoint_projection_state(chat_id, 7).await.is_ok());
}

#[tokio::test]
async fn projection_exclusions_preserve_an_unrelated_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    let chat_id = "excluded-tail";
    store.save_chat_header(&header(chat_id)).await.unwrap();
    append_turns(&store, chat_id, 6).await;
    store
        .publish_summary_checkpoint(chat_id, None, checkpoint(&store, chat_id, 8).await)
        .await
        .unwrap();
    store
        .save_projection_exclusions(chat_id, vec!["assistant-6".into()])
        .await
        .unwrap();

    let projected = store
        .build_effective_context_projection(
            chat_id,
            ContextProjectionRequest {
                current_user_input: None,
            },
            true,
            4.0,
        )
        .await
        .unwrap();
    assert_eq!(
        projected.active_checkpoint_id.as_deref(),
        Some("checkpoint-1")
    );
    assert_eq!(projected.tail_start_sequence, Some(9));
    assert_eq!(projected.tail_messages.len(), 3);
    assert!(projected
        .tail_messages
        .iter()
        .all(|m| m.message_id != "assistant-6"));
    assert_eq!(
        store
            .read_message_range(chat_id, 1, 20)
            .await
            .unwrap()
            .len(),
        12
    );
}

#[tokio::test]
async fn projection_exclusions_rebuild_covered_history_and_restore_it_when_cleared() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    let chat_id = "excluded-prefix";
    store.save_chat_header(&header(chat_id)).await.unwrap();
    append_turns(&store, chat_id, 6).await;
    store
        .publish_summary_checkpoint(chat_id, None, checkpoint(&store, chat_id, 2).await)
        .await
        .unwrap();
    let mut newer = checkpoint(&store, chat_id, 8).await;
    newer.checkpoint_id = "checkpoint-2".into();
    newer.previous_checkpoint_id = Some("checkpoint-1".into());
    newer.source_sequence_range.start_sequence = 3;
    store
        .publish_summary_checkpoint(chat_id, Some("checkpoint-1"), newer)
        .await
        .unwrap();
    store
        .save_projection_exclusions(chat_id, vec!["assistant-3".into()])
        .await
        .unwrap();

    assert_eq!(
        store
            .load_active_summary_checkpoint(chat_id)
            .await
            .unwrap()
            .unwrap()
            .checkpoint_id,
        "checkpoint-1",
    );
    let projected = store
        .build_context_projection(
            chat_id,
            ContextProjectionRequest {
                current_user_input: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(projected.tail_start_sequence, Some(3));
    assert!(projected
        .tail_messages
        .iter()
        .all(|m| m.message_id != "assistant-3"));
    assert!(projected
        .tail_messages
        .iter()
        .any(|m| m.message_id == "assistant-2"));

    let mut repaired = checkpoint(&store, chat_id, 8).await;
    repaired.checkpoint_id = "checkpoint-repaired".into();
    repaired.previous_checkpoint_id = Some("checkpoint-1".into());
    repaired.source_sequence_range.start_sequence = 3;
    repaired.summary = "summary generated without assistant-3".into();
    store
        .publish_summary_checkpoint(chat_id, Some("checkpoint-2"), repaired)
        .await
        .unwrap();
    let rebuilt = store
        .build_context_projection(
            chat_id,
            ContextProjectionRequest {
                current_user_input: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        rebuilt.active_checkpoint_id.as_deref(),
        Some("checkpoint-repaired")
    );

    store
        .save_projection_exclusions(chat_id, Vec::new())
        .await
        .unwrap();
    let restored = store
        .build_context_projection(
            chat_id,
            ContextProjectionRequest {
                current_user_input: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        restored.active_checkpoint_id.as_deref(),
        Some("checkpoint-1")
    );
    assert!(restored
        .tail_messages
        .iter()
        .any(|m| m.message_id == "assistant-3"));
}

#[tokio::test]
async fn projection_exclusions_changed_during_summary_reject_stale_publication() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    let chat_id = "excluded-during-summary";
    store.save_chat_header(&header(chat_id)).await.unwrap();
    append_turns(&store, chat_id, 5).await;
    let draft = checkpoint(&store, chat_id, 8).await;
    store
        .save_projection_exclusions(chat_id, vec!["assistant-2".into()])
        .await
        .unwrap();
    let error = store
        .publish_summary_checkpoint(chat_id, None, draft)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("projection lineage is stale"));
    assert!(store
        .load_chat_header(chat_id)
        .await
        .unwrap()
        .unwrap()
        .active_checkpoint_id
        .is_none());
}

#[tokio::test]
async fn projection_exclusions_apply_to_emergency_history() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    let chat_id = "excluded-emergency";
    store.save_chat_header(&header(chat_id)).await.unwrap();
    append_turns(&store, chat_id, 6).await;
    store
        .publish_summary_checkpoint(chat_id, None, checkpoint(&store, chat_id, 4).await)
        .await
        .unwrap();
    store
        .save_projection_exclusions(chat_id, vec!["assistant-5".into()])
        .await
        .unwrap();
    let projection = store
        .build_emergency_context_projection(
            &rua_core::ChatAddress::agent("agent-1", chat_id),
            9,
            ContextProjectionRequest {
                current_user_input: None,
            },
            true,
            4.0,
        )
        .await
        .unwrap();
    assert_eq!(
        projection.active_checkpoint_id.as_deref(),
        Some("checkpoint-1")
    );
    assert_eq!(
        projection
            .tail_messages
            .iter()
            .map(|m| m.message_id.as_str())
            .collect::<Vec<_>>(),
        vec!["user-5", "user-6", "assistant-6"]
    );
}

#[tokio::test]
async fn projection_exclusions_inherited_checkpoints_match_the_target_history() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    for chat in ["source", "unfiltered-branch", "filtered-branch"] {
        store.save_chat_header(&header(chat)).await.unwrap();
        append_turns(&store, chat, 5).await;
    }
    store
        .save_projection_exclusions("source", vec!["assistant-2".into()])
        .await
        .unwrap();
    store
        .publish_summary_checkpoint("source", None, checkpoint(&store, "source", 8).await)
        .await
        .unwrap();
    assert!(store
        .inherit_compatible_summary_checkpoint("source", "unfiltered-branch")
        .await
        .unwrap()
        .is_none());
    store
        .save_projection_exclusions("filtered-branch", vec!["assistant-2".into()])
        .await
        .unwrap();
    let inherited = store
        .inherit_compatible_summary_checkpoint("source", "filtered-branch")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(inherited.covered_excluded_message_ids, vec!["assistant-2"]);
    assert_eq!(
        store
            .load_active_summary_checkpoint("filtered-branch")
            .await
            .unwrap()
            .unwrap()
            .checkpoint_id,
        inherited.checkpoint_id
    );
}
