use agent_types::{ContentPart, ModelDialogueItem, Role};
use agent_types::{ToolAttachment, ToolCall, ToolResult, TruncationMeta};
use serde_json::json;

#[test]
fn managed_media_ref_is_the_only_location_field_for_uploaded_media() {
    let part = agent_types::content::content_part_from_value(json!({
        "type": "image",
        "media_ref": "media/uuid-image.png",
    }));

    assert!(matches!(
        part,
        ContentPart::Image { ref url, .. } if url == "media/uuid-image.png"
    ));
    assert_eq!(
        agent_types::content::content_part_to_stored_value(&part),
        json!({
            "type": "image",
            "media_ref": "media/uuid-image.png",
        })
    );
    assert!(agent_types::content::is_managed_media_ref(
        "media/uuid-image.png"
    ));
    assert!(!agent_types::content::is_managed_media_ref(
        "/tmp/uuid-image.png"
    ));
    assert!(!agent_types::content::is_managed_media_ref(
        "media/../uuid-image.png"
    ));
}

#[test]
fn tool_use_flat_wire_preserves_raw_input_as_raw_arguments() {
    let part: ContentPart = serde_json::from_value(json!({
        "type": "tool_use",
        "id": "call-1",
        "name": "bash",
        "input": {},
        "raw_input": "{\"cmd\":\"ls\"}"
    }))
    .expect("flat tool_use content part should deserialize");

    let ContentPart::ToolUse { call } = part else {
        panic!("expected tool_use content part");
    };

    assert_eq!(call.id, "call-1");
    assert_eq!(call.name, "bash");
    assert_eq!(call.arguments, json!({}));
    assert_eq!(call.raw_arguments.as_deref(), Some("{\"cmd\":\"ls\"}"));
}

#[test]
fn tool_use_flat_wire_roundtrips_raw_input() {
    let part: ContentPart = serde_json::from_value(json!({
        "type": "tool_use",
        "id": "call-1",
        "name": "read",
        "input": {"path": "/tmp/a.txt"},
        "raw_input": "{\"path\":\"/tmp/a.txt\"}"
    }))
    .expect("flat tool_use content part should deserialize");

    let encoded = serde_json::to_value(part).expect("tool_use content part should serialize");

    assert_eq!(
        encoded,
        json!({
            "type": "tool_use",
            "id": "call-1",
            "name": "read",
            "input": {"path": "/tmp/a.txt"},
            "raw_input": "{\"path\":\"/tmp/a.txt\"}"
        })
    );
}

#[test]
fn flat_tool_use_wire_preserves_provider_extra_content_metadata() {
    let part: ContentPart = serde_json::from_value(json!({
        "type": "tool_use",
        "id": "call-gemini",
        "name": "search",
        "input": {"q": "test"},
        "extra_content": {"thought_signature": "sig123"}
    }))
    .expect("flat tool_use content part should deserialize");

    let ContentPart::ToolUse { call } = &part else {
        panic!("expected tool_use content part");
    };

    assert_eq!(call.id, "call-gemini");
    assert_eq!(call.name, "search");
    assert_eq!(call.arguments, json!({"q": "test"}));
    assert_eq!(
        call.metadata
            .get("extra_content")
            .and_then(|v| v.get("thought_signature"))
            .and_then(|v| v.as_str()),
        Some("sig123"),
        "provider-private tool metadata must survive the typed Rust boundary"
    );

    let encoded = serde_json::to_value(part).expect("tool_use content part should serialize");
    assert_eq!(
        encoded["extra_content"]["thought_signature"].as_str(),
        Some("sig123"),
        "legacy flat wire should retain QwenPaw-compatible extra_content"
    );
}

#[test]
fn nested_tool_use_wire_merges_compat_metadata_without_losing_call_fields() {
    let part: ContentPart = serde_json::from_value(json!({
        "type": "tool_use",
        "call": {
            "id": "call-nested",
            "name": "read",
            "arguments": {"path": "/tmp/x"},
            "metadata": {"provider": "gemini"}
        },
        "extra_content": {"thought_signature": "sig456"}
    }))
    .expect("nested tool_use content part should deserialize");

    let ContentPart::ToolUse { call } = part else {
        panic!("expected tool_use content part");
    };

    assert_eq!(call.id, "call-nested");
    assert_eq!(call.name, "read");
    assert_eq!(call.arguments, json!({"path": "/tmp/x"}));
    assert_eq!(
        call.metadata.get("provider").and_then(|v| v.as_str()),
        Some("gemini")
    );
    assert_eq!(
        call.metadata
            .get("extra_content")
            .and_then(|v| v.get("thought_signature"))
            .and_then(|v| v.as_str()),
        Some("sig456")
    );
}

