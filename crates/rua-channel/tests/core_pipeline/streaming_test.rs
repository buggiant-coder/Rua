use futures::stream;
use rua_channel::core::content::OutgoingContentPart;
use rua_channel::core::renderer::{
    ChannelKind, MessageRenderOptions, MessageRenderer, RenderStyle,
};
use rua_channel::core::streaming::collect_reply;
use rua_core::RuntimeEvent;

#[tokio::test]
async fn merges_consecutive_text_deltas() {
    let events = vec![
        RuntimeEvent::TextDelta {
            delta: "Hello ".into(),
            message_id: None,
        },
        RuntimeEvent::TextDelta {
            delta: "world".into(),
            message_id: None,
        },
    ];
    let stream = Box::pin(stream::iter(events));
    let renderer = MessageRenderer::with_options(
        RenderStyle::Markdown,
        MessageRenderOptions::for_channel_kind(ChannelKind::DingTalk),
    );
    let parts = collect_reply(stream, &renderer).await;
    assert_eq!(parts.len(), 1);
    match &parts[0] {
        OutgoingContentPart::Markdown { text } => assert_eq!(text, "Hello world"),
        other => panic!("expected Markdown, got {other:?}"),
    }
}

#[tokio::test]
async fn does_not_merge_across_tool_boundary() {
    let events = vec![
        RuntimeEvent::TextDelta {
            delta: "Before ".into(),
            message_id: None,
        },
        RuntimeEvent::Error {
            error: rua_core::RuaError::Runtime {
                message: "test error".into(),
            },
        },
        RuntimeEvent::TextDelta {
            delta: "After".into(),
            message_id: None,
        },
    ];
    let stream = Box::pin(stream::iter(events));
    let renderer = MessageRenderer::new(RenderStyle::Markdown);
    let parts = collect_reply(stream, &renderer).await;

    assert_eq!(parts.len(), 3);
}

#[tokio::test]
async fn empty_stream_produces_empty_parts() {
    let events: Vec<RuntimeEvent> = vec![];
    let stream = Box::pin(stream::iter(events));
    let renderer = MessageRenderer::new(RenderStyle::Markdown);
    let parts = collect_reply(stream, &renderer).await;
    assert!(parts.is_empty());
}

#[tokio::test]
async fn filters_thinking_for_im_renderer() {
    let events = vec![
        RuntimeEvent::ThinkingDelta {
            delta: "internal reasoning".into(),
            signature: None,
            message_id: None,
        },
        RuntimeEvent::TextDelta {
            delta: "visible reply".into(),
            message_id: None,
        },
    ];
    let stream = Box::pin(stream::iter(events));
    let renderer = MessageRenderer::with_options(
        RenderStyle::Markdown,
        MessageRenderOptions::for_channel_kind(ChannelKind::DingTalk),
    );
    let parts = collect_reply(stream, &renderer).await;
    assert_eq!(parts.len(), 1);
    match &parts[0] {
        OutgoingContentPart::Markdown { text } => assert_eq!(text, "visible reply"),
        other => panic!("expected Markdown, got {other:?}"),
    }
}
