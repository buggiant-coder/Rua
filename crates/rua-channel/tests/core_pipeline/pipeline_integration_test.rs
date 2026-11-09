use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::stream;
use futures::stream::BoxStream;
use tokio::sync::Mutex;

use rua_channel::core::channel::Process;
use rua_channel::core::content::{
    AgentChannelRequest, ChannelTarget, IncomingContentPart, OutgoingMessage,
};
use rua_channel::core::debouncer::BufferDebouncer;
use rua_channel::core::pipeline;
use rua_channel::core::renderer::{
    ChannelKind, MessageRenderOptions, MessageRenderer, RenderStyle,
};
use rua_channel::core::ProcessingIndicator;
use rua_core::Result;
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

#[derive(Default)]
struct RecordingIndicator {
    events: Arc<Mutex<Vec<String>>>,
    notify: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl ProcessingIndicator for RecordingIndicator {
    async fn start(&self, req: &AgentChannelRequest) -> Result<()> {
        self.events
            .lock()
            .await
            .push(format!("start:{}", req.conversation_id));
        self.notify.notify_waiters();
        Ok(())
    }

    async fn stop(&self, req: &AgentChannelRequest) -> Result<()> {
        self.events
            .lock()
            .await
            .push(format!("stop:{}", req.conversation_id));
        self.notify.notify_waiters();
        Ok(())
    }
}

struct HangingProcess {
    notify: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl Process for HangingProcess {
    async fn handle(&self, _req: AgentChannelRequest) -> BoxStream<'static, RuntimeEvent> {
        self.notify.notify_waiters();
        Box::pin(stream::pending())
    }
}

fn make_request(metadata: serde_json::Value) -> AgentChannelRequest {
    AgentChannelRequest {
        agent_id: "agent-1".into(),
        user_id: "user-1".into(),
        session_id: "session-1".into(),
        input: vec![IncomingContentPart::Text {
            text: "hello".into(),
        }],
        channel_id: "dingtalk".into(),
        conversation_id: "conv-1".into(),
        message_id: None,
        metadata,
    }
}

#[tokio::test]
async fn pipeline_sends_reply_via_send_fn() {
    let process = Arc::new(MockProcess {
        events: vec![
            RuntimeEvent::TextDelta {
                delta: "Hello ".into(),
                message_id: None,
            },
            RuntimeEvent::TextDelta {
                delta: "from agent".into(),
                message_id: None,
            },
            RuntimeEvent::End {
                finish_reason: Some("stop".into()),
            },
        ],
    });

    let sent: Arc<Mutex<Vec<(ChannelTarget, OutgoingMessage)>>> = Arc::new(Mutex::new(Vec::new()));
    let sent_clone = sent.clone();
    let send_fn: pipeline::SendFn = Arc::new(move |to, msg| {
        let store = sent_clone.clone();
        Box::pin(async move {
            store.lock().await.push((to, msg));
            Ok(())
        })
    });

    let renderer = Arc::new(MessageRenderer::with_options(
        RenderStyle::Markdown,
        MessageRenderOptions::for_channel_kind(ChannelKind::DingTalk),
    ));

    let req = make_request(serde_json::json!({
        "session_webhook": "https://example.com/webhook"
    }));

    pipeline::run_inbound(process, req, None, renderer, send_fn)
        .await
        .unwrap();

    let messages = sent.lock().await;
    assert_eq!(messages.len(), 1);

    let (target, msg) = &messages[0];
    assert_eq!(target.agent_id, "agent-1");
    assert_eq!(target.conversation_id, "conv-1");
    assert_eq!(
        target.metadata["session_webhook"].as_str().unwrap(),
        "https://example.com/webhook"
    );
    assert_eq!(msg.content.len(), 1);
    match &msg.content[0] {
        rua_channel::core::content::OutgoingContentPart::Markdown { text } => {
            assert_eq!(text, "Hello from agent");
        }
        other => panic!("expected Markdown, got {other:?}"),
    }
}

#[tokio::test]
async fn processing_indicator_starts_after_gates_and_stops_after_success() {
    let process = Arc::new(MockProcess {
        events: vec![
            RuntimeEvent::TextDelta {
                delta: "ok".into(),
                message_id: None,
            },
            RuntimeEvent::End {
                finish_reason: Some("stop".into()),
            },
        ],
    });
    let sent: Arc<Mutex<Vec<(ChannelTarget, OutgoingMessage)>>> = Arc::new(Mutex::new(Vec::new()));
    let sent_clone = sent.clone();
    let send_fn: pipeline::SendFn = Arc::new(move |to, msg| {
        let store = sent_clone.clone();
        Box::pin(async move {
            store.lock().await.push((to, msg));
            Ok(())
        })
    });
    let renderer = Arc::new(MessageRenderer::new(RenderStyle::Markdown));
    let indicator = Arc::new(RecordingIndicator::default());
    let events = indicator.events.clone();

    pipeline::run_inbound_streaming_with_indicator(
        process,
        make_request(serde_json::json!({})),
        None,
        renderer,
        send_fn,
        None,
        Some(indicator),
    )
    .await
    .unwrap();

    assert_eq!(
        events.lock().await.as_slice(),
        &["start:conv-1".to_string(), "stop:conv-1".to_string()]
    );
    assert_eq!(sent.lock().await.len(), 1);
}

#[tokio::test]
async fn processing_indicator_does_not_start_when_mention_gate_discards() {
    let process = Arc::new(MockProcess {
        events: vec![RuntimeEvent::TextDelta {
            delta: "should not run".into(),
            message_id: None,
        }],
    });
    let send_fn: pipeline::SendFn = Arc::new(move |_to, _msg| Box::pin(async move { Ok(()) }));
    let renderer = Arc::new(MessageRenderer::new(RenderStyle::Markdown));
    let indicator = Arc::new(RecordingIndicator::default());
    let events = indicator.events.clone();

    pipeline::run_inbound_streaming_with_indicator(
        process,
        make_request(serde_json::json!({
            "require_mention": true,
            "is_group": true
        })),
        None,
        renderer,
        send_fn,
        None,
        Some(indicator),
    )
    .await
    .unwrap();

    assert!(events.lock().await.is_empty());
}

#[tokio::test]
async fn processing_indicator_stops_when_send_errors() {
    let process = Arc::new(MockProcess {
        events: vec![RuntimeEvent::TextDelta {
            delta: "will fail".into(),
            message_id: None,
        }],
    });
    let send_fn: pipeline::SendFn = Arc::new(move |_to, _msg| {
        Box::pin(async move {
            Err(rua_core::RuaError::Runtime {
                message: "send failed".into(),
            })
        })
    });
    let renderer = Arc::new(MessageRenderer::new(RenderStyle::Markdown));
    let indicator = Arc::new(RecordingIndicator::default());
    let events = indicator.events.clone();

    let result = pipeline::run_inbound_streaming_with_indicator(
        process,
        make_request(serde_json::json!({})),
        None,
        renderer,
        send_fn,
        None,
        Some(indicator),
    )
    .await;

    assert!(result.is_err());
    assert_eq!(
        events.lock().await.as_slice(),
        &["start:conv-1".to_string(), "stop:conv-1".to_string()]
    );
}

#[tokio::test]
async fn processing_indicator_stops_when_pipeline_task_is_cancelled() {
    let process_started = Arc::new(tokio::sync::Notify::new());
    let process = Arc::new(HangingProcess {
        notify: process_started.clone(),
    });
    let send_fn: pipeline::SendFn = Arc::new(move |_to, _msg| Box::pin(async move { Ok(()) }));
    let renderer = Arc::new(MessageRenderer::new(RenderStyle::Markdown));
    let indicator = Arc::new(RecordingIndicator::default());
    let events = indicator.events.clone();

    let task = tokio::spawn(pipeline::run_inbound_streaming_with_indicator(
        process,
        make_request(serde_json::json!({})),
        None,
        renderer,
        send_fn,
        None,
        Some(indicator),
    ));

    tokio::time::timeout(Duration::from_secs(2), process_started.notified())
        .await
        .expect("process should start");
    task.abort();
    let _ = task.await;

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if events.lock().await.len() >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("indicator should be stopped on cancellation");

    assert_eq!(
        events.lock().await.as_slice(),
        &["start:conv-1".to_string(), "stop:conv-1".to_string()]
    );
}

#[tokio::test]
async fn debounced_group_bot_command_batch_passes_mention_gate() {
    let process = Arc::new(MockProcess {
        events: vec![
            RuntimeEvent::TextDelta {
                delta: "ack".into(),
                message_id: None,
            },
            RuntimeEvent::End {
                finish_reason: Some("stop".into()),
            },
        ],
    });

    let sent: Arc<Mutex<Vec<(ChannelTarget, OutgoingMessage)>>> = Arc::new(Mutex::new(Vec::new()));
    let sent_clone = sent.clone();
    let send_fn: pipeline::SendFn = Arc::new(move |to, msg| {
        let store = sent_clone.clone();
        Box::pin(async move {
            store.lock().await.push((to, msg));
            Ok(())
        })
    });

    let renderer = Arc::new(MessageRenderer::new(RenderStyle::Markdown));
    let debouncer = BufferDebouncer::new(Duration::from_millis(25));
    let first = make_request(serde_json::json!({
        "require_mention": true,
        "is_group": true,
        "has_bot_command": true,
    }));
    let second = make_request(serde_json::json!({
        "require_mention": true,
        "is_group": true,
    }));

    let first_run = pipeline::run_inbound(
        process.clone(),
        first,
        Some((debouncer.clone(), "session-1".into())),
        renderer.clone(),
        send_fn.clone(),
    );
    let second_run = pipeline::run_inbound(
        process,
        second,
        Some((debouncer, "session-1".into())),
        renderer,
        send_fn,
    );

    let (first_result, second_result) = tokio::join!(first_run, second_run);
    first_result.unwrap();
    second_result.unwrap();

    let messages = sent.lock().await;
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].1.content,
        vec![rua_channel::core::content::OutgoingContentPart::Markdown { text: "ack".into() }]
    );
}

