use clap::Parser;
use rua_cli::chat::{
    build_chat_body, build_response_json, chat_endpoint, chat_stream_endpoint,
    format_work_checklist,
};
use rua_cli::console::build_create_chat_body;
use rua_cli::Cli;
use serde_json::json;

#[test]
fn chat_json_response_exposes_chat_id() {
    let response = build_response_json(&[
        json!({
            "object": "message",
            "type": "message",
            "status": "completed",
            "content": [{"type": "text", "text": "done"}]
        }),
        json!({
            "object": "response",
            "status": "completed",
            "chat_id": "d1f2bb37-469f-4336-8d20-e7a5e0b006cd"
        }),
    ]);

    assert_eq!(
        response,
        json!({
            "text": "done",
            "chat_id": "d1f2bb37-469f-4336-8d20-e7a5e0b006cd"
        })
    );
    assert!(response.get("session_id").is_none());
}

#[test]
fn chat_uses_agent_scoped_single_request_endpoint() {
    assert_eq!(
        chat_stream_endpoint("agent/one", "chat/one"),
        "/api/agents/agent%2Fone/chats/chat%2Fone/console/chat"
    );
}

#[test]
fn chat_mode_commands_and_request_body_are_typed() {
    for args in [
        vec!["rua", "chat", "send", "hello", "--mode", "execute"],
        vec!["rua", "chat", "stream", "hello", "--mode", "plan"],
        vec![
            "rua",
            "chat",
            "update",
            "00000000-0000-4000-8000-000000000420",
            "--mode",
            "default",
        ],
    ] {
        Cli::try_parse_from(args).expect("typed Agent Collaboration Mode should parse");
    }
    assert!(Cli::try_parse_from(["rua", "chat", "send", "hello", "--mode", "invalid",]).is_err());
    assert_eq!(
        build_chat_body(
            "hello",
            Some("00000000-0000-4000-8000-000000000420"),
            None,
            Some(rua_core::AgentCollaborationMode::Execute),
        ),
        json!({
            "message": "hello",
            "chat_id": "00000000-0000-4000-8000-000000000420",
            "collaboration_mode": "execute",
        })
    );
    assert_eq!(
        chat_endpoint("default", "chat/one"),
        "/api/agents/default/chats/chat%2Fone"
    );
}

#[test]
fn chat_json_response_accepts_delta_output_and_terminal_chat_id() {
    let response = build_response_json(&[
        json!({"type": "text_delta", "delta": "hel"}),
        json!({"type": "text_delta", "delta": "lo"}),
        json!({
            "type": "turn_terminal",
            "status": "completed",
            "chat_id": "00000000-0000-4000-8000-000000000405"
        }),
    ]);

    assert_eq!(
        response,
        json!({
            "text": "hello",
            "chat_id": "00000000-0000-4000-8000-000000000405"
        })
    );
}

#[test]
fn chat_json_response_accepts_webui_content_deltas() {
    let response = build_response_json(&[
        json!({
            "object": "message",
            "status": "in_progress",
            "type": "message",
            "role": "assistant",
            "id": "msg-answer",
            "content": null
        }),
        json!({
            "object": "content",
            "status": "in_progress",
            "type": "text",
            "delta": true,
            "msg_id": "msg-answer",
            "text": "hel"
        }),
        json!({
            "object": "content",
            "status": "in_progress",
            "type": "text",
            "delta": true,
            "msg_id": "msg-answer",
            "text": "lo"
        }),
        json!({
            "type": "turn_terminal",
            "status": "completed",
            "chat_id": "00000000-0000-4000-8000-000000000406"
        }),
    ]);

    assert_eq!(
        response,
        json!({
            "text": "hello",
            "chat_id": "00000000-0000-4000-8000-000000000406"
        })
    );
}

#[test]
fn chat_json_response_projects_the_latest_valid_work_checklist() {
    let response = build_response_json(&[
        json!({
            "type": "tool_call",
            "id": "plan-1",
            "name": "update_plan",
            "arguments": serde_json::json!({
                "explanation": "Starting investigation.",
                "plan": [
                    {"step": "Old step", "status": "in_progress"}
                ]
            }).to_string()
        }),
        json!({
            "object": "message",
            "type": "plugin_call",
            "role": "assistant",
            "content": [{
                "type": "data",
                "data": {
                    "id": "plan-2",
                    "name": "update_plan",
                    "arguments": {
                        "explanation": "Implementation is complete.",
                        "plan": [
                            {"step": "Implement the change", "status": "completed"},
                            {"step": "Run focused tests", "status": "in_progress"}
                        ]
                    }
                }
            }]
        }),
        json!({
            "object": "message",
            "type": "message",
            "status": "completed",
            "content": [{"type": "text", "text": "done"}]
        }),
        json!({
            "type": "turn_terminal",
            "status": "completed",
            "chat_id": "00000000-0000-4000-8000-000000000407"
        }),
    ]);

    assert_eq!(response["text"], json!("done"));
    assert_eq!(
        response["work_checklist"],
        json!({
            "explanation": "Implementation is complete.",
            "plan": [
                {"step": "Implement the change", "status": "completed"},
                {"step": "Run focused tests", "status": "in_progress"}
            ]
        })
    );
    assert_eq!(
        format_work_checklist(&response["work_checklist"]),
        "Plan\n  Implementation is complete.\n  [x] Implement the change\n  [>] Run focused tests"
    );
}

#[test]
fn chats_create_has_no_session_id_input() {
    Cli::try_parse_from([
        "rua",
        "chats",
        "create",
        "--user-id",
        "default",
        "--name",
        "CLI Chat",
    ])
    .expect("Chat creation should not require a second Session identity");

    let help = Cli::try_parse_from(["rua", "chats", "create", "--help"])
        .unwrap_err()
        .to_string();
    assert!(!help.contains("--session-id"));
}

#[test]
fn chats_create_body_contains_only_chat_metadata() {
    let body = build_create_chat_body("CLI Chat", "default", "console");

    assert_eq!(
        body,
        json!({
            "name": "CLI Chat",
            "user_id": "default",
            "channel": "console"
        })
    );
    assert!(body.get("session_id").is_none());
}
