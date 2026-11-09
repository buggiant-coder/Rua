use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use futures::stream;
use rua_channel::core::renderer::{
    ChannelKind, MessageRenderOptions, MessageRenderer, RenderStyle,
};
use rua_channel::core::streaming::{
    collect_reply, collect_reply_with_handler, StreamHandler, StreamType,
};
use rua_core::{Result, RuntimeEvent};

type Call = (&'static str, StreamType, String);

struct RecordingHandler {
    calls: Arc<Mutex<Vec<Call>>>,
}

#[async_trait]
impl StreamHandler for RecordingHandler {
    async fn on_start(&self, stream_type: StreamType) -> Result<()> {
        self.calls
            .lock()
            .await
            .push(("start", stream_type, String::new()));
        Ok(())
    }
    async fn on_delta(&self, stream_type: StreamType, accumulated: &str) -> Result<()> {
        self.calls
            .lock()
            .await
            .push(("delta", stream_type, accumulated.to_string()));
        Ok(())
    }
    async fn on_end(&self, stream_type: StreamType, accumulated: &str) -> Result<()> {
        self.calls
            .lock()
            .await
            .push(("end", stream_type, accumulated.to_string()));
        Ok(())
    }
}

fn text_delta(s: &str) -> RuntimeEvent {
    RuntimeEvent::TextDelta {
        delta: s.into(),
        message_id: None,
    }
}

#[tokio::test]
async fn handler_receives_start_delta_end_with_accumulation() {
    let events = vec![
        text_delta("Hello "),
        text_delta("world"),
        RuntimeEvent::End {
            finish_reason: Some("stop".into()),
        },
    ];
    let s = Box::pin(stream::iter(events));
    let renderer = MessageRenderer::with_options(
        RenderStyle::Markdown,
        MessageRenderOptions::for_channel_kind(ChannelKind::DingTalk),
    );

    let calls = Arc::new(Mutex::new(Vec::new()));
    let handler = RecordingHandler {
        calls: calls.clone(),
    };

    let outcome = collect_reply_with_handler(s, &renderer, Some(&handler)).await;

    assert!(outcome.message_streamed, "message body was streamed");
    assert_eq!(outcome.parts.len(), 1);

    let recorded = calls.lock().await;
    assert_eq!(
        *recorded,
        vec![
            ("start", StreamType::Message, String::new()),
            ("delta", StreamType::Message, "Hello ".to_string()),
            ("delta", StreamType::Message, "Hello world".to_string()),
            ("end", StreamType::Message, "Hello world".to_string()),
        ]
    );
}

#[tokio::test]
async fn handler_closes_on_stream_exhaustion_without_end() {
    let events = vec![text_delta("abc")];
    let s = Box::pin(stream::iter(events));
    let renderer = MessageRenderer::new(RenderStyle::Markdown);

    let calls = Arc::new(Mutex::new(Vec::new()));
    let handler = RecordingHandler {
        calls: calls.clone(),
    };

    collect_reply_with_handler(s, &renderer, Some(&handler)).await;

    let recorded = calls.lock().await;
    assert_eq!(recorded.last().unwrap().0, "end");
    assert_eq!(recorded.last().unwrap().1, StreamType::Message);
    assert_eq!(recorded.last().unwrap().2, "abc");
}

#[tokio::test]
async fn reasoning_hook_fires_when_thinking_not_filtered() {
    let events = vec![
        RuntimeEvent::ThinkingDelta {
            delta: "think ".into(),
            signature: None,
            message_id: None,
        },
        RuntimeEvent::ThinkingDelta {
            delta: "more".into(),
            signature: None,
            message_id: None,
        },
        text_delta("answer"),
        RuntimeEvent::End {
            finish_reason: Some("stop".into()),
        },
    ];
    let s = Box::pin(stream::iter(events));

    let renderer = MessageRenderer::new(RenderStyle::Markdown);

    let calls = Arc::new(Mutex::new(Vec::new()));
    let handler = RecordingHandler {
        calls: calls.clone(),
    };

    collect_reply_with_handler(s, &renderer, Some(&handler)).await;

    let recorded = calls.lock().await;
    assert_eq!(
        *recorded,
        vec![
            ("start", StreamType::Reasoning, String::new()),
            ("delta", StreamType::Reasoning, "think ".to_string()),
            ("delta", StreamType::Reasoning, "think more".to_string()),
            ("start", StreamType::Message, String::new()),
            ("delta", StreamType::Message, "answer".to_string()),
            ("end", StreamType::Reasoning, "think more".to_string()),
            ("end", StreamType::Message, "answer".to_string()),
        ]
    );
}

#[tokio::test]
async fn reasoning_hook_suppressed_when_thinking_filtered() {
    let events = vec![
        RuntimeEvent::ThinkingDelta {
            delta: "internal".into(),
            signature: None,
            message_id: None,
        },
        text_delta("visible"),
        RuntimeEvent::End {
            finish_reason: Some("stop".into()),
        },
    ];
    let s = Box::pin(stream::iter(events));

    let renderer = MessageRenderer::with_options(
        RenderStyle::Markdown,
        MessageRenderOptions::for_channel_kind(ChannelKind::DingTalk),
    );

    let calls = Arc::new(Mutex::new(Vec::new()));
    let handler = RecordingHandler {
        calls: calls.clone(),
    };

    collect_reply_with_handler(s, &renderer, Some(&handler)).await;

    let recorded = calls.lock().await;
    assert!(
        recorded.iter().all(|(_, t, _)| *t == StreamType::Message),
        "no Reasoning callbacks when thinking is filtered: {recorded:?}"
    );
}

#[tokio::test]
async fn none_handler_matches_collect_reply_and_does_not_stream() {
    let make_events = || {
        vec![
            text_delta("Hello "),
            text_delta("world"),
            RuntimeEvent::End {
                finish_reason: Some("stop".into()),
            },
        ]
    };
    let renderer = MessageRenderer::with_options(
        RenderStyle::Markdown,
        MessageRenderOptions::for_channel_kind(ChannelKind::DingTalk),
    );

    let baseline = collect_reply(Box::pin(stream::iter(make_events())), &renderer).await;
    let outcome =
        collect_reply_with_handler(Box::pin(stream::iter(make_events())), &renderer, None).await;

    assert!(!outcome.message_streamed, "no handler => not streamed");
    assert_eq!(outcome.parts, baseline);
}
