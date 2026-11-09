use futures::StreamExt;
use rua_channel::core::console::SessionPersistence;
use rua_channel::core::console::{
    AttachmentInfo, ConsoleChannelService, ConsoleChatRequest, ConsoleSubmitKind, ProcessRequest,
};
use rua_contract::agent::{AgentInputMessage, AgentRequest, AgentRuntime, RuntimeWarmupContext};
use rua_core::message::ContentPart;
use rua_core::{RuaError, RunControl, RuntimeEvent, ToolCall, ToolResult};
use rua_server::endpoints::dto;
use rua_server::services::chat_index;
use rua_server::services::console_chat::{
    materialize_targeted_chat, persist_conversation_user_stop_to_session,
    WorkspaceSessionPersistence, TURN_ABORTED_MARKER_TEXT,
};
use rua_server::services::conversation_authority::ConversationAuthority;
use rua_server::services::conversation_authority_placement::{
    ConversationAuthorityEpoch, ConversationAuthorityPlacement, CHAT_AUTHORITY_PLACEMENT_META_KEY,
};
use rua_server::{AppState, ServerProfile};
use rua_sync::evergreen_chat_adapter::collect_evergreen_chat_file_snapshots;
use serde_json::json;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::{Mutex, Notify};

fn epoch(wall_ms: u64) -> ConversationAuthorityEpoch {
    ConversationAuthorityEpoch::from_parts(wall_ms, 0, [0; 8])
}

fn running_cancel_rx() -> tokio::sync::watch::Receiver<RunControl> {
    tokio::sync::watch::channel(RunControl::Running).1
}

fn authority_placement(device_id: &str) -> ConversationAuthorityPlacement {
    ConversationAuthorityPlacement {
        device_id: device_id.to_string(),
        authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
        base_manifest_hash:
            "sha256:1111111111111111111111111111111111111111111111111111111111111111".to_string(),
    }
}

async fn materialized_chat_header(
    state: &AppState,
    workspace_dir: &std::path::Path,
    metadata: rua_session::ChatMetadata,
) -> rua_session::ChatSessionHeader {
    materialize_local_chat(
        state,
        workspace_dir,
        &metadata.agent_id,
        &metadata.chat_id,
        &metadata.title,
    )
    .await;
    let store = rua_session::WorkspaceSessionStore::new(workspace_dir);
    let mut header = store
        .load_chat_header(&metadata.chat_id)
        .await
        .expect("load materialized Chat Header")
        .expect("materialized Chat Header");
    header.agent_id = metadata.agent_id.clone();
    header.metadata = metadata;
    header
}

async fn materialize_local_chat(
    state: &AppState,
    workspace_dir: &std::path::Path,
    agent_id: &str,
    chat_id: &str,
    title: &str,
) {
    let device_id =
        rua_server::services::device_context::current_sync_device_id(&state.sync_device)
            .expect("test Device identity");
    materialize_targeted_chat(
        workspace_dir,
        agent_id,
        chat_id,
        title,
        &authority_placement(&device_id),
    )
    .await
    .expect("materialize local Chat Index placement");
    let store = rua_session::WorkspaceSessionStore::new(workspace_dir);
    let mut header = store.load_chat_header(chat_id).await.unwrap().unwrap();
    seed_runtime_configuration(state, agent_id, &mut header).await;
    store.save_chat_header(&header).await.unwrap();
}

async fn targeted_chat_header(
    workspace_dir: &std::path::Path,
    authority_device_id: &str,
    metadata: rua_session::ChatMetadata,
) -> rua_session::ChatSessionHeader {
    materialize_targeted_chat(
        workspace_dir,
        &metadata.agent_id,
        &metadata.chat_id,
        &metadata.title,
        &authority_placement(authority_device_id),
    )
    .await
    .expect("materialize Chat Index placement");
    let store = rua_session::WorkspaceSessionStore::new(workspace_dir);
    let mut header = store
        .load_chat_header(&metadata.chat_id)
        .await
        .expect("load targeted Chat Header")
        .expect("targeted Chat Header");
    header.agent_id = metadata.agent_id.clone();
    header.metadata = metadata;
    header
}

fn text(role: &str, value: &str) -> AgentInputMessage {
    AgentInputMessage {
        role: role.to_string(),
        content: vec![ContentPart::Text {
            text: value.to_string(),
        }],
    }
}

fn history_replacement_trace(marker: &str) -> Vec<AgentInputMessage> {
    let mut tool_result = ToolResult::text("call-1", format!("{marker}{}", "x".repeat(70_000)));
    tool_result.name = Some("read".to_string());
    vec![
        text("user", "Inspect src/lib.rs"),
        AgentInputMessage {
            role: "assistant".to_string(),
            content: vec![
                ContentPart::Thinking {
                    text: "I need to inspect the source.".to_string(),
                    signature: None,
                },
                ContentPart::ToolUse {
                    call: ToolCall::new("call-1", "read", json!({"path": "src/lib.rs"})),
                },
            ],
        },
        AgentInputMessage {
            role: "tool".to_string(),
            content: vec![ContentPart::ToolResult {
                result: tool_result,
            }],
        },
        text(
            "assistant",
            "The repository uses a durable runtime boundary.",
        ),
    ]
}

fn message_log_text_for_test(message: &rua_session::MessageLogEntry) -> String {
    match &message.content {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        value => value.to_string(),
    }
}

fn tool_call_message(call_id: &str, name: &str) -> AgentInputMessage {
    AgentInputMessage {
        role: "assistant".to_string(),
        content: vec![ContentPart::ToolUse {
            call: ToolCall::new(
                call_id,
                name,
                json!({
                    "command": "du -sh .",
                    "timeout": 30,
                }),
            ),
        }],
    }
}

fn tool_result_message(call_id: &str, output: &str) -> AgentInputMessage {
    AgentInputMessage {
        role: "system".to_string(),
        content: vec![ContentPart::ToolResult {
            result: ToolResult::text(call_id, output),
        }],
    }
}

fn conversation_message_from_input(
    message_id: &str,
    message: &AgentInputMessage,
) -> rua_session::NewConversationMessage {
    rua_session::NewConversationMessage {
        message_id: message_id.to_string(),
        role: message.role.clone(),
        created_at: "2026-07-24T00:00:01Z".to_string(),
        status: "completed".to_string(),
        content: serde_json::to_value(&message.content).expect("message content"),
        provider: None,
        tool: None,
        metadata: serde_json::Map::new(),
    }
}

#[tokio::test]
async fn console_persistence_writes_evergreen_message_log_and_turn_index() {
    let dir = tempfile::tempdir().unwrap();
    let persistence = WorkspaceSessionPersistence::new(dir.path().to_path_buf());

    seed_persistence_chat(dir.path(), "chat-1").await;
    persistence
        .save_chat_entry("agent-1", "chat-1", "First request")
        .await
        .unwrap();
    persistence
        .save_messages(
            "agent-1",
            "chat-1",
            &[text("user", "hello"), text("assistant", "hi")],
        )
        .await
        .unwrap();

    let store = rua_session::WorkspaceSessionStore::new(dir.path());
    let header = store
        .load_chat_header("chat-1")
        .await
        .unwrap()
        .expect("chat header");
    assert_eq!(header.schema_version, 3);
    assert_eq!(header.metadata.title, "First request");
    assert_eq!(header.message_log.latest_sequence, 2);
    assert_eq!(header.turn_index.latest_turn, 1);
    assert_eq!(
        header
            .metadata
            .preview
            .as_ref()
            .map(|preview| preview.text.as_str()),
        Some("hi")
    );

    let messages = store.read_message_range("chat-1", 1, 10).await.unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[0].content[0]["text"], "hello");
    assert_eq!(messages[1].role, "assistant");
    assert_eq!(messages[1].content[0]["text"], "hi");

    let turns = store.read_turn_range("chat-1", 1, 10).await.unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].first_sequence, 1);
    assert_eq!(turns[0].last_sequence, 2);
    assert!(
        turns[0].selected_variant_id.is_some(),
        "Evergreen Turn Index must retain selected assistant variant state"
    );
}

#[tokio::test]
async fn console_persistence_completes_eager_user_turn_without_duplicate_user() {
    let dir = tempfile::tempdir().unwrap();
    let persistence = WorkspaceSessionPersistence::new(dir.path().to_path_buf())
        .with_active_turn_id("turn-eager".to_string());

    seed_persistence_chat(dir.path(), "chat-1").await;
    persistence
        .save_chat_entry("agent-1", "chat-1", "Run a slow command")
        .await
        .unwrap();

    let store = rua_session::WorkspaceSessionStore::new(dir.path());
    store
        .append_conversation_turn(
            "chat-1",
            rua_session::NewConversationTurn {
                turn_id: "turn-eager".to_string(),
                lifecycle: rua_session::TurnLifecycle {
                    user_submitted_at: "2026-06-20T00:00:00Z".to_string(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![conversation_message(
                    "eager-user-1",
                    "user",
                    "Run a slow command",
                )],
            },
        )
        .await
        .unwrap();

    persistence
        .save_messages(
            "agent-1",
            "chat-1",
            &[text("assistant", "slow command complete")],
        )
        .await
        .unwrap();

    let messages = store.read_message_range("chat-1", 1, 10).await.unwrap();
    let roles: Vec<_> = messages
        .iter()
        .map(|message| message.role.as_str())
        .collect();
    assert_eq!(roles, vec!["user", "assistant"]);
    assert_eq!(messages[0].content[0]["text"], "Run a slow command");
    assert_eq!(messages[1].content[0]["text"], "slow command complete");

    let turns = store.read_turn_range("chat-1", 1, 10).await.unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].turn_id, "turn-eager");
    assert_eq!(turns[0].lifecycle.status, "completed");
    assert_eq!(turns[0].first_sequence, 1);
    assert_eq!(turns[0].last_sequence, 2);
}

