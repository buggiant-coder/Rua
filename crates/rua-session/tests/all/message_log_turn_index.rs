use rua_session::{
    ChatMetadata, ChatSessionHeader, MessageLogEntry, MessageToolLink, NewConversationMessage,
    NewConversationTurn, TurnLifecycle, TurnWriter, WorkspaceSessionStore,
};
use serde_json::json;

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

fn message(message_id: &str, role: &str, text: &str) -> NewConversationMessage {
    NewConversationMessage {
        message_id: message_id.to_string(),
        role: role.to_string(),
        created_at: format!("2026-06-19T00:00:0{}Z", message_id.len()),
        status: "completed".to_string(),
        content: json!([{"type": "text", "text": text}]),
        provider: None,
        tool: None,
        metadata: serde_json::Map::new(),
    }
}

fn tool_message(
    message_id: &str,
    role: &str,
    text: &str,
    tool_name: &str,
    tool_call_id: &str,
) -> NewConversationMessage {
    NewConversationMessage {
        tool: Some(MessageToolLink {
            tool_name: tool_name.to_string(),
            tool_call_id: Some(tool_call_id.to_string()),
        }),
        ..message(message_id, role, text)
    }
}

fn turn_aborted_marker(message_id: &str, text: &str) -> NewConversationMessage {
    NewConversationMessage {
        metadata: serde_json::Map::from_iter([(
            "model_visible_marker".to_string(),
            json!("turn_aborted"),
        )]),
        ..message(message_id, "user", text)
    }
}

fn lifecycle(status: &str) -> TurnLifecycle {
    TurnLifecycle {
        user_submitted_at: "2026-06-19T00:00:01Z".to_string(),
        assistant_started_at: None,
        assistant_completed_at: None,
        status: status.to_string(),
    }
}

#[tokio::test]
async fn concurrent_turn_appends_allocate_distinct_sequences() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store.save_chat_header(&header("chat-race")).await.unwrap();
    store
        .append_conversation_turn(
            "chat-race",
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:00:01Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:00:02Z".to_string()),
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![message("user-1", "user", "run slow command")],
            },
        )
        .await
        .unwrap();

    let assistant_store = WorkspaceSessionStore::new(dir.path());
    let marker_store = WorkspaceSessionStore::new(dir.path());
    let assistant_append = tokio::spawn(async move {
        assistant_store
            .append_messages_to_turn(
                "chat-race",
                "turn-1",
                TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:00:01Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:00:02Z".to_string()),
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                Some("assistant-1".to_string()),
                vec![tool_message(
                    "assistant-1",
                    "assistant",
                    "thinking and tool call",
                    "bash",
                    "call-1",
                )],
            )
            .await
    });
    let marker_append = tokio::spawn(async move {
        marker_store
            .append_messages_to_turn(
                "chat-race",
                "turn-1",
                TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:00:01Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:00:02Z".to_string()),
                    assistant_completed_at: Some("2026-06-19T00:00:05Z".to_string()),
                    status: "interrupted".to_string(),
                },
                Some("assistant-1".to_string()),
                vec![turn_aborted_marker(
                    "turn-aborted-turn-1",
                    "<turn_aborted />",
                )],
            )
            .await
    });

    assistant_append.await.unwrap().unwrap();
    marker_append.await.unwrap().unwrap();

    let messages = store.read_message_range("chat-race", 1, 10).await.unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(
        messages
            .iter()
            .map(|message| message.sequence)
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[1].role, "assistant");
    assert_eq!(messages[2].role, "user");

    let projectable = store
        .read_projectable_message_range("chat-race", 1, 10)
        .await
        .unwrap();
    assert_eq!(
        projectable
            .iter()
            .map(|message| (message.sequence, message.role.as_str()))
            .collect::<Vec<_>>(),
        vec![(1, "user"), (2, "assistant"), (3, "user")]
    );
}

