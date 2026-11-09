use std::sync::Arc;

use agent_types::{
    AgentEvent, ContentPart, FinishReason, ModelDialogueItem, Role, TokenUsage, ToolResult,
};
use futures::StreamExt;
use react_agent::test_support::MockChatModel;
use react_agent::{
    has_media_blocks, is_multimodal_error_message, strip_media_blocks_from_memory, ReactAgent,
    ReactConfig,
};
use rua_provider::capability_cache;

fn make_image_message() -> ModelDialogueItem {
    ModelDialogueItem {
        id: None,
        role: Role::User,
        content: vec![
            ContentPart::Text {
                text: "Describe this image".to_string(),
            },
            ContentPart::Image {
                url: "https://example.com/cat.png".to_string(),
                media_type: Some("image/png".to_string()),
                alt: None,
            },
        ],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    }
}

#[test]
fn strip_media_blocks_replaces_with_placeholders() {
    let mut messages = vec![make_image_message()];
    let count = strip_media_blocks_from_memory(&mut messages);
    assert_eq!(count, 1);
    assert_eq!(messages[0].content.len(), 2);
    assert!(
        matches!(&messages[0].content[0], ContentPart::Text { text } if text == "Describe this image")
    );
    assert!(
        matches!(&messages[0].content[1], ContentPart::Text { text } if text.contains("Image was here"))
    );
}

#[test]
fn strip_media_blocks_handles_multiple_media_types() {
    let mut messages = vec![ModelDialogueItem {
        id: None,
        role: Role::User,
        content: vec![
            ContentPart::Image {
                url: "img.png".into(),
                media_type: None,
                alt: None,
            },
            ContentPart::Audio {
                url: "audio.mp3".into(),
                media_type: None,
            },
            ContentPart::Video {
                url: "video.mp4".into(),
                media_type: None,
            },
            ContentPart::File {
                url: "doc.pdf".into(),
                name: "doc.pdf".into(),
                media_type: None,
                size: None,
            },
            ContentPart::Text {
                text: "keep this".into(),
            },
        ],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    }];
    let count = strip_media_blocks_from_memory(&mut messages);

    assert_eq!(count, 3);
    assert_eq!(messages[0].content.len(), 5);

    assert!(matches!(&messages[0].content[3], ContentPart::File { name, .. } if name == "doc.pdf"));
    assert!(matches!(&messages[0].content[4], ContentPart::Text { text } if text == "keep this"));
}

#[test]
fn strip_media_blocks_handles_tool_result_media_without_flattening_tool_result() {
    let mut messages = vec![ModelDialogueItem {
        id: None,
        role: Role::Tool,
        content: vec![ContentPart::ToolResult {
            result: ToolResult {
                call_id: "call_view_image".into(),
                name: Some("view_image".into()),
                content: vec![
                    ContentPart::Text {
                        text: "Rendered image:".into(),
                    },
                    ContentPart::Image {
                        url: "file:///tmp/result.png".into(),
                        media_type: Some("image/png".into()),
                        alt: None,
                    },
                ],
                is_error: false,
                duration_ms: None,
                attachments: None,
                truncation: None,
                metadata: serde_json::Map::new(),
            },
        }],
        name: Some("view_image".into()),
        created_at: None,
        metadata: serde_json::Map::new(),
    }];

    let count = strip_media_blocks_from_memory(&mut messages);

    assert_eq!(count, 1);
    let ContentPart::ToolResult { result } = &messages[0].content[0] else {
        panic!("tool result block should remain a tool_result");
    };
    assert_eq!(result.call_id, "call_view_image");
    assert!(matches!(&result.content[0], ContentPart::Text { text } if text == "Rendered image:"));
    assert!(
        matches!(&result.content[1], ContentPart::Text { text } if text.contains("Image was here")),
        "nested image should become a text placeholder: {:?}",
        result.content
    );
}

#[test]
fn strip_media_blocks_preserves_file_blocks() {
    let mut messages = vec![ModelDialogueItem {
        id: None,
        role: Role::User,
        content: vec![
            ContentPart::Text {
                text: "look at this doc".into(),
            },
            ContentPart::File {
                url: "/local/path/doc.pdf".into(),
                name: "doc.pdf".into(),
                media_type: None,
                size: None,
            },
        ],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    }];
    let count = strip_media_blocks_from_memory(&mut messages);
    assert_eq!(count, 0, "File blocks must not be classified as media");
    assert_eq!(messages[0].content.len(), 2);
    assert!(matches!(&messages[0].content[1], ContentPart::File { .. }));
}

#[test]
fn strip_returns_zero_when_no_media() {
    let mut messages = vec![ModelDialogueItem::text(Role::User, "hello")];
    let count = strip_media_blocks_from_memory(&mut messages);
    assert_eq!(count, 0);
}

#[test]
fn has_media_blocks_detects_images() {
    let messages = vec![make_image_message()];
    assert!(has_media_blocks(&messages));
}

