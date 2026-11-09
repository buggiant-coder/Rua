use rua_channel::core::console::SessionPersistence;
use rua_contract::agent::AgentInputMessage;
use rua_core::message::ContentPart;
use rua_core::ToolCall;
use rua_server::services::console_chat::{
    persist_conversation_runtime_failure_to_session, reconcile_orphan_in_progress_turns,
    WorkspaceSessionPersistence,
};
use rua_session::{
    ChatMetadata, ChatSessionHeader, NewConversationMessage, NewConversationTurn, RuntimeNote,
    RuntimeNotePayload, TaskAttemptTerminalRuntimeNote, TurnLifecycle, WorkspaceSessionStore,
};

fn runtime_note(status: &str) -> RuntimeNote {
    RuntimeNote {
        runtime_note_id: format!("runtime-note:background-task:task-1:{status}"),
        created_at: "2026-06-18T00:00:00Z".to_string(),
        payload: RuntimeNotePayload::TaskAttemptTerminal(TaskAttemptTerminalRuntimeNote {
            task_id: "task-1".to_string(),
            attempt_no: 1,
            terminal_status: status.to_string(),
            output: "background completed with compact summary".to_string(),
            runner_name: "bash".to_string(),
            tool_call_id: Some("tool-call-1".to_string()),
            full_result_ref: Some(serde_json::json!({
                "task_id": "task-1",
                "ref_type": "background_task",
            })),
            artifacts: serde_json::json!([]),
            device: serde_json::json!({}),
        }),
    }
}

fn input_tool_use(call_id: &str) -> AgentInputMessage {
    AgentInputMessage {
        role: "assistant".to_string(),
        content: vec![ContentPart::ToolUse {
            call: ToolCall::new(
                call_id.to_string(),
                "bash".to_string(),
                serde_json::json!({"command": "sleep 10"}),
            ),
        }],
    }
}

#[tokio::test]
async fn task_runtime_note_persists_without_system_authority() {
    let workspace = tempfile::tempdir().unwrap();
    let note = runtime_note("completed");
    let store = WorkspaceSessionStore::new(workspace.path());
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: "session-1".to_string(),
            agent_id: "agent-1".to_string(),
            title: "Runtime Note".to_string(),
            created_at: "2026-06-18T00:00:00Z".to_string(),
            last_activity_at: "2026-06-18T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .unwrap();
    store
        .append_conversation_turn(
            "session-1",
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-18T00:00:00Z".to_string(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "completed".to_string(),
                },
                selected_variant_id: None,
                messages: vec![NewConversationMessage {
                    message_id: "user-1".to_string(),
                    role: "user".to_string(),
                    created_at: "2026-06-18T00:00:00Z".to_string(),
                    status: "completed".to_string(),
                    content: serde_json::json!([{"type": "text", "text": "start"}]),
                    provider: None,
                    tool: None,
                    metadata: serde_json::Map::new(),
                }],
            },
        )
        .await
        .unwrap();
    store
        .append_runtime_note("session-1", "turn-1", note.clone())
        .await
        .unwrap();

    let messages = store.read_message_range("session-1", 1, 10).await.unwrap();
    let persisted = RuntimeNote::from_message_log_entry(&messages[1]).expect("Runtime Note");
    assert_eq!(persisted, note);
    assert_eq!(messages[1].role, "runtime");

    let rendered = persisted.render_for_model();
    assert!(rendered.contains("<task_attempt_terminal>"));
    assert!(rendered.contains("task_id: task-1"));
}