#[tokio::test]
async fn workspace_store_appends_complete_turn_and_reads_by_sequence_and_turn() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store.save_chat_header(&header("chat-1")).await.unwrap();

    let turn = NewConversationTurn {
        turn_id: "turn-1".to_string(),
        lifecycle: TurnLifecycle {
            user_submitted_at: "2026-06-19T00:00:01Z".to_string(),
            assistant_started_at: Some("2026-06-19T00:00:02Z".to_string()),
            assistant_completed_at: Some("2026-06-19T00:00:03Z".to_string()),
            status: "completed".to_string(),
        },
        selected_variant_id: Some("variant-1".to_string()),
        messages: vec![
            message("user-1", "user", "hello"),
            message("assistant-1", "assistant", "hi"),
        ],
    };

    let indexed_turn = store
        .append_conversation_turn("chat-1", turn)
        .await
        .unwrap();

    assert_eq!(indexed_turn.turn_id, "turn-1");
    assert_eq!(indexed_turn.user_message_sequence, 1);
    assert_eq!(indexed_turn.assistant_response_sequences, vec![2]);
    assert_eq!(indexed_turn.first_sequence, 1);
    assert_eq!(indexed_turn.last_sequence, 2);
    assert_eq!(
        indexed_turn.selected_variant_id.as_deref(),
        Some("variant-1")
    );

    let loaded = store.load_chat_header("chat-1").await.unwrap().unwrap();
    assert_eq!(loaded.message_log.latest_sequence, 2);
    assert_eq!(loaded.turn_index.latest_turn, 1);
    assert_eq!(loaded.metadata.preview.as_ref().unwrap().text, "hi");
    assert_eq!(
        loaded.metadata.preview.unwrap().last_message_sequence,
        Some(2)
    );

    let by_sequence = store.read_message_range("chat-1", 1, 10).await.unwrap();
    assert_messages(&by_sequence);

    let by_turn = store.read_turn_messages("chat-1", "turn-1").await.unwrap();
    assert_messages(&by_turn);
}

#[tokio::test]
async fn workspace_store_reads_latest_message_from_header() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&header("chat-latest"))
        .await
        .unwrap();
    store
        .append_conversation_turn(
            "chat-latest",
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: lifecycle("completed"),
                selected_variant_id: None,
                messages: vec![
                    message("user-1", "user", &"large-prefix-".repeat(10_000)),
                    message(
                        "assistant-1",
                        "assistant",
                        &"large-final-answer-".repeat(2_000),
                    ),
                ],
            },
        )
        .await
        .unwrap();

    let loaded = store
        .load_chat_header("chat-latest")
        .await
        .unwrap()
        .unwrap();
    let latest = store
        .read_latest_message_from_header("chat-latest", &loaded.message_log)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(latest.sequence, loaded.message_log.latest_sequence);
    assert_eq!(latest.message_id, "assistant-1");
}

fn assert_messages(messages: &[MessageLogEntry]) {
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].sequence, 1);
    assert_eq!(messages[0].turn_id, "turn-1");
    assert_eq!(messages[0].message_id, "user-1");
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[0].content[0]["text"], "hello");

    assert_eq!(messages[1].sequence, 2);
    assert_eq!(messages[1].turn_id, "turn-1");
    assert_eq!(messages[1].message_id, "assistant-1");
    assert_eq!(messages[1].role, "assistant");
    assert_eq!(messages[1].content[0]["text"], "hi");
}

#[tokio::test]
async fn workspace_store_updates_turn_lifecycle_when_assistant_response_completes() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store.save_chat_header(&header("chat-1")).await.unwrap();

    let started = store
        .append_conversation_turn(
            "chat-1",
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: lifecycle("waiting_for_assistant"),
                selected_variant_id: None,
                messages: vec![message("user-1", "user", "draft this")],
            },
        )
        .await
        .unwrap();

    assert_eq!(started.user_message_sequence, 1);
    assert!(started.assistant_response_sequences.is_empty());
    assert_eq!(started.lifecycle.status, "waiting_for_assistant");

    let completed = store
        .append_messages_to_turn(
            "chat-1",
            "turn-1",
            TurnLifecycle {
                user_submitted_at: "2026-06-19T00:00:01Z".to_string(),
                assistant_started_at: Some("2026-06-19T00:00:02Z".to_string()),
                assistant_completed_at: Some("2026-06-19T00:00:07Z".to_string()),
                status: "completed".to_string(),
            },
            Some("variant-1".to_string()),
            vec![message("assistant-1", "assistant", "drafted")],
        )
        .await
        .unwrap();

    assert_eq!(completed.assistant_response_sequences, vec![2]);
    assert_eq!(completed.lifecycle.status, "completed");
    assert_eq!(
        completed.lifecycle.assistant_started_at.as_deref(),
        Some("2026-06-19T00:00:02Z")
    );
    assert_eq!(
        completed.lifecycle.assistant_completed_at.as_deref(),
        Some("2026-06-19T00:00:07Z")
    );
    assert_eq!(completed.selected_variant_id.as_deref(), Some("variant-1"));

    let messages = store.read_turn_messages("chat-1", "turn-1").await.unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].message_id, "user-1");
    assert_eq!(messages[1].message_id, "assistant-1");
}