#[tokio::test]
async fn debounced_group_batch_without_mention_or_command_is_discarded() {
    let process = Arc::new(MockProcess {
        events: vec![RuntimeEvent::TextDelta {
            delta: "should not send".into(),
            message_id: None,
        }],
    });

    let sent: Arc<Mutex<Vec<(ChannelTarget, OutgoingMessage)>>> = Arc::new(Mutex::new(Vec::new()));
    let sent_clone = sent.clone();
    let send_fn: pipeline::SendFn = Arc::new(move |to, msg| {
        let store = sent_clone.clone();
        Box::pin(async move {
            store.lock().await.push((to, msg));
            Ok(())
        })
    });

    let renderer = Arc::new(MessageRenderer::new(RenderStyle::Markdown));
    let debouncer = BufferDebouncer::new(Duration::from_millis(25));
    let first = make_request(serde_json::json!({
        "require_mention": true,
        "is_group": true,
    }));
    let second = make_request(serde_json::json!({
        "require_mention": true,
        "is_group": true,
    }));

    let first_run = pipeline::run_inbound(
        process.clone(),
        first,
        Some((debouncer.clone(), "session-1".into())),
        renderer.clone(),
        send_fn.clone(),
    );
    let second_run = pipeline::run_inbound(
        process,
        second,
        Some((debouncer, "session-1".into())),
        renderer,
        send_fn,
    );

    let (first_result, second_result) = tokio::join!(first_run, second_run);
    first_result.unwrap();
    second_result.unwrap();

    assert!(sent.lock().await.is_empty());
}

