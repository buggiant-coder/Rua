use rua_core::event::RuntimeEvent;
use rua_core::tool::ToolResult;

use rua_server::services::console_chat::map_runtime_event_to_chat_events;

#[test]
fn tool_approval_pending_emits_ui_card() {
    let event = RuntimeEvent::ToolApprovalPending {
        call_id: "call-1".to_string(),
        tool_name: "bash".to_string(),
        request_id: Some("req-abc".to_string()),
        prompt: Some("Bash requires confirmation".to_string()),
    };

    let chat_events = map_runtime_event_to_chat_events(event, "chat-1", "agent-1");

    assert_eq!(chat_events.len(), 1);
    let serialized = serde_json::to_value(&chat_events[0]).unwrap();
    assert_eq!(serialized["type"], "tool_approval_request");
    assert_eq!(serialized["id"], "req-abc");
    assert_eq!(serialized["content"][0]["data"]["call_id"], "call-1");
    assert_eq!(serialized["content"][0]["data"]["status"], "waiting");
}

#[test]
fn normal_tool_result_emits_tool_result_block() {
    let mut result = ToolResult::text("call-2", "file deleted");
    result.name = Some("bash".into());

    let event = RuntimeEvent::ToolResult { result };
    let chat_events = map_runtime_event_to_chat_events(event, "chat-1", "agent-1");

    assert!(!chat_events.is_empty());
    let serialized = serde_json::to_value(&chat_events[0]).unwrap();
    assert_eq!(
        serialized["type"], "tool_result",
        "normal result should emit tool_result"
    );
    assert_eq!(serialized["id"], "call-2");
    assert_eq!(serialized["content"], "file deleted");
}

#[test]
fn approved_result_same_call_id_emits_single_tool_result() {
    let mut result = ToolResult::text("call-1", "actual output from rm -rf");
    result.name = Some("bash".into());

    let event = RuntimeEvent::ToolResult { result };
    let chat_events = map_runtime_event_to_chat_events(event, "chat-1", "agent-1");

    assert_eq!(chat_events.len(), 1);
    let serialized = serde_json::to_value(&chat_events[0]).unwrap();
    assert_eq!(serialized["type"], "tool_result");
    assert_eq!(serialized["id"], "call-1");

    let content = &serialized["content"];
    assert_eq!(content, "actual output from rm -rf");
}