#[tokio::test]
async fn setting_terminal_lifecycle_updates_selected_variant_status() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store.save_chat_header(&header("chat-1")).await.unwrap();
    store
        .append_conversation_turn(
            "chat-1",
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: lifecycle("in_progress"),
                selected_variant_id: Some("variant-1".to_string()),
                messages: vec![
                    message("user-1", "user", "hello"),
                    message("assistant-1", "assistant", "working"),
                ],
            },
        )
        .await
        .unwrap();

    store
        .set_turn_lifecycle(
            "chat-1",
            "turn-1",
            TurnLifecycle {
                user_submitted_at: "2026-06-19T00:00:01Z".to_string(),
                assistant_started_at: Some("2026-06-19T00:00:02Z".to_string()),
                assistant_completed_at: Some("2026-06-19T00:00:03Z".to_string()),
                status: "completed".to_string(),
            },
        )
        .await
        .unwrap();

    let turn = store
        .read_turn_range("chat-1", 1, 1)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(turn.lifecycle.status, "completed");
    assert_eq!(
        turn.selected_assistant_response_variant().unwrap().status,
        "completed"
    );
}

#[tokio::test]
async fn workspace_store_keeps_tool_use_and_tool_result_chain_inside_one_turn_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store.save_chat_header(&header("chat-1")).await.unwrap();

    let indexed = store
        .append_conversation_turn(
            "chat-1",
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:00:01Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:00:02Z".to_string()),
                    assistant_completed_at: Some("2026-06-19T00:00:09Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("variant-tool".to_string()),
                messages: vec![
                    message("user-1", "user", "inspect files"),
                    tool_message("assistant-tool-use", "assistant", "rg call", "rg", "call-1"),
                    tool_message("tool-result", "tool", "match output", "rg", "call-1"),
                    message("assistant-final", "assistant", "found it"),
                ],
            },
        )
        .await
        .unwrap();

    assert_eq!(indexed.first_sequence, 1);
    assert_eq!(indexed.last_sequence, 4);
    assert_eq!(indexed.user_message_sequence, 1);
    assert_eq!(indexed.assistant_response_sequences, vec![2, 4]);

    let messages = store.read_turn_messages("chat-1", "turn-1").await.unwrap();
    let roles: Vec<_> = messages
        .iter()
        .map(|message| message.role.as_str())
        .collect();
    assert_eq!(roles, vec!["user", "assistant", "tool", "assistant"]);
    assert!(messages.iter().all(|message| message.turn_id == "turn-1"));
    assert_eq!(
        messages[1].tool.as_ref().unwrap().tool_call_id.as_deref(),
        Some("call-1")
    );
    assert_eq!(
        messages[2].tool.as_ref().unwrap().tool_call_id.as_deref(),
        Some("call-1")
    );
}

#[tokio::test]
async fn model_call_boundary_waits_for_every_durable_tool_result() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store.save_chat_header(&header("chat-1")).await.unwrap();
    store
        .append_conversation_turn(
            "chat-1",
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: lifecycle("in_progress"),
                selected_variant_id: None,
                messages: vec![message("user-1", "user", "inspect files")],
            },
        )
        .await
        .unwrap();
    let writer = TurnWriter::new(&store, "chat-1", "turn-1");

    writer
        .append_output(
            vec![message("assistant-text", "assistant", "first answer")],
            true,
            None,
        )
        .await
        .unwrap();
    assert!(writer.is_complete_model_call_boundary().await.unwrap());

    let mut tool_use = message("assistant-tool", "assistant", "inspect");
    tool_use.content = json!([{
        "type": "tool_use",
        "id": "call-1",
        "name": "read",
        "input": {"path": "src/lib.rs"}
    }]);
    writer
        .append_output(vec![tool_use], true, None)
        .await
        .unwrap();
    assert!(!writer.is_complete_model_call_boundary().await.unwrap());

    let mut tool_result = message("tool-result", "tool", "contents");
    tool_result.content = json!([{
        "type": "tool_result",
        "call_id": "call-1",
        "content": [{"type": "text", "text": "contents"}]
    }]);
    writer
        .append_output(vec![tool_result], true, None)
        .await
        .unwrap();
    assert!(writer.is_complete_model_call_boundary().await.unwrap());
}

