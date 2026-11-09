use std::sync::Arc;

use async_trait::async_trait;
use futures::stream;
use futures::stream::BoxStream;
use tempfile::tempdir;
use tokio::sync::Mutex;

use rua_channel::core::acl_store::AccessControlStore;
use rua_channel::core::channel::Process;
use rua_channel::core::content::{
    AgentChannelRequest, ChannelTarget, IncomingContentPart, OutgoingContentPart, OutgoingMessage,
};
use rua_channel::core::pipeline;
use rua_channel::core::renderer::{
    ChannelKind, MessageRenderOptions, MessageRenderer, RenderStyle,
};
use rua_core::RuntimeEvent;

struct MockProcess;

#[async_trait]
impl Process for MockProcess {
    async fn handle(&self, _req: AgentChannelRequest) -> BoxStream<'static, RuntimeEvent> {
        Box::pin(stream::iter(vec![
            RuntimeEvent::TextDelta {
                delta: "AGENT REPLY".into(),
                message_id: None,
            },
            RuntimeEvent::End {
                finish_reason: Some("stop".into()),
            },
        ]))
    }
}

type Sent = Arc<Mutex<Vec<(ChannelTarget, OutgoingMessage)>>>;

fn recording_send_fn(sent: Sent) -> pipeline::SendFn {
    Arc::new(move |to, msg| {
        let store = sent.clone();
        Box::pin(async move {
            store.lock().await.push((to, msg));
            Ok(())
        })
    })
}

fn make_request(metadata: serde_json::Value) -> AgentChannelRequest {
    make_request_for_user("u1", metadata)
}

fn make_request_for_user(user_id: &str, metadata: serde_json::Value) -> AgentChannelRequest {
    AgentChannelRequest {
        agent_id: "agent-1".into(),
        user_id: user_id.into(),
        session_id: "s1".into(),
        input: vec![IncomingContentPart::Text {
            text: "hi bot".into(),
        }],
        channel_id: "dingtalk".into(),
        conversation_id: "conv-1".into(),
        message_id: None,
        metadata,
    }
}

fn renderer() -> Arc<MessageRenderer> {
    Arc::new(MessageRenderer::with_options(
        RenderStyle::Markdown,
        MessageRenderOptions::for_channel_kind(ChannelKind::DingTalk),
    ))
}

fn part_text(part: &OutgoingContentPart) -> Option<&str> {
    match part {
        OutgoingContentPart::Text { text } | OutgoingContentPart::Markdown { text } => Some(text),
        _ => None,
    }
}

#[tokio::test]
async fn blacklisted_sender_gets_blocked_notice_and_no_agent_reply() {
    let dir = tempdir().unwrap();
    let store = Arc::new(AccessControlStore::new(
        dir.path().join("access_control.json"),
    ));

    std::fs::write(
        dir.path().join("access_control.json"),
        r#"{"dingtalk":{"whitelist":{},"blacklist":{"u1":{"remark":"","username":""}},"pending":[]}}"#,
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));

    let sent: Sent = Arc::new(Mutex::new(Vec::new()));
    let req = make_request(serde_json::json!({ "language": "en" }));

    pipeline::run_inbound_acl(
        Arc::new(MockProcess),
        req,
        None,
        renderer(),
        recording_send_fn(sent.clone()),
        None,
        Some(store),
    )
    .await
    .unwrap();

    let messages = sent.lock().await;
    assert_eq!(messages.len(), 1, "exactly one deny message expected");
    let text = part_text(&messages[0].1.content[0]).unwrap();
    assert_eq!(text, "You have been blocked from this agent.");
    assert_ne!(
        text, "AGENT REPLY",
        "agent must not run for blacklisted user"
    );
}

#[tokio::test]
async fn unlisted_sender_with_ac_enabled_is_recorded_pending() {
    let dir = tempdir().unwrap();
    let store = Arc::new(AccessControlStore::new(
        dir.path().join("access_control.json"),
    ));

    let sent: Sent = Arc::new(Mutex::new(Vec::new()));

    let req = make_request(serde_json::json!({
        "access_control_dm": true,
        "language": "en",
        "user_name": "Bob"
    }));

    pipeline::run_inbound_acl(
        Arc::new(MockProcess),
        req,
        None,
        renderer(),
        recording_send_fn(sent.clone()),
        None,
        Some(store.clone()),
    )
    .await
    .unwrap();

    let messages = sent.lock().await;
    assert_eq!(messages.len(), 1);
    let text = part_text(&messages[0].1.content[0]).unwrap();
    assert!(text.contains("Approval required"), "pending notice: {text}");
    assert!(text.contains("u1"), "pending notice must carry sender id");

    let pending = store.pending("dingtalk");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].user_id, "u1");
    assert_eq!(pending[0].first_message, "hi bot");
    assert_eq!(pending[0].username, "Bob");
}

