use rua_core::RuntimeEvent;
use rua_server::services::console_chat::{
    chat_event_to_runtime_event, map_runtime_event_to_chat_events,
};

#[test]
fn roundtrip_preserves_tool_approval_pending() {
    let pending = RuntimeEvent::ToolApprovalPending {
        call_id: "call-1".into(),
        tool_name: "bash".into(),
        request_id: Some("approval-123".into()),
        prompt: Some("Bash requires confirmation".into()),
    };

    let chat_events = map_runtime_event_to_chat_events(pending, "chat-1", "agent-1");

    let approval_events: Vec<_> = chat_events
        .iter()
        .filter(|e| {
            matches!(
                e,
                rua_server::endpoints::dto::ChatEvent::ToolApprovalRequest { .. }
            )
        })
        .collect();
    assert_eq!(
        approval_events.len(),
        1,
        "go trip should produce exactly one ToolApprovalRequest ChatEvent"
    );

    let approval_chat_event = approval_events[0].clone();
    let runtime_event = chat_event_to_runtime_event(approval_chat_event);

    match &runtime_event {
        RuntimeEvent::ToolApprovalPending {
            call_id,
            tool_name,
            request_id,
            prompt,
        } => {
            assert_eq!(call_id, "call-1");
            assert_eq!(tool_name, "bash");
            assert_eq!(request_id.as_deref(), Some("approval-123"));
            assert_eq!(prompt.as_deref(), Some("Bash requires confirmation"));
        }
        RuntimeEvent::TextDelta { delta, .. } => {
            panic!(
                "ToolApprovalRequest was swallowed into empty TextDelta (delta={:?}); \
                 this is the exact bug this feat fixes",
                delta
            );
        }
        other => {
            panic!(
                "expected RuntimeEvent::ToolApprovalRequest, got {:?}",
                other
            );
        }
    }
}

#[test]
fn roundtrip_tool_approval_request_runtime_event() {
    let original = RuntimeEvent::ToolApprovalRequest {
        id: "park-456".into(),
        name: "edit".into(),
        arguments: serde_json::json!({"path": "/etc/passwd"}),
    };

    let chat_events = map_runtime_event_to_chat_events(original, "chat-2", "agent-2");
    assert_eq!(chat_events.len(), 1);

    let runtime_event = chat_event_to_runtime_event(chat_events.into_iter().next().unwrap());
    match &runtime_event {
        RuntimeEvent::ToolApprovalRequest {
            id,
            name,
            arguments,
        } => {
            assert_eq!(id, "park-456");
            assert_eq!(name, "edit");
            assert_eq!(arguments, &serde_json::json!({"path": "/etc/passwd"}));
        }
        other => {
            panic!(
                "expected RuntimeEvent::ToolApprovalRequest, got {:?}",
                other
            );
        }
    }
}
