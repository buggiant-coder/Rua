use std::sync::Arc;

use agent_types::{ModelDialogueItem, Role};
use chrono::Utc;
use react_agent::hooks::AgentHook;
use react_agent::ModelCallProjector;
use rua_agent::projection_preparation::ContextProjectionProjector;
use rua_agent::task_runtime_notes::TaskRuntimeNoteSink;
use rua_agent::turn_persistence::{TurnPersistenceHook, LOADED_MESSAGE_COUNT_METADATA_KEY};
use rua_contract::agent::{AgentInputMessage, AgentRequest};
use rua_session::{
    ChatMetadata, ChatSessionHeader, NewConversationMessage, NewConversationTurn, RuntimeNote,
    RuntimeNotePayload, TurnLifecycle, WorkspaceSessionStore,
};
use rua_task::{
    TaskAttempt, TaskCompletionNotification, TaskDeviceContext, TaskNotificationSink, TaskOrigin,
    TaskOwner, TaskRecord, TaskResult, TaskRunnerSpec, TaskSpec, TaskState,
};

fn task_record(chat_id: &str, attempt_no: u32, state: TaskState, output: &str) -> TaskRecord {
    task_record_for_turn(chat_id, "turn-1", attempt_no, state, output)
}

fn task_record_for_turn(
    chat_id: &str,
    turn_id: &str,
    attempt_no: u32,
    state: TaskState,
    output: &str,
) -> TaskRecord {
    let now = Utc::now();
    let result = TaskResult {
        status: state,
        output: output.to_string(),
        full_result_ref: None,
        artifacts: Vec::new(),
        device: TaskDeviceContext::default(),
        error: None,
    };
    TaskRecord {
        task_id: "task-1".to_string(),
        spec: TaskSpec {
            owner: TaskOwner::new("agent-1", chat_id),
            instruction: "fixture".to_string(),
            runner: TaskRunnerSpec::Tool {
                tool_name: "Fixture_Task".to_string(),
                arguments: serde_json::json!({}),
            },
            context_refs: Vec::new(),
            depends_on: Vec::new(),
            origin: TaskOrigin {
                turn_id: Some(turn_id.to_string()),
                tool_call_id: None,
            },
        },
        state,
        attempt_no,
        input_capability: rua_task::TaskInputCapability::None,
        watch_active: false,
        created_at: now,
        updated_at: now,
        started_at: Some(now),
        finished_at: Some(now),
        result: Some(result.clone()),
        attempts: vec![TaskAttempt {
            configuration_epoch: None,
            attempt_no,
            state,
            authorization_agent_id: "agent-1".to_string(),
            device: TaskDeviceContext::default(),
            created_at: now,
            updated_at: now,
            started_at: Some(now),
            finished_at: Some(now),
            result: Some(result),
        }],
        waiting_on: Vec::new(),
        blocked_by: Vec::new(),
    }
}

