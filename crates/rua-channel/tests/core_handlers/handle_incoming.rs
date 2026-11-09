use std::sync::Arc;

use async_trait::async_trait;
use rua_channel::core::content::{AgentChannelRequest, IncomingContentPart};
use rua_channel::core::queue::{QueueConsumer, UnifiedQueueManager};
use rua_channel::core::{extract_query_text, extract_query_text_from_request, ChannelManager};
use tokio::sync::mpsc;

struct RecordingConsumer {
    tx: tokio::sync::mpsc::UnboundedSender<(String, String, u32, serde_json::Value)>,
}

#[async_trait]
impl QueueConsumer for RecordingConsumer {
    async fn consume(
        &self,
        mut rx: mpsc::Receiver<serde_json::Value>,
        channel_id: String,
        session_id: String,
        priority: u32,
    ) {
        while let Some(payload) = rx.recv().await {
            let _ = self
                .tx
                .send((channel_id.clone(), session_id.clone(), priority, payload));
        }
    }
}

fn make_manager_with_recorder() -> (
    ChannelManager,
    tokio::sync::mpsc::UnboundedReceiver<(String, String, u32, serde_json::Value)>,
) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let consumer = Arc::new(RecordingConsumer { tx });
    let qm = UnifiedQueueManager::new(consumer);
    let mgr = ChannelManager::new().with_queue_manager(qm);
    (mgr, rx)
}

#[test]
fn extract_text_field() {
    let payload = serde_json::json!({"text": "hello world"});
    assert_eq!(extract_query_text(&payload), "hello world");
}

#[test]
fn extract_content_field() {
    let payload = serde_json::json!({"content": "hello"});
    assert_eq!(extract_query_text(&payload), "hello");
}

#[test]
fn extract_query_field() {
    let payload = serde_json::json!({"query": "hello"});
    assert_eq!(extract_query_text(&payload), "hello");
}

#[test]
fn extract_text_preferred_over_content() {
    let payload = serde_json::json!({"text": "text_val", "content": "content_val"});
    assert_eq!(extract_query_text(&payload), "text_val");
}

#[test]
fn extract_no_text_returns_empty() {
    let payload = serde_json::json!({"image": "data:..."});
    assert_eq!(extract_query_text(&payload), "");
}

#[test]
fn extract_multiline_returns_first_line() {
    let payload = serde_json::json!({"text": "/stop\nextra stuff\nmore"});
    assert_eq!(extract_query_text(&payload), "/stop");
}

#[test]
fn extract_single_slash_returns_slash() {
    let payload = serde_json::json!({"text": "/"});
    assert_eq!(extract_query_text(&payload), "/");
}

#[test]
fn extract_from_request_with_text() {
    let req = AgentChannelRequest {
        agent_id: String::new(),
        user_id: "u1".into(),
        session_id: "s1".into(),
        input: vec![IncomingContentPart::Text {
            text: "/stop please".into(),
        }],
        channel_id: "dingtalk".into(),
        conversation_id: "c1".into(),
        message_id: None,
        metadata: serde_json::json!({}),
    };
    assert_eq!(extract_query_text_from_request(&req), "/stop please");
}

#[test]
fn extract_from_request_multiline() {
    let req = AgentChannelRequest {
        agent_id: String::new(),
        user_id: "u1".into(),
        session_id: "s1".into(),
        input: vec![IncomingContentPart::Text {
            text: "/stop\nother stuff".into(),
        }],
        channel_id: "dingtalk".into(),
        conversation_id: "c1".into(),
        message_id: None,
        metadata: serde_json::json!({}),
    };
    assert_eq!(extract_query_text_from_request(&req), "/stop");
}

#[test]
fn extract_from_request_no_text_parts() {
    let req = AgentChannelRequest {
        agent_id: String::new(),
        user_id: "u1".into(),
        session_id: "s1".into(),
        input: vec![IncomingContentPart::Image {
            file_id: "img-001".into(),
            path: std::path::PathBuf::from("/tmp/img.png"),
            mime: None,
        }],
        channel_id: "dingtalk".into(),
        conversation_id: "c1".into(),
        message_id: None,
        metadata: serde_json::json!({}),
    };
    assert_eq!(extract_query_text_from_request(&req), "");
}

#[tokio::test]
async fn normal_message_enqueues_with_default_priority() {
    let (mgr, mut rx) = make_manager_with_recorder();
    let payload = serde_json::json!({
        "channel_id": "dingtalk",
        "text": "hello agent",
    });

    mgr.handle_incoming("dingtalk".into(), "session-1".into(), payload)
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let (ch, sess, prio, _) = rx.try_recv().unwrap();
    assert_eq!(ch, "dingtalk");
    assert_eq!(sess, "session-1");
    assert_eq!(prio, 20);

    mgr.stop_all().await.unwrap();
}

#[tokio::test]
async fn control_command_does_not_enqueue() {
    let (mgr, mut rx) = make_manager_with_recorder();
    let payload = serde_json::json!({
        "channel_id": "dingtalk",
        "text": "/stop",
    });

    mgr.handle_incoming("dingtalk".into(), "session-1".into(), payload)
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert!(rx.try_recv().is_err());

    mgr.stop_all().await.unwrap();
}

#[tokio::test]
async fn control_command_case_insensitive() {
    let (mgr, mut rx) = make_manager_with_recorder();
    let payload = serde_json::json!({"text": "/STOP"});

    mgr.handle_incoming("dingtalk".into(), "s1".into(), payload)
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(rx.try_recv().is_err());

    mgr.stop_all().await.unwrap();
}

#[tokio::test]
async fn no_text_field_enqueues_with_default_priority() {
    let (mgr, mut rx) = make_manager_with_recorder();
    let payload = serde_json::json!({"image": "base64data"});

    mgr.handle_incoming("telegram".into(), "s1".into(), payload)
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let (_, _, prio, _) = rx.try_recv().unwrap();
    assert_eq!(prio, 20);

    mgr.stop_all().await.unwrap();
}

#[tokio::test]
async fn single_slash_enqueues_normally() {
    let (mgr, mut rx) = make_manager_with_recorder();
    let payload = serde_json::json!({"text": "/"});

    mgr.handle_incoming("matrix".into(), "s1".into(), payload)
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let (_, _, prio, _) = rx.try_recv().unwrap();
    assert_eq!(prio, 20);

    mgr.stop_all().await.unwrap();
}

#[tokio::test]
async fn multiline_first_line_stop_dispatches_control() {
    let (mgr, mut rx) = make_manager_with_recorder();
    let payload = serde_json::json!({"text": "/stop\nsome more text"});

    mgr.handle_incoming("feishu".into(), "s1".into(), payload)
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(rx.try_recv().is_err());

    mgr.stop_all().await.unwrap();
}

#[tokio::test]
async fn high_priority_command_enqueues_with_level_10() {
    let (mgr, mut rx) = make_manager_with_recorder();
    let payload = serde_json::json!({"text": "/status"});

    assert!(mgr.command_registry().is_control_command("/status"));

    mgr.handle_incoming("mqtt".into(), "s1".into(), payload)
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(rx.try_recv().is_err());

    mgr.stop_all().await.unwrap();
}
