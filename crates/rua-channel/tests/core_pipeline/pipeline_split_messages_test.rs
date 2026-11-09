use std::sync::Arc;

use async_trait::async_trait;
use futures::stream;
use futures::stream::BoxStream;
use tokio::sync::Mutex;

use rua_channel::core::channel::Process;
use rua_channel::core::content::{
    AgentChannelRequest, ChannelTarget, IncomingContentPart, OutgoingContentPart, OutgoingMessage,
};
use rua_channel::core::pipeline;
use rua_channel::core::renderer::{MessageRenderOptions, MessageRenderer, RenderStyle};
use rua_core::tool::{ToolCall, ToolResult};
use rua_core::RuntimeEvent;

struct MockProcess {
    events: Vec<RuntimeEvent>,
}

#[async_trait]
impl Process for MockProcess {
    async fn handle(&self, _req: AgentChannelRequest) -> BoxStream<'static, RuntimeEvent> {
        Box::pin(stream::iter(self.events.clone()))
    }
}

fn make_request() -> AgentChannelRequest {
    AgentChannelRequest {
        agent_id: "agent-1".into(),
        user_id: "user-1".into(),
        session_id: "session-1".into(),
        input: vec![IncomingContentPart::Text {
            text: "hello".into(),
        }],
        channel_id: "test".into(),
        conversation_id: "conv-1".into(),
        message_id: None,
        metadata: serde_json::json!({}),
    }
}

fn renderer_all_visible() -> Arc<MessageRenderer> {
    Arc::new(MessageRenderer::with_options(
        RenderStyle::Markdown,
        MessageRenderOptions {
            show_thinking: true,
            show_tool_details: false,
            markdown: true,
            max_length: None,
            ..Default::default()
        },
    ))
}

type SentMessages = Arc<Mutex<Vec<(ChannelTarget, OutgoingMessage)>>>;

fn make_send_fn(store: &SentMessages) -> pipeline::SendFn {
    let store = store.clone();
    Arc::new(move |to, msg| {
        let store = store.clone();
        Box::pin(async move {
            store.lock().await.push((to, msg));
            Ok(())
        })
    })
}

fn extract_texts(messages: &[(ChannelTarget, OutgoingMessage)]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|(_, msg)| {
            msg.content.first().and_then(|p| match p {
                OutgoingContentPart::Markdown { text } => Some(text.clone()),
                OutgoingContentPart::Text { text } => Some(text.clone()),
                _ => None,
            })
        })
        .collect()
}

#[tokio::test]
async fn thinking_tool_text_sent_as_separate_messages() {
    let process = Arc::new(MockProcess {
        events: vec![
            RuntimeEvent::ThinkingDelta {
                delta: "Let me think...".into(),
                signature: None,
                message_id: None,
            },
            RuntimeEvent::ToolCall {
                call: ToolCall::new("call-1", "search", serde_json::json!({"q": "rust"})),
            },
            RuntimeEvent::ToolResult {
                result: ToolResult::text("call-1", "search result"),
            },
            RuntimeEvent::TextDelta {
                delta: "Here is the answer.".into(),
                message_id: None,
            },
            RuntimeEvent::End {
                finish_reason: Some("stop".into()),
            },
        ],
    });

    let sent: SentMessages = Arc::new(Mutex::new(Vec::new()));
    let send_fn = make_send_fn(&sent);
    let renderer = renderer_all_visible();

    pipeline::run_inbound(process, make_request(), None, renderer, send_fn)
        .await
        .unwrap();

    let messages = sent.lock().await;
    let texts = extract_texts(&messages);

    assert!(
        texts.len() >= 3,
        "expected >= 3 separate messages (thinking + tool + text), got {}: {:?}",
        texts.len(),
        texts
    );

    assert!(
        texts[0].contains("💭"),
        "first message should be thinking with 💭 prefix, got: {}",
        texts[0]
    );
    assert!(
        texts[0].contains("Let me think"),
        "first message should contain thinking text, got: {}",
        texts[0]
    );

    assert!(
        texts[1].contains("Tool:") && texts[1].contains("search"),
        "second message should be tool info, got: {}",
        texts[1]
    );

    let last = texts.last().unwrap();
    assert!(
        last.contains("Here is the answer"),
        "last text message should be the body, got: {}",
        last
    );
}

#[tokio::test]
async fn text_only_sends_single_message() {
    let process = Arc::new(MockProcess {
        events: vec![
            RuntimeEvent::TextDelta {
                delta: "Just text.".into(),
                message_id: None,
            },
            RuntimeEvent::End {
                finish_reason: Some("stop".into()),
            },
        ],
    });

    let sent: SentMessages = Arc::new(Mutex::new(Vec::new()));
    let send_fn = make_send_fn(&sent);
    let renderer = renderer_all_visible();

    pipeline::run_inbound(process, make_request(), None, renderer, send_fn)
        .await
        .unwrap();

    let messages = sent.lock().await;
    assert_eq!(messages.len(), 1, "text-only should produce 1 message");
    let texts = extract_texts(&messages);
    assert_eq!(texts[0], "Just text.");
}

#[tokio::test]
async fn thinking_only_sends_one_message_with_prefix() {
    let process = Arc::new(MockProcess {
        events: vec![
            RuntimeEvent::ThinkingDelta {
                delta: "Reasoning here.".into(),
                signature: None,
                message_id: None,
            },
            RuntimeEvent::End {
                finish_reason: Some("stop".into()),
            },
        ],
    });

    let sent: SentMessages = Arc::new(Mutex::new(Vec::new()));
    let send_fn = make_send_fn(&sent);
    let renderer = renderer_all_visible();

    pipeline::run_inbound(process, make_request(), None, renderer, send_fn)
        .await
        .unwrap();

    let messages = sent.lock().await;
    assert_eq!(messages.len(), 1);
    let texts = extract_texts(&messages);
    assert!(texts[0].starts_with("💭"));
    assert!(texts[0].contains("Reasoning here"));
}

#[tokio::test]
async fn multiple_tool_calls_each_get_own_message() {
    let process = Arc::new(MockProcess {
        events: vec![
            RuntimeEvent::ToolCall {
                call: ToolCall::new("call-1", "search", serde_json::json!({})),
            },
            RuntimeEvent::ToolResult {
                result: ToolResult::text("call-1", "result 1"),
            },
            RuntimeEvent::ToolCall {
                call: ToolCall::new("call-2", "read", serde_json::json!({})),
            },
            RuntimeEvent::ToolResult {
                result: ToolResult::text("call-2", "result 2"),
            },
            RuntimeEvent::TextDelta {
                delta: "Done.".into(),
                message_id: None,
            },
            RuntimeEvent::End {
                finish_reason: Some("stop".into()),
            },
        ],
    });

    let sent: SentMessages = Arc::new(Mutex::new(Vec::new()));
    let send_fn = make_send_fn(&sent);
    let renderer = renderer_all_visible();

    pipeline::run_inbound(process, make_request(), None, renderer, send_fn)
        .await
        .unwrap();

    let messages = sent.lock().await;
    let texts = extract_texts(&messages);

    assert!(
        texts.len() >= 3,
        "expected >= 3 messages (2+ tool + 1 text), got {}: {:?}",
        texts.len(),
        texts
    );

    assert!(
        texts.last().unwrap().contains("Done"),
        "last message should be text body"
    );
}