#[tokio::test]
async fn workspace_store_rejects_selected_variant_outside_turn_assistants() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store.save_chat_header(&header("chat-1")).await.unwrap();

    store
        .append_conversation_turn(
            "chat-1",
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: lifecycle("completed"),
                selected_variant_id: Some("assistant-1".to_string()),
                messages: vec![
                    message("user-1", "user", "question"),
                    message("assistant-1", "assistant", "first"),
                ],
            },
        )
        .await
        .unwrap();
    store
        .append_messages_to_turn_as_new_variant(
            "chat-1",
            "turn-1",
            lifecycle("completed"),
            Some("assistant-2".to_string()),
            vec![message("assistant-2", "assistant", "second")],
        )
        .await
        .unwrap();

    assert!(store
        .set_turn_selected_variant("chat-1", "turn-1", "assistant-1".to_string())
        .await
        .unwrap());
    assert!(!store
        .set_turn_selected_variant("chat-1", "turn-1", "not-a-variant".to_string())
        .await
        .unwrap());
}

#[tokio::test]
async fn workspace_store_reads_bounded_sequence_range_without_parsing_older_chunks() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    let mut chat = header("chat-1");
    chat.message_log.chunk_size = 2;
    store.save_chat_header(&chat).await.unwrap();

    for turn_number in 1..=3 {
        store
            .append_conversation_turn(
                "chat-1",
                NewConversationTurn {
                    turn_id: format!("turn-{turn_number}"),
                    lifecycle: lifecycle("completed"),
                    selected_variant_id: Some(format!("variant-{turn_number}")),
                    messages: vec![
                        message(&format!("user-{turn_number}"), "user", "hello"),
                        message(&format!("assistant-{turn_number}"), "assistant", "hi"),
                    ],
                },
            )
            .await
            .unwrap();
    }

    let first_chunk = dir
        .path()
        .join("sessions/chat-1/messages/000000000000.jsonl");
    std::fs::write(first_chunk, "{not valid json").unwrap();

    let page = store.read_message_range("chat-1", 3, 2).await.unwrap();

    let ids: Vec<_> = page
        .iter()
        .map(|message| message.message_id.as_str())
        .collect();
    assert_eq!(ids, vec!["user-2", "assistant-2"]);
}

#[tokio::test]
async fn workspace_store_appends_without_reparsing_committed_turns() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    let mut chat = header("chat-1");
    chat.message_log.chunk_size = 2;
    store.save_chat_header(&chat).await.unwrap();

    for turn_number in 1..=3 {
        store
            .append_conversation_turn(
                "chat-1",
                NewConversationTurn {
                    turn_id: format!("turn-{turn_number}"),
                    lifecycle: lifecycle("completed"),
                    selected_variant_id: Some(format!("variant-{turn_number}")),
                    messages: vec![
                        message(&format!("user-{turn_number}"), "user", "hello"),
                        message(&format!("assistant-{turn_number}"), "assistant", "hi"),
                    ],
                },
            )
            .await
            .unwrap();
    }

    let first_chunk = dir
        .path()
        .join("sessions/chat-1/messages/000000000000.jsonl");
    std::fs::write(first_chunk, "{not valid json").unwrap();

    let appended = store
        .append_conversation_turn(
            "chat-1",
            NewConversationTurn {
                turn_id: "turn-4".to_string(),
                lifecycle: lifecycle("completed"),
                selected_variant_id: Some("variant-4".to_string()),
                messages: vec![
                    message("user-4", "user", "hello"),
                    message("assistant-4", "assistant", "hi"),
                ],
            },
        )
        .await
        .unwrap();

    assert_eq!(appended.turn_id, "turn-4");
    assert_eq!(appended.first_sequence, 7);
    assert_eq!(appended.last_sequence, 8);
}

