use rua_channel::core::approval_card::{ApprovalCardAction, ApprovalCardParseError};
use rua_channel::wecom::approval_card::{
    build_approval_card, build_resolved_card, parse_card_event, TASK_ID_PREFIX,
};

#[test]
fn approval_card_uses_wecom_button_interaction_shape() {
    let card = build_approval_card(
        "req-1",
        "Bash",
        "high",
        serde_json::json!({
            "session_id": "wecom:user-1",
            "sender_id": "user-1"
        }),
    )
    .expect("card");

    assert_eq!(card["card_type"], "button_interaction");
    assert_eq!(card["task_id"], format!("{TASK_ID_PREFIX}req-1"));
    assert_eq!(card["main_title"]["title"], "🛡️ Tool Approval Required");
    assert_eq!(card["main_title"]["desc"], "Bash | high");
    assert_eq!(card["button_list"][0]["text"], "Approve");
    assert_eq!(card["button_list"][0]["style"], 1);
    assert_eq!(card["button_list"][1]["text"], "Deny");
    assert_eq!(card["button_list"][1]["style"], 2);

    let approve_key: serde_json::Value =
        serde_json::from_str(card["button_list"][0]["key"].as_str().unwrap()).unwrap();
    assert_eq!(approve_key["a"], "approve");
    assert_eq!(approve_key["rid"], "req-1");
    assert_eq!(approve_key["tool"], "Bash");
    assert_eq!(approve_key["sev"], "high");
    assert_eq!(approve_key["session_id"], "wecom:user-1");
    assert_eq!(approve_key["sender_id"], "user-1");
}

#[test]
fn parse_card_event_extracts_command_context() {
    let key = serde_json::json!({
        "a": "deny",
        "rid": "req-2",
        "tool": "Read",
        "sev": "medium",
        "session_id": "wecom:user-1",
        "sender_id": "user-1"
    })
    .to_string();
    let event = serde_json::json!({
        "event": {
            "template_card_event": {
                "task_id": "tg_approval_req-2",
                "event_key": key
            }
        },
        "from": {"userid": "manager-1"}
    });

    let parsed = parse_card_event(&event).expect("card event");
    assert_eq!(parsed.request_id, "req-2");
    assert_eq!(parsed.action, ApprovalCardAction::Deny);
    assert_eq!(parsed.task_id, "tg_approval_req-2");
    assert_eq!(parsed.tool_name, "Read");
    assert_eq!(parsed.user_id, "manager-1");
    assert_eq!(parsed.session_ctx["session_id"], "wecom:user-1");
    assert_eq!(parsed.command_text(), "/approval deny req-2");
}

#[test]
fn parse_card_event_rejects_bad_payloads() {
    assert_eq!(
        parse_card_event(&serde_json::json!({
            "event": {"template_card_event": {"event_key": "not-json"}}
        }))
        .unwrap_err(),
        ApprovalCardParseError::Malformed
    );
    assert_eq!(
        parse_card_event(&serde_json::json!({
            "event": {"template_card_event": {"event_key": "{\"a\":\"approve\"}"}}
        }))
        .unwrap_err(),
        ApprovalCardParseError::MissingRequestId
    );
}

#[test]
fn resolved_card_uses_text_notice_shape_required_by_wecom() {
    let card = build_resolved_card(
        "tg_approval_req-1",
        "Bash",
        ApprovalCardAction::Approve,
        "alice",
    );

    assert_eq!(card["card_type"], "text_notice");
    assert_eq!(card["task_id"], "tg_approval_req-1");
    assert_eq!(card["main_title"]["title"], "✅ Approved");
    assert_eq!(card["main_title"]["desc"], "by alice\nBash");
    assert_eq!(card["card_action"]["type"], 1);
    assert_eq!(card["card_action"]["url"], "https://qwenpaw.agentscope.io");
}