#[tokio::test]
async fn provider_failure_persists_runtime_fact_without_fake_assistant_message() {
    let workspace = tempfile::tempdir().unwrap();
    let store = rua_session::WorkspaceSessionStore::new(workspace.path());
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: "session-1".to_string(),
            agent_id: "agent-1".to_string(),
            title: "failure test".to_string(),
            created_at: "2026-06-23T00:00:00Z".to_string(),
            last_activity_at: "2026-06-23T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .unwrap();
    store
        .append_conversation_turn(
            "session-1",
            NewConversationTurn {
                turn_id: "turn-failed-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-23T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-06-23T00:00:01Z".to_string()),
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![NewConversationMessage {
                    message_id: "msg-user-1".to_string(),
                    role: "user".to_string(),
                    created_at: "2026-06-23T00:00:00Z".to_string(),
                    status: "completed".to_string(),
                    content: serde_json::json!([{ "type": "text", "text": "fail me" }]),
                    provider: None,
                    tool: None,
                    metadata: serde_json::Map::new(),
                }],
            },
        )
        .await
        .unwrap();

    persist_conversation_runtime_failure_to_session(
        workspace.path(),
        "agent-1",
        "session-1",
        "turn-failed-1",
        "provider stream disconnected",
        None,
    )
    .await
    .unwrap();

    let turns = store.read_turn_range("session-1", 1, 10).await.unwrap();
    assert_eq!(turns[0].lifecycle.status, "failed");
    let messages = store.read_message_range("session-1", 1, 10).await.unwrap();
    assert_eq!(messages.len(), 2);
    let note =
        RuntimeNote::from_message_log_entry(&messages[1]).expect("provider failure Runtime Note");
    let RuntimeNotePayload::ConversationRuntimeTerminal(note) = note.payload else {
        panic!("provider failure must persist as Conversation Runtime Terminal");
    };
    assert_eq!(note.turn_id, "turn-failed-1");
    assert_eq!(note.terminal_status, "failed");
    assert_eq!(note.reason, "runtime_failure");
    assert!(note.summary.contains("provider stream disconnected"));
    assert!(!serde_json::to_string(&messages)
        .unwrap()
        .contains("请求失败"));
}

#[tokio::test]
async fn orphan_in_progress_turn_reconciles_to_interrupted_runtime_fact() {
    let workspace = tempfile::tempdir().unwrap();
    let store = rua_session::WorkspaceSessionStore::new(workspace.path());
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: "session-1".to_string(),
            agent_id: "agent-1".to_string(),
            title: "orphan test".to_string(),
            created_at: "2026-06-24T00:00:00Z".to_string(),
            last_activity_at: "2026-06-24T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .unwrap();
    store
        .append_conversation_turn(
            "session-1",
            NewConversationTurn {
                turn_id: "turn-orphan-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-24T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-06-24T00:00:01Z".to_string()),
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![NewConversationMessage {
                    message_id: "msg-user-orphan-1".to_string(),
                    role: "user".to_string(),
                    created_at: "2026-06-24T00:00:00Z".to_string(),
                    status: "completed".to_string(),
                    content: serde_json::json!([{ "type": "text", "text": "needs approval" }]),
                    provider: None,
                    tool: None,
                    metadata: serde_json::Map::new(),
                }],
            },
        )
        .await
        .unwrap();

    let reconciled =
        reconcile_orphan_in_progress_turns(workspace.path(), "agent-1", "session-1", false, None)
            .await
            .unwrap();

    assert_eq!(reconciled, 1);
    let turns = store.read_turn_range("session-1", 1, 10).await.unwrap();
    assert_eq!(turns[0].lifecycle.status, "interrupted");
    assert!(turns[0].lifecycle.assistant_completed_at.is_some());
    let messages = store.read_message_range("session-1", 1, 10).await.unwrap();
    assert_eq!(messages.len(), 2);
    let note = RuntimeNote::from_message_log_entry(&messages[1])
        .expect("orphan reconciliation Runtime Note");
    let RuntimeNotePayload::ConversationRuntimeTerminal(note) = note.payload else {
        panic!("orphan reconciliation must persist Conversation Runtime Terminal");
    };
    assert_eq!(note.turn_id, "turn-orphan-1");
    assert_eq!(note.terminal_status, "interrupted");
    assert_eq!(note.reason, "runtime_restart");
    assert!(note.summary.contains("runtime restart"));

    let reconciled_again =
        reconcile_orphan_in_progress_turns(workspace.path(), "agent-1", "session-1", false, None)
            .await
            .unwrap();
    assert_eq!(reconciled_again, 0, "reconciliation must be idempotent");
}

