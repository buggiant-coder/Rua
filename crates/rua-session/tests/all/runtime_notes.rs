use rua_session::{
    ChatMetadata, ChatSessionHeader, ContextProjectionRequest, ConversationRecallRequest,
    MemorySignalRuntimeNote, NewConversationMessage, NewConversationTurn, RuntimeNote,
    RuntimeNotePayload, TaskWatchOutputProjection, TaskWatchOutputRecord,
    TaskWatchOutputRuntimeNote, TurnLifecycle, WorkspaceSessionStore,
};
use serde_json::json;

fn header(chat_id: &str) -> ChatSessionHeader {
    ChatSessionHeader::new(ChatMetadata {
        chat_id: chat_id.to_string(),
        agent_id: "agent-1".to_string(),
        title: "Runtime Notes".to_string(),
        created_at: "2026-08-03T00:00:00Z".to_string(),
        last_activity_at: "2026-08-03T00:00:00Z".to_string(),
        pinned: false,
        archived: false,
        preview: None,
    })
}

#[tokio::test]
async fn configuration_compare_and_swap_rejects_stale_epoch_without_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    let mut initial = header("config-cas");
    initial.extra.insert(
        "hook_configuration".into(),
        json!({"epoch": "one", "future": true}),
    );
    store.save_chat_header(&initial).await.unwrap();
    let second = json!({"epoch": "two", "future": {"enabled": true}});
    store
        .replace_chat_configuration("config-cas", "one", second.clone())
        .await
        .unwrap();
    assert!(store
        .replace_chat_configuration("config-cas", "one", json!({"epoch": "three"}))
        .await
        .is_err());
    let restored = store.load_chat_header("config-cas").await.unwrap().unwrap();
    assert_eq!(restored.extra["hook_configuration"], second);
    assert_eq!(
        store
            .load_configuration_snapshot("config-cas", "one")
            .await
            .unwrap(),
        Some(json!({"epoch": "one", "future": true}))
    );
    let mut invalid = restored.clone();
    invalid.extra["hook_configuration"]["future"] = json!(false);
    assert!(store.save_chat_header(&invalid).await.is_err());
    assert_eq!(
        store
            .load_chat_header("config-cas")
            .await
            .unwrap()
            .unwrap()
            .extra["hook_configuration"],
        second
    );
    assert_eq!(restored.metadata.title, initial.metadata.title);
    let mut stale = initial;
    stale.metadata.title = "late history update".to_string();
    store.save_chat_header(&stale).await.unwrap();
    let after = store.load_chat_header("config-cas").await.unwrap().unwrap();
    assert_eq!(after.extra["hook_configuration"], second);
    assert_eq!(after.metadata.title, "late history update");
}

fn message(message_id: &str, role: &str, text: &str) -> NewConversationMessage {
    NewConversationMessage {
        message_id: message_id.to_string(),
        role: role.to_string(),
        created_at: "2026-08-03T00:00:00Z".to_string(),
        status: "completed".to_string(),
        content: json!([{"type": "text", "text": text}]),
        provider: None,
        tool: None,
        metadata: serde_json::Map::new(),
    }
}

