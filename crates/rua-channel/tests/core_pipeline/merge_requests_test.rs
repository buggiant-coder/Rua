use rua_channel::core::content::{AgentChannelRequest, IncomingContentPart};
use rua_channel::core::pipeline::merge_requests;

fn request(text: &str, metadata: serde_json::Value, message_id: &str) -> AgentChannelRequest {
    AgentChannelRequest {
        agent_id: "agent-1".into(),
        user_id: "user-1".into(),
        session_id: "session-1".into(),
        input: vec![IncomingContentPart::Text { text: text.into() }],
        channel_id: "telegram".into(),
        conversation_id: "conv-1".into(),
        message_id: Some(message_id.into()),
        metadata,
    }
}

fn request_with_parts(
    input: Vec<IncomingContentPart>,
    metadata: serde_json::Value,
    message_id: &str,
) -> AgentChannelRequest {
    AgentChannelRequest {
        agent_id: "agent-1".into(),
        user_id: "user-1".into(),
        session_id: "session-1".into(),
        input,
        channel_id: "telegram".into(),
        conversation_id: "conv-1".into(),
        message_id: Some(message_id.into()),
        metadata,
    }
}

#[test]
fn merge_requests_concatenates_input_and_keeps_latest_routing_metadata() {
    let merged = merge_requests(vec![
        request("first", serde_json::json!({"chat_id": "100"}), "m1"),
        request("second", serde_json::json!({"chat_id": "200"}), "m2"),
    ]);

    assert_eq!(merged.input.len(), 2);
    assert_eq!(merged.metadata["chat_id"], "200");
    assert_eq!(merged.message_id, Some("m2".into()));
}

#[test]
fn merge_requests_preserves_mixed_media_and_text_part_order() {
    let merged = merge_requests(vec![
        request_with_parts(
            vec![IncomingContentPart::Image {
                file_id: "img-1".into(),
                path: "/tmp/first.png".into(),
                mime: Some("image/png".into()),
            }],
            serde_json::json!({"chat_id": "100"}),
            "m1",
        ),
        request_with_parts(
            vec![
                IncomingContentPart::File {
                    file_id: "file-1".into(),
                    path: "/tmp/report.pdf".into(),
                    mime: Some("application/pdf".into()),
                    name: Some("report.pdf".into()),
                },
                IncomingContentPart::Text {
                    text: "Use the image and report".into(),
                },
            ],
            serde_json::json!({"chat_id": "200"}),
            "m2",
        ),
    ]);

    assert_eq!(
        merged.input,
        vec![
            IncomingContentPart::Image {
                file_id: "img-1".into(),
                path: "/tmp/first.png".into(),
                mime: Some("image/png".into()),
            },
            IncomingContentPart::File {
                file_id: "file-1".into(),
                path: "/tmp/report.pdf".into(),
                mime: Some("application/pdf".into()),
                name: Some("report.pdf".into()),
            },
            IncomingContentPart::Text {
                text: "Use the image and report".into(),
            },
        ],
        "QwenPaw's native-item merge signal maps to Rust's debounced request merge: multipart user input must not be reordered or dropped"
    );
    assert_eq!(merged.metadata["chat_id"], "200");
    assert_eq!(merged.message_id, Some("m2".into()));
}

#[test]
fn merge_requests_preserves_mention_gate_signals_across_debounce_batch() {
    let merged = merge_requests(vec![
        request(
            "/start",
            serde_json::json!({
                "is_group": true,
                "has_bot_command": true,
                "chat_id": "-100",
            }),
            "m1",
        ),
        request(
            "follow up",
            serde_json::json!({
                "is_group": true,
                "chat_id": "-100",
            }),
            "m2",
        ),
    ]);

    assert_eq!(merged.metadata["has_bot_command"], true);
    assert_eq!(merged.metadata["is_group"], true);
    assert_eq!(merged.message_id, Some("m2".into()));
}

#[test]
fn merge_requests_preserves_mention_signal_from_earlier_message() {
    let merged = merge_requests(vec![
        request(
            "@bot hello",
            serde_json::json!({
                "is_group": true,
                "mentioned": true,
            }),
            "m1",
        ),
        request("next line", serde_json::json!({"is_group": true}), "m2"),
    ]);

    assert_eq!(merged.metadata["mentioned"], true);
}

#[test]
fn merge_requests_normalizes_bot_mentioned_alias_from_earlier_message() {
    let merged = merge_requests(vec![
        request(
            "@bot hello",
            serde_json::json!({
                "is_group": true,
                "bot_mentioned": true,
            }),
            "m1",
        ),
        request("next line", serde_json::json!({"is_group": true}), "m2"),
    ]);

    assert_eq!(merged.metadata["mentioned"], true);
}