#[tokio::test]
async fn producer_cancel_persists_turn_aborted_after_partial_assistant_output() {
    let workspace = tempfile::tempdir().unwrap();
    let store = rua_session::WorkspaceSessionStore::new(workspace.path());
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: "session-stop".to_string(),
            agent_id: "agent-1".to_string(),
            title: "stop ordering".to_string(),
            created_at: "2026-06-24T00:00:00Z".to_string(),
            last_activity_at: "2026-06-24T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .unwrap();
    store
        .append_conversation_turn(
            "session-stop",
            NewConversationTurn {
                turn_id: "turn-stop-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-24T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-06-24T00:00:01Z".to_string()),
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![NewConversationMessage {
                    message_id: "msg-user-stop-1".to_string(),
                    role: "user".to_string(),
                    created_at: "2026-06-24T00:00:00Z".to_string(),
                    status: "completed".to_string(),
                    content: serde_json::json!([{ "type": "text", "text": "sleep" }]),
                    provider: None,
                    tool: None,
                    metadata: serde_json::Map::new(),
                }],
            },
        )
        .await
        .unwrap();

    let persistence = WorkspaceSessionPersistence::new(workspace.path().to_path_buf())
        .with_active_turn_id("turn-stop-1".to_string())
        .with_assistant_started_at("2026-06-24T00:00:01Z".to_string());
    persistence
        .save_messages("agent-1", "session-stop", &[input_tool_use("call-1")])
        .await
        .unwrap();
    persistence
        .save_turn_aborted("agent-1", "session-stop")
        .await
        .unwrap();

    let messages = store
        .read_message_range("session-stop", 1, 10)
        .await
        .unwrap();
    assert_eq!(
        messages
            .iter()
            .map(|message| (message.sequence, message.role.as_str()))
            .collect::<Vec<_>>(),
        vec![(1, "user"), (2, "assistant"), (3, "user")]
    );
    assert_eq!(
        messages[2].metadata.get("model_visible_marker"),
        Some(&serde_json::json!("turn_aborted"))
    );

    let turns = store.read_turn_range("session-stop", 1, 10).await.unwrap();
    assert_eq!(turns[0].lifecycle.status, "interrupted");
    assert_eq!(turns[0].last_sequence, 3);
    assert_eq!(turns[0].assistant_response_sequences, vec![2]);
    let selected = turns[0].selected_assistant_response_variant().unwrap();
    assert_eq!(selected.first_sequence, 2);
    assert_eq!(selected.last_sequence, 3);
}

#[tokio::test]
async fn producer_interject_persists_turn_interjected_after_partial_assistant_output() {
    let workspace = tempfile::tempdir().unwrap();
    let store = rua_session::WorkspaceSessionStore::new(workspace.path());
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: "session-interject".to_string(),
            agent_id: "agent-1".to_string(),
            title: "interject ordering".to_string(),
            created_at: "2026-06-24T00:00:00Z".to_string(),
            last_activity_at: "2026-06-24T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .unwrap();
    store
        .append_conversation_turn(
            "session-interject",
            NewConversationTurn {
                turn_id: "turn-interject-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-24T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-06-24T00:00:01Z".to_string()),
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![NewConversationMessage {
                    message_id: "msg-user-interject-1".to_string(),
                    role: "user".to_string(),
                    created_at: "2026-06-24T00:00:00Z".to_string(),
                    status: "completed".to_string(),
                    content: serde_json::json!([{ "type": "text", "text": "long task" }]),
                    provider: None,
                    tool: None,
                    metadata: serde_json::Map::new(),
                }],
            },
        )
        .await
        .unwrap();

    let persistence = WorkspaceSessionPersistence::new(workspace.path().to_path_buf())
        .with_active_turn_id("turn-interject-1".to_string())
        .with_assistant_started_at("2026-06-24T00:00:01Z".to_string());
    persistence
        .save_messages(
            "agent-1",
            "session-interject",
            &[input_tool_use("call-interject-1")],
        )
        .await
        .unwrap();
    persistence
        .save_turn_interjected("agent-1", "session-interject")
        .await
        .unwrap();

    let messages = store
        .read_message_range("session-interject", 1, 10)
        .await
        .unwrap();
    assert_eq!(
        messages
            .iter()
            .map(|message| (message.sequence, message.role.as_str()))
            .collect::<Vec<_>>(),
        vec![(1, "user"), (2, "assistant"), (3, "system")]
    );
    assert_eq!(
        messages[2].metadata.get("model_visible_marker"),
        Some(&serde_json::json!("turn_interjected"))
    );

    let turns = store
        .read_turn_range("session-interject", 1, 10)
        .await
        .unwrap();
    assert_eq!(turns[0].lifecycle.status, "interjected");
    assert_eq!(turns[0].last_sequence, 3);
    assert_eq!(turns[0].assistant_response_sequences, vec![2]);
    let selected = turns[0].selected_assistant_response_variant().unwrap();
    assert_eq!(selected.first_sequence, 2);
    assert_eq!(selected.last_sequence, 3);
}

