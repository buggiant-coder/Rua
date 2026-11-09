use rua_session::{
    ChatMetadata, ChatSessionHeader, CheckpointModelIdentity, CheckpointSequenceRange,
    CheckpointTurnRange, ContextProjectionRequest, ConversationReadSpanRequest,
    ConversationRecallRequest, NewConversationMessage, NewConversationTurn, SummaryCheckpointDraft,
    TurnLifecycle, WorkspaceSessionStore,
};
use serde_json::json;

fn header(chat_id: &str, title: &str) -> ChatSessionHeader {
    let mut header = ChatSessionHeader::new(ChatMetadata {
        chat_id: chat_id.to_string(),
        agent_id: "agent-1".to_string(),
        title: title.to_string(),
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

async fn append_turn(store: &WorkspaceSessionStore, chat_id: &str, ordinal: u64, text: &str) {
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: format!("{chat_id}-turn-{ordinal}"),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-06-19T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some(format!("variant-{ordinal}")),
                messages: vec![
                    message(&format!("{chat_id}-user-{ordinal}"), "user", text),
                    message(
                        &format!("{chat_id}-assistant-{ordinal}"),
                        "assistant",
                        "ack",
                    ),
                ],
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn conversation_recall_searches_only_current_chat_active_messages() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&header("chat-1", "Current"))
        .await
        .unwrap();
    store
        .save_chat_header(&header("chat-2", "Other"))
        .await
        .unwrap();
    append_turn(&store, "chat-1", 1, "needle belongs to current chat").await;
    append_turn(&store, "chat-2", 1, "needle belongs to other chat").await;

    let result = store
        .conversation_recall(
            "chat-1",
            ConversationRecallRequest {
                query: "needle".to_string(),
                max_results: 10,
                surrounding_messages: 0,
            },
        )
        .await
        .unwrap();

    assert_eq!(result.chat_id, "chat-1");
    assert_eq!(result.hits.len(), 1);
    let hit = &result.hits[0];
    assert_eq!(hit.message.message_id, "chat-1-user-1");
    assert_eq!(hit.message.turn_id, "chat-1-turn-1");
    assert_eq!(hit.message.role, "user");
    assert!(hit.snippet.contains("needle belongs to current chat"));
    assert_eq!(hit.span.start_sequence, hit.message.sequence);
    assert_eq!(hit.span.end_sequence, hit.message.sequence);
}

#[tokio::test]
async fn conversation_recall_treats_whitespace_terms_as_alternatives() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&header("chat-1", "Current"))
        .await
        .unwrap();
    append_turn(&store, "chat-1", 1, "给我说一个笑话").await;

    let result = store
        .conversation_recall(
            "chat-1",
            ConversationRecallRequest {
                query: "笑话 joke".to_string(),
                max_results: 10,
                surrounding_messages: 0,
            },
        )
        .await
        .unwrap();

    assert_eq!(result.hits.len(), 1, "recall body: {result:?}");
    assert_eq!(result.hits[0].message.message_id, "chat-1-user-1");
    assert!(result.hits[0].snippet.contains("笑话"));
}

#[tokio::test]
async fn conversation_recall_and_read_span_respect_clear_projection_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&header("chat-1", "Current"))
        .await
        .unwrap();
    append_turn(&store, "chat-1", 1, "secret before clear").await;
    store
        .record_clear_projection_boundary("chat-1")
        .await
        .unwrap();
    append_turn(&store, "chat-1", 2, "needle after clear").await;

    let old = store
        .conversation_recall(
            "chat-1",
            ConversationRecallRequest {
                query: "secret".to_string(),
                max_results: 10,
                surrounding_messages: 2,
            },
        )
        .await
        .unwrap();
    assert!(
        old.hits.is_empty(),
        "conversation recall must not cross /clear boundary: {old:?}"
    );

    let new = store
        .conversation_recall(
            "chat-1",
            ConversationRecallRequest {
                query: "needle".to_string(),
                max_results: 10,
                surrounding_messages: 2,
            },
        )
        .await
        .unwrap();
    assert_eq!(new.hits.len(), 1, "recall body: {new:?}");

    let span = store
        .conversation_read_span(
            "chat-1",
            ConversationReadSpanRequest {
                start_sequence: 1,
                end_sequence: new.hits[0].span.end_sequence,
                expand_before: 10,
                expand_after: 0,
            },
        )
        .await
        .unwrap();
    let joined = span
        .messages
        .iter()
        .map(|message| message.content.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(joined.contains("needle after clear"));
    assert!(!joined.contains("secret before clear"));
}