#[tokio::test]
async fn typed_runtime_note_is_durable_message_log_conversation_state() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = WorkspaceSessionStore::new(dir.path());
    store.save_chat_header(&header("chat-1")).await.unwrap();
    store
        .append_conversation_turn(
            "chat-1",
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-08-03T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-08-03T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-08-03T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("variant-1".to_string()),
                messages: vec![
                    message("user-1", "user", "Remember this preference"),
                    message("assistant-1", "assistant", "I will remember it"),
                ],
            },
        )
        .await
        .unwrap();

    let note = RuntimeNote {
        runtime_note_id: "memory-signal:turn-1".to_string(),
        created_at: "2026-08-03T00:00:03Z".to_string(),
        payload: RuntimeNotePayload::MemorySignal(MemorySignalRuntimeNote {
            signal_kind: "durable_preference".to_string(),
            confidence: "high".to_string(),
            suggested_memory_kind: "preference".to_string(),
            suggested_scope: "user".to_string(),
            evidence: "User asked the agent to remember a stable preference".to_string(),
        }),
    };
    store
        .append_runtime_note("chat-1", "turn-1", note.clone())
        .await
        .unwrap();
    store
        .append_runtime_note("chat-1", "turn-1", note.clone())
        .await
        .unwrap();

    let messages = store.read_message_range("chat-1", 1, 10).await.unwrap();
    assert_eq!(messages.len(), 3);
    let persisted = RuntimeNote::from_message_log_entry(&messages[2])
        .expect("typed Runtime Note in Message Log");
    assert_eq!(persisted, note);
    assert_eq!(messages[2].role, "system");
    assert_eq!(messages[2].metadata["message_type"], "runtime_note");
    let turn = store
        .read_turn_range("chat-1", 1, 10)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(turn.lifecycle.status, "completed");
    assert_eq!(turn.last_sequence, 3);
    assert_eq!(
        turn.assistant_response_variants[0].last_sequence, 2,
        "Runtime Notes belong to the Turn chronology but not its Assistant Response Variant"
    );

    let header = store.load_chat_header("chat-1").await.unwrap().unwrap();
    assert!(
        !header.extra.contains_key("runtime_notes"),
        "Runtime Notes must not be stored in Session header extras"
    );
}

#[tokio::test]
async fn terminalizing_user_only_turn_does_not_underflow_variant_index() {
    let dir = tempfile::tempdir().unwrap();
    let store = WorkspaceSessionStore::new(dir.path());
    store.save_chat_header(&header("user-only")).await.unwrap();
    let lifecycle = TurnLifecycle {
        user_submitted_at: "2026-08-03T00:00:00Z".to_string(),
        assistant_started_at: None,
        assistant_completed_at: None,
        status: "in_progress".to_string(),
    };
    store
        .append_conversation_turn(
            "user-only",
            NewConversationTurn {
                turn_id: "turn-only".to_string(),
                lifecycle: lifecycle.clone(),
                selected_variant_id: None,
                messages: vec![message("user-only-message", "user", "hello")],
            },
        )
        .await
        .unwrap();
    assert!(store
        .set_turn_lifecycle(
            "user-only",
            "turn-only",
            TurnLifecycle {
                status: "interrupted".to_string(),
                ..lifecycle
            }
        )
        .await
        .unwrap());
}

#[tokio::test]
async fn next_turn_preserves_prior_runtime_note_outside_assistant_variant() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&header("chat-next-turn"))
        .await
        .unwrap();
    store
        .append_conversation_turn(
            "chat-next-turn",
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-08-03T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-08-03T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-08-03T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("variant-1".to_string()),
                messages: vec![
                    message("user-1", "user", "first"),
                    message("assistant-1", "assistant", "first response"),
                ],
            },
        )
        .await
        .unwrap();
    let note = RuntimeNote {
        runtime_note_id: "runtime-note-after-response".to_string(),
        created_at: "2026-08-03T00:00:03Z".to_string(),
        payload: RuntimeNotePayload::MemorySignal(MemorySignalRuntimeNote {
            signal_kind: "durable_preference".to_string(),
            confidence: "high".to_string(),
            suggested_memory_kind: "preference".to_string(),
            suggested_scope: "user".to_string(),
            evidence: "preserve after the selected response".to_string(),
        }),
    };
    store
        .append_runtime_note("chat-next-turn", "turn-1", note)
        .await
        .unwrap();
    store
        .append_conversation_turn(
            "chat-next-turn",
            NewConversationTurn {
                turn_id: "turn-2".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-08-03T00:01:00Z".to_string(),
                    assistant_started_at: Some("2026-08-03T00:01:01Z".to_string()),
                    assistant_completed_at: Some("2026-08-03T00:01:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("variant-2".to_string()),
                messages: vec![
                    message("user-2", "user", "second"),
                    message("assistant-2", "assistant", "second response"),
                ],
            },
        )
        .await
        .unwrap();

    let turns = store
        .read_turn_range("chat-next-turn", 1, 10)
        .await
        .unwrap();
    assert_eq!(turns[0].last_sequence, 3);
    assert_eq!(turns[0].assistant_response_variants[0].last_sequence, 2);
    let projection = store
        .build_context_projection(
            "chat-next-turn",
            ContextProjectionRequest {
                current_user_input: None,
            },
        )
        .await
        .unwrap();
    assert!(projection
        .tail_messages
        .iter()
        .any(|message| message.message_id == "runtime-note-after-response"));
}