#[tokio::test]
async fn device_delivery_in_progress_turn_is_not_marked_interrupted_on_open() {
    let workspace = tempfile::tempdir().unwrap();
    let store = rua_session::WorkspaceSessionStore::new(workspace.path());
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: "session-remote".to_string(),
            agent_id: "agent-1".to_string(),
            title: "remote conversation input".to_string(),
            created_at: "2026-06-28T00:00:00Z".to_string(),
            last_activity_at: "2026-06-28T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .unwrap();
    store
        .append_conversation_turn(
            "session-remote",
            NewConversationTurn {
                turn_id: "turn-remote-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-06-28T00:00:00Z".to_string(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![NewConversationMessage {
                    message_id: "msg-user-remote-1".to_string(),
                    role: "user".to_string(),
                    created_at: "2026-06-28T00:00:00Z".to_string(),
                    status: "completed".to_string(),
                    content: serde_json::json!([{ "type": "text", "text": "run remotely" }]),
                    provider: None,
                    tool: None,
                    metadata: serde_json::json!({
                        "device_delivery_id": "device-delivery-1",
                        "source_device_id": "phone",
                        "target_device_id": "desktop",
                    })
                    .as_object()
                    .expect("metadata object")
                    .clone(),
                }],
            },
        )
        .await
        .unwrap();

    let reconciled = reconcile_orphan_in_progress_turns(
        workspace.path(),
        "agent-1",
        "session-remote",
        false,
        None,
    )
    .await
    .unwrap();

    assert_eq!(reconciled, 0);
    let turns = store
        .read_turn_range("session-remote", 1, 10)
        .await
        .unwrap();
    assert_eq!(turns[0].lifecycle.status, "in_progress");
    assert!(
        store
            .read_message_range("session-remote", 1, 10)
            .await
            .unwrap()
            .iter()
            .all(|message| RuntimeNote::from_message_log_entry(message).is_none()),
        "device deliverys are normal background runs, not orphaned runtime restarts"
    );
}

#[tokio::test]
async fn saving_after_context_projection_appends_only_new_evergreen_messages() {
    let workspace = tempfile::tempdir().unwrap();
    let store = rua_session::WorkspaceSessionStore::new(workspace.path());
    store
        .save_chat_header(&rua_session::ChatSessionHeader::new(
            rua_session::ChatMetadata {
                chat_id: "session-1".to_string(),
                agent_id: "agent-1".to_string(),
                title: "session-1".to_string(),
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
            "session-1",
            rua_session::NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: rua_session::TurnLifecycle {
                    user_submitted_at: "2026-06-18T00:00:00Z".to_string(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![rua_session::NewConversationMessage {
                    message_id: "msg-3".to_string(),
                    role: "user".to_string(),
                    created_at: "2026-06-18T00:00:00Z".to_string(),
                    status: "completed".to_string(),
                    content: serde_json::json!([{ "type": "text", "text": "fresh user" }]),
                    provider: None,
                    tool: None,
                    metadata: serde_json::Map::new(),
                }],
            },
        )
        .await
        .unwrap();

    let persistence = WorkspaceSessionPersistence::new(workspace.path().to_path_buf())
        .with_loaded_message_count(1)
        .with_active_turn_id("turn-1".to_string());

    persistence
        .save_messages(
            "agent-1",
            "session-1",
            &[
                AgentInputMessage {
                    role: "user".to_string(),
                    content: vec![ContentPart::Text {
                        text: "fresh user".to_string(),
                    }],
                },
                AgentInputMessage {
                    role: "assistant".to_string(),
                    content: vec![ContentPart::Text {
                        text: "new assistant".to_string(),
                    }],
                },
            ],
        )
        .await
        .unwrap();

    let turns = store.read_turn_range("session-1", 1, 10).await.unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].lifecycle.status, "completed");
    let messages = store.read_message_range("session-1", 1, 10).await.unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].message_id, "msg-3");
    assert_eq!(messages[1].role, "assistant");
}

#[test]
fn runtime_note_rendering_covers_all_terminal_statuses_without_full_output() {
    for status in ["completed", "failed", "cancelled", "interrupted"] {
        let note = runtime_note(status);
        let rendered = note.render_for_model();
        assert!(rendered.contains("<task_attempt_terminal>"));
        assert!(rendered.contains("task_id: task-1"));
        assert!(rendered.contains("runner: bash"));
        assert!(rendered.contains("background completed with compact summary"));
        assert!(!rendered.contains("background-complete-output-that-was-never-inlined"));
    }
}
