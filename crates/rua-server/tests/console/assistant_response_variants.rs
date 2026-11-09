use rua_channel::core::console::SessionPersistence;
use rua_contract::agent::AgentInputMessage;
use rua_core::message::ContentPart;
use rua_server::services::console_chat::{StoredMessage, WorkspaceSessionPersistence};

fn stored(id: &str, role: &str, text: &str) -> StoredMessage {
    StoredMessage {
        id: id.to_string(),
        role: role.to_string(),
        content: vec![serde_json::json!({"type": "text", "text": text})],
        name: role.to_string(),
        timestamp: "2026-06-18 00:00:00.000".to_string(),
        device_id: String::new(),
        metadata: serde_json::json!({}),
        extra: serde_json::Map::new(),
    }
}

async fn seed_evergreen_turn(
    store: &rua_session::WorkspaceSessionStore,
    chat_id: &str,
    user: StoredMessage,
    assistant: StoredMessage,
) {
    store
        .save_chat_header(&rua_session::ChatSessionHeader::new(
            rua_session::ChatMetadata {
                chat_id: chat_id.to_string(),
                agent_id: "agent-1".to_string(),
                title: chat_id.to_string(),
                created_at: "2026-06-18T00:00:00Z".to_string(),
                last_activity_at: "2026-06-18T00:00:00Z".to_string(),
                pinned: false,
                archived: false,
                preview: None,
            },
        ))
        .await
        .unwrap();
    store
        .append_conversation_turn(
            chat_id,
            rua_session::NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: rua_session::TurnLifecycle {
                    user_submitted_at: user.timestamp.clone(),
                    assistant_started_at: Some(assistant.timestamp.clone()),
                    assistant_completed_at: Some(assistant.timestamp.clone()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some(assistant.id.clone()),
                messages: vec![
                    stored_to_new_conversation_message(user),
                    stored_to_new_conversation_message(assistant),
                ],
            },
        )
        .await
        .unwrap();
}

fn stored_to_new_conversation_message(
    message: StoredMessage,
) -> rua_session::NewConversationMessage {
    rua_session::NewConversationMessage {
        message_id: message.id,
        role: message.role,
        created_at: message.timestamp,
        status: "completed".to_string(),
        content: serde_json::Value::Array(message.content),
        provider: None,
        tool: None,
        metadata: serde_json::Map::new(),
    }
}

#[tokio::test]
async fn regenerate_persistence_appends_variant_to_existing_turn() {
    let workspace = tempfile::tempdir().unwrap();
    let store = rua_session::WorkspaceSessionStore::new(workspace.path());
    let user = stored("user-1", "user", "Regenerate me");
    let first = stored("assistant-1", "assistant", "First answer");
    seed_evergreen_turn(&store, "session-1", user, first).await;

    let persistence = WorkspaceSessionPersistence::new(workspace.path().to_path_buf())
        .with_regenerate_turn_id("turn-1".to_string());

    persistence
        .save_messages(
            "agent-1",
            "session-1",
            &[AgentInputMessage {
                role: "assistant".to_string(),
                content: vec![ContentPart::Text {
                    text: "Regenerated answer".to_string(),
                }],
            }],
        )
        .await
        .unwrap();

    let turns = store.read_turn_range("session-1", 1, 10).await.unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].turn_id, "turn-1");
    assert_eq!(turns[0].assistant_response_variants.len(), 2);
    let selected_variant_id = turns[0].selected_variant_id.as_deref().unwrap();
    assert_ne!(selected_variant_id, "assistant-1");
    assert_eq!(
        turns[0].assistant_response_sequences,
        turns[0].assistant_response_variants[1].assistant_message_sequences
    );
    let messages = store.read_message_range("session-1", 1, 10).await.unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[2].message_id, selected_variant_id);
    assert_eq!(messages[2].content[0]["text"], "Regenerated answer");
}

#[tokio::test]
async fn regenerate_persistence_ignores_submitted_user_echo() {
    let workspace = tempfile::tempdir().unwrap();
    let store = rua_session::WorkspaceSessionStore::new(workspace.path());
    let user = stored("user-1", "user", "Regenerate me");
    let first = stored("assistant-1", "assistant", "First answer");
    seed_evergreen_turn(&store, "session-1", user, first).await;

    let persistence = WorkspaceSessionPersistence::new(workspace.path().to_path_buf())
        .with_regenerate_turn_id("turn-1".to_string());

    persistence
        .save_messages(
            "agent-1",
            "session-1",
            &[
                AgentInputMessage {
                    role: "user".to_string(),
                    content: vec![ContentPart::Text {
                        text: "Regenerate me".to_string(),
                    }],
                },
                AgentInputMessage {
                    role: "assistant".to_string(),
                    content: vec![ContentPart::Text {
                        text: "Second answer".to_string(),
                    }],
                },
            ],
        )
        .await
        .unwrap();

    let turns = store.read_turn_range("session-1", 1, 10).await.unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].user_message_sequence, 1);
    assert_eq!(turns[0].assistant_response_sequences, vec![3]);
    assert_eq!(turns[0].assistant_response_variants.len(), 2);
    let selected_variant_id = turns[0].selected_variant_id.as_deref().unwrap();
    let messages = store.read_message_range("session-1", 1, 10).await.unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[1].role, "assistant");
    assert_eq!(messages[2].role, "assistant");
    assert_eq!(messages[2].message_id, selected_variant_id);
    assert_eq!(messages[2].content[0]["text"], "Second answer");
}

#[tokio::test]
async fn repeated_user_text_without_explicit_regenerate_creates_new_turn() {
    let workspace = tempfile::tempdir().unwrap();
    let store = rua_session::WorkspaceSessionStore::new(workspace.path());
    let user = stored("user-1", "user", "Same prompt");
    let first = stored("assistant-1", "assistant", "First answer");
    seed_evergreen_turn(&store, "session-1", user, first).await;

    let persistence = WorkspaceSessionPersistence::new(workspace.path().to_path_buf())
        .with_loaded_message_count(2);

    persistence
        .save_messages(
            "agent-1",
            "session-1",
            &[
                AgentInputMessage {
                    role: "user".to_string(),
                    content: vec![ContentPart::Text {
                        text: "Same prompt".to_string(),
                    }],
                },
                AgentInputMessage {
                    role: "assistant".to_string(),
                    content: vec![ContentPart::Text {
                        text: "First answer".to_string(),
                    }],
                },
                AgentInputMessage {
                    role: "user".to_string(),
                    content: vec![ContentPart::Text {
                        text: "Same prompt".to_string(),
                    }],
                },
                AgentInputMessage {
                    role: "assistant".to_string(),
                    content: vec![ContentPart::Text {
                        text: "Second answer".to_string(),
                    }],
                },
            ],
        )
        .await
        .unwrap();

    let turns = store.read_turn_range("session-1", 1, 10).await.unwrap();
    assert_eq!(turns.len(), 2);
    let messages = store.read_message_range("session-1", 1, 10).await.unwrap();
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[2].role, "user");
    assert_eq!(messages[2].content[0]["text"], "Same prompt");
    assert_eq!(messages[3].content[0]["text"], "Second answer");
}