#[tokio::test]
async fn delegated_task_watch_note_is_durable_but_excluded_from_model_projection() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&header("chat-task-watch"))
        .await
        .unwrap();
    store
        .append_conversation_turn(
            "chat-task-watch",
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-08-03T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-08-03T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-08-03T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("variant-1".to_string()),
                messages: vec![
                    message("user-1", "user", "delegate"),
                    message("assistant-1", "assistant", "task accepted"),
                ],
            },
        )
        .await
        .unwrap();
    let note = RuntimeNote {
        runtime_note_id: "task-watch:delegated:1".to_string(),
        created_at: "2026-08-03T00:00:03Z".to_string(),
        payload: RuntimeNotePayload::TaskWatchOutput(TaskWatchOutputRuntimeNote {
            task_id: "task-1".to_string(),
            attempt_no: 1,
            projection: TaskWatchOutputProjection::ActivityOnly,
            records: vec![TaskWatchOutputRecord {
                source: "acp".to_string(),
                text: "large delegated execution transcript".to_string(),
            }],
        }),
    };
    store
        .append_runtime_note("chat-task-watch", "turn-1", note)
        .await
        .unwrap();

    let raw = store
        .read_message_range("chat-task-watch", 1, 10)
        .await
        .unwrap();
    assert_eq!(raw.len(), 3);
    assert_eq!(raw[2].role, "runtime");
    assert_eq!(raw[2].metadata["model_visible"], false);

    let projection = store
        .build_context_projection(
            "chat-task-watch",
            ContextProjectionRequest {
                current_user_input: None,
            },
        )
        .await
        .unwrap();
    assert!(projection
        .tail_messages
        .iter()
        .all(|message| message.message_id != "task-watch:delegated:1"));
}

#[tokio::test]
async fn legacy_untyped_task_watch_note_defaults_to_activity_only() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&header("chat-legacy-task-watch"))
        .await
        .unwrap();
    let mut legacy_note = message(
        "legacy-task-watch",
        "system",
        "<task_watch_output>legacy delegated transcript</task_watch_output>",
    );
    legacy_note
        .metadata
        .insert("message_type".to_string(), json!("runtime_note"));
    legacy_note.metadata.insert(
        "runtime_note".to_string(),
        json!({
            "runtime_note_id": "legacy-task-watch",
            "created_at": "2026-08-03T00:00:03Z",
            "kind": "task_watch_output",
            "payload": {
                "task_id": "task-legacy",
                "attempt_no": 1,
                "records": [{
                    "source": "acp",
                    "text": "legacy delegated transcript"
                }]
            }
        }),
    );
    store
        .append_conversation_turn(
            "chat-legacy-task-watch",
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-08-03T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-08-03T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-08-03T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("variant-1".to_string()),
                messages: vec![
                    message("user-1", "user", "delegate"),
                    message("assistant-1", "assistant", "task accepted"),
                    legacy_note,
                ],
            },
        )
        .await
        .unwrap();

    let projection = store
        .build_context_projection(
            "chat-legacy-task-watch",
            ContextProjectionRequest {
                current_user_input: None,
            },
        )
        .await
        .unwrap();
    assert!(projection
        .tail_messages
        .iter()
        .all(|message| message.message_id != "legacy-task-watch"));
}