#[tokio::test]
async fn console_persistence_completes_after_incremental_output_without_duplicates() {
    let dir = tempfile::tempdir().unwrap();
    let persistence = WorkspaceSessionPersistence::new(dir.path().to_path_buf())
        .with_active_turn_id("turn-boundary".to_string());
    seed_persistence_chat(dir.path(), "chat-boundary").await;
    persistence
        .save_chat_entry("agent-1", "chat-boundary", "Inspect disk usage")
        .await
        .unwrap();

    let user = text("user", "Inspect disk usage");
    let tool_call = tool_call_message("call-1", "bash");
    let tool_result = tool_result_message("call-1", "done");
    let final_answer = text("assistant", "The command completed.");
    let store = rua_session::WorkspaceSessionStore::new(dir.path());
    store
        .append_conversation_turn(
            "chat-boundary",
            rua_session::NewConversationTurn {
                turn_id: "turn-boundary".to_string(),
                lifecycle: rua_session::TurnLifecycle {
                    user_submitted_at: "2026-07-24T00:00:00Z".to_string(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![conversation_message_from_input("user-1", &user)],
            },
        )
        .await
        .unwrap();

    rua_session::TurnWriter::new(&store, "chat-boundary", "turn-boundary")
        .append_in_progress_output(
            vec![
                conversation_message_from_input("assistant-tool", &tool_call),
                conversation_message_from_input("tool-result", &tool_result),
            ],
            Some("2026-07-24T00:00:01Z"),
        )
        .await
        .unwrap();
    store
        .append_runtime_note(
            "chat-boundary",
            "turn-boundary",
            rua_session::RuntimeNote {
                runtime_note_id: "capability-note".to_string(),
                created_at: "2026-07-24T00:00:02Z".to_string(),
                payload: rua_session::RuntimeNotePayload::CapabilityState(
                    rua_session::CapabilityStateRuntimeNote {
                        previous_generation: 1,
                        current_generation: 2,
                        changes: Vec::new(),
                    },
                ),
            },
        )
        .await
        .unwrap();
    store
        .flush_pending_runtime_notes("chat-boundary", "turn-boundary")
        .await
        .unwrap();
    rua_session::TurnWriter::new(&store, "chat-boundary", "turn-boundary")
        .append_in_progress_output(
            vec![conversation_message_from_input(
                "assistant-final",
                &final_answer,
            )],
            None,
        )
        .await
        .unwrap();

    persistence
        .finalize_turn(
            "agent-1",
            "chat-boundary",
            Some("2026-07-24T00:00:01Z"),
            None,
            None,
        )
        .await
        .unwrap();

    let messages = store
        .read_turn_messages("chat-boundary", "turn-boundary")
        .await
        .unwrap();
    assert_eq!(messages.len(), 5);
    assert_eq!(
        messages
            .iter()
            .map(|message| message.role.as_str())
            .collect::<Vec<_>>(),
        vec!["user", "assistant", "system", "system", "assistant"]
    );
    assert_eq!(
        messages
            .iter()
            .map(|message| message.sequence)
            .collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 5]
    );
    assert!(rua_session::RuntimeNote::from_message_log_entry(&messages[3]).is_some());

    let turn = store
        .read_turn_range("chat-boundary", 1, 1)
        .await
        .unwrap()
        .pop()
        .expect("turn");
    assert_eq!(turn.lifecycle.status, "completed");
    assert!(turn.metadata.get("model_call_boundaries").is_none());
}

#[tokio::test]
async fn console_persistence_uses_runtime_started_at_for_eager_turn() {
    let dir = tempfile::tempdir().unwrap();
    let persistence = WorkspaceSessionPersistence::new(dir.path().to_path_buf())
        .with_active_turn_id("turn-eager".to_string())
        .with_assistant_started_at("2026-06-20T00:00:01Z".to_string());

    seed_persistence_chat(dir.path(), "chat-1").await;
    persistence
        .save_chat_entry("agent-1", "chat-1", "Run a slow command")
        .await
        .unwrap();

    let store = rua_session::WorkspaceSessionStore::new(dir.path());
    store
        .append_conversation_turn(
            "chat-1",
            rua_session::NewConversationTurn {
                turn_id: "turn-eager".to_string(),
                lifecycle: rua_session::TurnLifecycle {
                    user_submitted_at: "2026-06-20T00:00:00Z".to_string(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![conversation_message(
                    "eager-user-1",
                    "user",
                    "Run a slow command",
                )],
            },
        )
        .await
        .unwrap();

    persistence
        .save_messages(
            "agent-1",
            "chat-1",
            &[
                text("user", "Run a slow command"),
                text("assistant", "slow command complete"),
            ],
        )
        .await
        .unwrap();

    let turns = store.read_turn_range("chat-1", 1, 10).await.unwrap();
    assert_eq!(
        turns[0].lifecycle.assistant_started_at.as_deref(),
        Some("2026-06-20T00:00:01Z")
    );
    assert_eq!(turns[0].lifecycle.status, "completed");
    assert!(turns[0].lifecycle.assistant_completed_at.is_some());
    assert_ne!(
        turns[0].lifecycle.assistant_started_at,
        turns[0].lifecycle.assistant_completed_at
    );
}

#[tokio::test]
async fn console_persistence_stop_repairs_legacy_sequence_reservation_gap() {
    let dir = tempfile::tempdir().unwrap();
    let persistence = WorkspaceSessionPersistence::new(dir.path().to_path_buf());
    seed_persistence_chat(dir.path(), "chat-1").await;
    persistence
        .save_chat_entry("agent-1", "chat-1", "first request")
        .await
        .unwrap();
    let store = rua_session::WorkspaceSessionStore::new(dir.path());
    store
        .append_conversation_turn(
            "chat-1",
            rua_session::NewConversationTurn {
                turn_id: "turn-first".to_string(),
                lifecycle: rua_session::TurnLifecycle {
                    user_submitted_at: "2026-06-20T00:00:00Z".to_string(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![conversation_message("first-user", "user", "first request")],
            },
        )
        .await
        .unwrap();

    let mut header = store.load_chat_header("chat-1").await.unwrap().unwrap();
    header.message_log.latest_sequence = 14;
    header.message_log.allocated_sequence = 14;
    store.save_chat_header(&header).await.unwrap();

    persist_conversation_user_stop_to_session(dir.path(), "agent-1", "chat-1", "turn-first", None)
        .await
        .unwrap();

    let messages = store.read_message_range("chat-1", 1, 20).await.unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].sequence, 1);
    assert_eq!(messages[1].sequence, 2);
    assert_eq!(messages[1].content[0]["text"], TURN_ABORTED_MARKER_TEXT);
}

#[tokio::test]
async fn console_persistence_stop_then_late_save_then_second_submit_does_not_duplicate_users() {
    let dir = tempfile::tempdir().unwrap();
    let persistence = WorkspaceSessionPersistence::new(dir.path().to_path_buf())
        .with_active_turn_id("turn-first".to_string());

    seed_persistence_chat(dir.path(), "chat-1").await;
    persistence
        .save_chat_entry("agent-1", "chat-1", "First request")
        .await
        .unwrap();

    let store = rua_session::WorkspaceSessionStore::new(dir.path());
    store
        .append_conversation_turn(
            "chat-1",
            rua_session::NewConversationTurn {
                turn_id: "turn-first".to_string(),
                lifecycle: rua_session::TurnLifecycle {
                    user_submitted_at: "2026-06-20T00:00:00Z".to_string(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![conversation_message("first-user", "user", "first request")],
            },
        )
        .await
        .unwrap();

    persist_conversation_user_stop_to_session(dir.path(), "agent-1", "chat-1", "turn-first", None)
        .await
        .unwrap();

    persistence
        .save_messages(
            "agent-1",
            "chat-1",
            &[text("assistant", "partial first response")],
        )
        .await
        .unwrap();

    WorkspaceSessionPersistence::new(dir.path().to_path_buf())
        .save_messages(
            "agent-1",
            "chat-1",
            &[
                text("user", "second request"),
                text("assistant", "second response"),
            ],
        )
        .await
        .unwrap();

    let messages = store.read_message_range("chat-1", 1, 10).await.unwrap();
    assert!(
        messages.iter().any(|message| {
            message.role == "user"
                && message.metadata.get("model_visible_marker")
                    == Some(&serde_json::json!("turn_aborted"))
                && message.content[0]["text"] == TURN_ABORTED_MARKER_TEXT
        }),
        "user stop should persist a model-visible turn-aborted marker"
    );
    let user_texts = messages
        .iter()
        .filter(|message| {
            message.role == "user" && !message.metadata.contains_key("model_visible_marker")
        })
        .map(|message| message.content[0]["text"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(user_texts, vec!["first request", "second request"]);

    let turns = store.read_turn_range("chat-1", 1, 10).await.unwrap();
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[0].turn_id, "turn-first");
    assert_eq!(turns[0].lifecycle.status, "interrupted");
    assert_eq!(turns[1].lifecycle.status, "completed");
}

#[tokio::test]
async fn console_persistence_keeps_eager_turn_open_for_unmatched_tool_call() {
    let dir = tempfile::tempdir().unwrap();
    let persistence = WorkspaceSessionPersistence::new(dir.path().to_path_buf())
        .with_active_turn_id("turn-eager".to_string());

    seed_persistence_chat(dir.path(), "chat-1").await;
    persistence
        .save_chat_entry("agent-1", "chat-1", "Inspect disk usage")
        .await
        .unwrap();

    let store = rua_session::WorkspaceSessionStore::new(dir.path());
    store
        .append_conversation_turn(
            "chat-1",
            rua_session::NewConversationTurn {
                turn_id: "turn-eager".to_string(),
                lifecycle: rua_session::TurnLifecycle {
                    user_submitted_at: "2026-06-20T00:00:00Z".to_string(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![conversation_message(
                    "eager-user-1",
                    "user",
                    "Inspect disk usage",
                )],
            },
        )
        .await
        .unwrap();

    persistence
        .save_messages(
            "agent-1",
            "chat-1",
            &[
                text("user", "Inspect disk usage"),
                tool_call_message("call-1", "bash"),
            ],
        )
        .await
        .unwrap();

    let turns = store.read_turn_range("chat-1", 1, 10).await.unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].turn_id, "turn-eager");
    assert_eq!(turns[0].lifecycle.status, "in_progress");
    assert!(turns[0].lifecycle.assistant_started_at.is_some());
    assert!(turns[0].lifecycle.assistant_completed_at.is_none());
}

#[tokio::test]
async fn console_persistence_completes_turn_after_tool_result_arrives() {
    let dir = tempfile::tempdir().unwrap();
    let persistence = WorkspaceSessionPersistence::new(dir.path().to_path_buf());

    seed_persistence_chat(dir.path(), "chat-1").await;
    persistence
        .save_chat_entry("agent-1", "chat-1", "Inspect disk usage")
        .await
        .unwrap();
    persistence
        .save_messages(
            "agent-1",
            "chat-1",
            &[
                text("user", "Inspect disk usage"),
                tool_call_message("call-1", "bash"),
                tool_result_message("call-1", "done"),
                text("assistant", "The command completed."),
            ],
        )
        .await
        .unwrap();

    let store = rua_session::WorkspaceSessionStore::new(dir.path());
    let turns = store.read_turn_range("chat-1", 1, 10).await.unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].lifecycle.status, "completed");
    assert!(turns[0].lifecycle.assistant_completed_at.is_some());
}

#[tokio::test]
async fn console_persistence_rejects_output_without_active_turn_id() {
    let dir = tempfile::tempdir().unwrap();
    let persistence = WorkspaceSessionPersistence::new(dir.path().to_path_buf());

    seed_persistence_chat(dir.path(), "chat-1").await;
    persistence
        .save_chat_entry("agent-1", "chat-1", "Existing turn")
        .await
        .unwrap();
    let store = rua_session::WorkspaceSessionStore::new(dir.path());
    store
        .append_conversation_turn(
            "chat-1",
            rua_session::NewConversationTurn {
                turn_id: "turn-existing".to_string(),
                lifecycle: rua_session::TurnLifecycle {
                    user_submitted_at: "2026-06-20T00:00:00Z".to_string(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![conversation_message("existing-user", "user", "hello")],
            },
        )
        .await
        .unwrap();

    let err = persistence
        .save_messages("agent-1", "chat-1", &[text("assistant", "late output")])
        .await
        .expect_err("assistant output without an active turn must be rejected");
    assert!(
        err.to_string().contains("without active_turn_id"),
        "unexpected error: {err}"
    );

    let messages = store.read_message_range("chat-1", 1, 10).await.unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, "user");
}

#[tokio::test]
async fn evergreen_chat_snapshot_collects_syncable_session_files() {
    let dir = tempfile::tempdir().unwrap();
    let persistence = WorkspaceSessionPersistence::new(dir.path().to_path_buf());

    seed_persistence_chat(dir.path(), "chat-1").await;
    persistence
        .save_chat_entry("agent-1", "chat-1", "Syncable Chat")
        .await
        .unwrap();
    persistence
        .save_messages(
            "agent-1",
            "chat-1",
            &[text("user", "hello"), text("assistant", "hi")],
        )
        .await
        .unwrap();

    let items = collect_evergreen_chat_file_snapshots(dir.path(), "chat-1", "device-test");
    let relatives: Vec<_> = items.iter().map(|item| item.relative.as_str()).collect();
    assert!(relatives.contains(&"session"));
    assert!(relatives.contains(&"turn_index"));
    assert!(relatives.contains(&"messages/000000000000.jsonl"));
    let message_log = items
        .iter()
        .find(|item| item.relative == "messages/000000000000.jsonl")
        .and_then(|item| item.payload.as_str())
        .expect("message log payload");
    assert!(message_log.contains("\"message_id\""));
    assert!(message_log.contains("hello"));
    assert!(message_log.contains("hi"));
}

struct CapturingRuntime {
    captured: Arc<Mutex<Option<AgentRequest>>>,
}

#[async_trait::async_trait]
impl AgentRuntime for CapturingRuntime {
    async fn stream(
        &self,
        req: AgentRequest,
    ) -> rua_core::Result<
        Pin<Box<dyn futures::Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>,
    > {
        *self.captured.lock().await = Some(req.clone());
        let stream = Box::pin(futures::stream::iter(vec![
            Ok(RuntimeEvent::Start {
                session_id: "chat-1".into(),
                agent_id: Some("default".into()),
            }),
            Ok(RuntimeEvent::TextDelta {
                delta: "captured".to_string(),
                message_id: Some(rua_core::MessageId("assistant-captured".to_string())),
            }),
            Ok(RuntimeEvent::End {
                finish_reason: Some("stop".to_string()),
            }),
        ]));
        Ok(rua_agent::turn_persistence::persist_runtime_stream(
            &req, stream,
        ))
    }

    async fn cancel(&self, _session_id: &rua_core::id::SessionId) -> rua_core::Result<()> {
        Ok(())
    }

    async fn warmup(&self, _context: RuntimeWarmupContext) -> rua_core::Result<()> {
        Ok(())
    }

    async fn shutdown(&self) -> rua_core::Result<()> {
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_chat_persists_clean_attachment_content_but_keeps_model_path_hint() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::from_tempdir(dir.path().to_path_buf(), ServerProfile::Test);
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime");
    let captured = Arc::new(Mutex::new(None));
    runtime
        .set_react_runtime(Arc::new(CapturingRuntime {
            captured: Arc::clone(&captured),
        }))
        .await;

    let chat_id = "00000000-0000-0000-0000-000000000320";
    materialize_local_chat(
        &state,
        &runtime.workspace_dir(),
        "default",
        chat_id,
        "Attachment persistence",
    )
    .await;

    let events: Vec<_> = state
        .chat_service
        .chat_stream(
            "default",
            runtime.clone(),
            dto::ChatRequest {
                message: "Describe this".to_string(),
                chat_id: Some(chat_id.to_string()),
                input: Some(vec![json!({
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "Describe this"},
                        {
                            "type": "image",
                            "image_url": "/tmp/photo.png",
                            "file_name": "photo.png"
                        }
                    ]
                })]),
                messages: None,
                user_id: None,
                channel: None,
                stream: Some(true),
                model: None,
                attachments: Vec::new(),
                extra: serde_json::Map::new(),
            },
            running_cancel_rx(),
        )
        .collect()
        .await;
    assert!(
        events.iter().all(|event| event.is_ok()),
        "events: {events:?}"
    );

    let request = captured
        .lock()
        .await
        .clone()
        .expect("runtime request captured");
    let model_user = request
        .messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .expect("model user message");
    assert!(model_user
        .content
        .iter()
        .any(|part| matches!(part, ContentPart::Image { url, .. } if url == "/tmp/photo.png")));
    assert!(model_user.content.iter().any(
        |part| matches!(part, ContentPart::Text { text } if text == "用户上传图片 `photo.png`，已经下载到 /tmp/photo.png")
    ));

    let stored = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir())
        .read_message_range(chat_id, 1, 20)
        .await
        .expect("stored messages");
    let stored_users = stored
        .iter()
        .filter(|message| message.role == "user")
        .collect::<Vec<_>>();
    assert_eq!(
        stored_users.len(),
        1,
        "the runtime must not append a second model-only user message"
    );
    let stored_user = stored_users[0];
    let stored_parts = stored_user
        .content
        .as_array()
        .expect("stored user content array");
    assert!(stored_parts
        .iter()
        .any(|part| part.get("type").and_then(serde_json::Value::as_str) == Some("image")));
    assert!(
        stored_parts.iter().all(|part| {
            match part.get("text").and_then(serde_json::Value::as_str) {
                Some(text) => !text.contains("已经下载到"),
                None => true,
            }
        }),
        "stored user content must not include model-only path hints: {stored_parts:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_chat_contributes_execution_environment_without_persisting_runtime_state() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::from_tempdir(dir.path().to_path_buf(), ServerProfile::Test);
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime");
    let captured = Arc::new(Mutex::new(None));
    runtime
        .set_react_runtime(Arc::new(CapturingRuntime {
            captured: Arc::clone(&captured),
        }))
        .await;

    let chat_id = "00000000-0000-0000-0000-000000000321";
    materialize_local_chat(
        &state,
        &runtime.workspace_dir(),
        "default",
        chat_id,
        "Execution environment",
    )
    .await;
    let events: Vec<_> = state
        .chat_service
        .chat_stream(
            "default",
            runtime.clone(),
            dto::ChatRequest {
                message: "describe the current runtime".to_string(),
                chat_id: Some(chat_id.to_string()),
                input: None,
                messages: None,
                user_id: None,
                channel: None,
                stream: Some(true),
                model: None,
                attachments: Vec::new(),
                extra: serde_json::Map::new(),
            },
            running_cancel_rx(),
        )
        .collect()
        .await;
    assert!(
        events.iter().all(|event| event.is_ok()),
        "events: {events:?}"
    );

    let request = captured
        .lock()
        .await
        .clone()
        .expect("runtime request captured");
    let environment_prompt = request
        .metadata
        .get(rua_agent::runtime::EXECUTION_ENVIRONMENT_METADATA_KEY)
        .and_then(serde_json::Value::as_str)
        .expect("typed Execution Environment contribution");
    assert!(environment_prompt.contains("Host profile: `test`"));
    assert!(environment_prompt.contains(&runtime.workspace_dir().display().to_string()));

    let header = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir())
        .load_chat_header(chat_id)
        .await
        .unwrap()
        .expect("chat header");
    assert!(
        !header
            .context_snapshot
            .sys_prompt
            .as_deref()
            .is_some_and(|prompt| prompt.contains("## Execution environment")),
        "runtime environment must be rebuilt for each Model Call, not persisted as Agent Prompt Context"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_chat_turn_mode_override_does_not_replace_chat_default() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::from_tempdir(dir.path().to_path_buf(), ServerProfile::Test);
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime");
    let captured = Arc::new(Mutex::new(None));
    runtime
        .set_react_runtime(Arc::new(CapturingRuntime {
            captured: Arc::clone(&captured),
        }))
        .await;

    let chat_id = "00000000-0000-0000-0000-000000000322";
    materialize_local_chat(
        &state,
        &runtime.workspace_dir(),
        "default",
        chat_id,
        "Collaboration mode",
    )
    .await;
    let store = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir());
    let mut header = store
        .load_chat_header(chat_id)
        .await
        .unwrap()
        .expect("chat header");
    header.extra.insert(
        "agent_collaboration_mode".to_string(),
        serde_json::json!("plan"),
    );
    store.save_chat_header(&header).await.unwrap();

    let first_events: Vec<_> = state
        .chat_service
        .chat_stream(
            "default",
            runtime.clone(),
            dto::ChatRequest {
                message: "execute this turn".to_string(),
                chat_id: Some(chat_id.to_string()),
                input: None,
                messages: None,
                user_id: None,
                channel: None,
                stream: Some(true),
                model: None,
                attachments: Vec::new(),
                extra: serde_json::Map::from_iter([(
                    "collaboration_mode".to_string(),
                    serde_json::json!("execute"),
                )]),
            },
            running_cancel_rx(),
        )
        .collect()
        .await;
    assert!(first_events.iter().all(Result::is_ok));
    let first_request = captured.lock().await.take().expect("first runtime request");
    assert_eq!(
        first_request
            .metadata
            .get("agent_collaboration_mode")
            .and_then(serde_json::Value::as_str),
        Some("execute")
    );
    assert!(
        first_request
            .messages
            .iter()
            .all(|message| !matches!(message.role.as_str(), "system" | "developer")),
        "Console Chat must provide typed prompt inputs instead of leading model-visible messages"
    );

    let second_events: Vec<_> = state
        .chat_service
        .chat_stream(
            "default",
            runtime.clone(),
            dto::ChatRequest {
                message: "plan this turn".to_string(),
                chat_id: Some(chat_id.to_string()),
                input: None,
                messages: None,
                user_id: None,
                channel: None,
                stream: Some(true),
                model: None,
                attachments: Vec::new(),
                extra: serde_json::Map::new(),
            },
            running_cancel_rx(),
        )
        .collect()
        .await;
    assert!(second_events.iter().all(Result::is_ok));
    let second_request = captured
        .lock()
        .await
        .take()
        .expect("second runtime request");
    assert_eq!(
        second_request
            .metadata
            .get("agent_collaboration_mode")
            .and_then(serde_json::Value::as_str),
        Some("plan")
    );

    let turns = store.read_turn_range(chat_id, 1, usize::MAX).await.unwrap();
    assert_eq!(turns.len(), 2);
    assert_eq!(
        turns[0]
            .metadata
            .get("agent_collaboration_mode")
            .and_then(serde_json::Value::as_str),
        Some("execute")
    );
    assert_eq!(
        turns[1]
            .metadata
            .get("agent_collaboration_mode")
            .and_then(serde_json::Value::as_str),
        Some("plan")
    );
}

struct FailingRuntime;

#[async_trait::async_trait]
impl AgentRuntime for FailingRuntime {
    async fn stream(
        &self,
        _req: AgentRequest,
    ) -> rua_core::Result<
        Pin<Box<dyn futures::Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>,
    > {
        Ok(Box::pin(futures::stream::iter(vec![Err(
            RuaError::Provider {
                provider: "sync-e2e".to_string(),
                message: "model unavailable".to_string(),
                retryable: false,
            },
        )])))
    }

    async fn cancel(&self, _session_id: &rua_core::id::SessionId) -> rua_core::Result<()> {
        Ok(())
    }

    async fn warmup(&self, _context: RuntimeWarmupContext) -> rua_core::Result<()> {
        Ok(())
    }

    async fn shutdown(&self) -> rua_core::Result<()> {
        Ok(())
    }
}

struct ApprovalPendingRuntime;

#[async_trait::async_trait]
impl AgentRuntime for ApprovalPendingRuntime {
    async fn stream(
        &self,
        _req: AgentRequest,
    ) -> rua_core::Result<
        Pin<Box<dyn futures::Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>,
    > {
        Ok(Box::pin(futures::stream::iter(vec![
            Ok(RuntimeEvent::Start {
                session_id: "chat-approval-pending".into(),
                agent_id: Some("default".into()),
            }),
            Ok(RuntimeEvent::ToolApprovalPending {
                call_id: "call-approval-1".to_string(),
                tool_name: "bash".to_string(),
                request_id: Some("approval-1".to_string()),
                prompt: Some("Bash requires confirmation".to_string()),
            }),
            Ok(RuntimeEvent::End {
                finish_reason: Some("approval_wait_test".to_string()),
            }),
        ])))
    }

    async fn cancel(&self, _session_id: &rua_core::id::SessionId) -> rua_core::Result<()> {
        Ok(())
    }

    async fn warmup(&self, _context: RuntimeWarmupContext) -> rua_core::Result<()> {
        Ok(())
    }

    async fn shutdown(&self) -> rua_core::Result<()> {
        Ok(())
    }
}

struct AcpToolDeltaRuntime;

#[async_trait::async_trait]
impl AgentRuntime for AcpToolDeltaRuntime {
    async fn stream(
        &self,
        req: AgentRequest,
    ) -> rua_core::Result<
        Pin<Box<dyn futures::Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>,
    > {
        let stream = Box::pin(futures::stream::iter(vec![
            Ok(RuntimeEvent::Start {
                session_id: "chat-acp-delta".into(),
                agent_id: Some("default".into()),
            }),
            Ok(RuntimeEvent::TextDelta {
                delta: "Before tool. ".to_string(),
                message_id: Some(rua_core::MessageId("assistant-before-tool".to_string())),
            }),
            Ok(RuntimeEvent::ToolCallDelta {
                id: "call-read".to_string(),
                index: 0,
                name: Some("Read File".to_string()),
                arguments_delta: r#"{"path":"README.md"}"#.to_string(),
            }),
            Ok(RuntimeEvent::ToolResult {
                result: ToolResult::text("call-read", "file contents"),
            }),
            Ok(RuntimeEvent::TextDelta {
                delta: "After tool.".to_string(),
                message_id: Some(rua_core::MessageId("assistant-after-tool".to_string())),
            }),
            Ok(RuntimeEvent::End {
                finish_reason: Some("stop".to_string()),
            }),
        ]));
        Ok(rua_agent::turn_persistence::persist_runtime_stream(
            &req, stream,
        ))
    }

    async fn cancel(&self, _session_id: &rua_core::id::SessionId) -> rua_core::Result<()> {
        Ok(())
    }

    async fn warmup(&self, _context: RuntimeWarmupContext) -> rua_core::Result<()> {
        Ok(())
    }

    async fn shutdown(&self) -> rua_core::Result<()> {
        Ok(())
    }
}

fn conversation_message(
    message_id: &str,
    role: &str,
    body: &str,
) -> rua_session::NewConversationMessage {
    rua_session::NewConversationMessage {
        message_id: message_id.to_string(),
        role: role.to_string(),
        created_at: "2026-06-19T00:00:00Z".to_string(),
        status: "completed".to_string(),
        content: json!([{"type": "text", "text": body}]),
        provider: None,
        tool: None,
        metadata: serde_json::Map::new(),
    }
}

fn write_source_pending_remote_marker(
    data_dir: &std::path::Path,
    chat_id: &str,
    device_delivery_id: &str,
    device_delivery_sequence: u64,
    target_device_id: &str,
) {
    let path = source_pending_remote_marker_path(data_dir, chat_id);
    std::fs::create_dir_all(path.parent().expect("marker parent")).expect("marker dir");
    rua_config::atomic_write_json(
        &path,
        &json!({
            "address": rua_core::ChatAddress::agent("default", chat_id),
            "submission_id": device_delivery_id,
            "input_id": format!("input-{device_delivery_id}"),
            "mode": "started",
            "device_delivery_id": device_delivery_id,
            "device_delivery_sequence": device_delivery_sequence,
            "target_device_id": target_device_id,
            "accepted_at": "2026-06-27T00:00:03Z",
        }),
    )
    .expect("write pending remote marker");
}

fn write_source_pending_terminal_remote_marker(
    data_dir: &std::path::Path,
    chat_id: &str,
    device_delivery_id: &str,
    device_delivery_sequence: u64,
    target_device_id: &str,
    terminal_status: &str,
) {
    let path = source_pending_remote_marker_path(data_dir, chat_id);
    std::fs::create_dir_all(path.parent().expect("marker parent")).expect("marker dir");
    rua_config::atomic_write_json(
        &path,
        &json!({
            "address": rua_core::ChatAddress::agent("default", chat_id),
            "submission_id": device_delivery_id,
            "input_id": format!("input-{device_delivery_id}"),
            "mode": "started",
            "device_delivery_id": device_delivery_id,
            "device_delivery_sequence": device_delivery_sequence,
            "target_device_id": target_device_id,
            "accepted_at": "2026-06-27T00:00:03Z",
            "terminal_status": terminal_status,
            "terminal_seen_at": "2026-06-27T00:00:09Z",
        }),
    )
    .expect("write pending terminal remote marker");
}

fn source_pending_remote_marker_path(
    data_dir: &std::path::Path,
    chat_id: &str,
) -> std::path::PathBuf {
    rua_core::ChatScopeStore::resolve(data_dir, rua_core::ChatAddress::agent("default", chat_id))
        .root()
        .join("device_commands")
        .join("source_pending_conversation_inputs")
        .join(format!("{chat_id}.json"))
}

async fn append_test_turn(store: &rua_session::WorkspaceSessionStore, chat_id: &str, ordinal: u64) {
    append_test_turn_with_text(
        store,
        chat_id,
        ordinal,
        &format!("raw old user {ordinal}"),
        &format!("raw old assistant {ordinal}"),
    )
    .await;
}

async fn append_test_turn_with_text(
    store: &rua_session::WorkspaceSessionStore,
    chat_id: &str,
    ordinal: u64,
    user_text: &str,
    assistant_text: &str,
) {
    store
        .append_conversation_turn(
            chat_id,
            rua_session::NewConversationTurn {
                turn_id: format!("turn-{ordinal}"),
                lifecycle: rua_session::TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-06-19T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: None,
                messages: vec![
                    conversation_message(&format!("u-{ordinal}"), "user", user_text),
                    conversation_message(&format!("a-{ordinal}"), "assistant", assistant_text),
                ],
            },
        )
        .await
        .unwrap();
}

fn message_texts(messages: &[AgentInputMessage]) -> Vec<String> {
    messages
        .iter()
        .flat_map(|message| {
            message.content.iter().filter_map(|part| match part {
                ContentPart::Text { text } => Some(text.clone()),
                _ => None,
            })
        })
        .collect()
}

fn role_texts(messages: &[AgentInputMessage]) -> Vec<(String, String)> {
    messages
        .iter()
        .flat_map(|message| {
            message.content.iter().filter_map(|part| match part {
                ContentPart::Text { text } => Some((message.role.clone(), text.clone())),
                _ => None,
            })
        })
        .collect()
}

#[tokio::test]
async fn console_process_persists_tool_call_deltas_in_stream_order() {
    let dir = tempfile::tempdir().unwrap();
    let chat_id = "chat-acp-delta";
    let turn_id = "turn-acp-delta";
    let service = ConsoleChannelService::new(Arc::new(AcpToolDeltaRuntime));
    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(RunControl::Running);
    let persistence_impl = WorkspaceSessionPersistence::new(dir.path().to_path_buf())
        .with_active_turn_id(turn_id.to_string());
    seed_persistence_chat(dir.path(), chat_id).await;
    persistence_impl
        .save_chat_entry("agent-1", chat_id, "Read README")
        .await
        .unwrap();
    let store = rua_session::WorkspaceSessionStore::new(dir.path());
    store
        .append_conversation_turn(
            chat_id,
            rua_session::NewConversationTurn {
                turn_id: turn_id.to_string(),
                lifecycle: rua_session::TurnLifecycle {
                    user_submitted_at: "2026-08-04T00:00:00Z".to_string(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![conversation_message(
                    "user-acp-delta",
                    "user",
                    "Read README",
                )],
            },
        )
        .await
        .unwrap();
    let persistence: Arc<dyn SessionPersistence> = Arc::new(persistence_impl);

    let mut stream = service.process(ProcessRequest {
        agent_id: "agent-1".to_string(),
        chat_id: chat_id.to_string(),
        messages: vec![text("user", "Read README")],
        model_override: None,
        workspace_dir: Some(dir.path().to_path_buf()),
        conversation_store_dir: Some(dir.path().to_path_buf()),
        working_dir: None,
        cancel_rx,
        user_message: "Read README".to_string(),
        persistence,
        disabled_tools: Vec::new(),
        metadata: std::collections::HashMap::from([
            ("turn_id".to_string(), json!(turn_id)),
            (
                rua_agent::turn_persistence::LOADED_MESSAGE_COUNT_METADATA_KEY.to_string(),
                json!(1),
            ),
        ]),
    });

    while let Some(item) = stream.next().await {
        item.expect("runtime event should map cleanly");
    }

    let messages = store.read_message_range(chat_id, 1, 10).await.unwrap();
    assert_eq!(
        messages
            .iter()
            .map(|message| message.role.as_str())
            .collect::<Vec<_>>(),
        vec!["user", "assistant", "assistant", "tool", "assistant"]
    );
    assert_eq!(messages[1].content[0]["text"], "Before tool. ");
    let parts = messages[2]
        .content
        .as_array()
        .expect("assistant content array");
    let part_types = parts
        .iter()
        .map(|part| part["type"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(part_types, vec!["tool_use"]);
    assert_eq!(parts[0]["id"], "call-read");
    assert_eq!(parts[0]["name"], "Read File");
    assert_eq!(parts[0]["input"], json!({"path": "README.md"}));
    assert_eq!(messages[3].content[0]["type"], "tool_result");
    assert_eq!(messages[3].content[0]["id"], "call-read");
    assert_eq!(messages[4].content[0]["text"], "After tool.");
}

fn runtime_note() -> rua_session::RuntimeNote {
    rua_session::RuntimeNote {
        runtime_note_id: "runtime-note-evergreen-input-1".to_string(),
        created_at: "2026-06-19T00:00:03Z".to_string(),
        payload: rua_session::RuntimeNotePayload::TaskAttemptTerminal(
            rua_session::TaskAttemptTerminalRuntimeNote {
                task_id: "task-evergreen-input-1".to_string(),
                attempt_no: 1,
                terminal_status: "completed".to_string(),
                output: "evergreen runtime note should reach the model".to_string(),
                runner_name: "bash".to_string(),
                full_result_ref: None,
                artifacts: json!([]),
                device: json!({}),
                tool_call_id: Some("tool-call-evergreen-input-1".to_string()),
            },
        ),
    }
}

struct PartialThenFailingRuntime;

#[async_trait::async_trait]
impl AgentRuntime for PartialThenFailingRuntime {
    async fn stream(
        &self,
        req: AgentRequest,
    ) -> rua_core::Result<
        Pin<Box<dyn futures::Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>,
    > {
        let stream = Box::pin(futures::stream::iter(vec![
            Ok(RuntimeEvent::Start {
                session_id: "chat-1".into(),
                agent_id: Some("default".into()),
            }),
            Ok(RuntimeEvent::TextDelta {
                delta: "partial provider output".to_string(),
                message_id: Some(rua_core::MessageId(
                    "assistant-partial-provider-output".to_string(),
                )),
            }),
            Err(RuaError::Provider {
                provider: "sync-e2e".to_string(),
                message: "provider disconnected".to_string(),
                retryable: false,
            }),
        ]));
        Ok(rua_agent::turn_persistence::persist_runtime_stream(
            &req, stream,
        ))
    }

    async fn cancel(&self, _session_id: &rua_core::id::SessionId) -> rua_core::Result<()> {
        Ok(())
    }

    async fn warmup(&self, _context: RuntimeWarmupContext) -> rua_core::Result<()> {
        Ok(())
    }

    async fn shutdown(&self) -> rua_core::Result<()> {
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tool_approval_pending_renders_but_does_not_persist_or_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::from_tempdir(dir.path().to_path_buf(), ServerProfile::Test);
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime");
    runtime
        .set_react_runtime(Arc::new(ApprovalPendingRuntime))
        .await;

    let chat_id = "00000000-0000-0000-0000-000000000777";
    materialize_local_chat(
        &state,
        &runtime.workspace_dir(),
        "default",
        chat_id,
        "Tool approval pending",
    )
    .await;
    let events: Vec<_> = state
        .chat_service
        .chat_stream(
            "default",
            runtime.clone(),
            dto::ChatRequest {
                message: "try a gated tool".to_string(),
                chat_id: Some(chat_id.to_string()),
                input: None,
                messages: None,
                user_id: None,
                channel: None,
                stream: Some(true),
                model: None,
                attachments: Vec::new(),
                extra: serde_json::Map::new(),
            },
            running_cancel_rx(),
        )
        .collect()
        .await;

    assert!(
        events.iter().any(|event| {
            matches!(
                event,
                Ok(dto::ChatEvent::ToolApprovalRequest { id, .. }) if id == "approval-1"
            )
        }),
        "approval pending must still render as a live UI event: {events:?}"
    );

    let store = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir());
    let header = store
        .load_chat_header(chat_id)
        .await
        .unwrap()
        .expect("chat header");
    assert!(
        header.context_snapshot.pending_action.is_none(),
        "tool approval pending is runtime-only and must not be persisted in the Chat header"
    );

    let messages = store.read_message_range(chat_id, 1, 10).await.unwrap();
    assert!(
        messages.iter().all(|message| {
            !message
                .content
                .to_string()
                .contains("tool_approval_request")
        }),
        "tool approval pending must not become a Message Log record: {messages:?}"
    );

    let snapshots =
        collect_evergreen_chat_file_snapshots(&runtime.workspace_dir(), chat_id, "device-a");
    let serialized = serde_json::to_string(
        &snapshots
            .iter()
            .map(|snapshot| {
                json!({
                    "relative": snapshot.relative,
                    "payload": snapshot.payload,
                })
            })
            .collect::<Vec<_>>(),
    )
    .unwrap();
    assert!(
        !serialized.contains("tool_approval")
            && !serialized.contains("tool_approval_request")
            && !serialized.contains("approval-1"),
        "Evergreen Chat snapshots must not contain live tool approval pending state: {serialized}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn device_delivery_chat_stream_materializes_targeted_chat_and_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::from_tempdir(dir.path().to_path_buf(), ServerProfile::Test);
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime");
    runtime
        .set_react_runtime(Arc::new(CapturingRuntime {
            captured: Arc::new(Mutex::new(None)),
        }))
        .await;
    let chat_id = "00000000-0000-0000-0000-000000009001";
    let target_device_id = "target-device-1";
    let source_device_id = "source-device-1";
    let device_delivery_id = "device-delivery-1";
    materialize_targeted_chat(
        &runtime.workspace_dir(),
        "default",
        chat_id,
        "Device delivery",
        &authority_placement(target_device_id),
    )
    .await
    .expect("materialize targeted Chat Index placement");

    let mut extra = serde_json::Map::new();
    let store = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir());
    let mut header = store.load_chat_header(chat_id).await.unwrap().unwrap();
    seed_runtime_configuration(&state, "default", &mut header).await;
    store.save_chat_header(&header).await.unwrap();
    extra.insert(
        "device_delivery".to_string(),
        json!({
            "device_delivery_id": device_delivery_id,
            "device_delivery_sequence": 1,
            "source_device_id": source_device_id,
            "target_device_id": target_device_id
        }),
    );
    extra.insert("authority_device_id".to_string(), json!(target_device_id));
    extra.insert("authority_epoch".to_string(), json!(epoch(1)));

    let events: Vec<_> = state
        .chat_service
        .chat_stream(
            "default",
            runtime.clone(),
            dto::ChatRequest {
                message: "handle this on the target".to_string(),
                chat_id: Some(chat_id.to_string()),
                input: None,
                messages: None,
                user_id: None,
                channel: None,
                stream: Some(true),
                model: None,
                attachments: Vec::new(),
                extra,
            },
            running_cancel_rx(),
        )
        .collect()
        .await;
    assert!(
        events.iter().all(|event| event.is_ok()),
        "remote targeted chat stream should complete without errors: {events:?}"
    );

    let workspace = runtime.workspace_dir();
    let indexed = chat_index::get(&workspace, chat_id)
        .await
        .expect("read chat index")
        .expect("indexed chat");
    assert_eq!(
        indexed.meta[CHAT_AUTHORITY_PLACEMENT_META_KEY]["device_id"],
        target_device_id
    );
    assert_eq!(
        indexed.meta[CHAT_AUTHORITY_PLACEMENT_META_KEY]["authority_epoch"],
        "0000000000000001000000000000000000000000"
    );

    let store = rua_session::WorkspaceSessionStore::new(&workspace);
    let turns = store.read_turn_range(chat_id, 1, usize::MAX).await.unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].lifecycle.status, "completed");
    let messages = store
        .read_turn_messages(chat_id, &turns[0].turn_id)
        .await
        .unwrap();
    assert_eq!(messages[0].role, "user");
    assert_eq!(
        messages[0].metadata["device_delivery_id"],
        device_delivery_id
    );
    assert_eq!(messages[0].metadata["source_device_id"], source_device_id);
    assert_eq!(
        messages[0].metadata["executed_on_device_id"],
        target_device_id
    );
    assert_eq!(messages[1].role, "assistant");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_targeted_chat_submit_does_not_call_local_agent_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::from_tempdir(dir.path().to_path_buf(), ServerProfile::Test);
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime");
    let captured = Arc::new(Mutex::new(None));
    runtime
        .set_react_runtime(Arc::new(CapturingRuntime {
            captured: Arc::clone(&captured),
        }))
        .await;
    let chat_id = "00000000-0000-0000-0000-000000009002";
    let target_device_id = "target-device-1";
    let workspace = runtime.workspace_dir();
    materialize_targeted_chat(
        &workspace,
        "default",
        chat_id,
        "Remote target",
        &authority_placement(target_device_id),
    )
    .await
    .expect("seed targeted chat");

    let outcome = state
        .console_facade
        .submit_chat(
            "default",
            ConsoleChatRequest {
                message: "must not run locally".to_string(),
                chat_id: Some(chat_id.to_string()),
                model: None,
                is_reconnect: false,
                attachments: Vec::<AttachmentInfo>::new(),
                input: None,
                messages: None,
                extra: serde_json::Map::new(),
                channel_kind: None,
            },
        )
        .await;

    assert!(
        captured.lock().await.is_none(),
        "source-side remote targeted submit must not call local AgentRuntime"
    );
    let header = rua_session::WorkspaceSessionStore::new(&workspace)
        .load_chat_header(chat_id)
        .await
        .unwrap()
        .expect("source-side remote target should materialize chat header");
    assert_eq!(
        header.message_log.latest_sequence, 0,
        "source-side remote targeted submit must not write an official local turn"
    );
    assert!(header
        .extra
        .get(CHAT_AUTHORITY_PLACEMENT_META_KEY)
        .is_some());
    assert!(!outcome.accepted);
    assert_eq!(outcome.kind, ConsoleSubmitKind::Rejected);
    assert_eq!(outcome.target_device_id.as_deref(), Some(target_device_id));
    assert!(
        outcome
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("Device Network Runtime is suspended"),
        "error should explain remote delivery is unavailable: {outcome:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_targeted_chat_does_not_block_on_previous_source_pending_marker() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::from_tempdir(dir.path().to_path_buf(), ServerProfile::Test);
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime");
    let captured = Arc::new(Mutex::new(None));
    runtime
        .set_react_runtime(Arc::new(CapturingRuntime {
            captured: Arc::clone(&captured),
        }))
        .await;

    let chat_id = "00000000-0000-0000-0000-000000009012";
    let target_device_id = "target-device-1";
    let workspace = runtime.workspace_dir();
    materialize_targeted_chat(
        &workspace,
        "default",
        chat_id,
        "Remote target",
        &authority_placement(target_device_id),
    )
    .await
    .expect("materialize targeted chat");
    write_source_pending_remote_marker(
        dir.path(),
        chat_id,
        "device-delivery-prev",
        1,
        target_device_id,
    );

    let outcome = state
        .console_facade
        .submit_chat(
            "default",
            ConsoleChatRequest {
                message: "must wait for canonical sync".to_string(),
                chat_id: Some(chat_id.to_string()),
                model: None,
                is_reconnect: false,
                attachments: Vec::<AttachmentInfo>::new(),
                input: None,
                messages: None,
                extra: serde_json::Map::new(),
                channel_kind: None,
            },
        )
        .await;

    assert!(
        captured.lock().await.is_none(),
        "source-side pending remote finalization must not fall back to local AgentRuntime"
    );
    assert!(!outcome.accepted);
    assert_eq!(outcome.kind, ConsoleSubmitKind::Rejected);
    assert_eq!(outcome.target_device_id.as_deref(), Some(target_device_id));
    let detail = outcome.detail.as_deref().unwrap_or_default();
    assert!(
        detail.contains("Device Network Runtime is suspended")
            && !detail.contains("previous device delivery"),
        "source pending marker must not block ordinary remote delivery path: {detail}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_targeted_chat_clears_pending_marker_after_completed_turn_syncs_locally() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::from_tempdir(dir.path().to_path_buf(), ServerProfile::Test);
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime");
    let captured = Arc::new(Mutex::new(None));
    runtime
        .set_react_runtime(Arc::new(CapturingRuntime {
            captured: Arc::clone(&captured),
        }))
        .await;

    let chat_id = "00000000-0000-0000-0000-000000009013";
    let target_device_id = "target-device-1";
    let workspace = runtime.workspace_dir();
    materialize_targeted_chat(
        &workspace,
        "default",
        chat_id,
        "Remote target",
        &authority_placement(target_device_id),
    )
    .await
    .expect("materialize targeted chat");
    write_source_pending_remote_marker(
        dir.path(),
        chat_id,
        "device-delivery-prev",
        1,
        target_device_id,
    );

    let store = rua_session::WorkspaceSessionStore::new(&workspace);
    let mut completed_message = conversation_message("remote-user", "user", "previous remote");
    completed_message.metadata.insert(
        "device_delivery_id".to_string(),
        json!("device-delivery-prev"),
    );
    completed_message
        .metadata
        .insert("device_delivery_sequence".to_string(), json!(1));
    store
        .append_conversation_turn(
            chat_id,
            rua_session::NewConversationTurn {
                turn_id: "turn-remote-prev".to_string(),
                lifecycle: rua_session::TurnLifecycle {
                    user_submitted_at: "2026-06-27T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-06-27T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-06-27T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: None,
                messages: vec![completed_message],
            },
        )
        .await
        .expect("seed completed remote turn");

    let outcome = state
        .console_facade
        .submit_chat(
            "default",
            ConsoleChatRequest {
                message: "now may route remote".to_string(),
                chat_id: Some(chat_id.to_string()),
                model: None,
                is_reconnect: false,
                attachments: Vec::<AttachmentInfo>::new(),
                input: None,
                messages: None,
                extra: serde_json::Map::new(),
                channel_kind: None,
            },
        )
        .await;

    assert!(
        captured.lock().await.is_none(),
        "targeted chat should still route remote, not local"
    );
    assert!(
        !source_pending_remote_marker_path(dir.path(), chat_id).exists(),
        "completed canonical turn should clear source-side pending marker"
    );
    assert!(!outcome.accepted);
    assert_eq!(outcome.kind, ConsoleSubmitKind::Rejected);
    assert_eq!(outcome.target_device_id.as_deref(), Some(target_device_id));
    let detail = outcome.detail.as_deref().unwrap_or_default();
    assert!(
        detail.contains("Device Network Runtime is suspended")
            && !detail.contains("previous device delivery"),
        "after finalization, failure should come from ordinary remote delivery path: {detail}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_targeted_chat_unblocks_after_terminal_seen_before_turn_syncs_locally() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::from_tempdir(dir.path().to_path_buf(), ServerProfile::Test);
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime");
    let captured = Arc::new(Mutex::new(None));
    runtime
        .set_react_runtime(Arc::new(CapturingRuntime {
            captured: Arc::clone(&captured),
        }))
        .await;

    let chat_id = "00000000-0000-0000-0000-000000009014";
    let target_device_id = "target-device-1";
    let workspace = runtime.workspace_dir();
    materialize_targeted_chat(
        &workspace,
        "default",
        chat_id,
        "Remote target",
        &authority_placement(target_device_id),
    )
    .await
    .expect("materialize targeted chat");
    write_source_pending_terminal_remote_marker(
        dir.path(),
        chat_id,
        "device-delivery-prev",
        1,
        target_device_id,
        "completed",
    );

    let outcome = state
        .console_facade
        .submit_chat(
            "default",
            ConsoleChatRequest {
                message: "terminal seen should allow next remote delivery".to_string(),
                chat_id: Some(chat_id.to_string()),
                model: None,
                is_reconnect: false,
                attachments: Vec::<AttachmentInfo>::new(),
                input: None,
                messages: None,
                extra: serde_json::Map::new(),
                channel_kind: None,
            },
        )
        .await;

    assert!(
        captured.lock().await.is_none(),
        "targeted chat should still route remote, not local"
    );
    assert!(
        source_pending_remote_marker_path(dir.path(), chat_id).exists(),
        "live terminal fact should not clear the server marker before stable history can preserve remote sequence"
    );
    assert!(!outcome.accepted);
    assert_eq!(outcome.kind, ConsoleSubmitKind::Rejected);
    assert_eq!(outcome.target_device_id.as_deref(), Some(target_device_id));
    let detail = outcome.detail.as_deref().unwrap_or_default();
    assert!(
        detail.contains("Device Network Runtime is suspended")
            && !detail.contains("previous device delivery"),
        "after terminal_seen, failure should come from ordinary remote delivery path: {detail}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_targeted_chat_with_attachments_requires_source_device_without_local_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::from_tempdir(dir.path().to_path_buf(), ServerProfile::Test);
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime");
    let captured = Arc::new(Mutex::new(None));
    runtime
        .set_react_runtime(Arc::new(CapturingRuntime {
            captured: Arc::clone(&captured),
        }))
        .await;
    let chat_id = "00000000-0000-0000-0000-000000009005";
    let target_device_id = "target-device-1";
    let workspace = runtime.workspace_dir();
    materialize_targeted_chat(
        &workspace,
        "default",
        chat_id,
        "Remote target",
        &authority_placement(target_device_id),
    )
    .await
    .expect("seed targeted chat");

    let outcome = state
        .console_facade
        .submit_chat(
            "default",
            ConsoleChatRequest {
                message: "remote attachment".to_string(),
                chat_id: Some(chat_id.to_string()),
                model: None,
                is_reconnect: false,
                attachments: vec![AttachmentInfo {
                    url: "/uploads/file.txt".to_string(),
                    name: "file.txt".to_string(),
                    mime_type: Some("text/plain".to_string()),
                    stored_name: Some("file.txt".to_string()),
                    size: Some(4),
                }],
                input: None,
                messages: None,
                extra: serde_json::Map::new(),
                channel_kind: None,
            },
        )
        .await;

    assert!(
        captured.lock().await.is_none(),
        "remote attachment short-circuit must not call local AgentRuntime"
    );
    assert!(!outcome.accepted);
    assert_eq!(outcome.kind, ConsoleSubmitKind::Rejected);
    assert_eq!(outcome.target_device_id.as_deref(), Some(target_device_id));
    assert!(
        outcome
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("Device Network Runtime is suspended"),
        "remote attachment short-circuit should be explicit: {outcome:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn request_authority_metadata_cannot_override_new_chat_creation() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::from_tempdir(dir.path().to_path_buf(), ServerProfile::Test);
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime");
    let captured = Arc::new(Mutex::new(None));
    runtime
        .set_react_runtime(Arc::new(CapturingRuntime {
            captured: Arc::clone(&captured),
        }))
        .await;

    let chat_id = "00000000-0000-0000-0000-000000009003";
    let mut extra = serde_json::Map::new();
    extra.insert(
        CHAT_AUTHORITY_PLACEMENT_META_KEY.to_string(),
        json!({"mode": "fixed"}),
    );

    let outcome = state
        .console_facade
        .submit_chat(
            "default",
            ConsoleChatRequest {
                message: "must not run locally".to_string(),
                chat_id: Some(chat_id.to_string()),
                model: None,
                is_reconnect: false,
                attachments: Vec::<AttachmentInfo>::new(),
                input: None,
                messages: None,
                extra,
                channel_kind: None,
            },
        )
        .await;

    assert!(outcome.accepted);
    assert_eq!(outcome.kind, ConsoleSubmitKind::Started);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authority_placement_request_cannot_override_existing_history_authority() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::from_tempdir(dir.path().to_path_buf(), ServerProfile::Test);
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime");
    let captured = Arc::new(Mutex::new(None));
    runtime
        .set_react_runtime(Arc::new(CapturingRuntime {
            captured: Arc::clone(&captured),
        }))
        .await;

    let chat_id = "00000000-0000-0000-0000-000000009004";
    let store = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir());
    let header = targeted_chat_header(
        &runtime.workspace_dir(),
        "test-authority-device",
        rua_session::ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "default".to_string(),
            title: "Existing history".to_string(),
            created_at: "2026-06-27T00:00:00Z".to_string(),
            last_activity_at: "2026-06-27T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        },
    )
    .await;
    store
        .save_chat_header(&header)
        .await
        .expect("seed chat header");
    store
        .append_conversation_turn(
            chat_id,
            rua_session::NewConversationTurn {
                turn_id: "turn-existing".to_string(),
                lifecycle: rua_session::TurnLifecycle {
                    user_submitted_at: "2026-06-27T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-06-27T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-06-27T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: None,
                messages: vec![conversation_message(
                    "existing-user",
                    "user",
                    "already local",
                )],
            },
        )
        .await
        .expect("seed official history");

    let mut extra = serde_json::Map::new();
    extra.insert(
        CHAT_AUTHORITY_PLACEMENT_META_KEY.to_string(),
        json!({"mode": "fixed", "device_id": "target-device-1"}),
    );

    let outcome = state
        .console_facade
        .submit_chat(
            "default",
            ConsoleChatRequest {
                message: "must be remote".to_string(),
                chat_id: Some(chat_id.to_string()),
                model: None,
                is_reconnect: false,
                attachments: Vec::<AttachmentInfo>::new(),
                input: None,
                messages: None,
                extra,
                channel_kind: None,
            },
        )
        .await;

    assert!(
        captured.lock().await.is_none(),
        "request metadata must not override the stored remote authority"
    );
    assert!(!outcome.accepted);
    assert_eq!(outcome.kind, ConsoleSubmitKind::Rejected);
    assert_eq!(
        outcome.target_device_id.as_deref(),
        Some("test-authority-device")
    );
    assert!(
        outcome
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("Device Network Runtime is suspended"),
        "stored authority should route to remote delivery and report delivery failure: {outcome:?}"
    );
    let header = store
        .load_chat_header(chat_id)
        .await
        .expect("load header")
        .expect("header");
    assert_eq!(
        header.extra[CHAT_AUTHORITY_PLACEMENT_META_KEY]["device_id"],
        "test-authority-device"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_chat_persists_failed_runtime_as_closed_evergreen_turn() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::from_tempdir(dir.path().to_path_buf(), ServerProfile::Test);
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime");
    runtime.set_react_runtime(Arc::new(FailingRuntime)).await;

    let chat_id = "00000000-0000-0000-0000-000000000321";
    materialize_local_chat(
        &state,
        &runtime.workspace_dir(),
        "default",
        chat_id,
        "Failed runtime",
    )
    .await;
    let events: Vec<_> = state
        .chat_service
        .chat_stream(
            "default",
            runtime.clone(),
            dto::ChatRequest {
                message: "trigger runtime failure".to_string(),
                chat_id: Some(chat_id.to_string()),
                input: None,
                messages: None,
                user_id: None,
                channel: None,
                stream: Some(true),
                model: None,
                attachments: Vec::new(),
                extra: serde_json::Map::new(),
            },
            running_cancel_rx(),
        )
        .collect()
        .await;

    assert!(
        events.iter().any(|event| {
            matches!(
                event,
                Ok(dto::ChatEvent::Error { detail }) if detail.contains("model unavailable")
            )
        }),
        "events: {events:?}"
    );

    let store = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir());
    let turns = store.read_turn_range(chat_id, 1, 10).await.unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(
        turns[0].lifecycle.status, "failed",
        "runtime failures must close the turn as a failed runtime fact"
    );
    assert!(turns[0].lifecycle.assistant_completed_at.is_some());

    let messages = store.read_message_range(chat_id, 1, 10).await.unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, "user");
    let note = rua_session::RuntimeNote::from_message_log_entry(&messages[1])
        .expect("conversation runtime terminal note");
    let rua_session::RuntimeNotePayload::ConversationRuntimeTerminal(note) = note.payload else {
        panic!("expected Conversation Runtime Terminal");
    };
    assert_eq!(note.terminal_status, "failed");
    assert!(note.summary.contains("model unavailable"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_chat_keeps_partial_assistant_output_when_runtime_fails() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::from_tempdir(dir.path().to_path_buf(), ServerProfile::Test);
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime");
    runtime
        .set_react_runtime(Arc::new(PartialThenFailingRuntime))
        .await;

    let chat_id = "00000000-0000-0000-0000-000000000654";
    materialize_local_chat(
        &state,
        &runtime.workspace_dir(),
        "default",
        chat_id,
        "Partial runtime failure",
    )
    .await;
    let events: Vec<_> = state
        .chat_service
        .chat_stream(
            "default",
            runtime.clone(),
            dto::ChatRequest {
                message: "trigger partial runtime failure".to_string(),
                chat_id: Some(chat_id.to_string()),
                input: None,
                messages: None,
                user_id: None,
                channel: None,
                stream: Some(true),
                model: None,
                attachments: Vec::new(),
                extra: serde_json::Map::new(),
            },
            running_cancel_rx(),
        )
        .collect()
        .await;

    assert!(
        events.iter().any(|event| {
            matches!(
                event,
                Ok(dto::ChatEvent::Error { detail }) if detail.contains("provider disconnected")
            )
        }),
        "events: {events:?}"
    );

    let store = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir());
    let turns = store.read_turn_range(chat_id, 1, 10).await.unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].lifecycle.status, "failed");

    let messages = store.read_message_range(chat_id, 1, 10).await.unwrap();
    let role_texts = messages
        .iter()
        .take(2)
        .map(|message| {
            (
                message.role.as_str(),
                message.content[0]["text"].as_str().unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        role_texts,
        vec![
            ("user", "trigger partial runtime failure"),
            ("assistant", "partial provider output"),
        ],
        "provider-visible partial output must remain durable without duplicate users"
    );
    let note = rua_session::RuntimeNote::from_message_log_entry(&messages[2])
        .expect("provider failure Runtime Note");
    let rua_session::RuntimeNotePayload::ConversationRuntimeTerminal(terminal) = note.payload
    else {
        panic!("expected Conversation Runtime Terminal note");
    };
    assert_eq!(terminal.terminal_status, "failed");
    assert_eq!(terminal.reason, "runtime_failure");
    assert!(terminal.summary.contains("provider disconnected"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_runtime_fact_is_injected_as_system_context_on_next_turn() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::from_tempdir(dir.path().to_path_buf(), ServerProfile::Test);
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime");

    let chat_id = "00000000-0000-0000-0000-000000000322";
    materialize_local_chat(
        &state,
        &runtime.workspace_dir(),
        "default",
        chat_id,
        "Runtime failure continuation",
    )
    .await;
    runtime.set_react_runtime(Arc::new(FailingRuntime)).await;
    let failed_events: Vec<_> = state
        .chat_service
        .chat_stream(
            "default",
            runtime.clone(),
            dto::ChatRequest {
                message: "trigger runtime failure".to_string(),
                chat_id: Some(chat_id.to_string()),
                input: None,
                messages: None,
                user_id: None,
                channel: None,
                stream: Some(true),
                model: None,
                attachments: Vec::new(),
                extra: serde_json::Map::new(),
            },
            running_cancel_rx(),
        )
        .collect()
        .await;
    assert!(
        failed_events.iter().any(|event| {
            matches!(
                event,
                Ok(dto::ChatEvent::Error { detail }) if detail.contains("model unavailable")
            )
        }),
        "events: {failed_events:?}"
    );

    let captured = Arc::new(Mutex::new(None));
    runtime
        .set_react_runtime(Arc::new(CapturingRuntime {
            captured: Arc::clone(&captured),
        }))
        .await;
    let continued_events: Vec<_> = state
        .chat_service
        .chat_stream(
            "default",
            runtime.clone(),
            dto::ChatRequest {
                message: "continue after failure".to_string(),
                chat_id: Some(chat_id.to_string()),
                input: None,
                messages: None,
                user_id: None,
                channel: None,
                stream: Some(true),
                model: None,
                attachments: Vec::new(),
                extra: serde_json::Map::new(),
            },
            running_cancel_rx(),
        )
        .collect()
        .await;
    assert!(
        continued_events
            .iter()
            .any(|event| matches!(event, Ok(dto::ChatEvent::Done { .. }))),
        "continued events: {continued_events:?}"
    );

    let req = captured
        .lock()
        .await
        .clone()
        .expect("runtime request captured");
    let role_texts = role_texts(&req.messages);
    let failed_user_idx = role_texts
        .iter()
        .position(|(role, text)| role == "user" && text == "trigger runtime failure")
        .expect("failed turn user input is retained as history");
    let note_idx = role_texts
        .iter()
        .position(|(role, text)| {
            role == "system"
                && text.contains("<conversation_runtime_terminal>")
                && text.contains("terminal_status: failed")
                && text.contains("model unavailable")
        })
        .expect("failure runtime note is injected as system context");
    let current_user_idx = role_texts
        .iter()
        .position(|(role, text)| role == "user" && text == "continue after failure")
        .expect("current user input is present");
    assert!(
        failed_user_idx < note_idx && note_idx < current_user_idx,
        "runtime note should be appended after stable history and before current user input: {role_texts:?}"
    );
    assert!(
        !role_texts
            .iter()
            .any(|(role, text)| role == "assistant" && text.contains("model unavailable")),
        "runtime failure must not become fake assistant speech: {role_texts:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_chat_foreground_input_uses_evergreen_checkpoint_projection() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::from_tempdir(dir.path().to_path_buf(), ServerProfile::Test);
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime");
    let captured = Arc::new(Mutex::new(None));
    runtime
        .set_react_runtime(Arc::new(CapturingRuntime {
            captured: Arc::clone(&captured),
        }))
        .await;

    let store = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir());
    let chat_id = "00000000-0000-0000-0000-000000000123";
    let header = materialized_chat_header(
        &state,
        &runtime.workspace_dir(),
        rua_session::ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "default".to_string(),
            title: "Evergreen live path".to_string(),
            created_at: "2026-06-19T00:00:00Z".to_string(),
            last_activity_at: "2026-06-19T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        },
    )
    .await;
    store.save_chat_header(&header).await.unwrap();
    append_test_turn(&store, chat_id, 1).await;
    append_test_turn(&store, chat_id, 2).await;
    append_test_turn(&store, chat_id, 3).await;
    let projection_state = store.checkpoint_projection_state(chat_id, 4).await.unwrap();
    store
        .publish_summary_checkpoint(
            chat_id,
            None,
            rua_session::SummaryCheckpointDraft {
                checkpoint_id: "checkpoint-1".to_string(),
                previous_checkpoint_id: None,
                covered_turn_range: rua_session::CheckpointTurnRange {
                    start_turn_ordinal: 1,
                    end_turn_ordinal: 2,
                },
                covered_sequence_range: rua_session::CheckpointSequenceRange {
                    start_sequence: 1,
                    end_sequence: 4,
                },
                source_sequence_range: rua_session::CheckpointSequenceRange {
                    start_sequence: 1,
                    end_sequence: 4,
                },
                frontier: projection_state.frontier,
                retained_sequence_ranges: projection_state.retained_sequence_ranges,
                projection_lineage: projection_state.projection_lineage,
                summary: "checkpoint summary for first two turns".to_string(),
                token_estimate: 42,
                model: rua_session::CheckpointModelIdentity {
                    provider: "openai".to_string(),
                    model: "gpt-5".to_string(),
                },
                created_at: "2026-06-19T00:01:00Z".to_string(),
            },
        )
        .await
        .unwrap();

    let events: Vec<_> = state
        .chat_service
        .chat_stream(
            "default",
            Arc::clone(&runtime),
            dto::ChatRequest {
                message: "live user question".to_string(),
                chat_id: Some(chat_id.to_string()),
                input: None,
                messages: None,
                user_id: None,
                channel: None,
                stream: Some(true),
                model: None,
                attachments: Vec::new(),
                extra: serde_json::Map::new(),
            },
            running_cancel_rx(),
        )
        .collect()
        .await;
    assert!(
        events.iter().all(|event| event.is_ok()),
        "events: {events:?}"
    );

    let captured = captured
        .lock()
        .await
        .clone()
        .expect("runtime request captured");
    assert!(
        captured
            .metadata
            .get("turn_id")
            .and_then(|value| value.as_str())
            .is_some_and(|turn_id| turn_id.starts_with("turn-")),
        "foreground runtime request must carry the durable run turn id"
    );
    let texts = message_texts(&captured.messages);
    let joined = texts.join("\n");
    assert!(joined.contains("Earlier conversation was automatically compacted"));
    assert!(joined.contains("use conversation_search"));
    assert!(joined.contains("checkpoint summary for first two turns"));
    assert!(joined.contains("raw old user 3"));
    assert!(joined.contains("raw old assistant 3"));
    assert!(joined.contains("live user question"));
    assert!(!joined.contains("raw old user 1"));
    assert!(!joined.contains("raw old assistant 2"));
    assert!(!joined.contains("legacy folded summary must not be used"));
    assert!(!joined.contains("legacy session raw message"));
    for tool in ["conversation_search", "conversation_read_span"] {
        assert!(
            !captured
                .disabled_tools
                .iter()
                .any(|disabled| disabled == tool),
            "{tool} should stay available after compacted projection: {:?}",
            captured.disabled_tools
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_chat_hides_conversation_recall_tools_before_compaction() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::from_tempdir(dir.path().to_path_buf(), ServerProfile::Test);
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime");
    let captured = Arc::new(Mutex::new(None));
    runtime
        .set_react_runtime(Arc::new(CapturingRuntime {
            captured: Arc::clone(&captured),
        }))
        .await;

    let store = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir());
    let chat_id = "00000000-0000-0000-0000-000000000125";
    let header = materialized_chat_header(
        &state,
        &runtime.workspace_dir(),
        rua_session::ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "default".to_string(),
            title: "No compact recall gating".to_string(),
            created_at: "2026-06-19T00:00:00Z".to_string(),
            last_activity_at: "2026-06-19T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        },
    )
    .await;
    store.save_chat_header(&header).await.unwrap();
    append_test_turn(&store, chat_id, 1).await;

    let events: Vec<_> = state
        .chat_service
        .chat_stream(
            "default",
            Arc::clone(&runtime),
            dto::ChatRequest {
                message: "what did we say earlier".to_string(),
                chat_id: Some(chat_id.to_string()),
                input: None,
                messages: None,
                user_id: None,
                channel: None,
                stream: Some(true),
                model: None,
                attachments: Vec::new(),
                extra: serde_json::Map::new(),
            },
            running_cancel_rx(),
        )
        .collect()
        .await;
    assert!(
        events.iter().all(|event| event.is_ok()),
        "events: {events:?}"
    );

    let captured = captured
        .lock()
        .await
        .clone()
        .expect("runtime request captured");
    for tool in ["conversation_search", "conversation_read_span"] {
        assert!(
            captured
                .disabled_tools
                .iter()
                .any(|disabled| disabled == tool),
            "{tool} should be hidden before compacted projection: {:?}",
            captured.disabled_tools
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_chat_applies_selected_variant_before_building_llm_input() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::from_tempdir(dir.path().to_path_buf(), ServerProfile::Test);
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime");
    let captured = Arc::new(Mutex::new(None));
    runtime
        .set_react_runtime(Arc::new(CapturingRuntime {
            captured: Arc::clone(&captured),
        }))
        .await;

    let store = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir());
    let chat_id = "00000000-0000-0000-0000-000000000124";
    let header = materialized_chat_header(
        &state,
        &runtime.workspace_dir(),
        rua_session::ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "default".to_string(),
            title: "Variant order".to_string(),
            created_at: "2026-06-19T00:00:00Z".to_string(),
            last_activity_at: "2026-06-19T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        },
    )
    .await;
    store.save_chat_header(&header).await.unwrap();
    store
        .append_conversation_turn(
            chat_id,
            rua_session::NewConversationTurn {
                turn_id: "turn-variant".to_string(),
                lifecycle: rua_session::TurnLifecycle {
                    user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-06-19T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-06-19T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("assistant-first".to_string()),
                messages: vec![
                    conversation_message("user-variant", "user", "original question"),
                    conversation_message("assistant-first", "assistant", "first answer"),
                ],
            },
        )
        .await
        .unwrap();
    store
        .append_messages_to_turn_as_new_variant(
            chat_id,
            "turn-variant",
            rua_session::TurnLifecycle {
                user_submitted_at: "2026-06-19T00:00:00Z".to_string(),
                assistant_started_at: Some("2026-06-19T00:00:03Z".to_string()),
                assistant_completed_at: Some("2026-06-19T00:00:04Z".to_string()),
                status: "completed".to_string(),
            },
            Some("assistant-second".to_string()),
            vec![conversation_message(
                "assistant-second",
                "assistant",
                "second answer",
            )],
        )
        .await
        .unwrap();

    let events: Vec<_> = state
        .chat_service
        .chat_stream(
            "default",
            Arc::clone(&runtime),
            dto::ChatRequest {
                message: "follow up".to_string(),
                chat_id: Some(chat_id.to_string()),
                input: None,
                messages: None,
                user_id: None,
                channel: None,
                stream: Some(true),
                model: None,
                attachments: Vec::new(),
                extra: serde_json::Map::from_iter([
                    ("turn_id".to_string(), json!("turn-variant")),
                    ("selected_variant_id".to_string(), json!("assistant-first")),
                ]),
            },
            running_cancel_rx(),
        )
        .collect()
        .await;
    assert!(
        events.iter().all(|event| event.is_ok()),
        "events: {events:?}"
    );

    let captured = captured
        .lock()
        .await
        .clone()
        .expect("runtime request captured");
    let joined = message_texts(&captured.messages).join("\n");
    assert!(joined.contains("first answer"), "captured input: {joined}");
    assert!(
        !joined.contains("second answer"),
        "selected variant must be applied before Evergreen projection is read: {joined}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_chat_projects_durable_runtime_notes_from_message_log() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::from_tempdir(dir.path().to_path_buf(), ServerProfile::Test);
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime");
    let captured = Arc::new(Mutex::new(None));
    runtime
        .set_react_runtime(Arc::new(CapturingRuntime {
            captured: Arc::clone(&captured),
        }))
        .await;

    let store = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir());
    let chat_id = "00000000-0000-0000-0000-000000000789";
    let header = materialized_chat_header(
        &state,
        &runtime.workspace_dir(),
        rua_session::ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "default".to_string(),
            title: "Runtime notes from Evergreen".to_string(),
            created_at: "2026-06-19T00:00:00Z".to_string(),
            last_activity_at: "2026-06-19T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        },
    )
    .await;
    store.save_chat_header(&header).await.unwrap();
    append_test_turn(&store, chat_id, 1).await;
    let configuration = state
        .product_hooks
        .resolve_complete_chat_configuration(
            &rua_core::ChatAddress::agent("default", chat_id),
            rua_server::services::product_hooks::ChatConfigurationMaterialization {
                selected_agent_id: "default".into(),
                running_config: Default::default(),
                security_config: Default::default(),
                agent_assignment: None,
                prompt_file_content: None,
                tool_config_snapshot: Default::default(),
                capability_configuration: None,
                model: None,
                thinking: None,
                collaboration_mode: serde_json::json!("default"),
                harness_configuration: None,
                authority_device_id: rua_server::services::device_context::current_sync_device_id(
                    &state.sync_device,
                )
                .unwrap(),
            },
        )
        .await
        .unwrap();
    let mut header = store.load_chat_header(chat_id).await.unwrap().unwrap();
    header.extra.insert(
        "hook_configuration".into(),
        serde_json::to_value(configuration).unwrap(),
    );
    store.save_chat_header(&header).await.unwrap();
    store
        .append_runtime_note(chat_id, "turn-1", runtime_note())
        .await
        .unwrap();

    let events: Vec<_> = state
        .chat_service
        .chat_stream(
            "default",
            Arc::clone(&runtime),
            dto::ChatRequest {
                message: "live user question".to_string(),
                chat_id: Some(chat_id.to_string()),
                input: None,
                messages: None,
                user_id: None,
                channel: None,
                stream: Some(true),
                model: None,
                attachments: Vec::new(),
                extra: serde_json::Map::new(),
            },
            running_cancel_rx(),
        )
        .collect()
        .await;
    assert!(
        events.iter().all(|event| event.is_ok()),
        "events: {events:?}"
    );

    let captured = captured
        .lock()
        .await
        .clone()
        .expect("runtime request captured");
    let projected = serde_json::to_string(&captured.messages).unwrap();
    assert!(projected.contains("evergreen runtime note should reach the model"));
    assert!(captured.messages.iter().any(|message| {
        message.role == "tool"
            && message
                .content
                .iter()
                .any(|part| matches!(part, ContentPart::ToolResult { .. }))
    }));
    let messages = store.read_message_range(chat_id, 1, 10).await.unwrap();
    assert_eq!(
        messages
            .iter()
            .filter_map(rua_session::RuntimeNote::from_message_log_entry)
            .count(),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_chat_persists_memory_signal_as_typed_runtime_note() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::from_tempdir(dir.path().to_path_buf(), ServerProfile::Test);
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime");
    let captured = Arc::new(Mutex::new(None));
    runtime
        .set_react_runtime(Arc::new(CapturingRuntime {
            captured: Arc::clone(&captured),
        }))
        .await;

    let store = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir());
    let chat_id = "00000000-0000-0000-0000-000000000790";
    let header = materialized_chat_header(
        &state,
        &runtime.workspace_dir(),
        rua_session::ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "default".to_string(),
            title: "Memory signal note".to_string(),
            created_at: "2026-06-24T00:00:00Z".to_string(),
            last_activity_at: "2026-06-24T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        },
    )
    .await;
    store.save_chat_header(&header).await.unwrap();

    let events: Vec<_> = state
        .chat_service
        .chat_stream(
            "default",
            Arc::clone(&runtime),
            dto::ChatRequest {
                message: "记住一下：以后回答直接给工程结论。".to_string(),
                chat_id: Some(chat_id.to_string()),
                input: None,
                messages: None,
                user_id: None,
                channel: None,
                stream: Some(true),
                model: None,
                attachments: Vec::new(),
                extra: serde_json::Map::new(),
            },
            running_cancel_rx(),
        )
        .collect()
        .await;
    assert!(
        events.iter().all(|event| event.is_ok()),
        "events: {events:?}"
    );

    let captured = captured
        .lock()
        .await
        .clone()
        .expect("runtime request captured");
    let texts = message_texts(&captured.messages);
    let joined = texts.join("\n");
    assert!(
        !joined.contains("<memory_signal>"),
        "Console must not duplicate the durable Runtime Note as a transient message: {joined}"
    );

    let messages = store.read_message_range(chat_id, 1, 10).await.unwrap();
    let runtime_notes: Vec<_> = messages
        .iter()
        .filter_map(rua_session::RuntimeNote::from_message_log_entry)
        .collect();
    assert_eq!(runtime_notes.len(), 1, "messages: {messages:?}");
    let memory_signal_text = runtime_notes[0].render_for_model();
    assert!(memory_signal_text.contains("<memory_signal>"));
    assert!(memory_signal_text.contains("explicit_memory"));
    assert!(memory_signal_text.contains("memory_write"));
    assert!(
        !memory_signal_text.contains("MEMORY.md"),
        "memory signal must not mention removed markdown Memory: {memory_signal_text}"
    );
    assert!(
        !memory_signal_text.contains("memory/"),
        "memory signal must not mention removed markdown Memory: {memory_signal_text}"
    );
    let rua_session::RuntimeNotePayload::MemorySignal(signal) = &runtime_notes[0].payload else {
        panic!("expected Memory Signal Runtime Note");
    };
    assert_eq!(signal.signal_kind, "explicit_memory");
    let note_message_id = runtime_notes[0].runtime_note_id.as_str();
    assert!(
        messages.windows(2).any(|pair| pair[0].role == "user"
            && pair[0]
                .content
                .to_string()
                .contains("以后回答直接给工程结论")
            && pair[1].message_id == note_message_id),
        "Memory Signal Runtime Note should be adjacent to its triggering user message: {messages:?}"
    );

    let events: Vec<_> = state
        .chat_service
        .chat_stream(
            "default",
            runtime,
            dto::ChatRequest {
                message: "记住一下：以后回答直接给工程结论。".to_string(),
                chat_id: Some(chat_id.to_string()),
                input: None,
                messages: None,
                user_id: None,
                channel: None,
                stream: Some(true),
                model: None,
                attachments: Vec::new(),
                extra: serde_json::Map::new(),
            },
            running_cancel_rx(),
        )
        .collect()
        .await;
    assert!(
        events.iter().all(|event| event.is_ok()),
        "events: {events:?}"
    );
    let messages = store.read_message_range(chat_id, 1, 20).await.unwrap();
    let memory_signal_count = messages
        .iter()
        .filter_map(rua_session::RuntimeNote::from_message_log_entry)
        .filter(|note| {
            matches!(
                note.payload,
                rua_session::RuntimeNotePayload::MemorySignal(_)
            )
        })
        .count();
    assert_eq!(
        memory_signal_count, 2,
        "Each triggering user input should keep its own Memory Signal Runtime Note"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_chat_skips_memory_signal_note_when_memory_write_is_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::from_tempdir(dir.path().to_path_buf(), ServerProfile::Test);
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime");
    std::fs::write(
        runtime.workspace_dir().join("agent.json"),
        serde_json::to_string_pretty(&json!({
            "id": "default",
            "name": "Default",
            "description": "Default agent",
            "workspace_dir": runtime.workspace_dir().to_string_lossy(),
            "tools": {
                "builtin_tools": {
                    "memory_write": {
                        "name": "memory_write",
                        "enabled": false,
                        "description": "Save durable facts to Memory",
                        "display_to_user": true,
                        "async_execution": false,
                        "config": {}
                    }
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let captured = Arc::new(Mutex::new(None));
    runtime
        .set_react_runtime(Arc::new(CapturingRuntime {
            captured: Arc::clone(&captured),
        }))
        .await;

    let store = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir());
    let chat_id = "00000000-0000-0000-0000-000000000791";
    let header = materialized_chat_header(
        &state,
        &runtime.workspace_dir(),
        rua_session::ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "default".to_string(),
            title: "Memory signal disabled".to_string(),
            created_at: "2026-06-24T00:00:00Z".to_string(),
            last_activity_at: "2026-06-24T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        },
    )
    .await;
    store.save_chat_header(&header).await.unwrap();

    let events: Vec<_> = state
        .chat_service
        .chat_stream(
            "default",
            runtime,
            dto::ChatRequest {
                message: "记住一下：以后回答直接给工程结论。".to_string(),
                chat_id: Some(chat_id.to_string()),
                input: None,
                messages: None,
                user_id: None,
                channel: None,
                stream: Some(true),
                model: None,
                attachments: Vec::new(),
                extra: serde_json::Map::new(),
            },
            running_cancel_rx(),
        )
        .collect()
        .await;
    assert!(
        events.iter().all(|event| event.is_ok()),
        "events: {events:?}"
    );

    let captured = captured
        .lock()
        .await
        .clone()
        .expect("runtime request captured");
    let joined = message_texts(&captured.messages).join("\n");
    assert!(!joined.contains("<memory_signal>"), "messages: {joined}");

    let messages = store.read_message_range(chat_id, 1, 10).await.unwrap();
    assert!(
        messages.iter().all(|message| {
            message
                .metadata
                .get("model_visible_marker")
                .and_then(|value| value.as_str())
                != Some("memory_signal")
        }),
        "messages: {messages:?}"
    );
    assert!(messages
        .iter()
        .filter_map(rua_session::RuntimeNote::from_message_log_entry)
        .all(|note| !matches!(
            note.payload,
            rua_session::RuntimeNotePayload::MemorySignal(_)
        )));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_chat_recalls_and_searches_typed_memory_signal_without_polluting_preview() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::from_tempdir(dir.path().to_path_buf(), ServerProfile::Test);
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime");
    let captured = Arc::new(Mutex::new(None));
    runtime
        .set_react_runtime(Arc::new(CapturingRuntime {
            captured: Arc::clone(&captured),
        }))
        .await;

    let store = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir());
    let chat_id = "00000000-0000-0000-0000-000000000792";
    let header = materialized_chat_header(
        &state,
        &runtime.workspace_dir(),
        rua_session::ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "default".to_string(),
            title: "Memory signal hidden search".to_string(),
            created_at: "2026-06-24T00:00:00Z".to_string(),
            last_activity_at: "2026-06-24T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        },
    )
    .await;
    store.save_chat_header(&header).await.unwrap();

    let events: Vec<_> = state
        .chat_service
        .chat_stream(
            "default",
            runtime,
            dto::ChatRequest {
                message: "记住一下：以后回答直接给工程结论。".to_string(),
                chat_id: Some(chat_id.to_string()),
                input: None,
                messages: None,
                user_id: None,
                channel: None,
                stream: Some(true),
                model: None,
                attachments: Vec::new(),
                extra: serde_json::Map::new(),
            },
            running_cancel_rx(),
        )
        .collect()
        .await;
    assert!(
        events.iter().all(|event| event.is_ok()),
        "events: {events:?}"
    );

    let header = store
        .load_chat_header(chat_id)
        .await
        .unwrap()
        .expect("chat header");
    assert!(
        !header
            .metadata
            .preview
            .as_ref()
            .is_some_and(|preview| preview.text.contains("<memory_signal>")),
        "preview: {:?}",
        header.metadata.preview
    );
    let recall = store
        .conversation_recall(
            chat_id,
            rua_session::ConversationRecallRequest {
                query: "memory_signal".to_string(),
                max_results: 10,
                surrounding_messages: 0,
            },
        )
        .await
        .unwrap();
    assert_eq!(recall.hits.len(), 1, "recall: {recall:?}");
    assert!(recall.hits[0].snippet.contains("<memory_signal>"));
    let saved = store
        .search_saved_conversations(rua_session::SavedConversationSearchRequest {
            agent_id: "default".to_string(),
            query: "memory_signal".to_string(),
            limit: 10,
            offset: 0,
        })
        .await
        .unwrap();
    assert_eq!(saved.results.len(), 1, "search: {saved:?}");
    assert_eq!(
        saved.results[0].message.message_id,
        recall.hits[0].message.message_id
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_chat_defers_projection_fit_decisions_to_the_model_call_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::from_tempdir(dir.path().to_path_buf(), ServerProfile::Test);
    let runtime = state
        .registry
        .get_or_create("default")
        .await
        .expect("default runtime");
    std::fs::write(
        runtime.workspace_dir().join("agent.json"),
        serde_json::to_string_pretty(&json!({
            "id": "default",
            "name": "Default",
            "description": "Default agent",
            "workspace_dir": runtime.workspace_dir().to_string_lossy(),
            "running": {
                "max_input_length": 1000,
                "context_compact": {
                    "context_compact_enabled": true,
                    "blocking_compact_ratio": 0.4,
                    "context_reserve_ratio": 0.1
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let captured = Arc::new(Mutex::new(None));
    runtime
        .set_react_runtime(Arc::new(CapturingRuntime {
            captured: Arc::clone(&captured),
        }))
        .await;

    let store = rua_session::WorkspaceSessionStore::new(runtime.workspace_dir());
    let chat_id = "00000000-0000-0000-0000-000000000456";
    let mut header = materialized_chat_header(
        &state,
        &runtime.workspace_dir(),
        rua_session::ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "default".to_string(),
            title: "Emergency projection".to_string(),
            created_at: "2026-06-19T00:00:00Z".to_string(),
            last_activity_at: "2026-06-19T00:00:00Z".to_string(),
            pinned: false,
            archived: false,
            preview: None,
        },
    )
    .await;
    header.message_log.chunk_size = 2;
    store.save_chat_header(&header).await.unwrap();
    for ordinal in 1..=10 {
        let label = if ordinal == 1 {
            "oldest-marker-alpha"
        } else if ordinal == 10 {
            "latest-marker-omega"
        } else {
            "middle-marker"
        };
        let body = format!("{label} {ordinal} {}", "large context payload ".repeat(40));
        append_test_turn_with_text(&store, chat_id, ordinal, &body, &body).await;
    }
    let events: Vec<_> = state
        .chat_service
        .chat_stream(
            "default",
            runtime,
            dto::ChatRequest {
                message: "live user question".to_string(),
                chat_id: Some(chat_id.to_string()),
                input: None,
                messages: None,
                user_id: None,
                channel: None,
                stream: Some(true),
                model: None,
                attachments: Vec::new(),
                extra: serde_json::Map::new(),
            },
            running_cancel_rx(),
        )
        .collect()
        .await;
    assert!(
        events.iter().all(|event| event.is_ok()),
        "events: {events:?}"
    );

    let captured = captured
        .lock()
        .await
        .clone()
        .expect("runtime request captured");
    let joined = message_texts(&captured.messages).join("\n");
    assert!(joined.contains("latest-marker-omega"));
    assert!(joined.contains("live user question"));
    assert!(
        joined.contains("oldest-marker-alpha"),
        "server must not make a separate turn-start emergency fit decision"
    );
}

type SummaryRequestCapture =
    tokio::sync::oneshot::Sender<(String, Option<rua_session::CheckpointModelIdentity>)>;

struct BlockingContextManager {
    called: Mutex<Option<SummaryRequestCapture>>,
    release: Notify,
    dropped: Option<Arc<AtomicBool>>,
}

struct SummaryFutureDropProbe(Option<Arc<AtomicBool>>);

impl Drop for SummaryFutureDropProbe {
    fn drop(&mut self) {
        if let Some(dropped) = self.0.as_ref() {
            dropped.store(true, Ordering::SeqCst);
        }
    }
}

#[async_trait::async_trait]
impl rua_agent::context_manager::ContextManager for BlockingContextManager {
    async fn compact_context(
        &self,
        req: rua_agent::context_manager::CompactContextRequest,
    ) -> Result<rua_agent::context_manager::CompactContextResult, rua_agent::error::AgentError>
    {
        Ok(rua_agent::context_manager::CompactContextResult {
            compacted_messages: req.messages,
            tokens_saved: 0,
            summary_generated: None,
            grade: None,
        })
    }

    async fn summarize_context(
        &self,
        req: rua_agent::context_manager::ContextSummaryRequest,
    ) -> Result<rua_agent::context_manager::ContextSummaryResult, rua_agent::error::AgentError>
    {
        let _drop_probe = SummaryFutureDropProbe(self.dropped.clone());
        if let Some(tx) = self.called.lock().await.take() {
            let content = format!(
                "{}\n{}",
                req.previous_summary.unwrap_or_default(),
                serde_json::to_string(&req.messages).unwrap()
            );
            let _ = tx.send((content, req.model));
        }
        self.release.notified().await;
        Ok(rua_agent::context_manager::ContextSummaryResult {
            summary: "background checkpoint summary".to_string(),
        })
    }
}

type HistoryReplacementRequestCapture =
    tokio::sync::oneshot::Sender<rua_agent::context_manager::HistoryReplacementGenerationRequest>;

struct BlockingHistoryReplacementManager {
    called: Mutex<Option<HistoryReplacementRequestCapture>>,
    release: Notify,
}

enum StaticHistoryReplacementOutcome {
    Generated {
        digest: String,
        finish_reason: String,
    },
    ProviderFailure,
}

struct StaticHistoryReplacementManager {
    calls: AtomicUsize,
    outcome: StaticHistoryReplacementOutcome,
}

#[async_trait::async_trait]
impl rua_agent::context_manager::ContextManager for StaticHistoryReplacementManager {
    async fn compact_context(
        &self,
        req: rua_agent::context_manager::CompactContextRequest,
    ) -> Result<rua_agent::context_manager::CompactContextResult, rua_agent::error::AgentError>
    {
        Ok(rua_agent::context_manager::CompactContextResult {
            compacted_messages: req.messages,
            tokens_saved: 0,
            summary_generated: None,
            grade: None,
        })
    }

    async fn summarize_context(
        &self,
        _req: rua_agent::context_manager::ContextSummaryRequest,
    ) -> Result<rua_agent::context_manager::ContextSummaryResult, rua_agent::error::AgentError>
    {
        Ok(rua_agent::context_manager::ContextSummaryResult {
            summary: "unused".to_string(),
        })
    }

    async fn generate_history_replacement(
        &self,
        _req: rua_agent::context_manager::HistoryReplacementGenerationRequest,
    ) -> Result<
        rua_agent::context_manager::HistoryReplacementGenerationResult,
        rua_agent::error::AgentError,
    > {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match &self.outcome {
            StaticHistoryReplacementOutcome::Generated {
                digest,
                finish_reason,
            } => Ok(
                rua_agent::context_manager::HistoryReplacementGenerationResult {
                    digest: digest.clone(),
                    finish_reason: finish_reason.clone(),
                    usage: None,
                },
            ),
            StaticHistoryReplacementOutcome::ProviderFailure => {
                Err(rua_agent::error::AgentError::new(
                    rua_agent::error::AgentErrorKind::Provider,
                    "provider transport exhausted",
                ))
            }
        }
    }
}

#[async_trait::async_trait]
impl rua_agent::context_manager::ContextManager for BlockingHistoryReplacementManager {
    async fn compact_context(
        &self,
        req: rua_agent::context_manager::CompactContextRequest,
    ) -> Result<rua_agent::context_manager::CompactContextResult, rua_agent::error::AgentError>
    {
        Ok(rua_agent::context_manager::CompactContextResult {
            compacted_messages: req.messages,
            tokens_saved: 0,
            summary_generated: None,
            grade: None,
        })
    }

    async fn summarize_context(
        &self,
        _req: rua_agent::context_manager::ContextSummaryRequest,
    ) -> Result<rua_agent::context_manager::ContextSummaryResult, rua_agent::error::AgentError>
    {
        Ok(rua_agent::context_manager::ContextSummaryResult {
            summary: "unused".to_string(),
        })
    }

    async fn generate_history_replacement(
        &self,
        req: rua_agent::context_manager::HistoryReplacementGenerationRequest,
    ) -> Result<
        rua_agent::context_manager::HistoryReplacementGenerationResult,
        rua_agent::error::AgentError,
    > {
        if let Some(tx) = self.called.lock().await.take() {
            let _ = tx.send(req);
        }
        self.release.notified().await;
        Ok(
            rua_agent::context_manager::HistoryReplacementGenerationResult {
                digest: "The agent inspected src/lib.rs and found the durable runtime boundary."
                    .to_string(),
                finish_reason: "stop".to_string(),
                usage: None,
            },
        )
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_persistence_generates_history_replacement_in_background_with_active_model() {
    let dir = tempfile::tempdir().unwrap();
    let authority = Arc::new(ConversationAuthority::new());
    let (called_tx, called_rx) = tokio::sync::oneshot::channel();
    let context_manager = Arc::new(BlockingHistoryReplacementManager {
        called: Mutex::new(Some(called_tx)),
        release: Notify::new(),
    });
    let mut running = rua_config::AgentsRunningConfig::default();
    running.context_compact.history_replacement_enabled = true;
    running.context_compact.early_background_compaction_enabled = false;

    let persistence = WorkspaceSessionPersistence::new(dir.path().to_path_buf())
        .with_authority("authority-device".to_string(), epoch(1))
        .with_conversation_authority(authority)
        .with_context_manager(context_manager.clone())
        .with_checkpoint_model(rua_session::CheckpointModelIdentity {
            provider: "chat-provider".to_string(),
            model: "chat-model".to_string(),
        })
        .with_running_config(Arc::new(running));
    seed_persistence_chat(dir.path(), "chat-history-replacement").await;
    persistence
        .save_chat_entry("agent-1", "chat-history-replacement", "History replacement")
        .await
        .unwrap();

    let mut tool_result = ToolResult::text(
        "call-1",
        format!("raw-history-marker{}", "x".repeat(70_000)),
    );
    tool_result.name = Some("read".to_string());
    persistence
        .save_messages(
            "agent-1",
            "chat-history-replacement",
            &[
                text("user", "Inspect src/lib.rs"),
                AgentInputMessage {
                    role: "assistant".to_string(),
                    content: vec![
                        ContentPart::Thinking {
                            text: "I need to inspect the source.".to_string(),
                            signature: None,
                        },
                        ContentPart::ToolUse {
                            call: ToolCall::new("call-1", "read", json!({"path": "src/lib.rs"})),
                        },
                    ],
                },
                AgentInputMessage {
                    role: "tool".to_string(),
                    content: vec![ContentPart::ToolResult {
                        result: tool_result,
                    }],
                },
                text(
                    "assistant",
                    "The repository uses a durable runtime boundary.",
                ),
            ],
        )
        .await
        .unwrap();

    let request = tokio::time::timeout(std::time::Duration::from_secs(2), called_rx)
        .await
        .expect("history replacement generation should start")
        .expect("generation request");
    assert_eq!(request.max_tokens, 4096);
    assert_eq!(request.model.provider, "chat-provider");
    assert_eq!(request.model.model, "chat-model");
    assert!(request.source_content.contains("raw-history-marker"));
    assert!(!request
        .source_content
        .contains("The repository uses a durable runtime boundary."));

    let store = rua_session::WorkspaceSessionStore::new(dir.path());
    let block = store
        .detect_history_replacement_blocks("chat-history-replacement", 4.0)
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("eligible block");
    assert_eq!(request.source_content, block.source_content);
    assert_eq!(request.agent_id, "agent-1");
    assert_eq!(request.session_id, "chat-history-replacement");
    assert_eq!(request.turn_id, block.source.turn_id);
    assert_eq!(request.source_block_id, block.record_id);
    assert!(request.usage_id.starts_with("history-replacement-"));
    assert!(
        store
            .load_history_replacement("chat-history-replacement", &block.record_id)
            .await
            .unwrap()
            .is_none(),
        "save_messages must not wait for replacement generation"
    );

    context_manager.release.notify_waiters();
    let record = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if let Some(record) = store
                .load_history_replacement("chat-history-replacement", &block.record_id)
                .await
                .unwrap()
            {
                break record;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("ready history replacement should publish");
    assert_eq!(
        record.ready_digest(),
        Some("The agent inspected src/lib.rs and found the durable runtime boundary.")
    );

    let projected = store
        .build_context_projection_with_history_replacements(
            "chat-history-replacement",
            rua_session::ContextProjectionRequest {
                current_user_input: None,
            },
            4.0,
        )
        .await
        .unwrap();
    let projected_text = projected
        .tail_messages
        .iter()
        .map(message_log_text_for_test)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(projected_text.contains("[History Replacement; source:"));
    assert!(!projected_text.contains("raw-history-marker"));

    let recalled = store
        .conversation_read_span(
            "chat-history-replacement",
            rua_session::ConversationReadSpanRequest {
                start_sequence: block.source.sequence_range.start_sequence,
                end_sequence: block.source.sequence_range.end_sequence,
                expand_before: 0,
                expand_after: 0,
            },
        )
        .await
        .unwrap();
    assert!(recalled
        .messages
        .iter()
        .map(|message| message.content.to_string())
        .collect::<Vec<_>>()
        .join("\n")
        .contains("raw-history-marker"));

    let raw_history = store
        .read_message_range("chat-history-replacement", 1, usize::MAX)
        .await
        .unwrap();
    let raw_history_text = raw_history
        .iter()
        .map(|message| message.content.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(raw_history_text.contains("raw-history-marker"));
    assert!(!raw_history_text.contains("[History Replacement; source:"));

    let projected_messages = vec![
        text("system", "Compact summary for model context only"),
        text(
            "assistant",
            "[History Replacement; source: conversation_read_span(start_sequence=1, end_sequence=3)]\n\nProjected digest",
        ),
    ];
    let mut completed_messages = projected_messages.clone();
    completed_messages.push(text("user", "Continue"));
    completed_messages.push(text("assistant", "Visible continuation"));
    WorkspaceSessionPersistence::new(dir.path().to_path_buf())
        .with_loaded_message_count(projected_messages.len())
        .with_authority("authority-device".to_string(), epoch(1))
        .save_messages("agent-1", "chat-history-replacement", &completed_messages)
        .await
        .unwrap();

    let history_after_continuation = store
        .read_message_range("chat-history-replacement", 1, usize::MAX)
        .await
        .unwrap();
    let durable_text = history_after_continuation
        .iter()
        .map(|message| message.content.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(durable_text.contains("Visible continuation"));
    assert!(!durable_text.contains("Compact summary for model context only"));
    assert!(!durable_text.contains("Projected digest"));

    store
        .publish_manual_context_checkpoint(
            "chat-history-replacement",
            "Checkpoint consumed the effective replacement.".to_string(),
            rua_session::CheckpointModelIdentity {
                provider: "chat-provider".to_string(),
                model: "chat-model".to_string(),
            },
        )
        .await
        .unwrap();
    let after_checkpoint = store
        .build_context_projection_with_history_replacements(
            "chat-history-replacement",
            rua_session::ContextProjectionRequest {
                current_user_input: None,
            },
            4.0,
        )
        .await
        .unwrap();
    assert!(after_checkpoint
        .summary
        .as_deref()
        .is_some_and(|summary| summary.contains("effective replacement")));
    assert!(!after_checkpoint
        .tail_messages
        .iter()
        .map(message_log_text_for_test)
        .collect::<Vec<_>>()
        .join("\n")
        .contains("[History Replacement; source:"));
    assert!(store
        .load_history_replacement("chat-history-replacement", &block.record_id)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_persistence_remembers_terminal_rejection_for_same_model() {
    let dir = tempfile::tempdir().unwrap();
    let authority = Arc::new(ConversationAuthority::new());
    let context_manager = Arc::new(StaticHistoryReplacementManager {
        calls: AtomicUsize::new(0),
        outcome: StaticHistoryReplacementOutcome::Generated {
            digest: "partial output".to_string(),
            finish_reason: "length".to_string(),
        },
    });
    let mut running = rua_config::AgentsRunningConfig::default();
    running.context_compact.history_replacement_enabled = true;
    let running = Arc::new(running);
    let model = rua_session::CheckpointModelIdentity {
        provider: "chat-provider".to_string(),
        model: "chat-model".to_string(),
    };
    let persistence = WorkspaceSessionPersistence::new(dir.path().to_path_buf())
        .with_authority("authority-device".to_string(), epoch(1))
        .with_conversation_authority(Arc::clone(&authority))
        .with_context_manager(context_manager.clone())
        .with_checkpoint_model(model.clone())
        .with_running_config(Arc::clone(&running));
    seed_persistence_chat(dir.path(), "chat-rejected").await;
    persistence
        .save_chat_entry("agent-1", "chat-rejected", "Rejected replacement")
        .await
        .unwrap();
    let original_messages = history_replacement_trace("rejected-runtime-marker");
    persistence
        .save_messages("agent-1", "chat-rejected", &original_messages)
        .await
        .unwrap();

    let store = rua_session::WorkspaceSessionStore::new(dir.path());
    let block = store
        .detect_history_replacement_blocks("chat-rejected", 4.0)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let rejected = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if let Some(record) = store
                .load_history_replacement("chat-rejected", &block.record_id)
                .await
                .unwrap()
            {
                break record;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("terminal rejection should persist");
    assert!(rejected.is_rejected_for_model(&model));
    assert!(matches!(
        rejected.state,
        rua_session::HistoryReplacementRecordState::Rejected {
            ref generations
        } if generations[0].reason
            == rua_session::HistoryReplacementRejectionReason::OutputLimitReached
    ));
    assert_eq!(context_manager.calls.load(Ordering::SeqCst), 1);

    let mut next_messages = original_messages;
    next_messages.push(text("user", "Continue"));
    next_messages.push(text("assistant", "Continued."));
    WorkspaceSessionPersistence::new(dir.path().to_path_buf())
        .with_loaded_message_count(4)
        .with_authority("authority-device".to_string(), epoch(1))
        .with_conversation_authority(authority)
        .with_context_manager(context_manager.clone())
        .with_checkpoint_model(model)
        .with_running_config(running)
        .save_messages("agent-1", "chat-rejected", &next_messages)
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(
        context_manager.calls.load(Ordering::SeqCst),
        1,
        "same model generation identity must not spend again"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_persistence_keeps_provider_failure_retryable_without_record() {
    let dir = tempfile::tempdir().unwrap();
    let authority = Arc::new(ConversationAuthority::new());
    let context_manager = Arc::new(StaticHistoryReplacementManager {
        calls: AtomicUsize::new(0),
        outcome: StaticHistoryReplacementOutcome::ProviderFailure,
    });
    let mut running = rua_config::AgentsRunningConfig::default();
    running.context_compact.history_replacement_enabled = true;
    let persistence = WorkspaceSessionPersistence::new(dir.path().to_path_buf())
        .with_authority("authority-device".to_string(), epoch(1))
        .with_conversation_authority(authority)
        .with_context_manager(context_manager.clone())
        .with_checkpoint_model(rua_session::CheckpointModelIdentity {
            provider: "chat-provider".to_string(),
            model: "chat-model".to_string(),
        })
        .with_running_config(Arc::new(running));
    seed_persistence_chat(dir.path(), "chat-transient").await;
    persistence
        .save_chat_entry("agent-1", "chat-transient", "Transient replacement")
        .await
        .unwrap();
    persistence
        .save_messages(
            "agent-1",
            "chat-transient",
            &history_replacement_trace("transient-runtime-marker"),
        )
        .await
        .unwrap();

    let store = rua_session::WorkspaceSessionStore::new(dir.path());
    let block = store
        .detect_history_replacement_blocks("chat-transient", 4.0)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while context_manager.calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("generation should be attempted");
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(store
        .load_history_replacement("chat-transient", &block.record_id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_persistence_starts_early_background_compaction_after_target_band_pressure() {
    let dir = tempfile::tempdir().unwrap();
    let (called_tx, called_rx) = tokio::sync::oneshot::channel();
    let context_manager = Arc::new(BlockingContextManager {
        called: Mutex::new(Some(called_tx)),
        release: Notify::new(),
        dropped: None,
    });
    let running = rua_config::AgentsRunningConfig {
        max_input_length: 1200,
        context_compact: rua_config::ContextCompactConfig {
            blocking_compact_ratio: 0.5,
            early_background_compaction_enabled: true,
            early_compaction_target_ratio: 0.5,
            context_reserve_ratio: 0.1,
            compact_with_thinking_block: false,
            ..Default::default()
        },
        ..Default::default()
    };

    let persistence = WorkspaceSessionPersistence::new(dir.path().to_path_buf())
        .with_context_manager(context_manager.clone())
        .with_checkpoint_model(rua_session::CheckpointModelIdentity {
            provider: "chat-provider".to_string(),
            model: "chat-model".to_string(),
        })
        .with_running_config(Arc::new(running));
    seed_persistence_chat(dir.path(), "chat-1").await;
    persistence
        .save_chat_entry("agent-1", "chat-1", "Background maintenance")
        .await
        .unwrap();

    let long_first_turn = "first foldable turn ".repeat(80);
    let thinking_marker = "checkpoint-private-thinking-marker";
    persistence
        .save_messages(
            "agent-1",
            "chat-1",
            &[
                text("user", &long_first_turn),
                AgentInputMessage {
                    role: "assistant".to_string(),
                    content: vec![
                        ContentPart::Thinking {
                            text: thinking_marker.to_string(),
                            signature: None,
                        },
                        ContentPart::Text {
                            text: long_first_turn.clone(),
                        },
                    ],
                },
            ],
        )
        .await
        .unwrap();

    let long_tail_turn = "tail turn ".repeat(80);
    persistence
        .save_messages(
            "agent-1",
            "chat-1",
            &[
                text("user", &long_tail_turn),
                text("assistant", &long_tail_turn),
            ],
        )
        .await
        .unwrap();

    let (summarized_content, summarized_model) =
        tokio::time::timeout(std::time::Duration::from_secs(2), called_rx)
            .await
            .expect("background summary should start")
            .expect("summary content");
    assert!(summarized_content.contains("first foldable turn"));
    assert!(!summarized_content.contains(thinking_marker));
    assert!(!summarized_content.contains("tail turn"));
    assert_eq!(
        summarized_model,
        Some(rua_session::CheckpointModelIdentity {
            provider: "chat-provider".to_string(),
            model: "chat-model".to_string(),
        })
    );

    let store = rua_session::WorkspaceSessionStore::new(dir.path());
    assert!(
        store
            .load_active_summary_checkpoint("chat-1")
            .await
            .unwrap()
            .is_none(),
        "save_messages must not block on the background checkpoint publish"
    );

    let post_snapshot_turn = "post snapshot turn ".repeat(20);
    persistence
        .save_messages(
            "agent-1",
            "chat-1",
            &[
                text("user", &post_snapshot_turn),
                text("assistant", &post_snapshot_turn),
            ],
        )
        .await
        .unwrap();

    context_manager.release.notify_waiters();
    let checkpoint = eventually_active_checkpoint(&store, "chat-1").await;
    assert_eq!(checkpoint.summary, "background checkpoint summary");
    assert_eq!(checkpoint.covered_turn_range.end_turn_ordinal, 1);
    assert_eq!(checkpoint.covered_sequence_range.end_sequence, 2);

    let projection = store
        .build_context_projection(
            "chat-1",
            rua_session::ContextProjectionRequest {
                current_user_input: None,
            },
        )
        .await
        .unwrap();
    let tail_text = projection
        .tail_messages
        .iter()
        .map(message_log_text_for_test)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(tail_text.contains("tail turn"));
    assert!(
        tail_text.contains("post snapshot turn"),
        "messages appended while background compaction runs must remain active tail"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_persistence_does_not_start_early_background_compaction_before_target() {
    let dir = tempfile::tempdir().unwrap();
    let (called_tx, called_rx) = tokio::sync::oneshot::channel();
    let context_manager = Arc::new(BlockingContextManager {
        called: Mutex::new(Some(called_tx)),
        release: Notify::new(),
        dropped: None,
    });
    let running = rua_config::AgentsRunningConfig {
        max_input_length: 2000,
        context_compact: rua_config::ContextCompactConfig {
            blocking_compact_ratio: 0.9,
            early_background_compaction_enabled: true,
            early_compaction_target_ratio: 0.9,
            context_reserve_ratio: 0.1,
            ..Default::default()
        },
        ..Default::default()
    };

    let persistence = WorkspaceSessionPersistence::new(dir.path().to_path_buf())
        .with_context_manager(context_manager.clone())
        .with_checkpoint_model(rua_session::CheckpointModelIdentity {
            provider: "chat-provider".to_string(),
            model: "chat-model".to_string(),
        })
        .with_running_config(Arc::new(running));
    seed_persistence_chat(dir.path(), "chat-1").await;
    persistence
        .save_chat_entry("agent-1", "chat-1", "Growth maintenance")
        .await
        .unwrap();

    let first_turn = "stable first turn ".repeat(40);
    persistence
        .save_messages(
            "agent-1",
            "chat-1",
            &[text("user", &first_turn), text("assistant", &first_turn)],
        )
        .await
        .unwrap();

    let growth_turn = "recent growth turn ".repeat(40);
    persistence
        .save_messages(
            "agent-1",
            "chat-1",
            &[text("user", &growth_turn), text("assistant", &growth_turn)],
        )
        .await
        .unwrap();

    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(200), called_rx)
            .await
            .is_err(),
        "background checkpoint should wait until the early compaction target is crossed"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_context_checkpoint_preempts_slow_early_background_compaction_publish() {
    let dir = tempfile::tempdir().unwrap();
    let (called_tx, called_rx) = tokio::sync::oneshot::channel();
    let summary_future_dropped = Arc::new(AtomicBool::new(false));
    let context_manager = Arc::new(BlockingContextManager {
        called: Mutex::new(Some(called_tx)),
        release: Notify::new(),
        dropped: Some(Arc::clone(&summary_future_dropped)),
    });
    let running = rua_config::AgentsRunningConfig {
        max_input_length: 1200,
        context_compact: rua_config::ContextCompactConfig {
            blocking_compact_ratio: 0.5,
            early_background_compaction_enabled: true,
            early_compaction_target_ratio: 0.5,
            context_reserve_ratio: 0.1,
            ..Default::default()
        },
        ..Default::default()
    };

    let persistence = WorkspaceSessionPersistence::new(dir.path().to_path_buf())
        .with_context_manager(context_manager.clone())
        .with_checkpoint_model(rua_session::CheckpointModelIdentity {
            provider: "chat-provider".to_string(),
            model: "chat-model".to_string(),
        })
        .with_running_config(Arc::new(running));
    seed_persistence_chat(dir.path(), "chat-1").await;
    persistence
        .save_chat_entry("agent-1", "chat-1", "Manual wins")
        .await
        .unwrap();

    let long_first_turn = "first foldable turn ".repeat(80);
    persistence
        .save_messages(
            "agent-1",
            "chat-1",
            &[
                text("user", &long_first_turn),
                text("assistant", &long_first_turn),
            ],
        )
        .await
        .unwrap();

    let long_tail_turn = "tail turn ".repeat(80);
    persistence
        .save_messages(
            "agent-1",
            "chat-1",
            &[
                text("user", &long_tail_turn),
                text("assistant", &long_tail_turn),
            ],
        )
        .await
        .unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(2), called_rx)
        .await
        .expect("background summary should start")
        .expect("summary content");

    let store = rua_session::WorkspaceSessionStore::new(dir.path());
    let manual = store
        .publish_manual_context_checkpoint(
            "chat-1",
            "manual foreground compact summary".to_string(),
            rua_session::CheckpointModelIdentity {
                provider: "manual".to_string(),
                model: "compact".to_string(),
            },
        )
        .await
        .unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !summary_future_dropped.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("preempted background summary future should be dropped");
    assert!(
        !checkpoint_file_contains(dir.path(), "chat-1", "background checkpoint summary"),
        "preempted background checkpoint must not publish after foreground compact"
    );

    let active = store
        .load_active_summary_checkpoint("chat-1")
        .await
        .unwrap()
        .expect("active checkpoint");
    assert_eq!(active.checkpoint_id, manual.checkpoint_id);
    assert_eq!(active.summary, "manual foreground compact summary");
    assert_ne!(
        active.summary, "background checkpoint summary",
        "slow background checkpoint must not overwrite a foreground compact"
    );
}

async fn eventually_active_checkpoint(
    store: &rua_session::WorkspaceSessionStore,
    chat_id: &str,
) -> rua_session::SummaryCheckpoint {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        if let Some(checkpoint) = store.load_active_summary_checkpoint(chat_id).await.unwrap() {
            return checkpoint;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "checkpoint was not published"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

fn checkpoint_file_contains(
    workspace_dir: &std::path::Path,
    chat_id: &str,
    expected_summary: &str,
) -> bool {
    let checkpoint_dir = workspace_dir
        .join("sessions")
        .join(chat_id)
        .join("checkpoints");
    if let Ok(entries) = std::fs::read_dir(&checkpoint_dir) {
        for entry in entries.flatten() {
            let Ok(raw) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            if raw.contains(expected_summary) {
                return true;
            }
        }
    }
    false
}

async fn seed_persistence_chat(root: &std::path::Path, chat_id: &str) {
    let store = rua_session::WorkspaceSessionStore::new(root);
    assert!(
        store.load_chat_header(chat_id).await.unwrap().is_none(),
        "fixture must create a new Chat"
    );
    let mut header = rua_session::ChatSessionHeader::new(rua_session::ChatMetadata {
        chat_id: chat_id.into(),
        agent_id: "agent-1".into(),
        title: "New Chat".into(),
        created_at: "2026-07-24T00:00:00Z".into(),
        last_activity_at: "2026-07-24T00:00:00Z".into(),
        pinned: false,
        archived: false,
        preview: None,
    });
    rua_chat::apply_agent_scope_defaults(&mut header, "agent-1");
    store.save_chat_header(&header).await.unwrap();
}

async fn seed_runtime_configuration(
    state: &AppState,
    agent_id: &str,
    header: &mut rua_session::ChatSessionHeader,
) {
    let running: rua_config::AgentsRunningConfig = serde_json::from_value(
        state
            .conversation_input_intake
            .authority_running_config(agent_id)
            .await
            .unwrap(),
    )
    .unwrap();
    let configuration = state
        .product_hooks
        .resolve_complete_chat_configuration(
            &rua_core::ChatAddress::agent(agent_id, &header.chat_id),
            rua_server::services::product_hooks::ChatConfigurationMaterialization {
                selected_agent_id: agent_id.into(),
                running_config: rua_config::ChatRunningConfig::from_agent_defaults(&running),
                security_config: Default::default(),
                agent_assignment: None,
                prompt_file_content: None,
                tool_config_snapshot: Default::default(),
                capability_configuration: None,
                model: header.extra.get("model").cloned(),
                thinking: None,
                collaboration_mode: json!("default"),
                harness_configuration: None,
                authority_device_id: header.extra[CHAT_AUTHORITY_PLACEMENT_META_KEY]["device_id"]
                    .as_str()
                    .unwrap()
                    .into(),
            },
        )
        .await
        .unwrap();
    header.extra.insert(
        "hook_configuration".into(),
        serde_json::to_value(configuration).unwrap(),
    );
}

#[tokio::test]
async fn persistence_requires_existing_chat_and_never_creates_on_update() {
    let dir = tempfile::tempdir().unwrap();
    let persistence = WorkspaceSessionPersistence::new(dir.path().into());
    let store = rua_session::WorkspaceSessionStore::new(dir.path());
    assert!(persistence
        .save_chat_entry("agent-1", "missing", "Title")
        .await
        .is_err());
    assert!(persistence
        .save_messages("agent-1", "missing", &[text("user", "hello")])
        .await
        .is_err());
    assert!(
        rua_server::services::console_chat::clear_pending_action_snapshot(dir.path(), "missing")
            .await
            .is_err()
    );
    assert!(!store.chat_dir("missing").exists());
    assert!(chat_index::get(dir.path(), "missing")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn persistence_reports_corrupt_header_without_replacing_it() {
    let dir = tempfile::tempdir().unwrap();
    seed_persistence_chat(dir.path(), "corrupt").await;
    let store = rua_session::WorkspaceSessionStore::new(dir.path());
    let path = store.chat_dir("corrupt").join("session.json");
    assert!(path.exists());
    std::fs::write(&path, b"{broken").unwrap();
    let persistence = WorkspaceSessionPersistence::new(dir.path().into());
    assert!(persistence
        .save_chat_entry("agent-1", "corrupt", "Title")
        .await
        .is_err());
    assert!(persistence
        .save_messages("agent-1", "corrupt", &[text("user", "hello")])
        .await
        .is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"{broken");
}

#[tokio::test]
async fn persistence_reports_index_failure_after_saving_authoritative_header() {
    let dir = tempfile::tempdir().unwrap();
    seed_persistence_chat(dir.path(), "index-error").await;
    let scope = rua_core::ChatScopeStore::for_agent_workspace(
        dir.path(),
        rua_core::ChatAddress::agent("agent-1", "index-error"),
    )
    .unwrap();
    std::fs::create_dir_all(scope.chat_index_path()).unwrap();
    let persistence = WorkspaceSessionPersistence::new(dir.path().into());
    assert!(persistence
        .save_chat_entry("agent-1", "index-error", "Saved title")
        .await
        .is_err());
    let header = rua_session::WorkspaceSessionStore::new(dir.path())
        .load_chat_header("index-error")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(header.metadata.title, "Saved title");
}

#[tokio::test]
async fn missing_index_is_rebuilt_from_header_without_losing_owned_metadata() {
    let dir = tempfile::tempdir().unwrap();
    seed_persistence_chat(dir.path(), "projection").await;
    let store = rua_session::WorkspaceSessionStore::new(dir.path());
    store
        .update_chat_header("projection", |header| {
            header.metadata.title = "User title".into();
            header.metadata.pinned = true;
            header.metadata.archived = true;
            header
                .extra
                .insert("extension".into(), json!({"keep": true}));
        })
        .await
        .unwrap();
    let persistence = WorkspaceSessionPersistence::new(dir.path().into());
    persistence
        .save_chat_entry("agent-1", "projection", "Do not replace title")
        .await
        .unwrap();
    let entry = chat_index::get(dir.path(), "projection")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(entry.name, "User title");
    assert!(entry.pinned && entry.archived);
    assert_eq!(entry.meta["extension"], json!({"keep": true}));
    assert_eq!(entry.created_at.to_rfc3339(), "2026-07-24T00:00:00+00:00");
}
