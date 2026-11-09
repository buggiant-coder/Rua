use rua_server::services::context_projection::projection_message_group_estimates;
use rua_session::{
    ChatMetadata, ChatSessionHeader, NewConversationMessage, NewConversationTurn, TurnLifecycle,
    WorkspaceSessionStore,
};
use serde_json::json;

fn message(message_id: &str, role: &str, content: serde_json::Value) -> NewConversationMessage {
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

async fn active_turn(
    chat_id: &str,
    turn_id: &str,
    user_message_id: &str,
) -> (tempfile::TempDir, WorkspaceSessionStore, TurnLifecycle) {
    let workspace = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(workspace.path());
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "agent-1".to_string(),
            title: "Message groups".to_string(),
            created_at: "2026-07-24T00:00:00Z".to_string(),
            last_activity_at: "2026-07-24T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .unwrap();
    let lifecycle = TurnLifecycle {
        user_submitted_at: "2026-07-24T00:00:00Z".to_string(),
        assistant_started_at: None,
        assistant_completed_at: None,
        status: "in_progress".to_string(),
    };
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: turn_id.to_string(),
                lifecycle: lifecycle.clone(),
                selected_variant_id: None,
                messages: vec![message(
                    user_message_id,
                    "user",
                    json!([{"type": "text", "text": "run both steps"}]),
                )],
            },
        )
        .await
        .unwrap();
    (workspace, store, lifecycle)
}

#[tokio::test]
async fn active_turn_estimates_derive_complete_tool_groups_from_messages() {
    let (_workspace, store, lifecycle) = active_turn("chat-1", "turn-1", "user-1").await;
    let assistant_1 = message(
        "assistant-1",
        "assistant",
        json!([{"type": "tool_use", "id": "call-1", "name": "One"}]),
    );
    let result_1 = message(
        "result-1",
        "system",
        json!([{"type": "tool_result", "call_id": "call-1", "output": "one"}]),
    );
    let assistant_2 = message(
        "assistant-2",
        "assistant",
        json!([{"type": "tool_use", "id": "call-2", "name": "Two"}]),
    );
    let result_2 = message(
        "result-2",
        "system",
        json!([{"type": "tool_result", "call_id": "call-2", "output": "two"}]),
    );
    store
        .append_messages_to_turn(
            "chat-1",
            "turn-1",
            lifecycle.clone(),
            Some("assistant-1".to_string()),
            vec![assistant_1.clone(), result_1.clone()],
        )
        .await
        .unwrap();
    store
        .append_messages_to_turn(
            "chat-1",
            "turn-1",
            lifecycle,
            Some("assistant-2".to_string()),
            vec![assistant_2, result_2],
        )
        .await
        .unwrap();

    let turn = store
        .read_turn_range("chat-1", 1, 1)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert!(turn.metadata.get("model_call_boundaries").is_none());

    let groups = projection_message_group_estimates(&store, "chat-1")
        .await
        .unwrap();

    assert_eq!(groups.len(), 3);
    assert_eq!((groups[0].first_sequence, groups[0].last_sequence), (1, 1));
    assert_eq!((groups[1].first_sequence, groups[1].last_sequence), (2, 3));
    assert_eq!((groups[2].first_sequence, groups[2].last_sequence), (4, 5));
    assert_eq!(groups[0].retained_sequence_ranges[0].start_sequence, 1);
    assert_eq!(groups[1].retained_sequence_ranges[0].start_sequence, 1);
    assert_eq!(groups[2].retained_sequence_ranges[0].start_sequence, 1);
}

#[tokio::test]
async fn active_turn_waits_for_every_parallel_tool_result() {
    let (_workspace, store, lifecycle) =
        active_turn("chat-parallel", "turn-parallel", "user-parallel").await;
    store
        .append_messages_to_turn(
            "chat-parallel",
            "turn-parallel",
            lifecycle.clone(),
            Some("assistant-parallel".to_string()),
            vec![
                message(
                    "assistant-parallel",
                    "assistant",
                    json!([
                        {"type": "tool_use", "id": "call-1", "name": "One"},
                        {"type": "tool_use", "id": "call-2", "name": "Two"}
                    ]),
                ),
                message(
                    "result-1",
                    "system",
                    json!([{"type": "tool_result", "call_id": "call-1", "output": "one"}]),
                ),
            ],
        )
        .await
        .unwrap();

    let groups = projection_message_group_estimates(&store, "chat-parallel")
        .await
        .unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!((groups[0].first_sequence, groups[0].last_sequence), (1, 1));

    store
        .append_messages_to_turn(
            "chat-parallel",
            "turn-parallel",
            lifecycle,
            Some("assistant-parallel".to_string()),
            vec![message(
                "result-2",
                "system",
                json!([{"type": "tool_result", "call_id": "call-2", "output": "two"}]),
            )],
        )
        .await
        .unwrap();

    let groups = projection_message_group_estimates(&store, "chat-parallel")
        .await
        .unwrap();
    assert_eq!(groups.len(), 2);
    assert_eq!((groups[0].first_sequence, groups[0].last_sequence), (1, 1));
    assert_eq!((groups[1].first_sequence, groups[1].last_sequence), (2, 4));
}