#[test]
fn nested_tool_use_call_wire_keeps_existing_raw_arguments() {
    let part: ContentPart = serde_json::from_value(json!({
        "type": "tool_use",
        "call": {
            "id": "call-2",
            "name": "grep",
            "arguments": {"pattern": "TODO"},
            "raw_arguments": "{\"pattern\":\"TODO\"}",
            "provider_call_id": "provider-call-2"
        },
        "raw_input": "{\"pattern\":\"ignored\"}"
    }))
    .expect("nested tool_use call should deserialize");

    let ContentPart::ToolUse { call } = part else {
        panic!("expected tool_use content part");
    };

    assert_eq!(call.id, "call-2");
    assert_eq!(call.name, "grep");
    assert_eq!(call.arguments, json!({"pattern": "TODO"}));
    assert_eq!(
        call.raw_arguments.as_deref(),
        Some("{\"pattern\":\"TODO\"}")
    );
    assert_eq!(call.provider_call_id.as_deref(), Some("provider-call-2"));
}

#[test]
fn chat_message_text_content_concatenates_only_text_blocks_from_mixed_content() {
    let message: ModelDialogueItem = serde_json::from_value(json!({
        "role": "user",
        "content": [
            {"type": "image", "url": "file:///tmp/chart.png", "alt": "chart"},
            {"type": "text", "text": "first "},
            {"type": "thinking", "thinking": "hidden reasoning"},
            {"type": "text", "text": "second"}
        ]
    }))
    .expect("mixed content chat message should deserialize");

    assert_eq!(message.text_content(), "first second");
    assert_eq!(message.thinking_content(), "hidden reasoning");
}

#[test]
fn chat_message_media_only_content_has_no_user_text() {
    let message: ModelDialogueItem = serde_json::from_value(json!({
        "role": "user",
        "content": [
            {"type": "image", "url": "file:///tmp/chart.png", "alt": "chart"},
            {"type": "audio", "url": "file:///tmp/audio.mp3", "media_type": "audio/mpeg"}
        ]
    }))
    .expect("media-only chat message should deserialize");

    assert_eq!(message.text_content(), "");
    assert!(!message.has_text());
}

#[test]
fn flat_tool_result_output_string_becomes_text_content() {
    let part: ContentPart = serde_json::from_value(json!({
        "type": "tool_result",
        "id": "call-1",
        "name": "bash",
        "output": "done",
        "is_error": true
    }))
    .expect("flat tool_result content part should deserialize");

    let ContentPart::ToolResult { result } = part else {
        panic!("expected tool_result content part");
    };

    assert_eq!(result.call_id, "call-1");
    assert_eq!(result.name.as_deref(), Some("bash"));
    assert!(result.is_error);
    assert_eq!(result.text_content(), "done");
}

#[test]
fn flat_tool_result_wire_preserves_execution_metadata() {
    let part = ContentPart::ToolResult {
        result: ToolResult {
            call_id: "call-truncated".into(),
            name: Some("bash".into()),
            content: vec![ContentPart::Text {
                text: "<truncated_output>summary</truncated_output>".into(),
            }],
            is_error: true,
            duration_ms: Some(123),
            attachments: Some(vec![ToolAttachment {
                name: "raw.log".into(),
                mime_type: "text/plain".into(),
                content: "file:///tmp/raw.log".into(),
            }]),
            truncation: Some(TruncationMeta {
                tool: "bash".into(),
                total_lines: Some(167),
                total_bytes: Some(5168),
                est_tokens: Some(1292),
                shown_start: None,
                shown_end: None,
                continuation_hint: None,
                recall_key: Some("abc123".into()),
                retry_hint: None,
            }),
            metadata: serde_json::Map::from_iter([("exit_code".to_string(), json!(0))]),
        },
    };

    let encoded = serde_json::to_value(&part).expect("tool_result should serialize");
    assert_eq!(encoded["type"], "tool_result");
    assert_eq!(encoded["id"], "call-truncated");
    assert_eq!(encoded["is_error"], true);
    assert_eq!(encoded["duration_ms"], 123);
    assert_eq!(encoded["truncation"]["recall_key"], "abc123");
    assert_eq!(encoded["metadata"]["exit_code"], 0);

    let decoded: ContentPart =
        serde_json::from_value(encoded).expect("tool_result should deserialize");
    let ContentPart::ToolResult { result } = decoded else {
        panic!("expected tool_result content part");
    };

    assert!(result.is_error);
    assert_eq!(result.duration_ms, Some(123));
    assert_eq!(
        result
            .truncation
            .as_ref()
            .and_then(|meta| meta.recall_key.as_deref()),
        Some("abc123")
    );
    assert_eq!(
        result
            .metadata
            .get("exit_code")
            .and_then(|value| value.as_i64()),
        Some(0)
    );
    assert_eq!(
        result
            .attachments
            .as_ref()
            .and_then(|attachments| attachments.first())
            .map(|attachment| attachment.name.as_str()),
        Some("raw.log")
    );
}