#[tokio::test]
async fn pipeline_splits_long_text() {
    let long_text = "x".repeat(5000);
    let process = Arc::new(MockProcess {
        events: vec![RuntimeEvent::TextDelta {
            delta: long_text,
            message_id: None,
        }],
    });

    let sent: Arc<Mutex<Vec<(ChannelTarget, OutgoingMessage)>>> = Arc::new(Mutex::new(Vec::new()));
    let sent_clone = sent.clone();
    let send_fn: pipeline::SendFn = Arc::new(move |to, msg| {
        let store = sent_clone.clone();
        Box::pin(async move {
            store.lock().await.push((to, msg));
            Ok(())
        })
    });

    let renderer = Arc::new(MessageRenderer::with_options(
        RenderStyle::Markdown,
        MessageRenderOptions::for_channel_kind(ChannelKind::DingTalk),
    ));
    let req = make_request(serde_json::json!({}));

    pipeline::run_inbound(process, req, None, renderer, send_fn)
        .await
        .unwrap();

    let messages = sent.lock().await;
    assert!(messages.len() >= 2, "expected split into >=2 messages");
    for (_, msg) in messages.iter() {
        for part in &msg.content {
            if let rua_channel::core::content::OutgoingContentPart::Markdown { text } = part {
                assert!(
                    text.len() <= 3000,
                    "chunk exceeds 3000 bytes: {}",
                    text.len()
                );
            }
        }
    }
}

