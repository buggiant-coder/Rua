use agent_types::{ContentPart, ModelDialogueItem, Role, ToolCall, ToolResult};
use rua_agent::BoundaryCalculator;

fn user(text: &str) -> ModelDialogueItem {
    ModelDialogueItem::text(Role::User, text)
}

fn assistant(text: &str) -> ModelDialogueItem {
    ModelDialogueItem::text(Role::Assistant, text)
}

fn assistant_tool_calls(calls: &[(&str, &str)]) -> ModelDialogueItem {
    ModelDialogueItem {
        id: None,
        role: Role::Assistant,
        content: calls
            .iter()
            .map(|(id, name)| ContentPart::ToolUse {
                call: ToolCall::new(*id, *name, serde_json::json!({})),
            })
            .collect(),
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    }
}

fn system_tool_result(id: &str, name: &str, text: &str) -> ModelDialogueItem {
    ModelDialogueItem {
        id: None,

        role: Role::System,
        content: vec![ContentPart::ToolResult {
            result: ToolResult {
                call_id: id.to_string(),
                name: Some(name.to_string()),
                content: vec![ContentPart::Text {
                    text: text.to_string(),
                }],
                is_error: false,
                duration_ms: None,
                attachments: None,
                truncation: None,
                metadata: serde_json::Map::new(),
            },
        }],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    }
}

#[test]
fn boundary_protects_tool_use_and_system_tool_result_as_one_unit() {
    let messages = vec![
        ModelDialogueItem::text(Role::System, "system prompt"),
        user("search the repo"),
        assistant_tool_calls(&[("call-search", "grep")]),
        system_tool_result("call-search", "grep", "src/lib.rs: hit"),
        assistant("found one hit"),
        user("next turn"),
    ];

    let plan = BoundaryCalculator::calculate(&messages, 1, 0);

    assert!(
        plan.protected_ranges
            .iter()
            .any(|range| range.start == 2 && range.end == 4),
        "assistant tool_use and following system tool_result must be protected together: {:?}",
        plan.protected_ranges
    );
}

#[test]
fn boundary_protects_multiple_tool_results_from_one_assistant_message() {
    let messages = vec![
        user("inspect files"),
        assistant_tool_calls(&[("call-read", "read"), ("call-grep", "grep")]),
        system_tool_result("call-read", "read", "file contents"),
        system_tool_result("call-grep", "grep", "matching lines"),
        assistant("done"),
    ];

    let plan = BoundaryCalculator::calculate(&messages, 1, 0);

    assert!(
        plan.protected_ranges
            .iter()
            .any(|range| range.start == 1 && range.end == 4),
        "multi-tool assistant turns must keep every matching tool_result in the same protected range: {:?}",
        plan.protected_ranges
    );
}

#[test]
fn keep_range_expands_back_to_tool_use_when_turn_floor_lands_after_it() {
    let messages = vec![
        ModelDialogueItem::text(Role::System, "system prompt"),
        user("old turn"),
        assistant("old answer"),
        user("current turn"),
        assistant_tool_calls(&[("call-read", "read")]),
        system_tool_result("call-read", "read", "current file contents"),
    ];

    let plan = BoundaryCalculator::calculate(&messages, 1, 0);

    assert_eq!(
        plan.keep_range,
        3..6,
        "keeping the latest user turn must include the complete tool_use/tool_result tail"
    );
    assert!(
        plan.protected_ranges
            .iter()
            .any(|range| plan.keep_range.start <= range.start && plan.keep_range.end >= range.end),
        "kept tail should contain whole protected tool pair, got plan: {:?}",
        plan
    );
}

#[test]
fn keep_range_never_contains_only_one_side_of_a_tool_pair() {
    let messages = vec![
        ModelDialogueItem::text(Role::System, "system prompt"),
        user("old turn"),
        assistant("old answer"),
        user("current turn"),
        assistant_tool_calls(&[("call-grep", "grep")]),
        system_tool_result("call-grep", "grep", "grep output"),
        assistant("final response"),
    ];

    let plan = BoundaryCalculator::calculate(&messages, 1, 0);
    let kept = &messages[plan.keep_range.clone()];

    let kept_tool_uses: Vec<_> = kept
        .iter()
        .flat_map(ModelDialogueItem::tool_calls)
        .map(|call| call.id.as_str())
        .collect();
    let kept_tool_results: Vec<_> = kept
        .iter()
        .flat_map(ModelDialogueItem::tool_results)
        .map(|result| result.call_id.as_str())
        .collect();

    assert_eq!(
        kept_tool_uses, kept_tool_results,
        "compaction keep range must not split tool_use/tool_result pairs"
    );
}

#[test]
fn token_reserve_boundary_expands_to_entire_multi_tool_turn() {
    let huge_result = "large search output ".repeat(300);
    let messages = vec![
        ModelDialogueItem::text(Role::System, "system prompt"),
        user("old turn"),
        assistant("old answer"),
        user("inspect the workspace"),
        assistant_tool_calls(&[("call-read", "read"), ("call-grep", "grep")]),
        system_tool_result("call-read", "read", "small file contents"),
        system_tool_result("call-grep", "grep", &huge_result),
        assistant("done"),
    ];

    let plan = BoundaryCalculator::calculate(&messages, 0, 64);

    assert_eq!(
        plan.keep_range,
        4..8,
        "when the token reserve lands inside a large tool_result, Rust must keep the whole tool turn"
    );

    let kept = &messages[plan.keep_range.clone()];
    let kept_tool_uses: Vec<_> = kept
        .iter()
        .flat_map(ModelDialogueItem::tool_calls)
        .map(|call| call.id.as_str())
        .collect();
    let kept_tool_results: Vec<_> = kept
        .iter()
        .flat_map(ModelDialogueItem::tool_results)
        .map(|result| result.call_id.as_str())
        .collect();

    assert_eq!(
        kept_tool_uses, kept_tool_results,
        "QwenPaw-inspired token reserve compaction must not keep orphan tool_result blocks"
    );
}