#[test]
fn agentscope_source_media_blocks_deserialize_to_typed_content_parts() {
    let image: ContentPart = serde_json::from_value(json!({
        "type": "image",
        "source": {
            "type": "url",
            "url": "file:///tmp/result.png",
            "media_type": "image/png"
        },
        "alt": "result preview"
    }))
    .expect("legacy image source block should deserialize");
    assert_eq!(
        image,
        ContentPart::Image {
            url: "file:///tmp/result.png".into(),
            media_type: Some("image/png".into()),
            alt: Some("result preview".into()),
        }
    );

    let audio: ContentPart = serde_json::from_value(json!({
        "type": "audio",
        "source": {
            "type": "url",
            "url": "file:///tmp/result.mp3",
            "mime_type": "audio/mpeg"
        }
    }))
    .expect("legacy audio source block should deserialize");
    assert_eq!(
        audio,
        ContentPart::Audio {
            url: "file:///tmp/result.mp3".into(),
            media_type: Some("audio/mpeg".into()),
        }
    );

    let video: ContentPart = serde_json::from_value(json!({
        "type": "video",
        "source": {
            "type": "url",
            "url": "file:///tmp/result.mp4"
        },
        "media_type": "video/mp4"
    }))
    .expect("legacy video source block should deserialize");
    assert_eq!(
        video,
        ContentPart::Video {
            url: "file:///tmp/result.mp4".into(),
            media_type: Some("video/mp4".into()),
        }
    );
}

#[test]
fn flat_tool_result_output_array_accepts_agentscope_source_media_blocks() {
    let part: ContentPart = serde_json::from_value(json!({
        "type": "tool_result",
        "id": "call-view",
        "name": "view_image",
        "output": [
            {"type": "text", "text": "Rendered image:"},
            {
                "type": "image",
                "source": {
                    "type": "url",
                    "url": "file:///tmp/result.png",
                    "media_type": "image/png"
                }
            }
        ]
    }))
    .expect("flat tool_result output array should deserialize");

    let ContentPart::ToolResult { result } = part else {
        panic!("expected tool_result content part");
    };

    assert_eq!(result.call_id, "call-view");
    assert_eq!(result.name.as_deref(), Some("view_image"));
    assert_eq!(result.content.len(), 2);
    assert_eq!(
        result.content[0],
        ContentPart::Text {
            text: "Rendered image:".into()
        }
    );
    assert_eq!(
        result.content[1],
        ContentPart::Image {
            url: "file:///tmp/result.png".into(),
            media_type: Some("image/png".into()),
            alt: None,
        }
    );
}

