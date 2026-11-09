use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rua_device_network::DeviceSessionCommandHandler;
use rua_server::services::chat_completion_notification::request_for_completed_chat;
use rua_server::services::conversation_authority::ConversationTurnTerminalFact;
use rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch;
use rua_server::services::device_command_bus::DeviceCommandHandler;
use rua_server::services::user_notification::{
    LocalUserNotificationAdapter, RemoteUserNotificationTransport, UserNotificationRequest,
    UserNotificationSender, UserNotificationService, UserNotificationWireEnvelope,
    USER_NOTIFICATION_WIRE_VERSION,
};

fn terminal_fact(address: rua_core::ChatAddress, status: &str) -> ConversationTurnTerminalFact {
    ConversationTurnTerminalFact {
        address,
        turn_id: "turn-1".to_string(),
        status: status.to_string(),
        completed_at: Some("2026-08-24T00:00:00Z".to_string()),
        authority_device_id: "device-a".to_string(),
        authority_epoch: ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
        device_delivery_id: None,
    }
}

#[test]
fn completed_chat_notification_targets_the_canonical_chat_uri() {
    let request = request_for_completed_chat(
        &terminal_fact(
            rua_core::ChatAddress::agent("agent-a", "chat-42"),
            "completed",
        ),
        "device-b",
        Some("Rust 架构改造讨论"),
        "agent-a",
        Some("小Rua"),
    )
    .expect("completed chat notification");

    assert_eq!(request.target_device_id, "device-b");
    assert_eq!(request.sender.id, "agent-a");
    assert_eq!(request.sender.name, "小Rua");
    assert_eq!(request.title, "Rust 架构改造讨论");
    assert_eq!(request.body.as_deref(), Some("已完成回复"));
    assert_eq!(
        request.rua_uri.as_deref(),
        Some("rua://agents/agent-a/chats/chat-42?turn=turn-1")
    );
}

#[test]
fn failed_chat_does_not_create_a_completion_notification() {
    assert!(request_for_completed_chat(
        &terminal_fact(rua_core::ChatAddress::agent("agent-a", "chat-42"), "failed"),
        "device-b",
        Some("Rust 架构改造讨论"),
        "agent-a",
        Some("小Rua"),
    )
    .is_none());
}

#[test]
fn completed_chat_notification_falls_back_to_chat_and_agent_ids() {
    let request = request_for_completed_chat(
        &terminal_fact(
            rua_core::ChatAddress::agent("agent-a", "chat-42"),
            "completed",
        ),
        "device-b",
        Some("  "),
        "agent-a",
        Some(""),
    )
    .expect("completed chat notification");

    assert_eq!(request.title, "对话 chat-42");
    assert_eq!(request.sender.name, "agent-a");
}

#[derive(Debug)]
struct CapturingLocalAdapter {
    calls: AtomicUsize,
    accepted: bool,
    requests: Mutex<Vec<UserNotificationRequest>>,
}

