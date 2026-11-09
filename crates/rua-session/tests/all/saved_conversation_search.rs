use rua_session::{
    ChatMetadata, ChatPreview, ChatSessionHeader, CheckpointModelIdentity, CheckpointSequenceRange,
    CheckpointTurnRange, NewConversationMessage, NewConversationTurn,
    SavedConversationSearchRequest, SummaryCheckpointDraft, TurnLifecycle, WorkspaceSessionStore,
};
use serde_json::json;

fn header(chat_id: &str, title: &str, last_activity_at: &str) -> ChatSessionHeader {
    header_for_agent("agent-1", chat_id, title, last_activity_at)
}

fn header_for_agent(
    agent_id: &str,
    chat_id: &str,
    title: &str,
    last_activity_at: &str,
) -> ChatSessionHeader {
    let mut header = ChatSessionHeader::new(ChatMetadata {
        chat_id: chat_id.to_string(),
        agent_id: agent_id.to_string(),
        title: title.to_string(),
        created_at: "2026-06-19T00:00:00Z".to_string(),
        last_activity_at: last_activity_at.to_string(),
        pinned: false,
        archived: false,
        preview: Some(ChatPreview {
            last_message_sequence: Some(2),
            text: format!("preview for {title}"),
        }),
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

async fn append_turn(store: &WorkspaceSessionStore, chat_id: &str, text: &str) {
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: format!("{chat_id}-turn-1"),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-06-19T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: None,
                messages: vec![
                    message(&format!("{chat_id}-user-1"), "user", text),
                    message(&format!("{chat_id}-assistant-1"), "assistant", "ack"),
                ],
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn saved_conversation_search_finds_active_messages_across_saved_chats_with_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&header("chat-new", "New Match", "2026-06-19T03:00:00Z"))
        .await
        .unwrap();
    store
        .save_chat_header(&header("chat-old", "Old Match", "2026-06-19T01:00:00Z"))
        .await
        .unwrap();
    store
        .save_chat_header(&header("chat-miss", "Miss", "2026-06-19T02:00:00Z"))
        .await
        .unwrap();
    append_turn(&store, "chat-new", "alpha appears in newer saved chat").await;
    append_turn(&store, "chat-old", "alpha appears in older saved chat").await;
    append_turn(&store, "chat-miss", "no match here").await;

    let result = store
        .search_saved_conversations(SavedConversationSearchRequest {
            agent_id: "agent-1".to_string(),
            query: "ALPHA".to_string(),
            offset: 0,
            limit: 10,
        })
        .await
        .unwrap();

    assert_eq!(result.total, 2);
    assert_eq!(result.next_offset, None);
    assert_eq!(result.results.len(), 2);
    assert_eq!(result.results[0].chat_id, "chat-new");
    assert_eq!(result.results[0].title, "New Match");
    assert_eq!(
        result.results[0]
            .preview
            .as_ref()
            .map(|preview| preview.text.as_str()),
        Some("ack")
    );
    assert_eq!(result.results[0].message.message_id, "chat-new-user-1");
    assert_eq!(result.results[0].message.turn_id, "chat-new-turn-1");
    assert_eq!(result.results[0].message.role, "user");
    assert!(result.results[0].snippet.contains("alpha appears"));
    assert_eq!(result.results[0].span.start_sequence, 1);
    assert_eq!(result.results[0].span.end_sequence, 1);
    assert_eq!(result.results[1].chat_id, "chat-old");
}

#[tokio::test]
async fn saved_conversation_search_uses_selected_variants_not_raw_regenerations() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&header(
            "chat-variant",
            "Variant Search",
            "2026-06-19T03:00:00Z",
        ))
        .await
        .unwrap();
    store
        .append_conversation_turn(
            "chat-variant",
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
                    message("user-1", "user", "question"),
                    message(
                        "assistant-first",
                        "assistant",
                        "inactive first saved-marker",
                    ),
                ],
            },
        )
        .await
        .unwrap();
    store
        .append_messages_to_turn_as_new_variant(
            "chat-variant",
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
                "selected saved-marker answer",
            )],
        )
        .await
        .unwrap();
    store
        .append_messages_to_turn_as_new_variant(
            "chat-variant",
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
                "inactive third saved-marker answer",
            )],
        )
        .await
        .unwrap();
    assert!(store
        .set_turn_selected_variant("chat-variant", "turn-1", "assistant-second".to_string())
        .await
        .unwrap());
    append_turn(&store, "chat-variant", "continued").await;

    let result = store
        .search_saved_conversations(SavedConversationSearchRequest {
            agent_id: "agent-1".to_string(),
            query: "saved-marker".to_string(),
            offset: 0,
            limit: 10,
        })
        .await
        .unwrap();

    assert_eq!(result.total, 1, "search body: {result:?}");
    assert_eq!(result.results[0].message.message_id, "assistant-second");
    assert!(result.results[0].snippet.contains("selected"));
}

