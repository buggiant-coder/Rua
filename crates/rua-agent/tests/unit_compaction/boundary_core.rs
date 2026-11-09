use agent_types::{ContentPart, ModelDialogueItem, Role, ToolCall, ToolResult};
use rua_agent::BoundaryCalculator;

fn sys(text: &str) -> ModelDialogueItem {
    ModelDialogueItem::text(Role::System, text)
}

fn user(text: &str) -> ModelDialogueItem {
    ModelDialogueItem::text(Role::User, text)
}

fn asst(text: &str) -> ModelDialogueItem {
    ModelDialogueItem::text(Role::Assistant, text)
}

fn asst_tool_call(call_id: &str, name: &str) -> ModelDialogueItem {
    ModelDialogueItem {
        id: None,
        role: Role::Assistant,
        content: vec![ContentPart::ToolUse {
            call: ToolCall::new(call_id, name, serde_json::json!({})),
        }],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    }
}

fn tool_result(call_id: &str, text: &str) -> ModelDialogueItem {
    ModelDialogueItem {
        id: None,
        role: Role::Tool,
        content: vec![ContentPart::ToolResult {
            result: ToolResult::text(call_id, text),
        }],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    }
}

#[test]
fn boundary_empty_messages() {
    let plan = BoundaryCalculator::calculate(&[], 4, 0);

    assert!(plan.compact_range.is_empty());
    assert!(plan.keep_range.is_empty());
    assert_eq!(plan.reason, "empty messages");
}

#[test]
fn boundary_preserves_system_messages() {
    let messages = vec![
        sys("You are helpful"),
        user("Hi"),
        asst("Hello"),
        user("Bye"),
    ];

    let plan = BoundaryCalculator::calculate(&messages, 1, 0);

    assert_eq!(plan.system_range, 0..1);
    assert!(plan.keep_range.start >= 1);
}

#[test]
fn boundary_protects_tool_call_result_pairs() {
    let messages = vec![
        sys("system"),
        user("turn 1"),
        asst_tool_call("c1", "read"),
        tool_result("c1", "file contents"),
        asst("Here is the file"),
        user("turn 2"),
        asst("response 2"),
    ];

    let plan = BoundaryCalculator::calculate(&messages, 1, 0);

    assert!(
        plan.protected_ranges
            .iter()
            .any(|range| range.contains(&2) && range.contains(&3)),
        "tool call/result pair must be protected"
    );
}

#[test]
fn boundary_keeps_last_n_turns() {
    let messages = vec![
        sys("system"),
        user("turn 1"),
        asst("resp 1"),
        user("turn 2"),
        asst("resp 2"),
        user("turn 3"),
        asst("resp 3"),
    ];

    let plan = BoundaryCalculator::calculate(&messages, 2, 0);
    let kept = &messages[plan.keep_range.clone()];
    let user_turns = kept
        .iter()
        .filter(|message| message.role == Role::User)
        .count();

    assert!(
        user_turns >= 2,
        "must keep at least 2 user turns, got {user_turns}"
    );
}
