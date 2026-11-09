use agent_client_protocol::schema::v1::{
    ContentBlock, SessionUpdate, ToolCallContent, ToolCallStatus,
};
use rua_acp::server::v1::{available_commands, EventMapper};
use rua_acp::server::ACP_ERROR_META_KEY;
use rua_core::event::RuntimeEvent;

fn session_id() -> agent_client_protocol::schema::v1::SessionId {
    agent_client_protocol::schema::v1::SessionId::new("test-session")
}

fn notification(
    event: RuntimeEvent,
) -> Option<agent_client_protocol::schema::v1::SessionNotification> {
    EventMapper::default()
        .map(&session_id(), event)
        .notification
}

#[test]
fn text_delta_maps_to_agent_message_chunk() {
    let event = RuntimeEvent::TextDelta {
        delta: "hello world".to_string(),
        message_id: None,
    };

    let notif = notification(event).unwrap();
    assert_eq!(notif.session_id, session_id());

    match notif.update {
        SessionUpdate::AgentMessageChunk(chunk) => match chunk.content {
            ContentBlock::Text(text) => assert_eq!(text.text, "hello world"),
            other => panic!("expected Text, got {:?}", other),
        },
        other => panic!("expected AgentMessageChunk, got {:?}", other),
    }
}

#[test]
fn thinking_delta_maps_to_agent_thought_chunk() {
    let event = RuntimeEvent::ThinkingDelta {
        delta: "let me think...".to_string(),
        signature: Some("sig".to_string()),
        message_id: None,
    };

    let notif = notification(event).unwrap();

    match notif.update {
        SessionUpdate::AgentThoughtChunk(chunk) => match chunk.content {
            ContentBlock::Text(text) => assert_eq!(text.text, "let me think..."),
            other => panic!("expected Text, got {:?}", other),
        },
        other => panic!("expected AgentThoughtChunk, got {:?}", other),
    }
}

#[test]
fn tool_call_maps_to_tool_call_update() {
    let call = agent_types::tool::ToolCall::new(
        "call-1",
        "read_file",
        serde_json::json!({"path": "/tmp"}),
    );
    let event = RuntimeEvent::ToolCall { call };

    let notif = notification(event).unwrap();

    match notif.update {
        SessionUpdate::ToolCall(tc) => {
            assert_eq!(tc.tool_call_id.0.as_ref(), "call-1");
            assert_eq!(tc.title, "read_file");
            assert_eq!(tc.status, ToolCallStatus::InProgress);
            assert!(tc.raw_input.is_some());
        }
        other => panic!("expected ToolCall, got {:?}", other),
    }
}

#[test]
fn tool_result_maps_to_tool_call_update() {
    let result = agent_types::tool::ToolResult::text("call-1", "file content here");
    let event = RuntimeEvent::ToolResult { result };

    let notif = notification(event).unwrap();

    match notif.update {
        SessionUpdate::ToolCallUpdate(update) => {
            assert_eq!(update.tool_call_id.0.as_ref(), "call-1");
            assert_eq!(update.fields.status, Some(ToolCallStatus::Completed));
            assert!(update.fields.raw_output.is_some());
        }
        other => panic!("expected ToolCallUpdate, got {:?}", other),
    }
}

#[test]
fn tool_call_delta_maps_to_tool_call_update_raw_input() {
    let event = RuntimeEvent::ToolCallDelta {
        id: "tc-1".to_string(),
        index: 0,
        name: Some("bash".to_string()),
        arguments_delta: "{\"cmd\":".to_string(),
    };

    let notif = notification(event).unwrap();

    match notif.update {
        SessionUpdate::ToolCallUpdate(update) => {
            assert_eq!(update.tool_call_id.0.as_ref(), "tc-1");
            assert_eq!(update.fields.title.as_deref(), Some("bash"));
            assert_eq!(
                update.fields.raw_input,
                Some(serde_json::Value::String("{\"cmd\":".to_string()))
            );
        }
        other => panic!("expected ToolCallUpdate, got {:?}", other),
    }
}

#[test]
fn parked_tool_approval_maps_to_pending_tool_call() {
    let event = RuntimeEvent::ToolApprovalPending {
        call_id: "tc-2".to_string(),
        tool_name: "write_file".to_string(),
        request_id: None,
        prompt: None,
    };

    let notif = notification(event).unwrap();

    match notif.update {
        SessionUpdate::ToolCall(tc) => {
            assert_eq!(tc.tool_call_id.0.as_ref(), "tc-2");
            assert_eq!(tc.title, "write_file");
            assert_eq!(tc.status, ToolCallStatus::Pending);
        }
        other => panic!("expected ToolCall, got {:?}", other),
    }
}

