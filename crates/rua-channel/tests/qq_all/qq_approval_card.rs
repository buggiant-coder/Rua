use rua_channel::core::approval_card::{ApprovalCardAction, ApprovalCardParseError};
use rua_channel::qq::approval_card::{
    build_approval_message_body, build_resolved_text, parse_interaction_event, ACTION_DATA_PREFIX,
};

#[test]
fn approval_card_uses_qq_markdown_keyboard_shape() {
    let body = build_approval_message_body(
        "req-123456789",
        "Bash",
        "high",
        "Approve Bash?",
        serde_json::json!({
            "sid": "group:grp-1",
            "sender": "member-1",
            "mt": "group",
            "goid": "grp-1",
            "mid": "msg-1"
        }),
    )
    .expect("approval body");

    assert_eq!(body["msg_type"], 2);
    assert_eq!(body["markdown"]["content"], "Approve Bash?");
    let buttons = &body["keyboard"]["content"]["rows"][0]["buttons"];
    assert_eq!(buttons[0]["id"], "approve_req-1234");
    assert_eq!(buttons[0]["render_data"]["label"], "✅ Approve");
    assert_eq!(buttons[0]["action"]["type"], 1);
    assert_eq!(buttons[0]["action"]["permission"]["type"], 2);

    let approve_data: serde_json::Value =
        serde_json::from_str(buttons[0]["action"]["data"].as_str().unwrap()).unwrap();
    assert_eq!(approve_data["p"], ACTION_DATA_PREFIX);
    assert_eq!(approve_data["a"], "approve");
    assert_eq!(approve_data["rid"], "req-123456789");
    assert_eq!(approve_data["tool"], "Bash");
    assert_eq!(approve_data["sev"], "high");
    assert_eq!(approve_data["sid"], "group:grp-1");
    assert_eq!(approve_data["sender"], "member-1");
    assert_eq!(approve_data["mt"], "group");
    assert_eq!(approve_data["goid"], "grp-1");
    assert_eq!(approve_data["mid"], "msg-1");
}

#[test]
fn parse_interaction_event_extracts_command_context() {
    let button_data = serde_json::json!({
        "p": ACTION_DATA_PREFIX,
        "a": "deny",
        "rid": "req-2",
        "tool": "Read",
        "sev": "medium",
        "sid": "user-1",
        "sender": "user-1",
        "mt": "c2c"
    })
    .to_string();

    let parsed = parse_interaction_event(&serde_json::json!({
        "id": "interaction-1",
        "user_openid": "operator-1",
        "data": {
            "resolved": {
                "button_data": button_data
            }
        }
    }))
    .expect("interaction");

    assert_eq!(parsed.request_id, "req-2");
    assert_eq!(parsed.action, ApprovalCardAction::Deny);
    assert_eq!(parsed.tool_name, "Read");
    assert_eq!(parsed.operator_id, "operator-1");
    assert_eq!(parsed.interaction_id, "interaction-1");
    assert_eq!(parsed.session_ctx["sid"], "user-1");
    assert_eq!(parsed.command_text(), "/approval deny req-2");
}

#[test]
fn parse_interaction_event_rejects_bad_payloads() {
    assert_eq!(
        parse_interaction_event(&serde_json::json!({
            "data": {"resolved": {"button_data": "not-json"}}
        }))
        .unwrap_err(),
        ApprovalCardParseError::Malformed
    );
    assert_eq!(
        parse_interaction_event(&serde_json::json!({
            "data": {"resolved": {"button_data": "{\"p\":\"tg_\",\"a\":\"approve\"}"}}
        }))
        .unwrap_err(),
        ApprovalCardParseError::MissingRequestId
    );
}

#[test]
fn resolved_text_matches_qwenpaw_status_message() {
    assert_eq!(
        build_resolved_text("Bash", ApprovalCardAction::Approve, "ator_1"),
        "✅ **Approved** by **ator_1**\nTool: `Bash`"
    );
    assert_eq!(
        build_resolved_text("Read", ApprovalCardAction::Deny, ""),
        "🚫 **Denied**\nTool: `Read`"
    );
}