#[tokio::test]
async fn task_attempt_notes_are_durable_message_log_records() {
    let data_dir = tempfile::tempdir().unwrap();
    let workspace_dir = data_dir.path().join("workspaces").join("agent-1");
    let chat_id = "chat-1";
    let store = WorkspaceSessionStore::new(&workspace_dir);
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "agent-1".to_string(),
            title: "Task notes".to_string(),
            created_at: "2026-07-27T00:00:00Z".to_string(),
            last_activity_at: "2026-07-27T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .unwrap();
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-07-27T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-07-27T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-07-27T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("variant-1".to_string()),
                messages: vec![NewConversationMessage {
                    message_id: "user-1".to_string(),
                    role: "user".to_string(),
                    created_at: "2026-07-27T00:00:00Z".to_string(),
                    status: "completed".to_string(),
                    content: serde_json::json!([{"type": "text", "text": "start task"}]),
                    provider: None,
                    tool: None,
                    metadata: serde_json::Map::new(),
                }],
            },
        )
        .await
        .unwrap();

    let sink = TaskRuntimeNoteSink::new(data_dir.path());
    sink.terminal(TaskCompletionNotification {
        record: task_record(chat_id, 1, TaskState::Failed, "attempt one failed"),
    })
    .await
    .unwrap();

    sink.terminal(TaskCompletionNotification {
        record: task_record(chat_id, 2, TaskState::Completed, "attempt two completed"),
    })
    .await
    .unwrap();

    let messages = store.read_message_range(chat_id, 1, 10).await.unwrap();
    let notes = messages
        .iter()
        .filter_map(RuntimeNote::from_message_log_entry)
        .collect::<Vec<_>>();
    assert_eq!(notes.len(), 2);
    assert_eq!(notes[0].runtime_note_id, "task:task-1:attempt:1");
    let RuntimeNotePayload::TaskAttemptTerminal(first) = &notes[0].payload else {
        panic!("first note must be a Task Attempt Terminal");
    };
    assert_eq!(first.attempt_no, 1);
    assert_eq!(first.output, "attempt one failed");
    assert_eq!(first.terminal_status, "failed");
    assert_eq!(notes[1].runtime_note_id, "task:task-1:attempt:2");
    let RuntimeNotePayload::TaskAttemptTerminal(second) = &notes[1].payload else {
        panic!("second note must be a Task Attempt Terminal");
    };
    assert_eq!(second.attempt_no, 2);
    assert_eq!(second.output, "attempt two completed");
    assert_eq!(second.terminal_status, "completed");
    let header = store.load_chat_header(chat_id).await.unwrap().unwrap();
    assert!(!header.extra.contains_key("runtime_notes"));
}

