use rua_channel::core::approval_card::ApprovalCardParseError;
use rua_channel::core::ApprovalCardAction;
use rua_channel::feishu::approval_card::{
    build_approval_card, build_callback_response, build_resolved_card, build_toast,
    parse_action_value, TOOL_GUARD_ACTION_TYPE,
};

#[test]
fn approval_card_contains_qwenpaw_compatible_buttons_and_values() {
    let card: serde_json::Value = serde_json::from_str(&build_approval_card(
        "req-1",
        "Bash",
        "high",
        "Run rm -rf?",
        serde_json::json!({"chat_id": "chat-1"}),
    ))
    .unwrap();

    assert_eq!(card["header"]["template"], "red");
    assert_eq!(
        card["header"]["title"]["content"],
        "🛡️ Tool Approval Required"
    );
    assert_eq!(card["elements"][0]["tag"], "markdown");
    assert_eq!(card["elements"][0]["content"], "Run rm -rf?");
    let actions = &card["elements"][2]["actions"];
    assert_eq!(actions[0]["text"]["content"], "✅ Approve");
    assert_eq!(actions[0]["type"], "primary");
    assert_eq!(actions[0]["value"]["type"], TOOL_GUARD_ACTION_TYPE);
    assert_eq!(actions[0]["value"]["action"], "approve");
    assert_eq!(actions[0]["value"]["request_id"], "req-1");
    assert_eq!(actions[0]["value"]["tool_name"], "Bash");
    assert_eq!(actions[0]["value"]["session_ctx"]["chat_id"], "chat-1");
    assert_eq!(actions[1]["text"]["content"], "❌ Deny");
    assert_eq!(actions[1]["type"], "danger");
    assert_eq!(actions[1]["value"]["action"], "deny");
}

#[test]
fn parse_action_value_extracts_shared_approval_action() {
    let value = serde_json::json!({
        "type": TOOL_GUARD_ACTION_TYPE,
        "action": "approve",
        "request_id": "req-2",
        "tool_name": "Read",
        "body": "Read secret",
        "session_ctx": {"session": "s1"}
    });

    let parsed = parse_action_value(&value).unwrap();
    assert_eq!(parsed.request_id, "req-2");
    assert_eq!(parsed.action, ApprovalCardAction::Approve);
    assert_eq!(parsed.tool_name, "Read");
    assert_eq!(parsed.body, "Read secret");
    assert_eq!(parsed.session_ctx["session"], "s1");
    assert_eq!(parsed.command_text(), "/approval approve req-2");
}

#[test]
fn parse_action_value_rejects_missing_request_id_and_unknown_action() {
    assert_eq!(
        parse_action_value(&serde_json::json!({
            "type": TOOL_GUARD_ACTION_TYPE,
            "action": "approve",
            "request_id": ""
        }))
        .unwrap_err(),
        ApprovalCardParseError::MissingRequestId
    );
    assert_eq!(
        parse_action_value(&serde_json::json!({
            "type": TOOL_GUARD_ACTION_TYPE,
            "action": "maybe",
            "request_id": "req"
        }))
        .unwrap_err(),
        ApprovalCardParseError::UnknownAction
    );
}

#[test]
fn resolved_card_and_toast_match_action_status() {
    let approved: serde_json::Value = serde_json::from_str(&build_resolved_card(
        "Bash",
        ApprovalCardAction::Approve,
        "alice",
        "Run command",
    ))
    .unwrap();
    assert_eq!(approved["header"]["template"], "green");
    assert_eq!(approved["header"]["title"]["content"], "✅ Approved");
    assert_eq!(
        approved["elements"][2]["content"],
        "Tool `Bash` approved by `alice`."
    );

    let denied: serde_json::Value = serde_json::from_str(&build_resolved_card(
        "Bash",
        ApprovalCardAction::Deny,
        "",
        "",
    ))
    .unwrap();
    assert_eq!(denied["header"]["template"], "red");
    assert_eq!(denied["header"]["title"]["content"], "🚫 Denied");
    assert_eq!(denied["elements"][0]["content"], "Tool `Bash` denied.");

    assert_eq!(
        build_toast(ApprovalCardAction::Approve, "Bash"),
        serde_json::json!({"type": "success", "content": "Approved tool Bash"})
    );
    assert_eq!(
        build_toast(ApprovalCardAction::Deny, "Bash"),
        serde_json::json!({"type": "info", "content": "Denied tool Bash"})
    );
}

#[test]
fn callback_response_wraps_toast_and_raw_resolved_card() {
    let action = parse_action_value(&serde_json::json!({
        "type": TOOL_GUARD_ACTION_TYPE,
        "action": "approve",
        "request_id": "req-3",
        "tool_name": "Bash",
        "body": "Run command",
        "session_ctx": {"session": "s1"}
    }))
    .unwrap();

    let response = build_callback_response(&action, "alice").expect("callback response");

    assert_eq!(
        response["toast"],
        serde_json::json!({"type": "success", "content": "Approved tool Bash"})
    );
    assert_eq!(response["card"]["type"], "raw");
    let card = &response["card"]["data"];
    assert_eq!(card["header"]["template"], "green");
    assert_eq!(card["header"]["title"]["content"], "✅ Approved");
    assert_eq!(card["elements"][0]["content"], "Run command");
    assert_eq!(
        card["elements"][2]["content"],
        "Tool `Bash` approved by `alice`."
    );
}
