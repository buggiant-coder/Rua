use clap::Parser;
use rua_cli::agent::{
    agent_chat_endpoint, build_agent_chat_request, extract_final_agent_chat_text,
    extract_text_from_content, parse_sse_line,
};
use rua_cli::{Cli, Commands};
use serde_json::json;

#[test]
fn chat_uses_agent_scoped_console_endpoint() {
    assert_eq!(
        agent_chat_endpoint("bot/b", "chat/one"),
        "/api/agents/bot%2Fb/chats/chat%2Fone/console/chat"
    );
}

#[test]
fn chat_request_uses_explicit_chat_and_identity_prefix() {
    let chat_id = "d1f2bb37-469f-4336-8d20-e7a5e0b006cd";
    let request = build_agent_chat_request("hello", "bot_a", "bot_b", Some(chat_id.into()));

    assert_eq!(request.chat_id, chat_id);
    assert!(request.prefix_added);
    assert_eq!(
        request.body,
        json!({
            "chat_id": chat_id,
            "input": [{
                "role": "user",
                "content": [{
                    "type": "text",
                    "text": "[Agent bot_a requesting] hello"
                }]
            }]
        })
    );
}

#[test]
fn chat_request_preserves_existing_identity_prefixes() {
    for message in [
        "[Agent bot_a requesting] hello",
        "[来自智能体 bot_a 请求] hello",
    ] {
        let request = build_agent_chat_request(
            message,
            "bot_a",
            "bot_b",
            Some("d1f2bb37-469f-4336-8d20-e7a5e0b006cd".into()),
        );

        assert!(!request.prefix_added);
        assert_eq!(request.body["input"][0]["content"][0]["text"], message);
    }
}

#[test]
fn chat_request_generates_uuid_chat_id_when_missing() {
    let request = build_agent_chat_request("hello", "bot_a", "bot_b", None);

    assert!(
        uuid::Uuid::parse_str(&request.chat_id).is_ok(),
        "generated Chat ID must be a UUID"
    );
    assert_eq!(request.body["chat_id"], request.chat_id);
}

#[test]
fn chat_cli_help_uses_chat_id_and_no_new_session_flag() {
    let err = Cli::try_parse_from(["rua", "agent", "chat", "--help"]).unwrap_err();
    let help = err.to_string();

    assert!(help.contains("AGENT_ID"));
    assert!(help.contains("MESSAGE"));
    assert!(help.contains("--from-agent"));
    assert!(help.contains("--chat-id"));
    assert!(!help.contains("--session-id"));
    assert!(!help.contains("--new-session"));
    assert!(!help.contains("--to-agent"));
    assert!(!help.contains("--text"));
}

#[test]
fn chat_cli_parses_final_json_request_shape_flags() {
    let cli = Cli::try_parse_from([
        "rua",
        "--agent-id",
        "bot_a",
        "agent",
        "chat",
        "bot_b",
        "hello",
        "--chat-id",
        "d1f2bb37-469f-4336-8d20-e7a5e0b006cd",
        "--mode",
        "final",
        "--json-output",
    ])
    .expect("agent chat final json flags should parse");

    assert!(matches!(cli.command, Commands::Agent(_)));
    assert_eq!(cli.agent_id, "bot_a");
}

#[test]
fn chat_cli_from_agent_overrides_global_caller_for_request_body() {
    let cli = Cli::try_parse_from([
        "rua",
        "--agent-id",
        "global_bot",
        "agent",
        "chat",
        "target_bot",
        "hello",
        "--from-agent",
        "explicit_bot",
    ])
    .expect("agent chat should parse Rust positional target/message plus --from-agent override");

    assert!(matches!(cli.command, Commands::Agent(_)));
    assert_eq!(cli.agent_id, "global_bot");

    let request = build_agent_chat_request("hello", "explicit_bot", "target_bot", None);
    assert_eq!(
        request.body["input"][0]["content"][0]["text"],
        "[Agent explicit_bot requesting] hello"
    );
}

#[test]
fn chat_cli_registers_stream_mode_value() {
    let cli = Cli::try_parse_from(["rua", "agent", "chat", "bot_b", "hello", "--mode", "stream"])
        .expect("agent chat stream mode should parse");
    assert!(matches!(cli.command, Commands::Agent(_)));
}

#[test]
fn sse_line_parser_accepts_data_json_and_ignores_done_or_invalid_lines() {
    assert_eq!(
        parse_sse_line(r#"data: {"object":"response"}"#),
        Some(json!({"object": "response"}))
    );
    assert_eq!(parse_sse_line("data: [DONE]"), None);
    assert_eq!(parse_sse_line("event: message"), None);
    assert_eq!(parse_sse_line("data: {not-json}"), None);
}

#[test]
fn text_extraction_reads_first_text_content_block() {
    let event = json!({
        "content": [
            {"type": "image", "url": "https://example.test/a.png"},
            {"type": "text", "text": "first text"},
            {"type": "text", "text": "second text"}
        ]
    });

    assert_eq!(extract_text_from_content(&event), Some("first text".into()));
    assert_eq!(extract_text_from_content(&json!({"content": []})), None);
}

#[test]
fn final_text_uses_latest_completed_message_event() {
    let events = vec![
        json!({
            "object": "message",
            "type": "message",
            "status": "completed",
            "content": [{"type": "text", "text": "old reply"}]
        }),
        json!({
            "object": "message",
            "type": "message",
            "status": "in_progress",
            "content": [{"type": "text", "text": "partial"}]
        }),
        json!({
            "object": "message",
            "type": "message",
            "status": "completed",
            "content": [{"type": "text", "text": "final reply"}]
        }),
        json!({"object": "response"}),
    ];

    assert_eq!(extract_final_agent_chat_text(&events), "final reply");
}

#[test]
fn final_text_returns_empty_when_stream_has_only_errors_or_partial_messages() {
    let events = vec![
        json!({
            "object": "message",
            "type": "message",
            "status": "in_progress",
            "content": [{"type": "text", "text": "partial reply"}]
        }),
        json!({
            "object": "error",
            "error": "agent unavailable"
        }),
        json!({"object": "response"}),
    ];

    assert_eq!(extract_final_agent_chat_text(&events), "");
}

#[test]
fn final_text_ignores_completed_messages_without_text_content() {
    let events = vec![
        json!({
            "object": "message",
            "type": "message",
            "status": "completed",
            "content": [{"type": "image", "url": "file://plot.png"}]
        }),
        json!({"object": "response"}),
    ];

    assert_eq!(extract_final_agent_chat_text(&events), "");
}