#[tokio::test]
async fn workspace_store_does_not_reuse_sequence_when_header_watermark_lags_log() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store.save_chat_header(&header("chat-1")).await.unwrap();
    store
        .append_conversation_turn(
            "chat-1",
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: lifecycle("completed"),
                selected_variant_id: Some("variant-1".to_string()),
                messages: vec![
                    message("user-1", "user", "hello"),
                    message("assistant-1", "assistant", "hi"),
                ],
            },
        )
        .await
        .unwrap();

    let mut stale_header = store.load_chat_header("chat-1").await.unwrap().unwrap();
    stale_header.message_log.latest_sequence = 1;
    stale_header.message_log.allocated_sequence = 1;
    store.save_chat_header(&stale_header).await.unwrap();
    let turn_index_path = dir.path().join("sessions/chat-1/turn_index.json");
    let mut stale_index: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&turn_index_path).unwrap()).unwrap();
    stale_index["turns"][0]["last_sequence"] = serde_json::json!(1);
    stale_index["turns"][0]["assistant_response_sequences"] = serde_json::json!([]);
    std::fs::write(
        &turn_index_path,
        serde_json::to_vec_pretty(&stale_index).unwrap(),
    )
    .unwrap();

    let appended = store
        .append_conversation_turn(
            "chat-1",
            NewConversationTurn {
                turn_id: "turn-2".to_string(),
                lifecycle: lifecycle("completed"),
                selected_variant_id: Some("variant-2".to_string()),
                messages: vec![message("user-2", "user", "continue")],
            },
        )
        .await
        .unwrap();

    assert_eq!(appended.first_sequence, 3);
    assert_eq!(appended.last_sequence, 3);
    let messages = store.read_message_range("chat-1", 1, 10).await.unwrap();
    assert_eq!(
        messages
            .iter()
            .map(|message| message.sequence)
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
}

#[tokio::test]
async fn workspace_store_repairs_legacy_reserved_sequence_gap() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store.save_chat_header(&header("chat-1")).await.unwrap();
    store
        .append_conversation_turn(
            "chat-1",
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: lifecycle("completed"),
                selected_variant_id: Some("variant-1".to_string()),
                messages: vec![message("user-1", "user", "hello")],
            },
        )
        .await
        .unwrap();

    let mut reserved_header = store.load_chat_header("chat-1").await.unwrap().unwrap();
    reserved_header.message_log.latest_sequence = 2;
    reserved_header.message_log.allocated_sequence = 2;
    store.save_chat_header(&reserved_header).await.unwrap();

    let latest = store
        .read_latest_message_from_header("chat-1", &reserved_header.message_log)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(latest.sequence, 1);

    let appended = store
        .append_conversation_turn(
            "chat-1",
            NewConversationTurn {
                turn_id: "turn-2".to_string(),
                lifecycle: lifecycle("completed"),
                selected_variant_id: Some("variant-2".to_string()),
                messages: vec![message("user-2", "user", "continue")],
            },
        )
        .await
        .unwrap();

    assert_eq!(appended.first_sequence, 2);
    assert_eq!(appended.last_sequence, 2);
    let loaded = store.load_chat_header("chat-1").await.unwrap().unwrap();
    assert_eq!(loaded.message_log.latest_sequence, 2);
    assert_eq!(loaded.message_log.allocated_sequence, 2);
    let messages = store.read_message_range("chat-1", 1, 10).await.unwrap();
    assert_eq!(
        messages
            .iter()
            .map(|message| message.sequence)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
}

#[tokio::test]
async fn workspace_store_finishes_accepted_message_after_caller_is_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store.save_chat_header(&header("chat-1")).await.unwrap();
    store
        .append_conversation_turn(
            "chat-1",
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: lifecycle("in_progress"),
                selected_variant_id: None,
                messages: vec![message("user-1", "user", "start")],
            },
        )
        .await
        .unwrap();

    let interrupted_store = store.clone();
    let interrupted_append = tokio::spawn(async move {
        interrupted_store
            .append_messages_to_turn(
                "chat-1",
                "turn-1",
                lifecycle("in_progress"),
                Some("assistant-large".to_string()),
                vec![message(
                    "assistant-large",
                    "assistant",
                    &"accepted-before-stop".repeat(500_000),
                )],
            )
            .await
    });
    tokio::task::yield_now().await;
    assert!(!interrupted_append.is_finished());
    interrupted_append.abort();
    assert!(interrupted_append.await.unwrap_err().is_cancelled());

    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        store.append_messages_to_turn(
            "chat-1",
            "turn-1",
            lifecycle("completed"),
            Some("assistant-final".to_string()),
            vec![message("assistant-final", "assistant", "stopped")],
        ),
    )
    .await
    .unwrap()
    .unwrap();

    let messages = store.read_message_range("chat-1", 1, 10).await.unwrap();
    assert_eq!(
        messages
            .iter()
            .map(|message| (message.sequence, message.message_id.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (1, "user-1"),
            (2, "assistant-large"),
            (3, "assistant-final"),
        ]
    );
}

