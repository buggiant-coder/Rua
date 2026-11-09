use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::BoxStream;
use futures_util::StreamExt;
use tokio::sync::Mutex;

use rua_channel::core::channel::{AgentRequest, Channel, OutMessage, Process, ToHandle};
use rua_channel::core::content::{ChannelTarget, OutgoingContentPart};
use rua_channel::dingtalk::DingTalkChannel;
use rua_core::RuntimeEvent;

struct MockProcess {
    response_text: String,
}

#[async_trait]
impl Process for MockProcess {
    async fn handle(&self, _req: AgentRequest) -> BoxStream<'static, RuntimeEvent> {
        let events = vec![
            RuntimeEvent::Start {
                session_id: "test-session".into(),
                agent_id: Some("test-agent".into()),
            },
            RuntimeEvent::TextDelta {
                delta: self.response_text.clone(),
                message_id: Some("msg-resp-001".into()),
            },
            RuntimeEvent::End {
                finish_reason: Some("stop".into()),
            },
        ];
        futures::stream::iter(events).boxed()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn dingtalk_webhook_to_agent_to_reply_e2e() {
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

    let channel = DingTalkChannel::new().with_ws_config(String::new(), String::new(), false);

    let mock_process = Arc::new(MockProcess {
        response_text: "Hello from agent".to_string(),
    });

    channel.start(mock_process.clone()).await.unwrap();

    let native_payload = serde_json::json!({
        "msgType": "text",
        "msgId": "msg-e2e-001",
        "conversationId": "conv-e2e-123",
        "conversationType": "1",
        "senderId": "user-e2e-456",
        "senderNick": "TestUser",
        "robotCode": "bot-e2e",
        "sessionWebhook": webhook_url,
        "text": { "content": "hello agent" }
    });

    let req = channel
        .build_request_from_native(native_payload)
        .expect("parse native should succeed");

    assert_eq!(req.channel_id, "dingtalk");
    assert_eq!(req.user_id, "user-e2e-456");
    assert_eq!(req.conversation_id, "conv-e2e-123");

    let mut stream = mock_process.handle(req.clone()).await;
    let mut collected_text = String::new();
    while let Some(event) = stream.next().await {
        if let RuntimeEvent::TextDelta { delta, .. } = event {
            collected_text.push_str(&delta);
        }
    }
    assert_eq!(collected_text, "Hello from agent");

    let target: ToHandle = ChannelTarget {
        agent_id: "test-agent".into(),
        conversation_id: req.conversation_id.clone(),
        sender_id: Some(req.user_id.clone()),
        reply_to: None,
        metadata: req.metadata.clone(),
    };

    let reply: OutMessage = rua_channel::core::content::OutgoingMessage {
        channel_id: "dingtalk".into(),
        conversation_id: req.conversation_id.clone(),
        receiver_id: Some(req.user_id.clone()),
        content: vec![OutgoingContentPart::Markdown {
            text: "Hello from agent".into(),
        }],
        reply_to: None,
        metadata: serde_json::json!({}),
    };

    channel.send(target, reply).await.unwrap();

    let messages = received.lock().await;
    assert_eq!(messages.len(), 1, "mock receiver should get exactly 1 POST");
    assert_eq!(messages[0]["msgtype"], "markdown");
    assert_eq!(messages[0]["markdown"]["title"], "Reply");
    assert_eq!(messages[0]["markdown"]["text"], "Hello from agent");

    channel.stop().await.unwrap();
    server.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn dingtalk_reply_filters_thinking_content() {
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

    let channel = DingTalkChannel::new().with_ws_config(String::new(), String::new(), false);

    let mock_process = Arc::new(MockProcess {
        response_text: "visible reply".to_string(),
    });
    channel.start(mock_process.clone()).await.unwrap();

    let native_payload = serde_json::json!({
        "msgType": "text",
        "msgId": "msg-think-001",
        "conversationId": "conv-think",
        "conversationType": "1",
        "senderId": "user-think",
        "senderNick": "Thinker",
        "robotCode": "bot-think",
        "sessionWebhook": webhook_url,
        "text": { "content": "think about this" }
    });

    let req = channel.build_request_from_native(native_payload).unwrap();

    let mut stream = mock_process.handle(req.clone()).await;
    while stream.next().await.is_some() {}

    let target: ToHandle = ChannelTarget {
        agent_id: "test-agent".into(),
        conversation_id: req.conversation_id.clone(),
        sender_id: Some(req.user_id.clone()),
        reply_to: None,
        metadata: req.metadata.clone(),
    };

    let reply: OutMessage = rua_channel::core::content::OutgoingMessage {
        channel_id: "dingtalk".into(),
        conversation_id: req.conversation_id.clone(),
        receiver_id: Some(req.user_id.clone()),
        content: vec![
            OutgoingContentPart::Thinking {
                text: "internal reasoning".into(),
            },
            OutgoingContentPart::Markdown {
                text: "**visible reply**".into(),
            },
        ],
        reply_to: None,
        metadata: serde_json::json!({}),
    };

    channel.send(target, reply).await.unwrap();

    let messages = received.lock().await;
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["msgtype"], "markdown");

    assert_eq!(messages[0]["markdown"]["text"], "**visible reply**");

    channel.stop().await.unwrap();
    server.abort();
}