#[test]
fn tool_call_effective_arguments_repairs_empty_typed_arguments_from_raw_arguments() {
    let mut call = ToolCall::new("call-1", "search", json!({}));
    call.raw_arguments = Some(r#"{"query":"hello"}"#.to_string());

    assert_eq!(call.effective_arguments(), json!({"query": "hello"}));

    call.arguments = json!(null);
    assert_eq!(call.effective_arguments(), json!({"query": "hello"}));
}

#[test]
fn tool_call_effective_arguments_keeps_typed_arguments_when_raw_is_not_usable() {
    let mut call = ToolCall::new("call-1", "search", json!({"existing": true}));
    call.raw_arguments = Some(r#"{"query":"ignored"}"#.to_string());
    assert_eq!(
        call.effective_arguments(),
        json!({"existing": true}),
        "non-empty typed arguments win over raw_arguments"
    );

    for raw in ["", "{}", "not json"] {
        let mut call = ToolCall::new("call-1", "search", json!({}));
        call.raw_arguments = Some(raw.to_string());
        assert_eq!(
            call.effective_arguments(),
            json!({}),
            "raw_arguments={raw:?} must not repair to a non-empty value"
        );
    }
}

#[test]
fn chat_message_extracts_tool_call_and_result_ids_from_typed_content() {
    let message = ModelDialogueItem {
        id: None,
        role: Role::Assistant,
        content: vec![
            ContentPart::ToolUse {
                call: ToolCall::new("call-a", "bash", json!({"command": "pwd"})),
            },
            ContentPart::Text {
                text: "plain text is ignored by tool extraction".into(),
            },
            ContentPart::ToolResult {
                result: ToolResult::text("call-b", "done"),
            },
        ],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    };

    assert_eq!(
        message
            .tool_calls()
            .into_iter()
            .map(|call| call.id.as_str())
            .collect::<Vec<_>>(),
        vec!["call-a"]
    );
    assert_eq!(
        message
            .tool_results()
            .into_iter()
            .map(|result| result.call_id.as_str())
            .collect::<Vec<_>>(),
        vec!["call-b"]
    );
}

#[test]
fn chat_message_accepts_python_string_content_as_text_part() {
    let message: ModelDialogueItem = serde_json::from_value(json!({
        "role": "user",
        "content": "plain Python Msg content"
    }))
    .expect("Python-style string content should deserialize");

    assert_eq!(message.role, Role::User);
    assert_eq!(message.text_content(), "plain Python Msg content");
    assert_eq!(
        message.content,
        vec![ContentPart::Text {
            text: "plain Python Msg content".into()
        }]
    );
}

#[test]
fn tool_result_text_content_projects_payload_without_canonical_envelope() {
    let mut result = ToolResult::text("call-text", r#"{"timed_out":true}"#);
    result.name = Some("task_await".into());
    result.content.extend([
        ContentPart::Image {
            url: "data:image/png;base64,AA==".into(),
            media_type: Some("image/png".into()),
            alt: None,
        },
        ContentPart::Text {
            text: "second part".into(),
        },
    ]);

    assert_eq!(result.text_content(), "{\"timed_out\":true}second part");
}

#[test]
fn chat_message_treats_null_or_missing_content_as_empty_history_item() {
    let null_content: ModelDialogueItem = serde_json::from_value(json!({
        "role": "assistant",
        "content": null
    }))
    .expect("null content should deserialize as empty content");
    assert!(null_content.content.is_empty());
    assert!(null_content.is_empty());

    let missing_content: ModelDialogueItem = serde_json::from_value(json!({
        "role": "assistant"
    }))
    .expect("missing content should deserialize as empty content");
    assert!(missing_content.content.is_empty());
    assert!(missing_content.is_empty());
}

#[test]
fn chat_message_tool_helpers_ignore_non_tool_and_empty_content() {
    let text = ModelDialogueItem::text(Role::User, "hello");
    assert!(text.tool_calls().is_empty());
    assert!(text.tool_results().is_empty());
    assert!(!text.has_tool_calls());

    let empty = ModelDialogueItem {
        id: None,
        role: Role::Assistant,
        content: Vec::new(),
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    };
    assert!(empty.tool_calls().is_empty());
    assert!(empty.tool_results().is_empty());
    assert!(empty.is_empty());
}

#[test]
fn chat_message_tool_helpers_ignore_invalid_tool_block_identities() {
    let message: ModelDialogueItem = serde_json::from_value(json!({
        "role": "assistant",
        "content": [
            {"type": "text", "text": "before"},
            {"type": "tool_use", "id": "", "name": "bash", "input": {"command": "pwd"}},
            {"type": "tool_use", "id": "call-missing-name", "name": "", "input": {}},
            {"type": "tool_use", "id": "call-valid", "name": "read", "input": {"path": "README.md"}},
            {"type": "tool_result", "id": "", "name": "bash", "output": "orphan"},
            {"type": "tool_result", "id": "call-valid", "name": "read", "output": "ok"}
        ]
    }))
    .expect("QwenPaw-style tool blocks should deserialize");

    assert_eq!(
        message.content.len(),
        6,
        "Rust keeps the raw typed content for compatibility and diagnostics"
    );
    assert_eq!(
        message
            .tool_calls()
            .into_iter()
            .map(|call| call.id.as_str())
            .collect::<Vec<_>>(),
        vec!["call-valid"],
        "empty tool_use id/name blocks must not be surfaced as executable calls"
    );
    assert_eq!(
        message
            .tool_results()
            .into_iter()
            .map(|result| result.call_id.as_str())
            .collect::<Vec<_>>(),
        vec!["call-valid"],
        "empty tool_result ids must not be surfaced as paired results"
    );
    assert!(message.has_tool_calls());

    let invalid_only: ModelDialogueItem = serde_json::from_value(json!({
        "role": "assistant",
        "content": [
            {"type": "tool_use", "id": "   ", "name": "bash", "input": {}},
            {"type": "tool_use", "id": "call-empty-name", "name": "   ", "input": {}}
        ]
    }))
    .expect("invalid tool blocks should still deserialize");

    assert!(invalid_only.tool_calls().is_empty());
    assert!(
        !invalid_only.has_tool_calls(),
        "invalid compatibility blocks must not trigger tool-use control flow"
    );
}
