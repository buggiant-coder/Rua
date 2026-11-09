use rua_channel::core::renderer::{
    ChannelKind, MessageRenderOptions, MessageRenderer, RenderStyle,
};
use rua_core::RuntimeEvent;

#[test]
fn independent_display_flags_match_renderer_and_sse_without_hiding_control_events() {
    use rua_channel::core::renderer::should_send_sse_frame;
    use rua_core::tool::{ToolCall, ToolResult};
    let thinking = RuntimeEvent::ThinkingDelta {
        delta: "reason".into(),
        signature: None,
        message_id: None,
    };
    let call = RuntimeEvent::ToolCall {
        call: ToolCall::new("call", "tool", serde_json::json!({"argument":"detail"})),
    };
    let result = RuntimeEvent::ToolResult {
        result: ToolResult::text("call", "detail"),
    };
    for mask in 0..8 {
        let options = MessageRenderOptions {
            show_thinking: mask & 1 != 0,
            show_tool_calls: mask & 2 != 0,
            show_tool_results: mask & 4 != 0,
            show_tool_details: false,
            ..Default::default()
        };
        let renderer = MessageRenderer::with_options(RenderStyle::Structured, options.clone());
        for (event, visible) in [
            (&thinking, options.show_thinking),
            (&call, options.show_tool_calls),
            (&result, options.show_tool_results),
        ] {
            assert_eq!(should_send_sse_frame(event, &options), visible);
            assert_eq!(!renderer.render_event(event).is_empty(), visible);
        }
        for event in [
            RuntimeEvent::Error {
                error: rua_core::RuaError::Runtime {
                    message: "error".into(),
                },
            },
            RuntimeEvent::ToolApprovalPending {
                call_id: "call".into(),
                tool_name: "tool".into(),
                request_id: Some("approval".into()),
                prompt: None,
            },
            RuntimeEvent::ToolApprovalRequest {
                id: "approval".into(),
                name: "tool".into(),
                arguments: serde_json::json!({}),
            },
            RuntimeEvent::End {
                finish_reason: Some("stop".into()),
            },
        ] {
            assert!(should_send_sse_frame(&event, &options));
        }
        if options.show_tool_calls {
            assert!(matches!(
                &renderer.render_event(&call)[0],
                rua_channel::core::OutgoingContentPart::Tool { input: None, .. }
            ));
        }
        if options.show_tool_results {
            assert!(matches!(
                &renderer.render_event(&result)[0],
                rua_channel::core::OutgoingContentPart::Tool { output: None, .. }
            ));
        }
    }
}

#[test]
fn request_override_changes_only_the_explicit_display_flag() {
    let mut options = MessageRenderOptions::for_channel_kind(ChannelKind::DingTalk);
    options.apply_metadata(&serde_json::json!({"show_tool_calls":true}));
    assert!(options.show_tool_calls);
    assert!(!options.show_tool_results);
    assert!(!options.show_thinking);
    options.apply_metadata(
        &serde_json::json!({"show_tool_results":true,"show_thinking":true,"show_tool_calls":false}),
    );
    assert!(!options.show_tool_calls);
    assert!(options.show_tool_results);
    assert!(options.show_thinking);
}

#[test]
fn dingtalk_defaults_hide_thinking() {
    let opts = MessageRenderOptions::for_channel_kind(ChannelKind::DingTalk);
    assert!(
        !opts.show_thinking,
        "DingTalk should filter thinking by default"
    );
    assert!(
        !opts.show_tool_details,
        "DingTalk should hide tool details by default"
    );
    assert!(
        !opts.show_tool_calls && !opts.show_tool_results,
        "DingTalk IM should filter tool messages by default"
    );
}

#[test]
fn wecom_defaults_hide_thinking() {
    let opts = MessageRenderOptions::for_channel_kind(ChannelKind::WeCom);
    assert!(!opts.show_thinking);
    assert!(!opts.show_tool_details);
}

#[test]
fn console_defaults_no_filter() {
    let opts = MessageRenderOptions::for_channel_kind(ChannelKind::Console);
    assert!(opts.show_thinking, "Console should not filter thinking");
    assert!(opts.show_tool_details, "Console should show tool details");
}

#[test]
fn chat_defaults_no_filter() {
    let opts = MessageRenderOptions::for_channel_kind(ChannelKind::Chat);
    assert!(opts.show_thinking);
    assert!(opts.show_tool_details);
}

#[test]
fn channel_kind_is_im() {
    assert!(ChannelKind::DingTalk.is_im());
    assert!(ChannelKind::Weixin.is_im());
    assert!(ChannelKind::WeCom.is_im());
    assert!(ChannelKind::Discord.is_im());
    assert!(ChannelKind::Telegram.is_im());
    assert!(!ChannelKind::Console.is_im());
    assert!(!ChannelKind::Chat.is_im());
}

#[test]
fn dingtalk_renderer_filters_thinking_event() {
    let opts = MessageRenderOptions::for_channel_kind(ChannelKind::DingTalk);
    let renderer = MessageRenderer::with_options(RenderStyle::Markdown, opts);

    let thinking_event = RuntimeEvent::ThinkingDelta {
        delta: "internal reasoning".to_string(),
        signature: None,
        message_id: None,
    };
    let parts = renderer.render_event(&thinking_event);
    assert!(
        parts.is_empty(),
        "DingTalk renderer should suppress thinking events"
    );
}

#[test]
fn console_renderer_passes_thinking_event() {
    let opts = MessageRenderOptions::for_channel_kind(ChannelKind::Console);
    let renderer = MessageRenderer::with_options(RenderStyle::PlainText, opts);

    let thinking_event = RuntimeEvent::ThinkingDelta {
        delta: "internal reasoning".to_string(),
        signature: None,
        message_id: None,
    };
    let parts = renderer.render_event(&thinking_event);
    assert!(
        !parts.is_empty(),
        "Console renderer should pass thinking events through"
    );
}

#[test]
fn wecom_channel_kind_defaults() {
    let opts = MessageRenderOptions::for_channel_kind(ChannelKind::WeCom);
    assert!(!opts.show_thinking, "WeCom channel should filter thinking");
    assert!(
        !opts.show_tool_details,
        "WeCom channel should hide tool details"
    );
}

#[test]
fn wecom_render_filters_thinking_integration() {
    let opts = MessageRenderOptions::for_channel_kind(ChannelKind::WeCom);
    let renderer = MessageRenderer::with_options(RenderStyle::Markdown, opts);

    let thinking_event = RuntimeEvent::ThinkingDelta {
        delta: "secret reasoning".to_string(),
        signature: None,
        message_id: None,
    };
    let parts = renderer.render_event(&thinking_event);
    assert!(
        parts.is_empty(),
        "WeCom channel render should suppress thinking in production"
    );
}