#[tokio::test]
async fn pipeline_empty_stream_sends_nothing() {
    let process = Arc::new(MockProcess { events: vec![] });

    let sent: Arc<Mutex<Vec<(ChannelTarget, OutgoingMessage)>>> = Arc::new(Mutex::new(Vec::new()));
    let sent_clone = sent.clone();
    let send_fn: pipeline::SendFn = Arc::new(move |to, msg| {
        let store = sent_clone.clone();
        Box::pin(async move {
            store.lock().await.push((to, msg));
            Ok(())
        })
    });

    let renderer = Arc::new(MessageRenderer::new(RenderStyle::Markdown));
    let req = make_request(serde_json::json!({}));

    pipeline::run_inbound(process, req, None, renderer, send_fn)
        .await
        .unwrap();

    let messages = sent.lock().await;
    assert!(messages.is_empty());
}

#[tokio::test]
async fn dingtalk_roundtrip_via_pipeline() {
    let received = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let received_clone = received.clone();

    let app = axum::Router::new().route(
        "/robot/sendBySession",
        axum::routing::post(
            move |axum::extract::Json(body): axum::extract::Json<serde_json::Value>| {
                let store = received_clone.clone();
                async move {
                    store.lock().await.push(body);
                    axum::http::StatusCode::OK
                }
            },
        ),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let webhook_url = format!("http://127.0.0.1:{}/robot/sendBySession", addr.port());

    let process = Arc::new(MockProcess {
        events: vec![
            RuntimeEvent::TextDelta {
                delta: "agent reply".into(),
                message_id: None,
            },
            RuntimeEvent::End {
                finish_reason: Some("stop".into()),
            },
        ],
    });

    let renderer = Arc::new(MessageRenderer::with_options(
        RenderStyle::Markdown,
        MessageRenderOptions::for_channel_kind(ChannelKind::DingTalk),
    ));

    let send_fn: pipeline::SendFn = Arc::new({
        let sender = Arc::new(rua_channel::dingtalk::sender::DingTalkSender::new());
        move |to, msg| {
            let sender = sender.clone();
            Box::pin(async move {
                let webhook_url = to
                    .metadata
                    .get("session_webhook")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| rua_core::RuaError::Validation {
                        message: "missing session_webhook".into(),
                    })?;
                let text = rua_channel::core::content::render_parts_to_string(&msg.content);
                if text.is_empty() {
                    return Ok(());
                }
                let body = rua_channel::dingtalk::sender::DingTalkSender::build_markdown_body(
                    "Reply", &text,
                );
                sender.send_to_webhook(webhook_url, body).await
            })
        }
    });

    let req = make_request(serde_json::json!({
        "session_webhook": webhook_url,
    }));

    pipeline::run_inbound(process, req, None, renderer, send_fn)
        .await
        .unwrap();

    let messages = received.lock().await;
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["msgtype"], "markdown");
    assert_eq!(messages[0]["markdown"]["text"], "agent reply");

    server.abort();
}
