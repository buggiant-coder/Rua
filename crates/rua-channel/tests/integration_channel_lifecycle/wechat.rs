use super::support::{recv, wait_for_len};
use axum::{routing::post, Json, Router};
use rua_channel::{
    core::{
        channel::{AgentRequest, Channel, Process},
        ChannelTarget, OutgoingContentPart, OutgoingMessage,
    },
    wechat::WechatChannel,
};
use rua_core::{
    tool::{ToolCall, ToolResult},
    RuntimeEvent,
};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use tokio::sync::{mpsc, Mutex};

struct ToolReply(mpsc::UnboundedSender<AgentRequest>);
#[async_trait::async_trait]
impl Process for ToolReply {
    async fn handle(&self, req: AgentRequest) -> futures::stream::BoxStream<'static, RuntimeEvent> {
        self.0.send(req).unwrap();
        Box::pin(futures::stream::iter(vec![
            RuntimeEvent::ToolCall {
                call: ToolCall::new("tool-1", "search", json!({})),
            },
            RuntimeEvent::ToolResult {
                result: ToolResult::text("tool-1", "found"),
            },
            RuntimeEvent::TextDelta {
                delta: "final answer".into(),
                message_id: None,
            },
            RuntimeEvent::End {
                finish_reason: None,
            },
        ]))
    }
}

#[tokio::test]
async fn poll_merges_tool_output_and_answer_at_completion_and_restarts() {
    let (events, rx) = mpsc::unbounded_channel::<Value>();
    let rx = Arc::new(Mutex::new(rx));
    let sent = Arc::new(Mutex::new(Vec::new()));
    let captured = sent.clone();
    let app = Router::new()
        .route(
            "/ilink/bot/getupdates",
            post(move || {
                let rx = rx.clone();
                async move {
                    let event = rx.lock().await.try_recv().ok();
                    if event.is_none() {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    Json(event.unwrap_or(json!({"ret":0,"msgs":[]})))
                }
            }),
        )
        .route(
            "/ilink/bot/sendmessage",
            post(move |Json(body): Json<Value>| {
                let captured = captured.clone();
                async move {
                    captured.lock().await.push(body);
                    Json(json!({"ret":0}))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let channel=WechatChannel::new("token".into(),Some(format!("http://{}",listener.local_addr().unwrap())))
        .with_render_options(Default::default()).with_acl_config(json!({"message_merge_enabled":true,"message_merge_delay_ms":0,"show_typing":false,"no_text_debounce":true}));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let (tx, mut requests) = mpsc::unbounded_channel();
    let process: Arc<dyn Process> = Arc::new(ToolReply(tx));
    for n in 0..2 {
        channel.start(process.clone()).await.unwrap();
        events
            .send(json!({"ret":0,"get_updates_buf":"cursor","msgs":[{
                "message_type":1,"from_user_id":"user","context_token":"same-context",
                "item_list":[{"type":1,"text_item":{"text":"question"}}]
            }]}))
            .unwrap();
        let _ = recv(&mut requests).await;
        wait_for_len(&sent, n + 1).await;
        let messages = sent.lock().await;
        assert_eq!(messages.len(), n + 1);
        let text = messages[n]["msg"]["item_list"][0]["text_item"]["text"]
            .as_str()
            .unwrap();
        assert!(
            text.contains("search") && text.contains("found") && text.contains("final answer"),
            "{text}"
        );
        drop(messages);
        channel.stop().await.unwrap();
    }
    server.abort();
}

#[tokio::test]
async fn delayed_proactive_merge_isolated_by_context_and_flushed_once_on_stop() {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let captured = sent.clone();
    let app = Router::new().route(
        "/ilink/bot/sendmessage",
        post(move |Json(body): Json<Value>| {
            let captured = captured.clone();
            async move {
                captured.lock().await.push(body);
                Json(json!({"ret":0}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let channel = WechatChannel::new(
        "token".into(),
        Some(format!("http://{}", listener.local_addr().unwrap())),
    )
    .with_acl_config(json!({"message_merge_enabled":true,"message_merge_delay_ms":100}));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    for (context, text) in [
        ("ctx-1", "first"),
        ("ctx-1", "second"),
        ("ctx-2", "separate"),
    ] {
        let target = ChannelTarget {
            agent_id: String::new(),
            conversation_id: "user".into(),
            sender_id: Some("user".into()),
            reply_to: None,
            metadata: json!({"wechat_context_token":context,"wechat_from_user_id":"user"}),
        };
        let message = OutgoingMessage {
            channel_id: "wechat".into(),
            conversation_id: "user".into(),
            receiver_id: Some("user".into()),
            content: vec![OutgoingContentPart::Text { text: text.into() }],
            reply_to: None,
            metadata: json!({}),
        };
        channel.send(target, message).await.unwrap();
    }
    assert!(sent.lock().await.is_empty());
    channel.stop().await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    let messages = sent.lock().await;
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["msg"]["context_token"], "ctx-1");
    assert_eq!(
        messages[0]["msg"]["item_list"][0]["text_item"]["text"],
        "first\nsecond"
    );
    assert_eq!(messages[1]["msg"]["context_token"], "ctx-2");
    assert_eq!(
        messages[1]["msg"]["item_list"][0]["text_item"]["text"],
        "separate"
    );
    server.abort();
}
