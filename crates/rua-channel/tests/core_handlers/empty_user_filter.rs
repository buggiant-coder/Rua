use rua_contract::agent::AgentInputMessage;
use rua_core::ContentPart;

fn filter_persisted(messages: Vec<AgentInputMessage>) -> Vec<AgentInputMessage> {
    messages
        .into_iter()
        .filter(|m| {
            if m.role == "system" {
                return m
                    .content
                    .iter()
                    .any(|p| matches!(p, ContentPart::ToolResult { .. }));
            }
            if m.role == "user" {
                return !m.content.is_empty()
                    && !m.content.iter().all(
                        |p| matches!(p, ContentPart::Text { text } if text.trim().is_empty()),
                    );
            }
            true
        })
        .collect()
}

#[test]
fn empty_user_message_excluded() {
    let msgs = vec![AgentInputMessage {
        role: "user".into(),
        content: vec![],
    }];
    let result = filter_persisted(msgs);
    assert!(
        result.is_empty(),
        "empty-content user message must be filtered out"
    );
}

#[test]
fn whitespace_only_user_message_excluded() {
    let msgs = vec![AgentInputMessage {
        role: "user".into(),
        content: vec![ContentPart::Text { text: "   ".into() }],
    }];
    let result = filter_persisted(msgs);
    assert!(
        result.is_empty(),
        "whitespace-only user message must be filtered out"
    );
}

#[test]
fn user_message_with_text_preserved() {
    let msgs = vec![AgentInputMessage {
        role: "user".into(),
        content: vec![ContentPart::Text {
            text: "hello".into(),
        }],
    }];
    let result = filter_persisted(msgs);
    assert_eq!(result.len(), 1, "user message with text must be preserved");
}

#[test]
fn user_message_with_image_only_preserved() {
    let msgs = vec![AgentInputMessage {
        role: "user".into(),
        content: vec![ContentPart::Image {
            url: "data:image/png;base64,abc".into(),
            media_type: Some("image/png".into()),
            alt: None,
        }],
    }];
    let result = filter_persisted(msgs);
    assert_eq!(
        result.len(),
        1,
        "user message with image attachment must be preserved"
    );
}

#[test]
fn user_message_with_file_only_preserved() {
    let msgs = vec![AgentInputMessage {
        role: "user".into(),
        content: vec![ContentPart::File {
            url: "/tmp/doc.pdf".into(),
            name: "doc.pdf".into(),
            media_type: None,
            size: None,
        }],
    }];
    let result = filter_persisted(msgs);
    assert_eq!(
        result.len(),
        1,
        "user message with file attachment must be preserved"
    );
}

#[test]
fn system_tool_result_preserved() {
    let msgs = vec![AgentInputMessage {
        role: "system".into(),
        content: vec![ContentPart::ToolResult {
            result: rua_core::tool::ToolResult {
                call_id: "call-1".into(),
                name: Some("bash".into()),
                content: vec![ContentPart::Text { text: "ok".into() }],
                is_error: false,
                duration_ms: None,
                attachments: None,
                truncation: None,
                metadata: serde_json::Map::new(),
            },
        }],
    }];
    let result = filter_persisted(msgs);
    assert_eq!(result.len(), 1, "system tool_result must be preserved");
}

#[test]
fn assistant_message_preserved() {
    let msgs = vec![AgentInputMessage {
        role: "assistant".into(),
        content: vec![ContentPart::Text {
            text: "done".into(),
        }],
    }];
    let result = filter_persisted(msgs);
    assert_eq!(result.len(), 1, "assistant message must be preserved");
}

#[test]
fn mixed_sequence_filters_only_empty_user() {
    let msgs = vec![
        AgentInputMessage {
            role: "user".into(),
            content: vec![ContentPart::Text {
                text: "delete foo".into(),
            }],
        },
        AgentInputMessage {
            role: "assistant".into(),
            content: vec![ContentPart::Text {
                text: "thinking...".into(),
            }],
        },
        AgentInputMessage {
            role: "user".into(),
            content: vec![ContentPart::Text {
                text: String::new(),
            }],
        },
        AgentInputMessage {
            role: "assistant".into(),
            content: vec![ContentPart::Text {
                text: "done".into(),
            }],
        },
    ];
    let result = filter_persisted(msgs);
    assert_eq!(
        result.len(),
        3,
        "only the empty user message should be filtered"
    );
    assert_eq!(result[0].role, "user");
    assert_eq!(result[1].role, "assistant");
    assert_eq!(result[2].role, "assistant");
}