#[tokio::test]
async fn saved_conversation_search_paginates_hits() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    for index in 1..=3 {
        let chat_id = format!("chat-{index}");
        store
            .save_chat_header(&header(
                &chat_id,
                &format!("Chat {index}"),
                &format!("2026-06-19T0{index}:00:00Z"),
            ))
            .await
            .unwrap();
        append_turn(&store, &chat_id, "needle").await;
    }

    let first = store
        .search_saved_conversations(SavedConversationSearchRequest {
            agent_id: "agent-1".to_string(),
            query: "needle".to_string(),
            offset: 0,
            limit: 2,
        })
        .await
        .unwrap();
    let second = store
        .search_saved_conversations(SavedConversationSearchRequest {
            agent_id: "agent-1".to_string(),
            query: "needle".to_string(),
            offset: 2,
            limit: 2,
        })
        .await
        .unwrap();

    assert_eq!(first.total, 3);
    assert_eq!(first.results.len(), 2);
    assert_eq!(first.next_offset, Some(2));
    assert_eq!(second.results.len(), 1);
    assert_eq!(second.next_offset, None);
}

#[tokio::test]
async fn saved_conversation_search_does_not_search_summary_checkpoints() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&header("chat-1", "Checkpoint Chat", "2026-06-19T01:00:00Z"))
        .await
        .unwrap();
    append_turn(&store, "chat-1", "ordinary raw transcript").await;
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
                summary: "checkpoint-only-search-token".to_string(),
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
        .search_saved_conversations(SavedConversationSearchRequest {
            agent_id: "agent-1".to_string(),
            query: "checkpoint-only-search-token".to_string(),
            offset: 0,
            limit: 10,
        })
        .await
        .unwrap();

    assert!(result.results.is_empty());
    assert_eq!(result.total, 0);
}

#[tokio::test]
async fn saved_conversation_search_is_agent_scoped() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&header_for_agent(
            "agent-1",
            "chat-owned",
            "Owned",
            "2026-06-19T02:00:00Z",
        ))
        .await
        .unwrap();
    store
        .save_chat_header(&header_for_agent(
            "agent-2",
            "chat-other",
            "Other",
            "2026-06-19T03:00:00Z",
        ))
        .await
        .unwrap();
    append_turn(&store, "chat-owned", "scoped-needle belongs here").await;
    append_turn(&store, "chat-other", "scoped-needle must not leak").await;

    let result = store
        .search_saved_conversations(SavedConversationSearchRequest {
            agent_id: "agent-1".to_string(),
            query: "scoped-needle".to_string(),
            offset: 0,
            limit: 10,
        })
        .await
        .unwrap();

    assert_eq!(result.total, 1);
    assert_eq!(result.results[0].chat_id, "chat-owned");
}

#[tokio::test]
async fn saved_conversation_search_can_use_the_current_scope_without_agent_filtering() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&header_for_agent(
            "project-agent",
            "project-chat",
            "Project chat",
            "2026-06-19T03:00:00Z",
        ))
        .await
        .unwrap();
    append_turn(
        &store,
        "project-chat",
        "project-scope-needle is in the message",
    )
    .await;

    let result = store
        .search_saved_conversations_in_current_scope(SavedConversationSearchRequest {
            agent_id: String::new(),
            query: "project-scope-needle".to_string(),
            offset: 0,
            limit: 10,
        })
        .await
        .unwrap();

    assert_eq!(result.total, 1);
    assert_eq!(result.results[0].chat_id, "project-chat");
    store
        .update_chat_header("project-chat", |header| {
            header.extra.insert(
                "collaboration".into(),
                serde_json::json!({"source":{"chat_id":"source"}}),
            );
        })
        .await
        .unwrap();
    let hidden = store
        .search_saved_conversations_in_current_scope(SavedConversationSearchRequest {
            agent_id: String::new(),
            query: "project-scope-needle".into(),
            offset: 0,
            limit: 10,
        })
        .await
        .unwrap();
    assert_eq!(hidden.total, 0);
    assert!(hidden.results.is_empty());
}