#[tokio::test]
async fn tool_task_watch_note_remains_in_model_projection() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = WorkspaceSessionStore::new(dir.path());
    store
        .save_chat_header(&header("chat-tool-watch"))
        .await
        .unwrap();
    store
        .append_conversation_turn(
            "chat-tool-watch",
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-08-03T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-08-03T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-08-03T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("variant-1".to_string()),
                messages: vec![
                    message("user-1", "user", "run shell"),
                    message("assistant-1", "assistant", "task accepted"),
                ],
            },
        )
        .await
        .unwrap();
    store
        .append_runtime_note(
            "chat-tool-watch",
            "turn-1",
            RuntimeNote {
                runtime_note_id: "task-watch:tool:1".to_string(),
                created_at: "2026-08-03T00:00:03Z".to_string(),
                payload: RuntimeNotePayload::TaskWatchOutput(TaskWatchOutputRuntimeNote {
                    task_id: "task-1".to_string(),
                    attempt_no: 1,
                    projection: TaskWatchOutputProjection::ModelVisible,
                    records: vec![TaskWatchOutputRecord {
                        source: "stdout".to_string(),
                        text: "compiled successfully".to_string(),
                    }],
                }),
            },
        )
        .await
        .unwrap();

    let projection = store
        .build_context_projection(
            "chat-tool-watch",
            ContextProjectionRequest {
                current_user_input: None,
            },
        )
        .await
        .unwrap();
    assert!(projection
        .tail_messages
        .iter()
        .any(|message| message.message_id == "task-watch:tool:1"));
}

#[tokio::test]
async fn legacy_header_runtime_notes_are_discarded_on_read() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = WorkspaceSessionStore::new(dir.path());
    let mut header = header("chat-legacy");
    header.extra.insert(
        "runtime_notes".to_string(),
        json!([{"runtime_note_id": "legacy-note"}]),
    );
    store.save_chat_header(&header).await.unwrap();

    let loaded = store
        .load_chat_header("chat-legacy")
        .await
        .unwrap()
        .unwrap();
    assert!(!loaded.extra.contains_key("runtime_notes"));
}

#[tokio::test]
async fn in_progress_turn_queues_runtime_note_until_before_next_model_call() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = WorkspaceSessionStore::new(dir.path());
    store.save_chat_header(&header("chat-live")).await.unwrap();
    store
        .append_conversation_turn(
            "chat-live",
            NewConversationTurn {
                turn_id: "turn-live".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-08-03T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-08-03T00:00:01Z".to_string()),
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![message("user-live", "user", "start")],
            },
        )
        .await
        .unwrap();
    let note = RuntimeNote {
        runtime_note_id: "memory-signal:live".to_string(),
        created_at: "2026-08-03T00:00:02Z".to_string(),
        payload: RuntimeNotePayload::MemorySignal(MemorySignalRuntimeNote {
            signal_kind: "correction".to_string(),
            confidence: "high".to_string(),
            suggested_memory_kind: "fact".to_string(),
            suggested_scope: "user".to_string(),
            evidence: "corrected fact".to_string(),
        }),
    };

    store
        .append_runtime_note("chat-live", "turn-live", note.clone())
        .await
        .unwrap();
    assert_eq!(
        store
            .read_message_range("chat-live", 1, 10)
            .await
            .unwrap()
            .len(),
        1,
        "an in-flight model/tool group must not be split by a Runtime Note"
    );

    assert_eq!(
        store
            .flush_pending_runtime_notes("chat-live", "turn-live")
            .await
            .unwrap(),
        vec![note.clone()]
    );
    let messages = store.read_message_range("chat-live", 1, 10).await.unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(
        RuntimeNote::from_message_log_entry(&messages[1]),
        Some(note)
    );
    let projection = store
        .build_context_projection(
            "chat-live",
            ContextProjectionRequest {
                current_user_input: None,
            },
        )
        .await
        .unwrap();
    assert!(projection
        .tail_messages
        .iter()
        .any(|message| message.message_id == "memory-signal:live"));
    let recall = store
        .conversation_recall(
            "chat-live",
            ConversationRecallRequest {
                query: "corrected fact".to_string(),
                max_results: 10,
                surrounding_messages: 0,
            },
        )
        .await
        .unwrap();
    assert_eq!(recall.hits.len(), 1);
    assert_eq!(recall.hits[0].message.message_id, "memory-signal:live");
    assert!(store
        .flush_pending_runtime_notes("chat-live", "turn-live")
        .await
        .unwrap()
        .is_empty());
}