#[test]
fn has_media_blocks_detects_nested_tool_result_media() {
    let messages = vec![ModelDialogueItem {
        id: None,
        role: Role::Tool,
        content: vec![ContentPart::ToolResult {
            result: ToolResult {
                call_id: "call_view_video".into(),
                name: Some("view_video".into()),
                content: vec![ContentPart::Video {
                    url: "file:///tmp/result.mp4".into(),
                    media_type: Some("video/mp4".into()),
                }],
                is_error: false,
                duration_ms: None,
                attachments: None,
                truncation: None,
                metadata: serde_json::Map::new(),
            },
        }],
        name: Some("view_video".into()),
        created_at: None,
        metadata: serde_json::Map::new(),
    }];

    assert!(has_media_blocks(&messages));
}

#[test]
fn has_media_blocks_false_for_text_only() {
    let messages = vec![ModelDialogueItem::text(Role::User, "hello")];
    assert!(!has_media_blocks(&messages));
}

#[test]
fn is_multimodal_error_detects_openai_pattern() {
    assert!(is_multimodal_error_message(
        "Invalid content type. image_url is only supported by certain models"
    ));
}

#[test]
fn is_multimodal_error_detects_anthropic_pattern() {
    assert!(is_multimodal_error_message(
        "image content is not supported for this model"
    ));
}

#[test]
fn is_multimodal_error_detects_generic_patterns() {
    assert!(is_multimodal_error_message(
        "This model does not support image input"
    ));
    assert!(is_multimodal_error_message(
        "Vision is not available for this model"
    ));
    assert!(is_multimodal_error_message(
        "Multimodal input is not supported"
    ));
}

#[test]
fn is_multimodal_error_detects_text_only_deserializer_rejection() {
    assert!(is_multimodal_error_message(
        "Failed to deserialize the JSON body: unknown variant `image_url`, expected `text`"
    ));
}

#[test]
fn is_multimodal_error_ignores_unrelated_errors() {
    assert!(!is_multimodal_error_message("rate limit exceeded"));
    assert!(!is_multimodal_error_message("context length exceeded"));
    assert!(!is_multimodal_error_message("invalid JSON"));
}

#[tokio::test]
async fn proactive_strip_allows_non_multimodal_model_with_images() {
    let model =
        Arc::new(MockChatModel::text_response("I see text only").with_multimodal(Some(false)));
    let agent = ReactAgent::builder(model)
        .config(ReactConfig::default())
        .build();

    let mut stream = agent.run_stream(vec![make_image_message()]);
    let mut got_text = false;
    let mut got_end = false;
    while let Some(event) = stream.next().await {
        match event {
            AgentEvent::TextDelta { delta, .. } if delta.contains("I see text only") => {
                got_text = true;
            }
            AgentEvent::End {
                finish_reason: FinishReason::Stop,
                ..
            } => {
                got_end = true;
            }
            AgentEvent::Error { message, .. } => {
                panic!("unexpected error: {message}");
            }
            _ => {}
        }
    }
    assert!(got_text, "should have received text response");
    assert!(got_end, "should have received End event");
}