#[tokio::test]
async fn in_flight_task_note_flushes_after_complete_model_call_group() {
    let data_dir = tempfile::tempdir().unwrap();
    let workspace_dir = data_dir.path().join("workspaces").join("agent-1");
    let chat_id = "chat-boundary";
    let store = WorkspaceSessionStore::new(&workspace_dir);
    store
        .save_chat_header(&ChatSessionHeader::new(ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "agent-1".to_string(),
            title: "Runtime Note boundary".to_string(),
            created_at: "2026-07-27T00:00:00Z".to_string(),
            last_activity_at: "2026-07-27T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        }))
        .await
        .unwrap();
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: "turn-boundary".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-07-27T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-07-27T00:00:01Z".to_string()),
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![NewConversationMessage {
                    message_id: "user-boundary".to_string(),
                    role: "user".to_string(),
                    created_at: "2026-07-27T00:00:00Z".to_string(),
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

    TaskRuntimeNoteSink::new(data_dir.path())
        .terminal(TaskCompletionNotification {
            record: task_record_for_turn(
                chat_id,
                "turn-boundary",
                1,
                TaskState::Completed,
                "background task completed",
            ),
        })
        .await
        .unwrap();
    assert_eq!(
        store
            .read_message_range(chat_id, 1, 10)
            .await
            .unwrap()
            .len(),
        1
    );

    let request = AgentRequest {
        session_id: chat_id.into(),
        user_id: "user-1".to_string(),
        agent_id: "agent-1".into(),
        messages: vec![AgentInputMessage {
            role: "user".to_string(),
            content: vec![],
        }],
        model_override: None,
        tool_mode: "auto".to_string(),
        attachments: Vec::new(),
        display_preferences: None,
        disabled_tools: Vec::new(),
        metadata: std::collections::HashMap::from([
            (
                "turn_id".to_string(),
                serde_json::Value::String("turn-boundary".to_string()),
            ),
            (
                LOADED_MESSAGE_COUNT_METADATA_KEY.to_string(),
                serde_json::json!(1),
            ),
        ]),
        cancel: None,
        run_control: None,
        workspace_dir: Some(workspace_dir.clone()),
        conversation_store_dir: Some(workspace_dir.clone()),
        working_dir: Some(workspace_dir),
    };
    let hook = TurnPersistenceHook::from_request(&request).expect("Turn persistence hook");
    let mut model_response = ModelDialogueItem::text(Role::Assistant, "model response");
    model_response.metadata.insert(
        agent_types::conversation_message_identity::TEXT_MESSAGE_ID_METADATA_KEY.to_string(),
        serde_json::json!("msg-model-response"),
    );
    hook.after_model_call(
        &[model_response],
        1,
        &agent_types::TokenUsage::default(),
        None,
        false,
    )
    .await
    .unwrap();

    let messages = store.read_message_range(chat_id, 1, 10).await.unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[1].role, "assistant");
    assert!(RuntimeNote::from_message_log_entry(&messages[2]).is_some());

    let projector = ContextProjectionProjector::from_request(
        &request,
        None,
        Arc::new(rua_config::AgentsRunningConfig::default()),
        rua_session::CheckpointModelIdentity {
            provider: "test".to_string(),
            model: "runtime-note-boundary".to_string(),
        },
        Some(4_096),
        None,
    )
    .expect("Context Projection projector");
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(4);
    let projected = projector
        .project(
            agent_types::ChatRequest::streaming(vec![
                ModelDialogueItem::text(Role::User, "start"),
                ModelDialogueItem::text(Role::Assistant, "model response"),
            ]),
            &event_tx,
        )
        .await
        .expect("project durable Runtime Note");
    assert!(projected.messages.iter().any(|message| {
        message
            .metadata
            .get("message_type")
            .and_then(serde_json::Value::as_str)
            == Some("runtime_note")
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_note_survives_an_in_flight_turn_header_commit() {
    let data_dir = tempfile::tempdir().unwrap();
    let workspace_dir = data_dir.path().join("workspaces").join("agent-1");
    let chat_id = "chat-race";
    let store = WorkspaceSessionStore::new(&workspace_dir);
    let mut header = ChatSessionHeader::new(ChatMetadata {
        chat_id: chat_id.to_string(),
        agent_id: "agent-1".to_string(),
        title: "Task note race".to_string(),
        created_at: "2026-07-27T00:00:00Z".to_string(),
        last_activity_at: "2026-07-27T00:00:00Z".to_string(),
        pinned: false,
        archived: false,
        preview: None,
    });
    header.message_log.chunk_size = 1;
    store.save_chat_header(&header).await.unwrap();

    let append_store = WorkspaceSessionStore::new(&workspace_dir);
    let append = tokio::spawn(async move {
        let messages = (0..512)
            .map(|index| NewConversationMessage {
                message_id: format!("message-{index}"),
                role: "assistant".to_string(),
                created_at: "2026-07-27T00:00:01Z".to_string(),
                status: "completed".to_string(),
                content: serde_json::json!([{
                    "type": "text",
                    "text": format!("output-{index}")
                }]),
                provider: None,
                tool: None,
                metadata: serde_json::Map::new(),
            })
            .collect();
        append_store
            .append_conversation_turn(
                chat_id,
                NewConversationTurn {
                    turn_id: "turn-race".to_string(),
                    lifecycle: TurnLifecycle {
                        user_submitted_at: "2026-07-27T00:00:00Z".to_string(),
                        assistant_started_at: Some("2026-07-27T00:00:01Z".to_string()),
                        assistant_completed_at: Some("2026-07-27T00:00:02Z".to_string()),
                        status: "completed".to_string(),
                    },
                    selected_variant_id: None,
                    messages,
                },
            )
            .await
            .unwrap();
    });

    let first_chunk = workspace_dir
        .join("sessions")
        .join(chat_id)
        .join("messages")
        .join("000000000000.jsonl");
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !first_chunk.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("turn commit should start writing message chunks");
    assert!(
        !append.is_finished(),
        "the regression requires the turn commit to still hold the Chat write lock"
    );

    TaskRuntimeNoteSink::new(data_dir.path())
        .terminal(TaskCompletionNotification {
            record: task_record_for_turn(
                chat_id,
                "turn-race",
                1,
                TaskState::Completed,
                "fast task completed",
            ),
        })
        .await
        .unwrap();
    append.await.unwrap();

    let header = store.load_chat_header(chat_id).await.unwrap().unwrap();
    let messages = store.read_message_range(chat_id, 1, 600).await.unwrap();
    let notes = messages
        .iter()
        .filter_map(RuntimeNote::from_message_log_entry)
        .collect::<Vec<_>>();
    assert_eq!(notes.len(), 1);
    let RuntimeNotePayload::TaskAttemptTerminal(note) = &notes[0].payload else {
        panic!("Task terminal must persist as a Task Attempt Terminal Runtime Note");
    };
    assert_eq!(note.output, "fast task completed");
    assert_eq!(header.message_log.latest_sequence, 513);
}