#[test]
fn start_event_returns_none() {
    let event = RuntimeEvent::Start {
        session_id: "s1".into(),
        agent_id: None,
    };
    assert!(notification(event).is_none());
}

#[test]
fn end_event_returns_none() {
    let event = RuntimeEvent::End {
        finish_reason: Some("stop".to_string()),
    };
    let mapped = EventMapper::default().map(&session_id(), event);
    assert!(mapped.notification.is_none());
    assert_eq!(
        mapped.stop_reason,
        Some(agent_client_protocol::schema::v1::StopReason::EndTurn)
    );
}

#[test]
fn error_event_maps_to_visible_agent_message_chunk() {
    let event = RuntimeEvent::Error {
        error: rua_core::RuaError::Runtime {
            message: "boom".to_string(),
        },
    };
    let notif = notification(event).unwrap();

    match notif.update {
        SessionUpdate::AgentMessageChunk(chunk) => match chunk.content {
            ContentBlock::Text(text) => {
                assert!(text.text.contains("Error:"));
                assert!(text.text.contains("boom"));
                assert_eq!(
                    chunk
                        .meta
                        .as_ref()
                        .and_then(|meta| meta.get(ACP_ERROR_META_KEY)),
                    Some(&serde_json::Value::Bool(true))
                );
            }
            other => panic!("expected Text, got {:?}", other),
        },
        other => panic!("expected AgentMessageChunk, got {:?}", other),
    }
}

#[test]
fn usage_event_returns_none() {
    let event = RuntimeEvent::Usage {
        input_tokens: Some(100),
        output_tokens: Some(50),
        total_tokens: Some(150),
        cache_read_tokens: None,
        cache_write_tokens: None,
    };
    assert!(notification(event).is_none());
}

#[test]
fn available_commands_are_curated_for_acp() {
    let commands = available_commands();
    let names: Vec<_> = commands.iter().map(|cmd| cmd.name.as_str()).collect();
    assert_eq!(names, ["clear", "compact", "skills"]);
    assert!(!names.contains(&"mission"));
    assert!(commands.iter().all(|cmd| !cmd.description.is_empty()));
}

#[test]
fn tool_call_delta_can_carry_populated_raw_input_after_empty_start() {
    let mut mapper = EventMapper::default();
    let start = RuntimeEvent::ToolCall {
        call: agent_types::tool::ToolCall::new("tc-stream", "Bash", serde_json::json!({})),
    };
    let start_notif = mapper.map(&session_id(), start).notification.unwrap();
    match start_notif.update {
        SessionUpdate::ToolCall(call) => {
            assert_eq!(call.tool_call_id.0.as_ref(), "tc-stream");
            assert_eq!(call.title, "Bash");
            assert_eq!(call.raw_input, Some(serde_json::json!({})));
        }
        other => panic!("expected ToolCall, got {:?}", other),
    }

    let delta = RuntimeEvent::ToolCallDelta {
        id: "tc-stream".to_string(),
        index: 0,
        name: Some("Bash".to_string()),
        arguments_delta: r#"{"command":"echo hi"}"#.to_string(),
    };
    let delta_notif = mapper.map(&session_id(), delta).notification.unwrap();
    match delta_notif.update {
        SessionUpdate::ToolCallUpdate(update) => {
            assert_eq!(update.tool_call_id.0.as_ref(), "tc-stream");
            assert_eq!(update.fields.title.as_deref(), Some("Bash"));
            assert_eq!(
                update.fields.raw_input,
                Some(serde_json::json!({"command": "echo hi"}))
            );
        }
        other => panic!("expected ToolCallUpdate, got {:?}", other),
    }
}