#[tokio::test]
async fn passive_retry_on_multimodal_error() {
    let model = Arc::new(MockChatModel::from_sequences(vec![
        vec![
            AgentEvent::Start {
                run_id: "run-1".into(),
                model: Some("mock".into()),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::Error {
                code: None,
                message: "This model does not support image input".to_string(),
                retryable: false,
                metadata: serde_json::Value::Null,
            },
        ],
        vec![
            AgentEvent::Start {
                run_id: "run-2".into(),
                model: Some("mock".into()),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::TextDelta {
                delta: "retry succeeded".to_string(),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::End {
                finish_reason: FinishReason::Stop,
                usage: Some(TokenUsage::new(10, 5)),
                metadata: serde_json::Value::Null,
            },
        ],
    ]));

    let agent = ReactAgent::builder(model)
        .config(ReactConfig::default())
        .build();

    let mut stream = agent.run_stream(vec![make_image_message()]);
    let mut got_retry_text = false;
    let mut got_end = false;
    while let Some(event) = stream.next().await {
        match event {
            AgentEvent::TextDelta { delta, .. } if delta.contains("retry succeeded") => {
                got_retry_text = true;
            }
            AgentEvent::End {
                finish_reason: FinishReason::Stop,
                ..
            } => {
                got_end = true;
            }
            _ => {}
        }
    }
    assert!(got_retry_text, "passive retry should have produced text");
    assert!(got_end, "should have received End event");
}

#[tokio::test]
async fn passive_retry_only_once_no_infinite_loop() {
    let model = Arc::new(MockChatModel::from_sequences(vec![
        vec![
            AgentEvent::Start {
                run_id: "run-1".into(),
                model: Some("mock".into()),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::Error {
                code: None,
                message: "This model does not support image input".to_string(),
                retryable: false,
                metadata: serde_json::Value::Null,
            },
        ],
        vec![
            AgentEvent::Start {
                run_id: "run-2".into(),
                model: Some("mock".into()),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::Error {
                code: None,
                message: "Vision is not available for this model".to_string(),
                retryable: false,
                metadata: serde_json::Value::Null,
            },
        ],
    ]));
    let agent = ReactAgent::builder(model)
        .config(ReactConfig::default())
        .build();

    let mut stream = agent.run_stream(vec![make_image_message()]);
    let mut got_error = false;
    while let Some(event) = stream.next().await {
        if let AgentEvent::Error { .. } = event {
            got_error = true;
        }
    }
    assert!(
        got_error,
        "second multimodal error should propagate as Error event"
    );
}

#[tokio::test]
async fn passive_retry_learns_rejects_media() {
    let provider_id = "dev79-learn-pid";
    let key = capability_cache::cache_key(provider_id, "mock-model");
    assert!(
        !capability_cache::rejects_media(&key),
        "precondition: cache must be empty for this provider+model"
    );

    let model = Arc::new(MockChatModel::from_sequences(vec![
        vec![
            AgentEvent::Start {
                run_id: "run-1".into(),
                model: Some("mock".into()),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::Error {
                code: None,
                message: "This model does not support image input".to_string(),
                retryable: false,
                metadata: serde_json::Value::Null,
            },
        ],
        vec![
            AgentEvent::Start {
                run_id: "run-2".into(),
                model: Some("mock".into()),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::TextDelta {
                delta: "retry succeeded".to_string(),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::End {
                finish_reason: FinishReason::Stop,
                usage: Some(TokenUsage::new(10, 5)),
                metadata: serde_json::Value::Null,
            },
        ],
    ]));
    let agent = ReactAgent::builder(model)
        .config(ReactConfig::default())
        .provider_id(provider_id)
        .build();

    let mut stream = agent.run_stream(vec![make_image_message()]);
    while stream.next().await.is_some() {}

    assert!(
        capability_cache::rejects_media(&key),
        "passive multimodal retry must learn rejects_media=true into the cache"
    );
}

#[tokio::test]
async fn passive_retry_skips_learning_without_provider_id() {
    let provider_id = "dev79-noprovider-pid";
    let key = capability_cache::cache_key(provider_id, "mock-model");

    let model = Arc::new(MockChatModel::from_sequences(vec![
        vec![
            AgentEvent::Start {
                run_id: "run-1".into(),
                model: Some("mock".into()),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::Error {
                code: None,
                message: "This model does not support image input".to_string(),
                retryable: false,
                metadata: serde_json::Value::Null,
            },
        ],
        vec![
            AgentEvent::Start {
                run_id: "run-2".into(),
                model: Some("mock".into()),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::TextDelta {
                delta: "retry succeeded".to_string(),
                metadata: serde_json::Value::Null,
            },
            AgentEvent::End {
                finish_reason: FinishReason::Stop,
                usage: Some(TokenUsage::new(10, 5)),
                metadata: serde_json::Value::Null,
            },
        ],
    ]));

    let agent = ReactAgent::builder(model)
        .config(ReactConfig::default())
        .build();

    let mut stream = agent.run_stream(vec![make_image_message()]);
    while stream.next().await.is_some() {}

    assert!(
        !capability_cache::rejects_media(&key),
        "without provider_id the learn step must be skipped"
    );
}

#[tokio::test]
async fn proactive_strip_reads_learned_cache() {
    let provider_id = "dev79-read-pid";
    let key = capability_cache::cache_key(provider_id, "mock-model");
    capability_cache::learn_rejects_media(&key);

    let model = Arc::new(MockChatModel::text_response("ok").with_multimodal(None));
    let agent = ReactAgent::builder(model)
        .config(ReactConfig::default())
        .provider_id(provider_id)
        .build();

    let mut stream = agent.run_stream(vec![make_image_message()]);
    let mut got_end = false;
    while let Some(event) = stream.next().await {
        if let AgentEvent::Error { message, .. } = event {
            panic!("unexpected error: {message}");
        }
        if matches!(
            event,
            AgentEvent::End {
                finish_reason: FinishReason::Stop,
                ..
            }
        ) {
            got_end = true;
        }
    }
    assert!(
        got_end,
        "run with pre-seeded rejects_media cache should succeed"
    );
}

#[test]
fn execution_context_overwrites_untrusted_image_capability_with_selected_model() {
    for (supported, expected) in [(Some(true), true), (Some(false), false), (None, false)] {
        let mut tool_ctx = rua_core::tool::ToolContext::default();
        tool_ctx.metadata.insert(
            rua_core::tool::MODEL_SUPPORTS_IMAGE_METADATA_KEY.into(),
            serde_json::json!(!expected),
        );
        let model = MockChatModel::text_response("unused").with_multimodal(supported);
        let ctx = react_agent::AgentContext::new(
            Arc::new(model),
            Arc::new(react_agent::AgentToolkit::new()),
            ReactConfig::default(),
            Arc::new(react_agent::hooks::NoopHook),
            tool_ctx,
        );
        assert_eq!(
            ctx.tool_ctx.metadata[rua_core::tool::MODEL_SUPPORTS_IMAGE_METADATA_KEY],
            expected
        );
    }
}