#[tokio::test]
async fn workspace_store_sequence_range_keeps_end_message_after_duplicate_sequence() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store.save_chat_header(&header("chat-1")).await.unwrap();
    store
        .append_conversation_turn(
            "chat-1",
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: lifecycle("completed"),
                selected_variant_id: Some("variant-1".to_string()),
                messages: vec![
                    message("user-1", "user", "hello"),
                    message("assistant-1", "assistant", "working"),
                    message("assistant-final", "assistant", "done"),
                ],
            },
        )
        .await
        .unwrap();

    let chunk_path = dir
        .path()
        .join("sessions/chat-1/messages/000000000000.jsonl");
    let content = std::fs::read_to_string(&chunk_path).unwrap();
    let mut lines = content.lines();
    let first = lines.next().unwrap();
    let second = lines.next().unwrap();
    let third = lines.next().unwrap();
    let duplicate = second.replace("assistant-1", "assistant-duplicate");
    std::fs::write(
        &chunk_path,
        format!("{first}\n{second}\n{duplicate}\n{third}\n"),
    )
    .unwrap();

    let chat = store.load_chat_header("chat-1").await.unwrap().unwrap();
    let messages = store
        .read_message_sequence_range_from_header("chat-1", 1, 3, &chat.message_log)
        .await
        .unwrap();

    assert_eq!(messages.len(), 4);
    assert_eq!(messages.last().unwrap().message_id, "assistant-final");
    assert_eq!(messages.last().unwrap().sequence, 3);
}

#[tokio::test]
async fn workspace_store_reads_bounded_turn_range_for_history_pagination() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store.save_chat_header(&header("chat-1")).await.unwrap();

    for turn_number in 1..=4 {
        store
            .append_conversation_turn(
                "chat-1",
                NewConversationTurn {
                    turn_id: format!("turn-{turn_number}"),
                    lifecycle: lifecycle("completed"),
                    selected_variant_id: Some(format!("variant-{turn_number}")),
                    messages: vec![
                        message(&format!("user-{turn_number}"), "user", "hello"),
                        message(&format!("assistant-{turn_number}"), "assistant", "hi"),
                    ],
                },
            )
            .await
            .unwrap();
    }

    let page = store.read_turn_range("chat-1", 2, 2).await.unwrap();

    let ids: Vec<_> = page.iter().map(|turn| turn.turn_id.as_str()).collect();
    assert_eq!(ids, vec!["turn-2", "turn-3"]);
    assert_eq!(page[0].first_sequence, 3);
    assert_eq!(page[1].last_sequence, 6);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn latest_message_reads_remain_consistent_while_another_store_appends() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    let mut initial = header("latest-race");
    initial.message_log.chunk_size = 8;
    store.save_chat_header(&initial).await.unwrap();
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(5));
    let writer = WorkspaceSessionStore::new(dir.path());
    let start = barrier.clone();
    let writing = tokio::spawn(async move {
        start.wait().await;
        for index in 0..100 {
            writer
                .append_conversation_turn(
                    "latest-race",
                    NewConversationTurn {
                        turn_id: format!("turn-{index}"),
                        lifecycle: lifecycle("completed"),
                        selected_variant_id: None,
                        messages: vec![message(
                            &format!("user-{index}"),
                            "user",
                            "concurrent append",
                        )],
                    },
                )
                .await
                .unwrap();
        }
    });
    let mut readers = Vec::new();
    for _ in 0..4 {
        let reader = WorkspaceSessionStore::new(dir.path());
        let start = barrier.clone();
        let log = initial.message_log.clone();
        readers.push(tokio::spawn(async move {
            start.wait().await;
            for _ in 0..500 {
                reader
                    .read_latest_message_from_header("latest-race", &log)
                    .await?;
            }
            rua_core::Result::Ok(())
        }));
    }
    writing.await.unwrap();
    for reading in readers {
        reading.await.unwrap().unwrap();
    }
    let committed = store
        .load_chat_header("latest-race")
        .await
        .unwrap()
        .unwrap();
    let messages = store
        .read_message_sequence_range_from_header("latest-race", 1, 100, &committed.message_log)
        .await
        .unwrap();
    assert_eq!(
        messages
            .iter()
            .map(|entry| entry.sequence)
            .collect::<Vec<_>>(),
        (1..=100).collect::<Vec<_>>()
    );
    assert_eq!(
        store
            .read_latest_message_from_header("latest-race", &initial.message_log)
            .await
            .unwrap()
            .unwrap()
            .sequence,
        100
    );
}
