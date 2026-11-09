use agent_types::{AgentEvent, ToolResult};
use react_agent::agent_event_to_runtime_events;
use rua_core::event::RuntimeEvent;
use rua_core::id::SessionId;

#[test]
fn ask_user_tool_result_emits_answer_request_before_end() {
    let mut result = ToolResult::text("call-ask", "Pick one");
    result.name = Some("ask_user".to_string());
    result.metadata.insert(
        "_ask_user".to_string(),
        serde_json::json!({
            "request_id": "request-ask",
            "questions": [{
                "question": "Pick one",
                "options": ["A", "B"],
                "multi_select": false
            }]
        }),
    );

    let events = agent_event_to_runtime_events(
        AgentEvent::ToolResult {
            result: result.clone(),
        },
        &SessionId::new("session-1"),
    );

    assert_eq!(events.len(), 2);
    assert!(matches!(
        &events[0],
        RuntimeEvent::ToolResult { result: mapped } if mapped.call_id == result.call_id
    ));
    match &events[1] {
        RuntimeEvent::AskUser {
            request_id,
            questions,
        } => {
            assert_eq!(request_id, "request-ask");
            assert_eq!(questions.len(), 1);
            assert_eq!(questions[0].question, "Pick one");
            assert_eq!(questions[0].options, ["A", "B"]);
            assert!(!questions[0].multi_select);
        }
        other => panic!("expected AskUser event, got {other:?}"),
    }
}