impl CapturingLocalAdapter {
    fn new(accepted: bool) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            accepted,
            requests: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl LocalUserNotificationAdapter for CapturingLocalAdapter {
    async fn present(&self, request: UserNotificationRequest) -> bool {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().expect("requests lock").push(request);
        self.accepted
    }
}

#[derive(Debug)]
struct CapturingRemoteTransport {
    calls: AtomicUsize,
    accepted: bool,
    requests: Mutex<Vec<UserNotificationRequest>>,
}

impl CapturingRemoteTransport {
    fn new(accepted: bool) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            accepted,
            requests: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl RemoteUserNotificationTransport for CapturingRemoteTransport {
    async fn send(&self, request: UserNotificationRequest) -> bool {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().expect("requests lock").push(request);
        self.accepted
    }
}

fn request(target_device_id: &str) -> UserNotificationRequest {
    UserNotificationRequest {
        target_device_id: target_device_id.to_string(),
        sender: UserNotificationSender {
            id: "agent:researcher".to_string(),
            name: "Research Agent".to_string(),
        },
        title: "Run complete".to_string(),
        body: Some("The requested analysis is ready.".to_string()),
        rua_uri: Some("rua://agents/researcher/chats/chat-42?turn=7#result".to_string()),
    }
}

#[test]
fn notification_wire_roundtrip_preserves_sender_payload_and_uri() {
    let request = request("device-b");
    let envelope = UserNotificationWireEnvelope::new(request.clone());

    let encoded = serde_json::to_vec(&envelope).expect("serialize notification envelope");
    let decoded: UserNotificationWireEnvelope =
        serde_json::from_slice(&encoded).expect("deserialize notification envelope");

    assert_eq!(decoded.into_current_request(), Some(request));
    let json: serde_json::Value = serde_json::from_slice(&encoded).expect("wire json");
    assert_eq!(json["message_type"], "user_notification");
    assert_eq!(json["payload"]["version"], USER_NOTIFICATION_WIRE_VERSION);
    assert_eq!(
        json["payload"]["request"]["sender"]["id"],
        "agent:researcher"
    );
    assert_eq!(
        json["payload"]["request"]["rua_uri"],
        "rua://agents/researcher/chats/chat-42?turn=7#result"
    );
}

#[tokio::test]
async fn local_target_uses_local_adapter_exactly_once() {
    let local = Arc::new(CapturingLocalAdapter::new(true));
    let remote = Arc::new(CapturingRemoteTransport::new(true));
    let service = UserNotificationService::new(
        "device-a",
        Arc::clone(&local) as Arc<dyn LocalUserNotificationAdapter>,
        Arc::clone(&remote) as Arc<dyn RemoteUserNotificationTransport>,
    );
    let expected = request("device-a");

    service.send(expected.clone()).await;

    assert_eq!(local.calls.load(Ordering::SeqCst), 1);
    assert_eq!(remote.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        local.requests.lock().expect("requests lock").as_slice(),
        &[expected]
    );
}

#[tokio::test]
async fn remote_target_uses_remote_transport_exactly_once_and_preserves_payload() {
    let local = Arc::new(CapturingLocalAdapter::new(true));
    let remote = Arc::new(CapturingRemoteTransport::new(true));
    let service = UserNotificationService::new(
        "device-a",
        Arc::clone(&local) as Arc<dyn LocalUserNotificationAdapter>,
        Arc::clone(&remote) as Arc<dyn RemoteUserNotificationTransport>,
    );
    let expected = request("device-b");

    service.send(expected.clone()).await;

    assert_eq!(local.calls.load(Ordering::SeqCst), 0);
    assert_eq!(remote.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        remote.requests.lock().expect("requests lock").as_slice(),
        &[expected]
    );
}

#[tokio::test]
async fn adapter_and_offline_transport_failures_are_silent() {
    let local = Arc::new(CapturingLocalAdapter::new(false));
    let remote = Arc::new(CapturingRemoteTransport::new(false));
    let service = UserNotificationService::new(
        "device-a",
        Arc::clone(&local) as Arc<dyn LocalUserNotificationAdapter>,
        Arc::clone(&remote) as Arc<dyn RemoteUserNotificationTransport>,
    );

    service.send(request("device-a")).await;
    service.send(request("offline-device")).await;

    assert_eq!(local.calls.load(Ordering::SeqCst), 1);
    assert_eq!(remote.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn device_session_receive_dispatches_only_to_the_local_adapter() {
    let local = Arc::new(CapturingLocalAdapter::new(true));
    let remote = Arc::new(CapturingRemoteTransport::new(true));
    let service = Arc::new(UserNotificationService::new(
        "device-a",
        Arc::clone(&local) as Arc<dyn LocalUserNotificationAdapter>,
        Arc::clone(&remote) as Arc<dyn RemoteUserNotificationTransport>,
    ));
    let handler = DeviceCommandHandler::new(
        std::path::PathBuf::from("/tmp/rua-user-notification-test"),
        "device-a".to_string(),
    )
    .with_user_notifications(service);
    let expected = request("device-a");
    let payload = serde_json::to_vec(&UserNotificationWireEnvelope::new(expected.clone()))
        .expect("serialize notification");

    let response = handler
        .handle_command(&payload)
        .await
        .expect("handle notification");

    assert!(response.is_empty());
    assert_eq!(local.calls.load(Ordering::SeqCst), 1);
    assert_eq!(remote.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        local.requests.lock().expect("requests lock").as_slice(),
        &[expected]
    );
}

#[tokio::test]
async fn wrong_target_and_unknown_wire_version_are_silent_misses() {
    let local = Arc::new(CapturingLocalAdapter::new(true));
    let remote = Arc::new(CapturingRemoteTransport::new(true));
    let service = Arc::new(UserNotificationService::new(
        "device-a",
        Arc::clone(&local) as Arc<dyn LocalUserNotificationAdapter>,
        Arc::clone(&remote) as Arc<dyn RemoteUserNotificationTransport>,
    ));
    let handler = DeviceCommandHandler::new(
        std::path::PathBuf::from("/tmp/rua-user-notification-test"),
        "device-a".to_string(),
    )
    .with_user_notifications(service);

    for envelope in [
        UserNotificationWireEnvelope::new(request("device-b")),
        UserNotificationWireEnvelope::UserNotification {
            version: USER_NOTIFICATION_WIRE_VERSION + 1,
            request: request("device-a"),
        },
    ] {
        let payload = serde_json::to_vec(&envelope).expect("serialize notification");
        assert!(handler
            .handle_command(&payload)
            .await
            .expect("handle notification")
            .is_empty());
    }

    assert_eq!(local.calls.load(Ordering::SeqCst), 0);
    assert_eq!(remote.calls.load(Ordering::SeqCst), 0);
}