#[tokio::test]
async fn conversation_recall_reads_selected_variants_not_raw_regenerations() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&header("chat-1", "Variant recall"))
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
                selected_variant_id: Some("assistant-first".to_string()),
                messages: vec![
                    message("user-1", "user", "tell me a cold joke"),
                    message(
                        "assistant-first",
                        "assistant",
                        "inactive first marker answer",
                    ),
                ],
            },
        )
        .await
        .unwrap();
    store
        .append_messages_to_turn_as_new_variant(
            "chat-1",
            "turn-1",
            TurnLifecycle {
                user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                assistant_started_at: Some("2026-06-19T00:00:01Z".to_string()),
                assistant_completed_at: Some("2026-06-19T00:00:03Z".to_string()),
                status: "completed".to_string(),
            },
            Some("assistant-second".to_string()),
            vec![message(
                "assistant-second",
                "assistant",
                "selected marker marshmallow soft joke",
            )],
        )
        .await
        .unwrap();
    store
        .append_messages_to_turn_as_new_variant(
            "chat-1",
            "turn-1",
            TurnLifecycle {
                user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                assistant_started_at: Some("2026-06-19T00:00:01Z".to_string()),
                assistant_completed_at: Some("2026-06-19T00:00:04Z".to_string()),
                status: "completed".to_string(),
            },
            Some("assistant-third".to_string()),
            vec![message(
                "assistant-third",
                "assistant",
                "inactive third marker hot cold joke",
            )],
        )
        .await
        .unwrap();
    assert!(store
        .set_turn_selected_variant("chat-1", "turn-1", "assistant-second".to_string())
        .await
        .unwrap());
    append_turn(&store, "chat-1", 2, "continue after selected variant").await;

    let raw_messages = store.read_message_range("chat-1", 1, 10).await.unwrap();
    assert!(
        raw_messages
            .iter()
            .any(|message| message.message_id == "assistant-third"),
        "raw Message Log keeps unselected regenerate attempts"
    );

    let recall = store
        .conversation_recall(
            "chat-1",
            ConversationRecallRequest {
                query: "marker".to_string(),
                max_results: 10,
                surrounding_messages: 2,
            },
        )
        .await
        .unwrap();

    assert_eq!(recall.hits.len(), 1, "recall body: {recall:?}");
    assert_eq!(recall.hits[0].message.message_id, "assistant-second");
    assert!(recall.hits[0].snippet.contains("marshmallow soft"));
    assert!(!recall.hits[0].snippet.contains("hot cold"));

    let span = store
        .conversation_read_span(
            "chat-1",
            ConversationReadSpanRequest {
                start_sequence: recall.hits[0].span.start_sequence,
                end_sequence: recall.hits[0].span.end_sequence,
                expand_before: 0,
                expand_after: 0,
            },
        )
        .await
        .unwrap();
    let span_ids = span
        .messages
        .iter()
        .map(|message| message.message_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        span_ids,
        vec![
            "user-1",
            "assistant-second",
            "chat-1-user-2",
            "chat-1-assistant-2"
        ]
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
    let projected_ids = projection
        .tail_messages
        .iter()
        .map(|message| message.message_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        projected_ids,
        vec![
            "user-1",
            "assistant-second",
            "chat-1-user-2",
            "chat-1-assistant-2"
        ]
    );
}

#[tokio::test]
async fn conversation_read_span_expands_hit_by_sequence_without_search_query() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&header("chat-1", "Current"))
        .await
        .unwrap();
    append_turn(&store, "chat-1", 1, "first").await;
    append_turn(&store, "chat-1", 2, "needle in middle").await;
    append_turn(&store, "chat-1", 3, "third").await;

    let recall = store
        .conversation_recall(
            "chat-1",
            ConversationRecallRequest {
                query: "needle".to_string(),
                max_results: 1,
                surrounding_messages: 0,
            },
        )
        .await
        .unwrap();
    let hit = &recall.hits[0];

    let expanded = store
        .conversation_read_span(
            "chat-1",
            ConversationReadSpanRequest {
                start_sequence: hit.span.start_sequence,
                end_sequence: hit.span.end_sequence,
                expand_before: 1,
                expand_after: 2,
            },
        )
        .await
        .unwrap();

    assert_eq!(expanded.chat_id, "chat-1");
    assert_eq!(expanded.span.start_sequence, 2);
    assert_eq!(expanded.span.end_sequence, 5);
    let ids: Vec<_> = expanded
        .messages
        .iter()
        .map(|message| message.message_id.as_str())
        .collect();
    assert_eq!(
        ids,
        vec![
            "chat-1-assistant-1",
            "chat-1-user-2",
            "chat-1-assistant-2",
            "chat-1-user-3"
        ]
    );
}

#[tokio::test]
async fn conversation_recall_does_not_search_summary_checkpoints() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&header("chat-1", "Current"))
        .await
        .unwrap();
    append_turn(&store, "chat-1", 1, "ordinary raw message").await;
    let projection_state = store
        .checkpoint_projection_state("chat-1", 2)
        .await
        .unwrap();
    store
        .publish_summary_checkpoint(
            "chat-1",
            None,
            SummaryCheckpointDraft {
                checkpoint_id: "checkpoint-1".to_string(),
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
                frontier: projection_state.frontier,
                retained_sequence_ranges: projection_state.retained_sequence_ranges,
                projection_lineage: projection_state.projection_lineage,
                summary: "checkpoint-only-secret".to_string(),
                token_estimate: 12,
                model: CheckpointModelIdentity {
                    provider: "openai".to_string(),
                    model: "gpt-5".to_string(),
                },
                created_at: "2026-06-19T00:01:00Z".to_string(),
            },
        )
        .await
        .unwrap();

    let result = store
        .conversation_recall(
            "chat-1",
            ConversationRecallRequest {
                query: "checkpoint-only-secret".to_string(),
                max_results: 10,
                surrounding_messages: 1,
            },
        )
        .await
        .unwrap();

    assert!(
        result.hits.is_empty(),
        "conversation recall must search raw transcript messages, not checkpoint summaries"
    );
}