#[test]
fn tool_result_file_parts_emit_resource_links() {
    let result = agent_types::tool::ToolResult {
        call_id: "call-file".to_string(),
        name: Some("send_file".to_string()),
        content: vec![
            agent_types::ContentPart::File {
                url: "file:///tmp/report.pdf".to_string(),
                name: "report.pdf".to_string(),
                media_type: Some("application/pdf".to_string()),
                size: Some(123),
            },
            agent_types::ContentPart::File {
                url: "https://example.com/remote.pdf".to_string(),
                name: "remote.pdf".to_string(),
                media_type: Some("application/pdf".to_string()),
                size: None,
            },
        ],
        is_error: false,
        duration_ms: None,
        attachments: None,
        truncation: None,
        metadata: serde_json::Map::new(),
    };

    let notif = notification(RuntimeEvent::ToolResult { result }).unwrap();

    match notif.update {
        SessionUpdate::ToolCallUpdate(update) => {
            let content = update.fields.content.expect("content");
            let text = content
                .iter()
                .find_map(|item| match item {
                    ToolCallContent::Content(content) => match &content.content {
                        ContentBlock::Text(text) => Some(text.text.as_str()),
                        _ => None,
                    },
                    _ => None,
                })
                .expect("text block");
            assert!(text.contains("[File: report.pdf]"));
            assert!(text.contains("[File: remote.pdf]"));
            assert!(
                !text.contains(r#""type":"file""#) && !text.contains("'type'"),
                "file parts should render as readable text, not raw JSON: {text}"
            );
            let links: Vec<_> = content
                .iter()
                .filter_map(|item| match item {
                    ToolCallContent::Content(content) => match &content.content {
                        ContentBlock::ResourceLink(link) => Some(link),
                        _ => None,
                    },
                    _ => None,
                })
                .collect();
            assert_eq!(links.len(), 1);
            assert_eq!(links[0].uri, "file:///tmp/report.pdf");
            assert_eq!(links[0].name, "report.pdf");
            assert_eq!(links[0].mime_type.as_deref(), Some("application/pdf"));
            assert_eq!(links[0].size, Some(123));
        }
        other => panic!("expected ToolCallUpdate, got {:?}", other),
    }
}

#[test]
fn tool_result_local_image_uses_filename_resource_link_without_raw_json_text() {
    let result = agent_types::tool::ToolResult {
        call_id: "call-image".to_string(),
        name: Some("send_file_to_user".to_string()),
        content: vec![
            agent_types::ContentPart::Image {
                url: "file:///tmp/cat.png".to_string(),
                media_type: Some("image/png".to_string()),
                alt: None,
            },
            agent_types::ContentPart::Text {
                text: "File sent successfully.".to_string(),
            },
        ],
        is_error: false,
        duration_ms: None,
        attachments: None,
        truncation: None,
        metadata: serde_json::Map::new(),
    };

    let notif = notification(RuntimeEvent::ToolResult { result }).unwrap();

    match notif.update {
        SessionUpdate::ToolCallUpdate(update) => {
            let content = update.fields.content.expect("content");
            let text = content
                .iter()
                .find_map(|item| match item {
                    ToolCallContent::Content(content) => match &content.content {
                        ContentBlock::Text(text) => Some(text.text.as_str()),
                        _ => None,
                    },
                    _ => None,
                })
                .expect("text block");
            assert!(text.contains("File sent successfully."));
            assert!(text.contains("[Image: file:///tmp/cat.png]"));
            assert!(
                !text.contains(r#""type":"image""#) && !text.contains("'type'"),
                "media blocks should render as readable text, not raw JSON: {text}"
            );

            let links: Vec<_> = content
                .iter()
                .filter_map(|item| match item {
                    ToolCallContent::Content(content) => match &content.content {
                        ContentBlock::ResourceLink(link) => Some(link),
                        _ => None,
                    },
                    _ => None,
                })
                .collect();
            assert_eq!(links.len(), 1);
            assert_eq!(links[0].uri, "file:///tmp/cat.png");
            assert_eq!(links[0].name, "cat.png");
            assert_eq!(links[0].mime_type.as_deref(), Some("image/png"));
        }
        other => panic!("expected ToolCallUpdate, got {:?}", other),
    }
}

#[test]
fn tool_result_remote_media_is_text_only_without_resource_link() {
    let result = agent_types::tool::ToolResult {
        call_id: "call-remote-image".to_string(),
        name: Some("send_file_to_user".to_string()),
        content: vec![agent_types::ContentPart::Image {
            url: "https://example.com/a.png".to_string(),
            media_type: Some("image/png".to_string()),
            alt: Some("a.png".to_string()),
        }],
        is_error: false,
        duration_ms: None,
        attachments: None,
        truncation: None,
        metadata: serde_json::Map::new(),
    };

    let notif = notification(RuntimeEvent::ToolResult { result }).unwrap();

    match notif.update {
        SessionUpdate::ToolCallUpdate(update) => {
            let content = update.fields.content.expect("content");
            assert_eq!(content.len(), 1);
            match &content[0] {
                ToolCallContent::Content(content) => match &content.content {
                    ContentBlock::Text(text) => assert_eq!(text.text, "[Image: a.png]"),
                    other => panic!("expected text content, got {:?}", other),
                },
                other => panic!("expected content block, got {:?}", other),
            }
        }
        other => panic!("expected ToolCallUpdate, got {:?}", other),
    }
}