#[tokio::test]
async fn whitelisted_sender_passes_through_to_agent() {
    let dir = tempdir().unwrap();
    let store = Arc::new(AccessControlStore::new(
        dir.path().join("access_control.json"),
    ));
    std::fs::write(
        dir.path().join("access_control.json"),
        r#"{"dingtalk":{"whitelist":{"u1":{"remark":"","username":""}},"blacklist":{},"pending":[]}}"#,
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));

    let sent: Sent = Arc::new(Mutex::new(Vec::new()));
    let req = make_request(serde_json::json!({
        "access_control_dm": true,
        "language": "en"
    }));

    pipeline::run_inbound_acl(
        Arc::new(MockProcess),
        req,
        None,
        renderer(),
        recording_send_fn(sent.clone()),
        None,
        Some(store),
    )
    .await
    .unwrap();

    let messages = sent.lock().await;
    assert_eq!(messages.len(), 1);
    let text = part_text(&messages[0].1.content[0]).unwrap();
    assert_eq!(
        text, "AGENT REPLY",
        "whitelisted user should reach the agent"
    );
}

#[tokio::test]
async fn ac_disabled_unlisted_sender_passes_through() {
    let dir = tempdir().unwrap();
    let store = Arc::new(AccessControlStore::new(
        dir.path().join("access_control.json"),
    ));

    let sent: Sent = Arc::new(Mutex::new(Vec::new()));

    let req = make_request(serde_json::json!({ "language": "en" }));

    pipeline::run_inbound_acl(
        Arc::new(MockProcess),
        req,
        None,
        renderer(),
        recording_send_fn(sent.clone()),
        None,
        Some(store.clone()),
    )
    .await
    .unwrap();

    let messages = sent.lock().await;
    assert_eq!(messages.len(), 1);
    let text = part_text(&messages[0].1.content[0]).unwrap();
    assert_eq!(text, "AGENT REPLY");
    assert!(
        store.pending("dingtalk").is_empty(),
        "disabled access control must not record pending"
    );
}

#[tokio::test]
async fn shared_group_session_uses_real_sender_for_blacklist() {
    let dir = tempdir().unwrap();
    let store = Arc::new(AccessControlStore::new(
        dir.path().join("access_control.json"),
    ));
    std::fs::write(
        dir.path().join("access_control.json"),
        r#"{"dingtalk":{"whitelist":{},"blacklist":{"real-user":{"remark":"","username":""}},"pending":[]}}"#,
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));

    let sent: Sent = Arc::new(Mutex::new(Vec::new()));
    let req = make_request_for_user(
        "group",
        serde_json::json!({
            "access_control_group": true,
            "is_group": true,
            "language": "en",
            "sender_id": "real-user"
        }),
    );

    pipeline::run_inbound_acl(
        Arc::new(MockProcess),
        req,
        None,
        renderer(),
        recording_send_fn(sent.clone()),
        None,
        Some(store),
    )
    .await
    .unwrap();

    let messages = sent.lock().await;
    assert_eq!(messages.len(), 1);
    let text = part_text(&messages[0].1.content[0]).unwrap();
    assert_eq!(text, "You have been blocked from this agent.");
}

#[tokio::test]
async fn shared_group_session_uses_real_sender_for_whitelist() {
    let dir = tempdir().unwrap();
    let store = Arc::new(AccessControlStore::new(
        dir.path().join("access_control.json"),
    ));
    std::fs::write(
        dir.path().join("access_control.json"),
        r#"{"dingtalk":{"whitelist":{"real-user":{"remark":"","username":""}},"blacklist":{},"pending":[]}}"#,
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));

    let sent: Sent = Arc::new(Mutex::new(Vec::new()));
    let req = make_request_for_user(
        "group",
        serde_json::json!({
            "access_control_group": true,
            "is_group": true,
            "language": "en",
            "sender_id": "real-user"
        }),
    );

    pipeline::run_inbound_acl(
        Arc::new(MockProcess),
        req,
        None,
        renderer(),
        recording_send_fn(sent.clone()),
        None,
        Some(store),
    )
    .await
    .unwrap();

    let messages = sent.lock().await;
    assert_eq!(messages.len(), 1);
    let text = part_text(&messages[0].1.content[0]).unwrap();
    assert_eq!(text, "AGENT REPLY");
}

#[tokio::test]
async fn shared_group_session_records_pending_under_real_sender() {
    let dir = tempdir().unwrap();
    let store = Arc::new(AccessControlStore::new(
        dir.path().join("access_control.json"),
    ));

    let sent: Sent = Arc::new(Mutex::new(Vec::new()));
    let req = make_request_for_user(
        "group",
        serde_json::json!({
            "access_control_group": true,
            "is_group": true,
            "language": "en",
            "sender_id": "real-user",
            "user_name": "Alice"
        }),
    );

    pipeline::run_inbound_acl(
        Arc::new(MockProcess),
        req,
        None,
        renderer(),
        recording_send_fn(sent.clone()),
        None,
        Some(store.clone()),
    )
    .await
    .unwrap();

    let messages = sent.lock().await;
    assert_eq!(messages.len(), 1);
    let text = part_text(&messages[0].1.content[0]).unwrap();
    assert!(text.contains("real-user"), "pending notice: {text}");
    assert!(
        !text.contains("group"),
        "pending notice must not expose the shared session id: {text}"
    );

    let pending = store.pending("dingtalk");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].user_id, "real-user");
    assert_eq!(pending[0].username, "Alice");
}
