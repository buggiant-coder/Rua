use rua_core::message::ContentPart;
use rua_server::services::context_projection::{
    estimate_log_message_tokens, message_log_entries_to_input_messages, message_log_text,
};
use rua_session::{
    MessageLogEntry, RuntimeNote, RuntimeNotePayload, TaskAttemptTerminalRuntimeNote,
    TaskWatchOutputProjection, TaskWatchOutputRecord, TaskWatchOutputRuntimeNote,
};
use serde_json::json;

fn entry(sequence: u64, role: &str, content: serde_json::Value) -> MessageLogEntry {
    MessageLogEntry {
        message_id: format!("msg-{sequence}"),
        sequence,
        turn_id: "turn-1".to_string(),
        role: role.to_string(),
        created_at: "2026-06-29T00:00:00Z".to_string(),
        status: "completed".to_string(),
        content,
        provider: None,
        tool: None,
        metadata: serde_json::Map::new(),
    }
}

fn text_content(text: &str) -> serde_json::Value {
    json!([{ "type": "text", "text": text }])
}

fn input_text(message: &rua_contract::agent::AgentInputMessage) -> String {
    message
        .content
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

fn runtime_note_entry(sequence: u64, note: RuntimeNote) -> MessageLogEntry {
    let message = note.into_new_conversation_message().unwrap();
    MessageLogEntry {
        message_id: message.message_id,
        sequence,
        turn_id: "turn-1".to_string(),
        role: message.role,
        created_at: message.created_at,
        status: message.status,
        content: message.content,
        provider: message.provider,
        tool: message.tool,
        metadata: message.metadata,
    }
}

#[test]
fn evergreen_projection_input_messages_apply_legacy_command_window_filter() {
    let entries = vec![
        entry(1, "user", text_content("/status")),
        entry(2, "assistant", text_content("status details")),
        entry(3, "user", text_content("real request")),
        entry(4, "assistant", text_content("real response")),
    ];

    let messages = message_log_entries_to_input_messages(&entries);

    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, "user");
    assert_eq!(input_text(&messages[0]), "real request");
    assert_eq!(messages[1].role, "assistant");
    assert_eq!(input_text(&messages[1]), "real response");
}

#[test]
fn evergreen_projection_input_messages_keep_tool_round_shape_after_filtering() {
    let entries = vec![
        entry(1, "user", text_content("/clear")),
        entry(2, "assistant", text_content("cleared")),
        entry(3, "user", text_content("inspect")),
        entry(
            4,
            "assistant",
            json!([{
                "type": "tool_use",
                "id": "call-1",
                "name": "bash",
                "input": {"command": "pwd"}
            }]),
        ),
        entry(
            5,
            "system",
            json!([{
                "type": "tool_result",
                "id": "call-1",
                "name": "bash",
                "output": "done"
            }]),
        ),
    ];

    let messages = message_log_entries_to_input_messages(&entries);

    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].role, "user");
    assert!(matches!(
        messages[1].content.first(),
        Some(ContentPart::ToolUse { call }) if call.id == "call-1"
    ));
    assert!(matches!(
        messages[2].content.first(),
        Some(ContentPart::ToolResult { result }) if result.call_id == "call-1"
    ));
}

#[test]
fn evergreen_projection_reassembles_one_model_dialogue_item() {
    let mut reasoning = entry(
        1,
        "assistant",
        json!([{"type": "thinking", "thinking": "inspect"}]),
    );
    let mut text = entry(2, "assistant", text_content("answer"));
    let mut tool_use = entry(
        3,
        "assistant",
        json!([{
            "type": "tool_use",
            "id": "call-1",
            "name": "bash",
            "input": {"command": "pwd"}
        }]),
    );
    for message in [&mut reasoning, &mut text, &mut tool_use] {
        message.metadata.insert(
            agent_types::conversation_message_identity::MODEL_DIALOGUE_ITEM_ID_METADATA_KEY
                .to_string(),
            json!("model-call-1"),
        );
    }

    let messages = message_log_entries_to_input_messages(&[reasoning, text, tool_use]);

    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, "assistant");
    assert_eq!(messages[0].content.len(), 3);
    assert!(matches!(
        &messages[0].content[0],
        ContentPart::Thinking { .. }
    ));
    assert!(matches!(&messages[0].content[1], ContentPart::Text { .. }));
    assert!(matches!(
        &messages[0].content[2],
        ContentPart::ToolUse { .. }
    ));
    assert_eq!(messages[1].role, "tool");
    assert!(
        matches!(&messages[1].content[0], ContentPart::ToolResult { result } if result.call_id == "call-1")
    );
}

#[test]
fn evergreen_projection_input_messages_repair_aborted_tool_rounds() {
    let entries = vec![
        entry(1, "user", text_content("run a slow command")),
        entry(
            2,
            "assistant",
            json!([{
                "type": "tool_use",
                "id": "call-1",
                "name": "bash",
                "input": {"command": "sleep 10"}
            }]),
        ),
        entry(
            3,
            "system",
            text_content("<turn_aborted>\nThe user interrupted the previous turn on purpose.\n</turn_aborted>"),
        ),
        entry(4, "user", text_content("continue with the current request")),
    ];

    let messages = message_log_entries_to_input_messages(&entries);

    assert_eq!(messages.len(), 5);
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[1].role, "assistant");
    assert_eq!(messages[2].role, "tool");
    assert!(matches!(
        messages[2].content.first(),
        Some(ContentPart::ToolResult { result })
            if result.call_id == "call-1"
                && result.name.as_deref() == Some("bash")
                && result.is_error
                && result.text_content().contains("interrupted")
    ));
    assert_eq!(messages[3].role, "user");
    assert!(input_text(&messages[3]).contains("<turn_aborted>"));
    assert_eq!(messages[4].role, "user");
    assert_eq!(
        input_text(&messages[4]),
        "continue with the current request"
    );
}

