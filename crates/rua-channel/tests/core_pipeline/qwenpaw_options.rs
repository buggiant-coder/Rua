use async_trait::async_trait;
use futures::{stream, stream::BoxStream, StreamExt};
use rua_channel::core::channel::{AgentRequest, Process};
use rua_channel::core::content::{IncomingContentPart, OutgoingContentPart};
use rua_channel::core::debouncer::NoTextDebounceProcess;
use rua_channel::core::pipeline;
use rua_channel::core::renderer::{MessageRenderOptions, MessageRenderer, RenderStyle};
use rua_core::{
    tool::{ToolCall, ToolResult},
    RuntimeEvent,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Default)]
struct RecordingProcess {
    requests: Mutex<Vec<AgentRequest>>,
}
#[async_trait]
impl Process for RecordingProcess {
    async fn handle(&self, request: AgentRequest) -> BoxStream<'static, RuntimeEvent> {
        self.requests.lock().await.push(request);
        Box::pin(stream::iter(vec![
            RuntimeEvent::ToolCall {
                call: ToolCall::new("1", "search", json!({"query":"内容"})),
            },
            RuntimeEvent::ToolResult {
                result: ToolResult::text("1", "中文结果很长"),
            },
            RuntimeEvent::TextDelta {
                delta: "answer".into(),
                message_id: None,
            },
        ]))
    }
}

fn request(metadata: Value) -> AgentRequest {
    AgentRequest {
        agent_id: "agent".into(),
        channel_id: "test".into(),
        conversation_id: "group".into(),
        user_id: "user".into(),
        session_id: "old".into(),
        message_id: None,
        input: vec![IncomingContentPart::Text { text: "hi".into() }],
        metadata,
    }
}

#[tokio::test]
async fn display_overrides_are_independent_and_limit_unicode_tool_results() {
    let process = Arc::new(RecordingProcess::default());
    let replies = Arc::new(Mutex::new(Vec::new()));
    let copy = replies.clone();
    let send: pipeline::SendFn = Arc::new(move |_, message| {
        let copy = copy.clone();
        Box::pin(async move {
            copy.lock().await.push(message);
            Ok(())
        })
    });
    let options = MessageRenderOptions {
        show_tool_calls: false,
        show_tool_results: false,
        ..Default::default()
    };
    let renderer = Arc::new(MessageRenderer::with_options(
        RenderStyle::Markdown,
        options,
    ));
    pipeline::run_inbound(
        process.clone(),
        request(json!({
            "is_group":true,"share_session_in_group":false,"show_tool_calls":false,
            "show_tool_results":true,"tool_result_max_length":2,"bot_prefix":"[Rua]"
        })),
        None,
        renderer,
        send,
    )
    .await
    .unwrap();
    assert_eq!(
        process.requests.lock().await[0].session_id,
        "test:group:group:user"
    );
    let replies = replies.lock().await;
    assert_eq!(replies.len(), 2);
    assert!(
        matches!(&replies[0].content[0], OutgoingContentPart::Markdown { text } if text.contains("中文…") && !text.contains("结果"))
    );
    assert!(
        matches!(&replies[1].content[0], OutgoingContentPart::Markdown { text } if text == "[Rua]  answer")
    );
}

#[tokio::test]
async fn disabled_dms_and_groups_do_not_invoke_the_process() {
    let process = Arc::new(RecordingProcess::default());
    for metadata in [
        json!({"is_group":true,"group_disabled":true}),
        json!({"is_group":false,"dm_disabled":true}),
    ] {
        pipeline::run_inbound(
            process.clone(),
            request(metadata),
            None,
            Arc::new(MessageRenderer::new(RenderStyle::Markdown)),
            Arc::new(|_, _| Box::pin(async { panic!("should not send") })),
        )
        .await
        .unwrap();
    }
    assert!(process.requests.lock().await.is_empty());
}

#[tokio::test]
async fn media_waits_for_caption_but_audio_is_immediate_and_sessions_are_isolated() {
    let process = Arc::new(RecordingProcess::default());
    let wrapper = NoTextDebounceProcess::new(process.clone());
    let mut media = request(json!({"no_text_debounce":true}));
    media.input = vec![IncomingContentPart::Image {
        file_id: "photo".into(),
        path: "photo.png".into(),
        mime: None,
    }];
    let _ = wrapper
        .handle(media.clone())
        .await
        .collect::<Vec<_>>()
        .await;
    assert!(process.requests.lock().await.is_empty());
    let mut other = request(json!({"no_text_debounce":true}));
    other.session_id = "other".into();
    let _ = wrapper.handle(other).await.collect::<Vec<_>>().await;
    let _ = wrapper
        .handle(request(json!({"no_text_debounce":true})))
        .await
        .collect::<Vec<_>>()
        .await;
    let requests = process.requests.lock().await;
    assert_eq!(requests[0].input.len(), 1);
    assert_eq!(requests[1].input.len(), 2);
    assert_eq!(requests[1].input[0], media.input[0]);
    drop(requests);
    media.input = vec![IncomingContentPart::Audio {
        file_id: "audio".into(),
        path: "audio.mp3".into(),
        mime: None,
        name: None,
    }];
    let _ = wrapper.handle(media).await.collect::<Vec<_>>().await;
    assert_eq!(process.requests.lock().await.len(), 3);
}

#[tokio::test]
async fn group_sharing_preserves_distinct_platform_threads() {
    let process = Arc::new(RecordingProcess::default());
    for thread in ["thread-1", "thread-2"] {
        let mut req = request(
            json!({"is_group":true, "share_session_in_group":true, "feishu_thread_id":thread}),
        );
        req.session_id = format!("feishu:group:{thread}");
        pipeline::run_inbound(
            process.clone(),
            req,
            None,
            Arc::new(MessageRenderer::new(RenderStyle::Markdown)),
            Arc::new(|_, _| Box::pin(async { Ok(()) })),
        )
        .await
        .unwrap();
    }
    let requests = process.requests.lock().await;
    assert_ne!(requests[0].session_id, requests[1].session_id);
}