#[test]
fn evergreen_projection_input_messages_skip_ui_only_preview_messages() {
    let mut ui_entry = entry(2, "ui", text_content("previous ACP transcript"));
    ui_entry.metadata.insert("ui_only".to_string(), json!(true));
    ui_entry
        .metadata
        .insert("model_visible".to_string(), json!(false));
    ui_entry
        .metadata
        .insert("message_type".to_string(), json!("acp_history_preview"));
    let entries = vec![
        entry(1, "assistant", text_content("previous answer")),
        ui_entry,
        entry(3, "user", text_content("next request")),
    ];

    let messages = message_log_entries_to_input_messages(&entries);

    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, "assistant");
    assert_eq!(input_text(&messages[0]), "previous answer");
    assert_eq!(messages[1].role, "user");
    assert_eq!(input_text(&messages[1]), "next request");
    assert!(!messages
        .iter()
        .any(|message| input_text(message).contains("previous ACP transcript")));
    assert_eq!(
        estimate_log_message_tokens(&[entries[1].clone()]),
        0,
        "UI-only messages must not consume model context budget"
    );
}

#[test]
fn task_notifications_enter_agent_input_as_tool_protocol_only_when_model_visible() {
    let delegated = runtime_note_entry(
        2,
        RuntimeNote {
            runtime_note_id: "delegated-watch".to_string(),
            created_at: "2026-08-26T00:00:00Z".to_string(),
            payload: RuntimeNotePayload::TaskWatchOutput(TaskWatchOutputRuntimeNote {
                task_id: "task-agent".to_string(),
                attempt_no: 1,
                projection: TaskWatchOutputProjection::ActivityOnly,
                records: vec![TaskWatchOutputRecord {
                    source: "acp".to_string(),
                    text: "internal transcript".to_string(),
                }],
            }),
        },
    );
    let tool_watch = runtime_note_entry(
        3,
        RuntimeNote {
            runtime_note_id: "tool-watch".to_string(),
            created_at: "2026-08-26T00:00:01Z".to_string(),
            payload: RuntimeNotePayload::TaskWatchOutput(TaskWatchOutputRuntimeNote {
                task_id: "task-shell".to_string(),
                attempt_no: 1,
                projection: TaskWatchOutputProjection::ModelVisible,
                records: vec![TaskWatchOutputRecord {
                    source: "stdout".to_string(),
                    text: "line one".to_string(),
                }],
            }),
        },
    );
    let terminal = runtime_note_entry(
        4,
        RuntimeNote {
            runtime_note_id: "task-terminal".to_string(),
            created_at: "2026-08-26T00:00:02Z".to_string(),
            payload: RuntimeNotePayload::TaskAttemptTerminal(TaskAttemptTerminalRuntimeNote {
                task_id: "task-agent".to_string(),
                attempt_no: 1,
                terminal_status: "completed".to_string(),
                output: "Outcome\nDelegated work complete".to_string(),
                runner_name: "subagent".to_string(),
                tool_call_id: None,
                full_result_ref: None,
                artifacts: json!([]),
                device: json!({}),
            }),
        },
    );
    let messages = message_log_entries_to_input_messages(&[
        entry(1, "user", text_content("start")),
        delegated,
        tool_watch,
        terminal,
    ]);

    assert_eq!(
        messages
            .iter()
            .map(|message| message.role.as_str())
            .collect::<Vec<_>>(),
        ["user", "assistant", "tool", "assistant", "tool"]
    );
    assert!(!messages
        .iter()
        .any(|message| input_text(message).contains("internal transcript")));
    assert!(matches!(
        messages[1].content.first(),
        Some(ContentPart::ToolUse { call }) if call.name == "Task_Notification"
    ));
    assert!(matches!(
        messages[2].content.first(),
        Some(ContentPart::ToolResult { result })
            if result.text_content().contains("line one")
    ));
    assert!(matches!(
        messages[4].content.first(),
        Some(ContentPart::ToolResult { result })
            if result.text_content().contains("Delegated work complete")
    ));
}

#[test]
fn message_log_text_renders_tool_use_and_tool_result_payloads_for_summary() {
    let tool_use = entry(
        1,
        "assistant",
        json!([{
            "type": "tool_use",
            "id": "call-1",
            "name": "bash",
            "input": {"command": "pwd"}
        }]),
    );
    let tool_result = entry(
        2,
        "system",
        json!([{
            "type": "tool_result",
            "id": "call-1",
            "name": "bash",
            "output": [
                {"type": "text", "text": "workspace path"}
            ],
            "truncation": {"recall_key": "abc123"}
        }]),
    );

    let use_text = message_log_text(&tool_use);
    assert!(use_text.contains("Tool call: bash"));
    assert!(use_text.contains("call-1"));
    assert!(use_text.contains("pwd"));

    let result_text = message_log_text(&tool_result);
    assert!(result_text.contains("Tool result: bash"));
    assert!(result_text.contains("workspace path"));
    assert!(result_text.contains("recall_key=abc123"));
}
