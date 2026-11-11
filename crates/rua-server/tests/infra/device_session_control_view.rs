use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_types::{ToolAnnotations, ToolCall, ToolDefinition, ToolResult};
use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine};
use futures::StreamExt;
use react_agent::{AgentToolkit, ToolExecutor};
use rua_channel::core::console::{AttachmentInfo, ConsoleChatRequest, ConsoleSubmitKind};
use rua_command::{AcpChatController, AcpChatResumeResult, AcpChatSessionManager};
use rua_contract::agent::{AgentRequest, AgentRuntime, RuntimeWarmupContext};
use rua_core::{ChatAddress, RuaError, RuntimeEvent};
use rua_device_network::device_list::{DeviceEntry, DeviceList};
use rua_device_network::{
    BoundDeviceNetworkMembership, DeviceNetworkBootstrap, DeviceNetworkIdentity,
    DeviceNetworkPreset, DeviceNetworkRuntime, DevicePeerDirectory, DeviceSessionCommandHandler,
    DeviceSessionConfig, DeviceSessionLiveActivityHandler, ProtocolHandlerRegistration,
    DEVICE_SESSION_PROTOCOL_VERSION,
};
use rua_provider::config_loader::{DefaultModel, ProviderConfig, ProviderModelEntry};
use rua_security::{ApprovalRequestInput, ApprovalTrigger, ToolCallSnapshot};
use rua_server::services::acp_chat_registry::AcpChatRuntimeHandle;
use rua_server::services::active_stream_broadcast::{
    stream_active_broadcast_via_session, ActiveStreamBroadcastDeviceSessionHandler,
    ActiveStreamBroadcastFrame, ActiveStreamBroadcastHub, ActiveStreamBroadcastSubscribeRequest,
    ActiveStreamOwner,
};
use rua_server::services::chat_activity_wake::{
    broadcast_placement_changed, spawn_chat_activity_gossip_listener,
    spawn_chat_activity_gossip_publisher, ChatActivityWakeHub, ChatLiveWake,
};
use rua_server::services::chat_metadata_store::ChatMetadataRecord;
use rua_server::services::console_chat::{ConsoleChatFacadeImpl, CONSOLE_CHANNEL_ID};
use rua_server::services::conversation_authority::{
    ConversationAcpControlView, ConversationActiveTurnStart, ConversationAllowedControls,
    ConversationAuthorityPlacementView, ConversationDriverKind, ConversationInputAcceptanceMode,
    ConversationInputPayload, ConversationInputQueueView,
};
use rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch;
use rua_server::services::conversation_history_readiness::ConversationHistorySyncRequest;
use rua_server::services::conversation_live_activity::ConversationLiveActivity;
use rua_server::services::conversation_manifest::{
    load_conversation_manifest, ConversationManifest,
};
use rua_server::services::device_command_bus::{
    deliver_acp_chat_command_via_session, deliver_collaboration_device_command_via_session,
    deliver_conversation_authority_adopt_via_session,
    deliver_conversation_authority_supersede_via_session, deliver_conversation_control_via_session,
    deliver_conversation_manifest_command_via_session, deliver_project_device_command_via_session,
    deliver_remote_chat_control_via_session, deliver_remote_conversation_input_via_session,
    deliver_terminal_device_command_via_session, AcpChatDeviceAction, AcpChatDeviceCommand,
    AcpChatDeviceResponse, AcpChatDeviceStatus, CollaborationDeviceAction,
    CollaborationDeviceCommand, CollaborationDeviceResponse, ConversationApprovalDecision,
    ConversationAuthorityAdoptDeviceCommand, ConversationAuthorityAdoptDeviceResponse,
    ConversationAuthorityRedirect, ConversationAuthoritySupersedeDeviceCommand,
    ConversationAuthoritySupersedeDeviceResponse, ConversationControlDeviceAction,
    ConversationControlDeviceCommand, ConversationControlDeviceResponse,
    ConversationManifestDeviceCommand, DeviceCommandBusError, DeviceCommandDeliveryError,
    DeviceCommandHandler, DeviceCommandResponse, ProjectDeviceAction, ProjectDeviceCommand,
    ProjectDeviceResponse, RemoteChatControlAction, RemoteChatControlCommand,
    RemoteConversationInputRequest, SyncTransferDeviceAction, SyncTransferDeviceCommand,
    SyncTransferDeviceResponse, SyncTransferFileFingerprint, TerminalDeviceAction,
    TerminalDeviceCommand, TerminalDeviceResponse,
};
use rua_server::services::durable_publication_watermark::DurablePublicationWatermarkStore;
use rua_server::services::sync_transfer::{
    SourceTransferOperationSnapshot, SyncTransferBulkHandler, TransferJobStatus,
};
use rua_server::services::terminal::{
    terminal_activity_frame_json, DeviceSessionTerminalRemoteActivityClient,
    NativeTerminalProcessSpawner, TerminalActivityFrame, TerminalAuthorityConfig,
    TerminalPtyGeometry, TerminalRemoteActivityClient, TerminalRemoteActivityRequest,
    TerminalSessionAuthority, TerminalSessionSnapshot, TerminalStreamCursor,
};
use rua_server::services::user_notification::{
    DeviceSessionUserNotificationTransport, LocalUserNotificationAdapter,
    RemoteUserNotificationTransport, UserNotificationRequest, UserNotificationSender,
    UserNotificationService,
};
use rua_server::{AppState, Priority, ServerProfile, StreamKey};
use rua_session::{
    ChatMetadata, ChatSessionHeader, NewConversationMessage, NewConversationTurn, TurnLifecycle,
    WorkspaceSessionStore,
};
use tempfile::TempDir;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

fn epoch(wall_ms: u64) -> ConversationAuthorityEpoch {
    ConversationAuthorityEpoch::from_parts(wall_ms, 0, [0; 8])
}

fn copy_tree(source: &std::path::Path, target: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&source_path, &target_path)?;
        } else {
            std::fs::copy(source_path, target_path)?;
        }
    }
    Ok(())
}

fn relative_file_paths(root: &std::path::Path) -> BTreeSet<std::path::PathBuf> {
    fn collect(
        root: &std::path::Path,
        current: &std::path::Path,
        files: &mut BTreeSet<std::path::PathBuf>,
    ) {
        let Ok(entries) = std::fs::read_dir(current) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect(root, &path, files);
            } else if let Ok(relative) = path.strip_prefix(root) {
                files.insert(relative.to_path_buf());
            }
        }
    }

    let mut files = BTreeSet::new();
    collect(root, root, &mut files);
    files
}

async fn read_project_live_snapshot<S, E>(viewer: &mut S) -> (serde_json::Value, serde_json::Value)
where
    S: futures::Stream<Item = Result<bytes::Bytes, E>> + Unpin,
    E: std::fmt::Debug,
{
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut control = None;
        let mut replay = None;
        let mut replay_events = Vec::new();
        let mut buffered = String::new();
        while control.is_none() || replay.is_none() {
            let chunk = viewer
                .next()
                .await
                .expect("Project live viewer remains open")
                .expect("Project live payload is infallible");
            buffered.push_str(std::str::from_utf8(&chunk).expect("Project live UTF-8"));
            while let Some(newline) = buffered.find('\n') {
                let line = buffered[..newline].trim().to_string();
                buffered.drain(..=newline);
                let Some(payload) = parse_project_live_sse_line(&line) else {
                    continue;
                };
                match payload.get("type").and_then(serde_json::Value::as_str) {
                    Some("conversation_control") => control = Some(payload),
                    Some("stream_replay") => {
                        replay_events.extend(
                            payload
                                .get("events")
                                .and_then(serde_json::Value::as_array)
                                .into_iter()
                                .flatten()
                                .cloned(),
                        );
                        if replay_events.iter().any(|event| {
                            event.get("type").and_then(serde_json::Value::as_str)
                                == Some("user_message")
                        }) && replay_events.iter().any(|event| {
                            event.get("type").and_then(serde_json::Value::as_str)
                                == Some("text_delta")
                        }) {
                            replay = Some(serde_json::json!({
                                "type": "stream_replay",
                                "events": replay_events,
                            }));
                        }
                    }
                    Some("error") => panic!("Project Live attachment failed: {payload}"),
                    _ => {}
                }
            }
        }
        (control.unwrap(), replay.unwrap())
    })
    .await
    .expect("Project Live snapshot timeout")
}

fn parse_project_live_sse_line(line: &str) -> Option<serde_json::Value> {
    if line.is_empty() || line.starts_with(':') {
        return None;
    }
    let data = line
        .strip_prefix("data: ")
        .expect("Project live payload uses SSE data framing");
    Some(serde_json::from_str(data).expect("Project live payload JSON"))
}

async fn read_project_history_watermark<S, E>(viewer: &mut S) -> serde_json::Value
where
    S: futures::Stream<Item = Result<bytes::Bytes, E>> + Unpin,
    E: std::fmt::Debug,
{
    let mut buffered = String::new();
    loop {
        let chunk = viewer
            .next()
            .await
            .expect("Project live viewer remains open")
            .expect("Project live payload is readable");
        buffered.push_str(std::str::from_utf8(&chunk).expect("Project live UTF-8"));
        while let Some(newline) = buffered.find('\n') {
            let line = buffered[..newline].trim().to_string();
            buffered.drain(..=newline);
            let Some(payload) = parse_project_live_sse_line(&line) else {
                continue;
            };
            if payload["type"] == "history_watermark_reached" {
                return payload;
            }
        }
    }
}

async fn read_project_live_control<S, E>(viewer: &mut S, reason: &str) -> serde_json::Value
where
    S: futures::Stream<Item = Result<bytes::Bytes, E>> + Unpin,
    E: std::fmt::Debug,
{
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut buffered = String::new();
        loop {
            let chunk = viewer
                .next()
                .await
                .expect("Project live viewer remains open")
                .expect("Project live payload is infallible");
            buffered.push_str(std::str::from_utf8(&chunk).expect("Project live UTF-8"));
            while let Some(newline) = buffered.find('\n') {
                let line = buffered[..newline].trim().to_string();
                buffered.drain(..=newline);
                let Some(payload) = parse_project_live_sse_line(&line) else {
                    continue;
                };
                if payload["type"] == "conversation_control" && payload["reason"] == reason {
                    return payload;
                }
                if payload["type"] == "error" {
                    panic!("Project Live attachment failed: {payload}");
                }
            }
        }
    })
    .await
    .expect("Project Live Control timeout")
}

async fn read_project_history_mutation<S, E>(viewer: &mut S, action: &str) -> serde_json::Value
where
    S: futures::Stream<Item = Result<bytes::Bytes, E>> + Unpin,
    E: std::fmt::Debug,
{
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut buffered = String::new();
        loop {
            let chunk = viewer
                .next()
                .await
                .expect("Project live viewer remains open")
                .expect("Project live payload is infallible");
            buffered.push_str(std::str::from_utf8(&chunk).expect("Project live UTF-8"));
            while let Some(newline) = buffered.find('\n') {
                let line = buffered[..newline].trim().to_string();
                buffered.drain(..=newline);
                let Some(payload) = parse_project_live_sse_line(&line) else {
                    continue;
                };
                if payload["type"] == "conversation_history_mutated" && payload["action"] == action
                {
                    return payload;
                }
                if payload["type"] == "error" {
                    panic!("Project Live attachment failed: {payload}");
                }
            }
        }
    })
    .await
    .expect("Project Live history mutation timeout")
}

struct RemoteProjectBlockingTool {
    started: tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

#[async_trait]
impl ToolExecutor for RemoteProjectBlockingTool {
    async fn execute(
        &self,
        call: &ToolCall,
        _ctx: &rua_core::tool::ToolContext,
    ) -> Result<ToolResult, react_agent::AgentError> {
        if let Some(started) = self.started.lock().await.take() {
            let _ = started.send(());
        }
        if let Some(release) = self.release.lock().await.take() {
            let _ = release.await;
        }
        let mut result = ToolResult::text(call.id.clone(), "remote Project background result");
        result.name = Some(call.name.clone());
        Ok(result)
    }
}

async fn read_queue_control_for_input(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<ActiveStreamBroadcastFrame>,
    input_id: &str,
) -> serde_json::Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let frame = rx.recv().await.expect("Project viewer remains attached");
            let ActiveStreamBroadcastFrame::Event { payload, .. } = frame else {
                continue;
            };
            let payload: serde_json::Value =
                serde_json::from_str(&payload).expect("Project control event JSON");
            if payload["type"] != "conversation_control" {
                continue;
            }
            let contains_input = payload["control"]["queue"]["entries"]
                .as_array()
                .is_some_and(|entries| entries.iter().any(|entry| entry["input_id"] == input_id));
            if contains_input {
                return payload;
            }
        }
    })
    .await
    .expect("Project queue Control View timeout")
}

async fn read_control_for_reason(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<ActiveStreamBroadcastFrame>,
    reason: &str,
) -> serde_json::Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let frame = rx.recv().await.expect("Project viewer remains attached");
            let ActiveStreamBroadcastFrame::Event { payload, .. } = frame else {
                continue;
            };
            let payload: serde_json::Value =
                serde_json::from_str(&payload).expect("Project control event JSON");
            if payload["type"] == "conversation_control" && payload["reason"] == reason {
                return payload;
            }
        }
    })
    .await
    .expect("Project Control View timeout")
}

struct TestRuntime {
    _dir: TempDir,
    runtime: DeviceNetworkRuntime,
    device_id: String,
    addr: iroh::EndpointAddr,
}

#[derive(Debug, Default)]
struct SessionNotificationAdapter {
    calls: AtomicUsize,
    requests: Mutex<Vec<UserNotificationRequest>>,
}

#[async_trait]
impl LocalUserNotificationAdapter for SessionNotificationAdapter {
    async fn present(&self, request: UserNotificationRequest) -> bool {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests
            .lock()
            .expect("notification lock")
            .push(request);
        true
    }
}

#[derive(Debug, Default)]
struct NeverNotificationTransport;

#[async_trait]
impl RemoteUserNotificationTransport for NeverNotificationTransport {
    async fn send(&self, _request: UserNotificationRequest) -> bool {
        panic!("receiving a notification must not route it again")
    }
}

#[derive(Clone, Default)]
struct CapturingAgentRuntime {
    requests: Arc<tokio::sync::Mutex<Vec<AgentRequest>>>,
}

#[async_trait]
impl AgentRuntime for CapturingAgentRuntime {
    async fn stream(
        &self,
        req: AgentRequest,
    ) -> rua_core::Result<Pin<Box<dyn futures::Stream<Item = Result<RuntimeEvent, RuaError>> + Send>>>
    {
        self.requests.lock().await.push(req.clone());
        Ok(Box::pin(futures::stream::iter(vec![
            Ok(RuntimeEvent::Start {
                session_id: req.session_id.clone(),
                agent_id: Some(req.agent_id.clone()),
            }),
            Ok(RuntimeEvent::End {
                finish_reason: Some("stop".to_string()),
            }),
        ])))
    }

    async fn cancel(&self, _session_id: &rua_core::id::SessionId) -> rua_core::Result<()> {
        Ok(())
    }

    async fn warmup(&self, _context: RuntimeWarmupContext) -> rua_core::Result<()> {
        Ok(())
    }

    async fn shutdown(&self) -> rua_core::Result<()> {
        Ok(())
    }
}

async fn build_runtime(
    secret_byte: u8,
    network_secret: [u8; 32],
    peers: Vec<DeviceEntry>,
    command_handler: Option<Arc<dyn DeviceSessionCommandHandler>>,
    protocols: Vec<ProtocolHandlerRegistration>,
    config: DeviceSessionConfig,
) -> TestRuntime {
    let dir = TempDir::new().expect("tempdir");
    let sync_dir = dir.path().join("sync");
    std::fs::create_dir_all(&sync_dir).expect("sync dir");
    std::fs::write(
        sync_dir.join("device_list.json"),
        serde_json::to_vec_pretty(&DeviceList {
            devices: peers,
            ..Default::default()
        })
        .expect("serialize peers"),
    )
    .expect("write peers");
    let identity = DeviceNetworkIdentity::new(iroh::SecretKey::from_bytes(&[secret_byte; 32]));
    let device_id = hex::encode(identity.local_device_id());
    let bootstrap = DeviceNetworkBootstrap::bind(
        DeviceNetworkPreset::Local,
        identity,
        BoundDeviceNetworkMembership::new(network_secret),
        DevicePeerDirectory::from_sync_dir(&sync_dir),
    )
    .await
    .expect("bind runtime");
    let runtime = bootstrap.spawn_with_session_config(protocols, command_handler, config);
    let mut addr = iroh::EndpointAddr::new(runtime.endpoint().id());
    for socket in runtime.endpoint().bound_sockets() {
        let socket = match socket {
            std::net::SocketAddr::V4(socket) if socket.ip().is_unspecified() => {
                (std::net::Ipv4Addr::LOCALHOST, socket.port()).into()
            }
            std::net::SocketAddr::V6(socket) if socket.ip().is_unspecified() => {
                (std::net::Ipv6Addr::LOCALHOST, socket.port()).into()
            }
            socket => socket,
        };
        addr.addrs.insert(iroh::TransportAddr::Ip(socket));
    }
    TestRuntime {
        _dir: dir,
        runtime,
        device_id,
        addr,
    }
}

fn peer_entry(secret_byte: u8) -> DeviceEntry {
    peer_entry_with_direct_addrs(secret_byte, Vec::new())
}

fn peer_entry_with_direct_addrs(secret_byte: u8, direct_addrs: Vec<String>) -> DeviceEntry {
    let public_key = *iroh::SecretKey::from_bytes(&[secret_byte; 32])
        .public()
        .as_bytes();
    let mut device_id = [0; 8];
    device_id.copy_from_slice(&public_key[..8]);
    DeviceEntry {
        device_id,
        public_key,
        label: format!("device-{secret_byte}"),
        friendly_name: format!("Device {secret_byte}"),
        added_at_ms: 1,
        last_seen_ms: 1,
        direct_addrs,
    }
}

fn direct_addrs(addr: &iroh::EndpointAddr) -> Vec<String> {
    addr.addrs
        .iter()
        .filter_map(|addr| match addr {
            iroh::TransportAddr::Ip(addr) => Some(addr.to_string()),
            _ => None,
        })
        .collect()
}

fn install_connectable_peer(source: &TestRuntime, target: &TestRuntime) {
    install_connectable_peers(source, &[target]);
}

fn install_connectable_peers(source: &TestRuntime, targets: &[&TestRuntime]) {
    let device_list = DeviceList {
        devices: targets
            .iter()
            .map(|target| {
                let public_key = *target.addr.id.as_bytes();
                let mut device_id = [0; 8];
                device_id.copy_from_slice(&public_key[..8]);
                DeviceEntry {
                    device_id,
                    public_key,
                    label: target.device_id.clone(),
                    friendly_name: target.device_id.clone(),
                    added_at_ms: 1,
                    last_seen_ms: 1,
                    direct_addrs: direct_addrs(&target.addr),
                }
            })
            .collect(),
        ..Default::default()
    };
    std::fs::write(
        source._dir.path().join("sync/device_list.json"),
        serde_json::to_vec_pretty(&device_list).expect("serialize connectable peer"),
    )
    .expect("write connectable peer");
}

fn test_config() -> DeviceSessionConfig {
    DeviceSessionConfig {
        connect_timeout: Duration::from_secs(2),
        handshake_timeout: Duration::from_secs(2),
        reconnect_attempts: 1,
        ..Default::default()
    }
}

fn remote_input_request(target_device_id: &str) -> RemoteConversationInputRequest {
    RemoteConversationInputRequest {
        address: rua_core::ChatAddress::agent("default".to_string(), "chat-a".to_string()),
        agent_id: "default".to_string(),
        source_device_id: "source-device".to_string(),
        target_device_id: target_device_id.to_string(),
        authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
        base_manifest_hash:
            "sha256:1111111111111111111111111111111111111111111111111111111111111111".to_string(),
        submission_id: "submission-a".to_string(),
        device_delivery_id: "delivery-a".to_string(),
        device_delivery_sequence: 1,
        text: "hello".to_string(),
        input: None,
        media_requirements: Vec::new(),
        created_at: chrono::Utc::now().to_rfc3339(),
        delivery_deadline_at: (chrono::Utc::now() + chrono::Duration::seconds(30)).to_rfc3339(),
        metadata: serde_json::Map::new(),
    }
}

#[test]
fn remote_conversation_input_wire_omits_source_history_state() {
    let value =
        serde_json::to_value(remote_input_request("target-device")).expect("serialize command");

    assert!(value.get("required_latest_sequence").is_none());
    assert!(value.get("history_tail_digest").is_none());
    assert!(value.get("history_requirement").is_none());
}

#[test]
fn acp_entered_response_supports_absent_history_preview() {
    let status = AcpChatDeviceStatus {
        active: true,
        runner: Some("codex".to_string()),
        session_id: Some("runner-session-1".to_string()),
        cwd: Some("/tmp/work".to_string()),
        chat_id: "chat-a".to_string(),
        workspace_dir: "/tmp/work".to_string(),
        target_device_id: "target-device".to_string(),
    };
    let response = AcpChatDeviceResponse::Entered {
        ok: true,
        message: "entered".to_string(),
        status: status.clone(),
        history_preview: Some("previous ACP transcript".to_string()),
    };

    let value = serde_json::to_value(&response).expect("serialize response");
    assert_eq!(value["history_preview"], "previous ACP transcript");

    let no_preview_value = serde_json::json!({
        "kind": "entered",
        "ok": true,
        "message": "entered",
        "status": status,
    });
    let decoded: AcpChatDeviceResponse =
        serde_json::from_value(no_preview_value).expect("deserialize without preview");
    assert!(matches!(
        decoded,
        AcpChatDeviceResponse::Entered {
            history_preview: None,
            ..
        }
    ));
}

#[test]
fn acp_list_runners_command_uses_snake_case_wire_action() {
    let command = AcpChatDeviceCommand {
        command_id: "acp-command-list-runners".to_string(),
        agent_id: "default".to_string(),
            address: rua_core::ChatAddress::agent("default".to_string(), "chat-a".to_string()),
        source_device_id: "source-device".to_string(),
        target_device_id: "target-device".to_string(),
        authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
        action: AcpChatDeviceAction::ListRunners,
    };

    let value = serde_json::to_value(&command).expect("serialize command");

    assert_eq!(value["action"]["action"], "list_runners");
}

fn assert_json_round_trip<T>(value: T)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let bytes = serde_json::to_vec(&value).expect("serialize wire value");
    let decoded: T = serde_json::from_slice(&bytes).expect("deserialize wire value");
    assert_eq!(decoded, value);
}

#[test]
fn v8_chat_command_variants_round_trip() {
    let running = serde_json::json!({"max_iters": 17});
    let control = rua_server::services::conversation_authority::ConversationControlView {
        background_work: Default::default(),
        revision: 3,
        authority_device_id: Some("target-device".to_string()),
        placement: ConversationAuthorityPlacementView {
            authority_device_id: Some("target-device".to_string()),
            authority_epoch: Some(epoch(2)),
            switchable: true,
            lock_reason: None,
        },
        active_turn: None,
        queue: ConversationInputQueueView {
            entries: Vec::new(),
            paused: false,
        },
        acp: ConversationAcpControlView {
            active: false,
            binding: None,
        },
        model: None,
        running_config: Some(running.clone()),
        project_chat_agent_state: None,
        allowed_controls: ConversationAllowedControls {
            stop: false,
            queue: true,
            interject: false,
            edit_queued_input: true,
            delete_queued_input: true,
            switch_authority: true,
            enter_acp: true,
            exit_acp: false,
        },
    };
    for action in [
        ConversationControlDeviceAction::ViewRunningConfig,
        ConversationControlDeviceAction::UpdateRunningConfig {
            running: running.clone(),
        },
    ] {
        assert_json_round_trip(action);
    }
    for response in [
        ConversationControlDeviceResponse::RunningConfig {
            running: running.clone(),
        },
        ConversationControlDeviceResponse::RunningConfigUpdated {
            running: running.clone(),
            control: control.clone(),
        },
    ] {
        assert_json_round_trip(response.clone());
        assert_json_round_trip(DeviceCommandResponse::ConversationControl {
            response: Box::new(response),
        });
    }

    let manifest = ConversationManifest {
        address: rua_core::ChatAddress::agent("default", "chat-a"),
        latest_turn: 1,
        latest_sequence: 2,
        turn_count: 1,
        message_count: 2,
        latest_turn_id: Some("turn-1".to_string()),
        latest_message_id: Some("message-2".to_string()),
        latest_turn_status: Some("completed".to_string()),
        extra: Default::default(),
    };
    let command = ConversationAuthorityAdoptDeviceCommand {
        command_id: "adopt-1".to_string(),
        agent_id: "default".to_string(),
            address: rua_core::ChatAddress::agent("default".to_string(), "chat-a".to_string()),
        source_device_id: "source-device".to_string(),
        authority_source_device_id: "source-device".to_string(),
        target_device_id: "target-device".to_string(),
        expected_authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
        required_manifest: manifest,
    };
    let response = ConversationAuthorityAdoptDeviceResponse {
        command_id: command.command_id.clone(),
        target_device_id: command.target_device_id.clone(),
        address: command.address.clone(),
        placement:
            rua_server::services::conversation_authority_placement::ConversationAuthorityPlacement {
                device_id: command.target_device_id.clone(),
                authority_epoch: epoch(2),
                base_manifest_hash:
                    "sha256:1111111111111111111111111111111111111111111111111111111111111111"
                        .to_string(),
            },
    };
    let gap_manifest = command.required_manifest.clone();
    assert_json_round_trip(command);
    assert_json_round_trip(response.clone());
    assert_json_round_trip(DeviceCommandResponse::ConversationAuthorityAdopt { response });
    let source_chat = ChatMetadataRecord {
        id: "chat-a".to_string(),
        name: "Chat A".to_string(),
        user_id: "default".to_string(),
        channel: "console".to_string(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        meta: serde_json::json!({
            "authority_placement": {
                "device_id": "source-device",
                "authority_epoch": epoch(1),
                "base_manifest_hash":
                    "sha256:1111111111111111111111111111111111111111111111111111111111111111",
            }
        }),
        status: "idle".to_string(),
        model: None,
        pinned: false,
        pin_order: 0,
        tag: rua_chat::ChatTag::None,
        archived: false,
    };
    let gap_action = SyncTransferDeviceAction::StartChatGapSend {
        operation_id: "gap-1".to_string(),
        agent_id: "default".to_string(),
        chat_id: "chat-a".to_string(),
        expected_authority_epoch: epoch(1),
        required_manifest: Box::new(gap_manifest),
        transfer_target_device_id: "target-device".to_string(),
        target_dir: "/target/workspace".to_string(),
        target_inventory: vec![SyncTransferFileFingerprint {
            relative_path: "chat-a/session.json".into(),
            size: 42,
            sha256: "sha256:2222222222222222222222222222222222222222222222222222222222222222"
                .to_string(),
        }],
    };
    assert_json_round_trip(gap_action.clone());
    assert_json_round_trip(SyncTransferDeviceCommand {
        command_id: "gap-command-1".to_string(),
        source_device_id: "target-device".to_string(),
        target_device_id: "source-device".to_string(),
        action: gap_action,
    });
    assert_json_round_trip(SyncTransferDeviceResponse::ChatGapTransferOperation {
        operation: SourceTransferOperationSnapshot {
            operation_id: "gap-1".to_string(),
            status: TransferJobStatus::Running,
            files_written: 0,
            bytes_written: 0,
            total_bytes: 42,
            error: None,
        },
        source_chat: Box::new(source_chat),
    });

    let replacement =
        rua_server::services::conversation_authority_placement::ConversationAuthorityPlacement {
            device_id: "target-device".to_string(),
            authority_epoch: epoch(2),
            base_manifest_hash:
                "sha256:1111111111111111111111111111111111111111111111111111111111111111"
                    .to_string(),
        };
    let supersede = ConversationAuthoritySupersedeDeviceCommand {
        command_id: "supersede-1".to_string(),
        agent_id: "default".to_string(),
        address: rua_core::ChatAddress::agent("default".to_string(), "chat-a".to_string()),
        source_device_id: "target-device".to_string(),
        target_device_id: "previous-device".to_string(),
        previous_authority_epoch: epoch(1),
        replacement: replacement.clone(),
    };
    let superseded = ConversationAuthoritySupersedeDeviceResponse {
        command_id: supersede.command_id.clone(),
        target_device_id: supersede.target_device_id.clone(),
        address: supersede.address.clone(),
        placement: replacement.clone(),
        superseded_input_ids: vec!["input-1".to_string()],
    };
    let redirect = ConversationAuthorityRedirect {
        address: rua_core::ChatAddress::agent("default".to_string(), "chat-a".to_string()),
        placement: replacement,
    };
    assert_json_round_trip(supersede);
    assert_json_round_trip(superseded.clone());
    assert_json_round_trip(DeviceCommandResponse::ConversationAuthoritySuperseded {
        response: superseded,
    });
    assert_json_round_trip(DeviceCommandResponse::AuthorityRedirect { redirect });
}

#[test]
fn wrong_target_device_is_rejected_before_conversation_input_intake() {
    let request = remote_input_request("other-device");

    let error = request
        .validate_for_target("target-device")
        .expect_err("wrong target");

    assert!(matches!(
        error,
        DeviceCommandBusError::WrongTarget { target, local }
            if target == "other-device" && local == "target-device"
    ));
}

#[test]
fn production_chat_paths_do_not_use_legacy_device_command_dials() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let sources = [
        root.join("services/remote_conversation_input.rs"),
        root.join("services/conversation_live_activity.rs"),
        root.join("endpoints/acp_chat.rs"),
        root.join("endpoints/chats.rs"),
        root.join("endpoints/console.rs"),
    ]
    .into_iter()
    .map(|path| std::fs::read_to_string(path).expect("read production Chat source"))
    .collect::<Vec<_>>()
    .join("\n");
    for legacy_call in [
        "deliver_remote_conversation_input(",
        "deliver_remote_chat_control(",
        "deliver_conversation_control(",
        "deliver_conversation_manifest_command(",
        "deliver_acp_chat_command(",
    ] {
        assert!(
            !sources.contains(legacy_call),
            "production Chat path still calls legacy helper {legacy_call}"
        );
    }
    assert!(
        !sources.contains("DEVICE_COMMAND_DELIVERY_ATTEMPTS"),
        "feature-owned Chat retry loop must be removed"
    );
    assert!(
        !sources.contains("stream_active_broadcast_from_peer("),
        "production Chat path must not use the legacy Active Stream Broadcast ALPN"
    );
}

#[test]
fn production_terminal_paths_do_not_use_legacy_device_dials() {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("services")
            .join("terminal.rs"),
    )
    .expect("read production Terminal source");
    for legacy_call in [
        "deliver_terminal_device_command(",
        "stream_active_broadcast_from_peer(",
    ] {
        assert!(
            !source.contains(legacy_call),
            "production Terminal path still calls legacy helper {legacy_call}"
        );
    }
}

#[test]
fn targeted_business_modules_cannot_own_peer_transport() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let paths = [
        "services/device_command_bus.rs",
        "services/active_stream_broadcast.rs",
        "services/remote_conversation_input.rs",
        "services/conversation_live_activity.rs",
        "services/terminal.rs",
        "services/sync_transfer.rs",
        "services/sync_transfer_tools.rs",
        "endpoints/acp_chat.rs",
        "endpoints/chats.rs",
        "endpoints/sync_transfer.rs",
    ];
    let sources = paths
        .into_iter()
        .map(|path| {
            std::fs::read_to_string(root.join(path))
                .unwrap_or_else(|error| panic!("read {path}: {error}"))
        })
        .collect::<Vec<_>>()
        .join("\n");
    for forbidden in [
        ".connect(",
        "iroh::Endpoint",
        "iroh::EndpointAddr",
        "peer_addr_for_target",
        "target_peer_addr",
        "rua-device-command/1",
        "rua-active-stream-broadcast/1",
        "rua-sync-transfer/1",
        "DeviceCommandProtocolHandler",
        "ActiveStreamBroadcastProtocolHandler",
        "SyncTransferProtocolHandler",
        "impl ProtocolHandler",
        "iroh::endpoint::Connection",
        "deliver_conversation_control_view",
        "send_conversation_control_view",
        "send_remote_conversation_control_view",
    ] {
        assert!(
            !sources.contains(forbidden),
            "targeted business transport reintroduced forbidden symbol {forbidden}"
        );
    }
}

#[test]
fn chat_protocol_cutover_has_no_superseded_runtime_paths() {
    let command_bus = include_str!("../../src/services/device_command_bus.rs");
    let chats = include_str!("../../src/endpoints/chats.rs");
    let live = include_str!("../../src/services/conversation_live_activity.rs");
    for (source, forbidden) in [
        (command_bus, "deliver_conversation_control_view"),
        (command_bus, "send_remote_conversation_control_view"),
        (chats, "verify_authority_switch_history"),
        (chats, "fetch_remote_conversation_manifest"),
        (live, "recovery_backoff"),
    ] {
        assert!(
            !source.contains(forbidden),
            "superseded Chat runtime path remains: {forbidden}"
        );
    }
    assert_eq!(DEVICE_SESSION_PROTOCOL_VERSION, 18);
    assert_eq!(
        command_bus.matches("ConversationAuthorityAdopt(").count(),
        4,
        "Authority adoption must have exactly one envelope variant and its dispatch/delivery matches"
    );
    assert_eq!(
        command_bus
            .matches("ConversationAuthoritySupersede(")
            .count(),
        4,
        "Authority supersession must have exactly one envelope variant and its dispatch/delivery matches"
    );
}

#[tokio::test]
async fn bare_legacy_conversation_input_payload_is_rejected() {
    let handler = DeviceCommandHandler::new(
        std::path::PathBuf::from("device-commands"),
        "target-device".to_string(),
    );
    let payload = serde_json::to_vec(&remote_input_request("target-device"))
        .expect("serialize legacy payload");

    let response = handler
        .handle_command(&payload)
        .await
        .expect("command response bytes");
    let response: DeviceCommandResponse =
        serde_json::from_slice(&response).expect("decode command response");
    assert!(matches!(
        response,
        DeviceCommandResponse::Rejected { ref reason, .. } if reason == "invalid_request"
    ));
}

async fn seed_authority_placement(workspace_dir: &std::path::Path, chat_id: &str, device_id: &str) {
    seed_authority_placement_at_epoch(workspace_dir, chat_id, device_id, epoch(1)).await;
}

async fn seed_authority_placement_at_epoch(
    workspace_dir: &std::path::Path,
    chat_id: &str,
    device_id: &str,
    authority_epoch: ConversationAuthorityEpoch,
) {
    let now = chrono::Utc::now();
    rua_server::services::chat_index::upsert(
        workspace_dir,
        rua_server::services::chat_index::ChatIndexEntry {
            id: chat_id.to_string(),
            name: "Authority fixture".to_string(),
            user_id: "default".to_string(),
            channel: "console".to_string(),
            created_at: now,
            updated_at: now,
            meta: serde_json::json!({
                "authority_placement": {
                    "device_id": device_id,
                    "authority_epoch": authority_epoch,
                    "base_manifest_hash":
                        "sha256:1111111111111111111111111111111111111111111111111111111111111111",
                }
            }),
            status: "idle".to_string(),
            pinned: false,
            pin_order: 0,
            tag: rua_session::ChatTag::None,
            archived: false,
        },
    )
    .await
    .expect("seed authority placement");

    let store = WorkspaceSessionStore::new(workspace_dir);
    let mut header = store
        .load_chat_header(chat_id)
        .await
        .expect("load Chat Session")
        .unwrap_or_else(|| {
            let now = now.to_rfc3339();
            ChatSessionHeader::new(ChatMetadata {
                chat_id: chat_id.to_string(),
                agent_id: "default".to_string(),
                title: "Authority fixture".to_string(),
                created_at: now.clone(),
                last_activity_at: now,
                pinned: false,
                archived: false,
                preview: None,
            })
        });
    header.extra.insert(
        "authority_placement".to_string(),
        serde_json::json!({
            "device_id": device_id,
            "authority_epoch": authority_epoch,
            "base_manifest_hash":
                "sha256:1111111111111111111111111111111111111111111111111111111111111111",
        }),
    );
    if !header.extra.contains_key("hook_configuration") {
        seed_configuration(&mut header, device_id);
    }
    store
        .save_chat_header(&header)
        .await
        .expect("seed Session authority placement");
}

async fn seed_chat_header(workspace_dir: &std::path::Path, chat_id: &str, device_id: &str) {
    let now = chrono::Utc::now().to_rfc3339();
    let mut header = ChatSessionHeader::new(ChatMetadata {
        chat_id: chat_id.to_string(),
        agent_id: "default".to_string(),
        title: "Authority fixture".to_string(),
        created_at: now.clone(),
        last_activity_at: now,
        pinned: false,
        archived: false,
        preview: None,
    });
    header.extra.insert(
        "authority_placement".to_string(),
        serde_json::json!({
            "device_id": device_id,
            "authority_epoch": "0000000000000001000000000000000000000000",
            "base_manifest_hash":
                "sha256:1111111111111111111111111111111111111111111111111111111111111111",
        }),
    );
    seed_configuration(&mut header, device_id);
    WorkspaceSessionStore::new(workspace_dir)
        .save_chat_header(&header)
        .await
        .expect("seed chat header");
}

async fn seed_project_chat_state(
    scope_root: &std::path::Path,
    address: &ChatAddress,
    device_id: &str,
) {
    seed_authority_placement(scope_root, address.chat_id().as_str(), device_id).await;
    let now = chrono::Utc::now().to_rfc3339();
    let mut header = ChatSessionHeader::new(ChatMetadata {
        chat_id: address.chat_id().to_string(),
        agent_id: "default".to_string(),
        title: "Project authority fixture".to_string(),
        created_at: now.clone(),
        last_activity_at: now.clone(),
        pinned: false,
        archived: false,
        preview: None,
    });
    header.extra.insert(
        "authority_placement".to_string(),
        serde_json::json!({
            "device_id": device_id,
            "authority_epoch": epoch(1),
            "base_manifest_hash":
                "sha256:1111111111111111111111111111111111111111111111111111111111111111",
        }),
    );
    rua_chat::apply_project_scope_defaults(&mut header, "default");
    seed_configuration(&mut header, device_id);
    WorkspaceSessionStore::new(scope_root)
        .save_chat_header(&header)
        .await
        .expect("seed Project Chat header");
}

fn seed_configuration(header: &mut ChatSessionHeader, device_id: &str) {
    let configuration = rua_server::services::product_hooks::ChatHookConfiguration {
        epoch: format!("configuration-{}", uuid::Uuid::new_v4()),
        selected_agent_id: "default".into(),
        running_config: Default::default(),
        security_config: Default::default(),
        agent_assignment: None,
        prompt_file_content: None,
        tool_config_snapshot: Default::default(),
        capability_configuration: None,
        model: header.extra.get("model").cloned(),
        thinking: None,
        collaboration_mode: serde_json::json!("default"),
        harness_configuration: None,
        authority_device_id: Some(device_id.into()),
        definition_revisions: vec![],
        effective_hooks: vec![],
        warnings: vec![],
        execution_requirements: vec![],
        extra: Default::default(),
    };
    header.extra.insert(
        "hook_configuration".into(),
        serde_json::to_value(configuration).unwrap(),
    );
}

fn fixture_provider_model(id: &str) -> ProviderModelEntry {
    ProviderModelEntry {
        id: id.to_string(),
        name: id.to_string(),
        supports_multimodal: None,
        supports_image: None,
        supports_video: None,
        probe_source: None,
        context_window: Some(128_000),
        max_output_tokens: Some(8_192),
        generate_kwargs: serde_json::Map::new(),
        thinking_enabled: None,
        thinking_budget: None,
        reasoning_effort: None,
        thinking_param_style: None,
        reasoning_effort_options: Vec::new(),
        thinking_budget_range: None,
        relay_reasoning: None,
        extra: serde_json::Map::new(),
    }
}

async fn seed_fixture_provider(state: &AppState, models: Vec<ProviderModelEntry>) {
    seed_fixture_provider_with_protocol(state, models, "OpenAIChatModel").await;
}

async fn seed_fixture_provider_with_protocol(
    state: &AppState,
    models: Vec<ProviderModelEntry>,
    chat_model: &str,
) {
    state
        .provider_cells
        .cell("fixture")
        .write(ProviderConfig {
            id: "fixture".to_string(),
            name: "Fixture".to_string(),
            base_url: "https://example.invalid/v1".to_string(),
            api_key: "test-key".to_string(),
            api_keys: Vec::new(),
            api_key_prefixes: Vec::new(),
            headers: serde_json::Map::new(),
            custom_headers: serde_json::Map::new(),
            chat_model: Some(chat_model.to_string()),
            models,
            extra_models: Vec::new(),
            timeout: None,
            generate_kwargs: serde_json::Map::new(),
            support_model_discovery: false,
            support_connection_check: false,
            is_local: false,
            api_key_prefix: String::new(),
            is_custom: true,
            is_plugin: false,
            freeze_url: false,
            require_api_key: false,
            provider_group: String::new(),
            provider_group_name: String::new(),
            provider_variant: String::new(),
            base_url_options: Vec::new(),
            thinking_param_style: None,
            reasoning_effort_options: Vec::new(),
            thinking_budget_range: None,
            extra: serde_json::Map::new(),
        })
        .await
        .expect("seed fixture provider");
    state
        .provider_manager
        .reload_provider("fixture")
        .await
        .expect("reload fixture provider");
}

#[tokio::test]
async fn responses_protocol_normalizes_effort_without_provider_or_model_catalog_entries() {
    let state_dir = TempDir::new().expect("state tempdir");
    let state = AppState::from_tempdir(state_dir.path().to_path_buf(), ServerProfile::Test);
    seed_fixture_provider_with_protocol(
        &state,
        vec![fixture_provider_model("custom-reasoning-model")],
        "OpenAIResponseModel",
    )
    .await;

    let normalized = state
        .conversation_input_intake
        .resolve_initial_authority_chat_model(Some(serde_json::json!({
            "provider_id": "fixture",
            "model": "custom-reasoning-model",
            "thinking_strength": "high"
        })))
        .await
        .expect("Responses protocol model should normalize")
        .expect("normalized model slot");

    assert_eq!(normalized["thinking_param_style"], "effort");
    assert_eq!(normalized["thinking_strength"], "high");
    assert!(match normalized
        .get("reasoning_effort_options")
        .and_then(serde_json::Value::as_array)
    {
        Some(options) => options.is_empty(),
        None => true,
    });
}

async fn seed_adoptable_chat(
    workspace_dir: &std::path::Path,
    chat_id: &str,
    authority_device_id: &str,
    model_id: &str,
) {
    seed_adoptable_empty_chat(workspace_dir, chat_id, authority_device_id, model_id).await;
    let store = WorkspaceSessionStore::new(workspace_dir);
    store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: format!("{chat_id}-turn-1"),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-07-30T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-07-30T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-07-30T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: None,
                messages: vec![
                    NewConversationMessage {
                        message_id: format!("{chat_id}-user-1"),
                        role: "user".to_string(),
                        created_at: "2026-07-30T00:00:00Z".to_string(),
                        status: "completed".to_string(),
                        content: serde_json::json!([{"type": "text", "text": "hello"}]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                    NewConversationMessage {
                        message_id: format!("{chat_id}-assistant-1"),
                        role: "assistant".to_string(),
                        created_at: "2026-07-30T00:00:02Z".to_string(),
                        status: "completed".to_string(),
                        content: serde_json::json!([{"type": "text", "text": "hi"}]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                ],
            },
        )
        .await
        .expect("append adoptable turn");
}

async fn seed_adoptable_empty_chat(
    workspace_dir: &std::path::Path,
    chat_id: &str,
    authority_device_id: &str,
    model_id: &str,
) {
    seed_authority_placement(workspace_dir, chat_id, authority_device_id).await;
    seed_chat_header(workspace_dir, chat_id, authority_device_id).await;
    let store = WorkspaceSessionStore::new(workspace_dir);
    let mut header = store
        .load_chat_header(chat_id)
        .await
        .expect("load adoptable chat header")
        .expect("adoptable chat header");
    header.extra.insert(
        "model".to_string(),
        serde_json::json!({
            "provider_id": "fixture",
            "model": model_id,
        }),
    );
    let mut configuration = header.extra["hook_configuration"].clone();
    let old_epoch = configuration["epoch"].as_str().unwrap().to_string();
    configuration["epoch"] = serde_json::json!(format!("configuration-{}", uuid::Uuid::new_v4()));
    configuration["model"] = header.extra["model"].clone();
    store
        .replace_chat_configuration(chat_id, &old_epoch, configuration)
        .await
        .unwrap();
    store
        .save_chat_header(&header)
        .await
        .expect("save adoptable chat model");
}

fn authority_adopt_command(
    command_id: &str,
    source_device_id: &str,
    target_device_id: &str,
    chat_id: &str,
    required_manifest: ConversationManifest,
) -> ConversationAuthorityAdoptDeviceCommand {
    ConversationAuthorityAdoptDeviceCommand {
        command_id: command_id.to_string(),
        agent_id: "default".to_string(),
            address: rua_core::ChatAddress::agent("default".to_string(), chat_id.to_string()),
        source_device_id: source_device_id.to_string(),
        authority_source_device_id: source_device_id.to_string(),
        target_device_id: target_device_id.to_string(),
        expected_authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
        required_manifest,
    }
}

fn command_handler(state: &AppState, target_device_id: &str) -> DeviceCommandHandler {
    let collaboration_binding = Arc::new(std::sync::OnceLock::new());
    collaboration_binding
        .set(Arc::downgrade(&state.collaboration))
        .expect("install collaboration binding");
    DeviceCommandHandler::new(
        state.rua_root.join("device_commands"),
        target_device_id.to_string(),
    )
    .with_acp_chat_registry(
        Arc::clone(&state.acp_chat_registry),
        Arc::clone(&state.registry),
    )
    .with_acp_permission_relay(Arc::clone(&state.acp_permission_relay))
    .with_remote_chat_control_streams(Arc::clone(&state.streams))
    .with_conversation_input_intake(state.conversation_input_intake.clone())
    .with_collaboration_binding(collaboration_binding)
    .with_conversation_authority(Arc::clone(&state.conversation_authority))
    .with_sync_transfer_session_manager(Arc::clone(&state.sync_transfer_session_manager))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_live_target_distinguishes_wrong_owner_from_unknown_chat() {
    let target_dir = TempDir::new().expect("target state");
    let target_state = AppState::from_tempdir(target_dir.path().to_path_buf(), ServerProfile::Test);
    let target_device_id =
        rua_server::services::device_context::current_sync_device_id(&target_state.sync_device)
            .expect("target Device identity");
    let first_root = target_dir.path().join("first-project");
    let second_root = target_dir.path().join("second-project");
    std::fs::create_dir_all(&first_root).unwrap();
    std::fs::create_dir_all(&second_root).unwrap();
    let first = target_state
        .project_catalog
        .create_bound(rua_project::ProjectCreateRequest {
            resource_device_id: target_device_id.clone(),
            resource_root: first_root.to_string_lossy().to_string(),
            display_name: Some("First".to_string()),
        })
        .unwrap()
        .project;
    let second = target_state
        .project_catalog
        .create_bound(rua_project::ProjectCreateRequest {
            resource_device_id: target_device_id.clone(),
            resource_root: second_root.to_string_lossy().to_string(),
            display_name: Some("Second".to_string()),
        })
        .unwrap()
        .project;
    let chat_id = "00000000-0000-4000-8000-000000000426";
    let first_address = ChatAddress::project(first.project_id.clone(), chat_id);
    let first_scope =
        rua_core::ChatScopeStore::resolve(&target_state.rua_root, first_address.clone());
    seed_project_chat_state(first_scope.root(), &first_address, &target_device_id).await;
    let handler = ActiveStreamBroadcastDeviceSessionHandler::new(Arc::clone(
        &target_state.active_stream_broadcast_hub,
    ))
    .with_conversation_control_initial_state(
        Arc::clone(&target_state.conversation_authority),
        target_state.conversation_input_intake.clone(),
        target_device_id,
    );

    for (address, expected) in [
        (
            ChatAddress::project(second.project_id.clone(), chat_id),
            format!("belongs to Project {}", first.project_id),
        ),
        (
            ChatAddress::project(second.project_id, "00000000-0000-4000-8000-000000000427"),
            "Project Chat not found".to_string(),
        ),
    ] {
        let payload = serde_json::to_vec(&ActiveStreamBroadcastSubscribeRequest {
            device_delivery_id: String::new(),
            owner: Some(ActiveStreamOwner::chat(address)),
            after_seq_by_stream: BTreeMap::new(),
            activity_generation: None,
            conversation_activity_position: None,
            collapse_replay: true,
            initial_conversation_control: true,
            authority_epoch: None,
            viewer: None,
        })
        .unwrap();
        let error = match handler.subscribe_live_activity(&payload).await {
            Ok(_) => panic!("invalid Project Chat address must be rejected"),
            Err(error) => error,
        };
        assert!(
            error.contains(&expected),
            "expected '{expected}', got '{error}'"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_project_live_replays_one_activity_to_two_viewer_devices() {
    let first_dir = TempDir::new().expect("first viewer state");
    let first_state = AppState::from_tempdir(first_dir.path().to_path_buf(), ServerProfile::Test);
    let second_dir = TempDir::new().expect("second viewer state");
    let second_state = AppState::from_tempdir(second_dir.path().to_path_buf(), ServerProfile::Test);
    let target_dir = TempDir::new().expect("target state");
    let target_state = AppState::from_tempdir(target_dir.path().to_path_buf(), ServerProfile::Test);
    let network_secret = [95; 32];
    let target_device_id = hex::encode(
        DeviceNetworkIdentity::new(iroh::SecretKey::from_bytes(&[97; 32])).local_device_id(),
    );
    let target_repo = target_dir.path().join("multi-viewer-project");
    std::fs::create_dir_all(&target_repo).unwrap();
    let project = target_state
        .project_catalog
        .create_bound(rua_project::ProjectCreateRequest {
            resource_device_id: target_device_id.clone(),
            resource_root: target_repo.to_string_lossy().to_string(),
            display_name: Some("Multi Viewer".to_string()),
        })
        .unwrap()
        .project;
    first_state
        .project_catalog
        .apply_descriptor(project.clone())
        .unwrap();
    second_state
        .project_catalog
        .apply_descriptor(project.clone())
        .unwrap();
    let chat_id = "00000000-0000-4000-8000-000000000428";
    let address = ChatAddress::project(project.project_id.clone(), chat_id);
    let target_scope = rua_core::ChatScopeStore::resolve(&target_state.rua_root, address.clone());
    seed_project_chat_state(target_scope.root(), &address, &target_device_id).await;

    let first = build_runtime(
        96,
        network_secret,
        vec![peer_entry(97)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let target = build_runtime(
        97,
        network_secret,
        vec![peer_entry(96), peer_entry(98)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let second = build_runtime(
        98,
        network_secret,
        vec![peer_entry(97)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    install_connectable_peer(&first, &target);
    install_connectable_peer(&second, &target);
    target
        .runtime
        .session_manager()
        .set_live_activity_handler(Arc::new(
            ActiveStreamBroadcastDeviceSessionHandler::new(Arc::clone(
                &target_state.active_stream_broadcast_hub,
            ))
            .with_conversation_control_initial_state(
                Arc::clone(&target_state.conversation_authority),
                target_state.conversation_input_intake.clone(),
                target_device_id.clone(),
            ),
        ));
    *first_state.device_network.lock().await = Some(first.runtime.clone());
    *second_state.device_network.lock().await = Some(second.runtime.clone());

    let placement =
        rua_server::services::conversation_authority_target::load_chat_authority_placement_target(
            target_scope.root(),
            chat_id,
        )
        .await
        .unwrap();
    target_state
        .active_stream_broadcast_hub
        .publish_conversation_event(
            "multi-viewer-user",
            &address,
            placement.authority_epoch,
            serde_json::json!({
                "type": "user_message",
                "message": {
                    "id": "user-multi-viewer",
                    "role": "user",
                    "turn_id": "turn-multi-viewer",
                    "content": [{"type": "text", "text": "Observe together"}],
                },
            })
            .to_string(),
        )
        .await;
    target_state
        .active_stream_broadcast_hub
        .publish_conversation_event(
            "multi-viewer-output",
            &address,
            placement.authority_epoch,
            serde_json::json!({
                "type": "text_delta",
                "delta": "shared output",
            })
            .to_string(),
        )
        .await;

    let first_app = rua_server::router::build_app(first_state.clone(), None);
    let first_response = first_app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/api/projects/{}/chats/{chat_id}/console/live",
                    project.project_id
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "viewer_id": "viewer-first",
                        "agent_id": "default",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first_response.status(), StatusCode::OK);
    assert_eq!(
        first_response.headers()["content-type"],
        "text/event-stream"
    );
    assert_eq!(first_response.headers()["x-accel-buffering"], "no");
    let mut first_viewer = first_response.into_body().into_data_stream();
    let (first_control, first_replay) = read_project_live_snapshot(&mut first_viewer).await;

    let second_app = rua_server::router::build_app(second_state.clone(), None);
    let second_response = second_app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/api/projects/{}/chats/{chat_id}/console/live",
                    project.project_id
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "viewer_id": "viewer-second",
                        "agent_id": "default",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second_response.status(), StatusCode::OK);
    assert_eq!(
        second_response.headers()["content-type"],
        "text/event-stream"
    );
    assert_eq!(second_response.headers()["x-accel-buffering"], "no");
    let mut second_viewer = second_response.into_body().into_data_stream();
    let (second_control, second_replay) = read_project_live_snapshot(&mut second_viewer).await;

    assert_eq!(
        first_control["control"]["placement"]["authority_device_id"],
        target_device_id
    );
    assert_eq!(
        first_control["activity_generation"],
        second_control["activity_generation"]
    );
    assert_eq!(first_replay["events"], second_replay["events"]);
    let sequences = first_replay["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|event| event["activity_sequence"].as_u64())
        .collect::<Vec<_>>();
    assert!(
        sequences.windows(2).all(|window| window[0] < window[1]),
        "Project replay sequence must be strictly monotonic"
    );

    drop(first_viewer);
    target_state
        .active_stream_broadcast_hub
        .publish_conversation_event(
            "multi-viewer-after-detach",
            &address,
            placement.authority_epoch,
            serde_json::json!({
                "type": "text_delta",
                "delta": "second viewer remains attached",
            })
            .to_string(),
        )
        .await;
    let remaining_payload = tokio::time::timeout(Duration::from_secs(5), async {
        let mut buffered = String::new();
        loop {
            let chunk = second_viewer
                .next()
                .await
                .expect("second Project viewer remains open")
                .expect("Project live payload is infallible");
            buffered.push_str(std::str::from_utf8(&chunk).unwrap());
            while let Some(newline) = buffered.find('\n') {
                let line = buffered[..newline].trim().to_string();
                buffered.drain(..=newline);
                let Some(payload) = parse_project_live_sse_line(&line) else {
                    continue;
                };
                if payload["delta"] == "second viewer remains attached" {
                    return payload;
                }
            }
        }
    })
    .await
    .expect("remaining Project viewer must continue receiving output");
    assert_eq!(remaining_payload["type"], "text_delta");

    drop(second_viewer);
    first
        .runtime
        .shutdown()
        .await
        .expect("shutdown first viewer");
    second
        .runtime
        .shutdown()
        .await
        .expect("shutdown second viewer");
    target.runtime.shutdown().await.expect("shutdown target");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_project_stop_and_queue_controls_do_not_require_controller_chat_storage() {
    let source_dir = TempDir::new().expect("source state");
    let source_state = AppState::from_tempdir(source_dir.path().to_path_buf(), ServerProfile::Test);
    let target_dir = TempDir::new().expect("target state");
    let target_state = AppState::from_tempdir(target_dir.path().to_path_buf(), ServerProfile::Test);
    let network_secret = [101; 32];
    let target_sync_identity: rua_sync::identity::DeviceIdentity =
        serde_json::from_value(serde_json::json!({"secret_bytes": vec![103_u8; 32]}))
            .expect("fixed target Device identity");
    *target_state.sync_device.write().expect("sync Device lock") =
        Some(Arc::new(target_sync_identity));
    let target_device_id = hex::encode(
        DeviceNetworkIdentity::new(iroh::SecretKey::from_bytes(&[103; 32])).local_device_id(),
    );
    let target_repo = target_dir.path().join("project-control-no-replica");
    std::fs::create_dir_all(&target_repo).unwrap();
    let project = target_state
        .project_catalog
        .create_bound(rua_project::ProjectCreateRequest {
            resource_device_id: target_device_id.clone(),
            resource_root: target_repo.to_string_lossy().to_string(),
            display_name: Some("Project Control".to_string()),
        })
        .unwrap()
        .project;
    source_state
        .project_catalog
        .apply_descriptor(project.clone())
        .unwrap();
    let chat_id = "00000000-0000-4000-8000-000000000429";
    let address = ChatAddress::project(project.project_id.clone(), chat_id);
    let target_scope = rua_core::ChatScopeStore::resolve(&target_state.rua_root, address.clone());
    seed_project_chat_state(target_scope.root(), &address, &target_device_id).await;
    let mut first_viewer = target_state
        .active_stream_broadcast_hub
        .subscribe_chat(&address, &BTreeMap::new())
        .await
        .rx
        .expect("first Project control viewer");
    let mut second_viewer = target_state
        .active_stream_broadcast_hub
        .subscribe_chat(&address, &BTreeMap::new())
        .await
        .rx
        .expect("second Project control viewer");
    let placement =
        rua_server::services::conversation_authority_target::load_chat_authority_placement_target(
            target_scope.root(),
            chat_id,
        )
        .await
        .unwrap();
    let active_turn = target_state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: address.clone(),
            driver: ConversationDriverKind::Rua,
            authority_device_id: target_device_id.clone(),
            authority_epoch: placement.authority_epoch,
            driver_detail: None,
        })
        .await;

    let target_handler = full_command_handler(&target_state, &target_device_id);
    let source = build_runtime(
        102,
        network_secret,
        vec![peer_entry(103)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let target = build_runtime(
        103,
        network_secret,
        vec![peer_entry(102)],
        Some(target_handler),
        Vec::new(),
        test_config(),
    )
    .await;
    install_connectable_peer(&source, &target);
    *source_state.device_network.lock().await = Some(source.runtime.clone());

    let outcome = source_state
        .conversation_input_intake
        .submit_project_delivery(
            &project.project_id,
            "default",
            ConsoleChatRequest {
                message: "edit only on authority".to_string(),
                chat_id: Some(chat_id.to_string()),
                model: None,
                is_reconnect: false,
                attachments: Vec::new(),
                input: None,
                messages: None,
                extra: serde_json::Map::new(),
                channel_kind: Some(rua_channel::core::renderer::ChannelKind::Chat),
            },
        )
        .await;
    let queued_input_id = match outcome {
        rua_server::services::conversation_input_intake::ConversationInputDeliveryOutcome::Delivered {
            transport: rua_server::services::conversation_input_intake::ConversationInputDeliveryTransport::RemoteAuthority,
            accepted: true,
            input_id: Some(input_id),
            ..
        } => input_id,
        outcome => panic!("unexpected queued Project Submit outcome: {outcome:?}"),
    };
    let queued_control = target_state
        .conversation_authority
        .control_view(&address, &serde_json::json!({}))
        .await;
    assert!(queued_control.active_turn.is_some());
    assert_eq!(queued_control.queue.entries.len(), 1);
    assert_eq!(queued_control.queue.entries[0].input_id, queued_input_id);
    let first_queue_control =
        read_queue_control_for_input(&mut first_viewer, &queued_input_id).await;
    let second_queue_control =
        read_queue_control_for_input(&mut second_viewer, &queued_input_id).await;
    assert_eq!(
        first_queue_control["control"]["revision"],
        second_queue_control["control"]["revision"]
    );
    assert_eq!(
        first_queue_control["control"]["placement"]["authority_device_id"],
        target_device_id
    );

    let source_scope = rua_core::ChatScopeStore::resolve(&source_state.rua_root, address.clone());
    assert!(
        rua_server::services::chat_index::get(source_scope.root(), chat_id)
            .await
            .unwrap()
            .is_none(),
        "controller has no Project Chat Index"
    );
    assert!(
        WorkspaceSessionStore::new(source_scope.root())
            .load_chat_header(chat_id)
            .await
            .unwrap()
            .is_none(),
        "controller has no Project Chat Header"
    );
    let controller_command_root = source_scope.root().join("device_commands");
    let controller_files_before_controls = relative_file_paths(&controller_command_root);
    assert!(
        !controller_files_before_controls.is_empty(),
        "remote Submit records its display-only pending delivery marker"
    );

    let source_app = rua_server::router::build_app(source_state.clone(), None);
    let response = source_app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri(format!(
                    "/api/projects/{}/chats/{chat_id}/queue/{}",
                    project.project_id, queued_input_id
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "message": "edited at authority" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let edited = target_state
        .conversation_authority
        .control_view(&address, &serde_json::json!({}))
        .await;
    assert_eq!(edited.queue.entries.len(), 1);
    assert_eq!(edited.queue.entries[0].input_id, queued_input_id);
    assert_eq!(edited.queue.entries[0].text, "edited at authority");

    let second_outcome = source_state
        .conversation_input_intake
        .submit_project_delivery(
            &project.project_id,
            "default",
            ConsoleChatRequest {
                message: "delete only on authority".to_string(),
                chat_id: Some(chat_id.to_string()),
                model: None,
                is_reconnect: false,
                attachments: Vec::new(),
                input: None,
                messages: None,
                extra: serde_json::Map::new(),
                channel_kind: Some(rua_channel::core::renderer::ChannelKind::Chat),
            },
        )
        .await;
    let second_input_id = match second_outcome {
        rua_server::services::conversation_input_intake::ConversationInputDeliveryOutcome::Delivered {
            transport: rua_server::services::conversation_input_intake::ConversationInputDeliveryTransport::RemoteAuthority,
            accepted: true,
            input_id: Some(input_id),
            ..
        } => input_id,
        outcome => panic!("unexpected second queued Project Submit outcome: {outcome:?}"),
    };
    let two_queued = target_state
        .conversation_authority
        .control_view(&address, &serde_json::json!({}))
        .await;
    assert_eq!(two_queued.queue.entries.len(), 2);
    assert_eq!(two_queued.queue.entries[0].input_id, queued_input_id);
    assert_eq!(two_queued.queue.entries[1].input_id, second_input_id);

    let stale_send_now = source_app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/api/projects/{}/chats/{chat_id}/queue/{queued_input_id}/send-now",
                    project.project_id
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "command_id": "project-send-now-stale",
                        "authority_epoch": epoch(0),
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stale_send_now.status(), StatusCode::CONFLICT);

    let non_head_send_now = source_app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/api/projects/{}/chats/{chat_id}/queue/{second_input_id}/send-now",
                    project.project_id
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "command_id": "project-send-now-selected",
                        "authority_epoch": placement.authority_epoch,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(non_head_send_now.status(), StatusCode::CONFLICT);
    let after_non_head = target_state
        .conversation_authority
        .control_view(&address, &serde_json::json!({}))
        .await;
    assert_eq!(after_non_head.queue.entries.len(), 2);
    assert_eq!(after_non_head.queue.entries[0].input_id, queued_input_id);
    assert_eq!(after_non_head.queue.entries[1].input_id, second_input_id);

    let head_send_now = source_app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/api/projects/{}/chats/{chat_id}/queue/{queued_input_id}/send-now",
                    project.project_id
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "command_id": "project-send-now-head",
                        "authority_epoch": placement.authority_epoch,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(head_send_now.status(), StatusCode::OK);

    let deleted = source_app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!(
                    "/api/projects/{}/chats/{chat_id}/queue/{second_input_id}",
                    project.project_id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::OK);
    let after_delete = target_state
        .conversation_authority
        .control_view(&address, &serde_json::json!({}))
        .await;
    assert_eq!(after_delete.queue.entries.len(), 1);
    assert_eq!(after_delete.queue.entries[0].input_id, queued_input_id);

    target_state
        .active_stream_broadcast_hub
        .publish_conversation_event(
            "project-stop-output",
            &address,
            placement.authority_epoch,
            serde_json::json!({
                "type": "text_delta",
                "delta": "partial output before stop",
            })
            .to_string(),
        )
        .await;
    let stream_key = StreamKey::new(CONSOLE_CHANNEL_ID, address.clone(), Priority::Normal);
    let (producer_id, mut cancel_rx) = target_state
        .streams
        .register_active_keyed(&stream_key)
        .await;
    let finish_authority = Arc::clone(&target_state.conversation_authority);
    let finish_streams = Arc::clone(&target_state.streams);
    let finish_hub = Arc::clone(&target_state.active_stream_broadcast_hub);
    let finish_address = address.clone();
    let finish_stream_key = stream_key.clone();
    let finish_turn_id = active_turn.turn_id.clone();
    let finish_epoch = placement.authority_epoch;
    let finish_task = tokio::spawn(async move {
        cancel_rx
            .changed()
            .await
            .expect("Project Stop cancellation");
        finish_authority.finish_active_turn(&finish_address).await;
        finish_hub
            .publish_conversation_event(
                "project-stop-output",
                &finish_address,
                finish_epoch,
                serde_json::json!({
                    "type": "turn_terminal",
                    "turn_id": finish_turn_id,
                    "status": "interrupted",
                })
                .to_string(),
            )
            .await;
        finish_streams
            .mark_completed(&finish_stream_key, producer_id)
            .await;
    });
    let stopped = source_app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/api/projects/{}/chats/{chat_id}/console/stop",
                    project.project_id
                ))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::json!({"turn_id": active_turn.turn_id, "authority_epoch": placement.authority_epoch}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stopped.status(), StatusCode::OK);
    finish_task.await.expect("finish Project Stop");
    let stopped_control = target_state
        .conversation_authority
        .control_view(&address, &serde_json::json!({}))
        .await;
    assert!(stopped_control.active_turn.is_none());
    assert_eq!(
        stopped_control.placement.authority_device_id,
        Some(target_device_id)
    );

    let replay = target_state
        .active_stream_broadcast_hub
        .subscribe_chat(&address, &BTreeMap::new())
        .await
        .replay;
    let replay_payloads = replay
        .iter()
        .filter_map(|frame| match frame {
            ActiveStreamBroadcastFrame::Event { payload, .. } => {
                serde_json::from_str::<serde_json::Value>(payload).ok()
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(replay_payloads
        .iter()
        .any(|payload| payload["delta"] == "partial output before stop"));
    assert!(replay_payloads.iter().any(|payload| {
        payload["type"] == "turn_terminal"
            && payload["turn_id"] == active_turn.turn_id
            && payload["status"] == "interrupted"
    }));
    assert_eq!(
        relative_file_paths(&controller_command_root),
        controller_files_before_controls,
        "Project controls must not add a durable controller outbox"
    );

    target.runtime.shutdown().await.expect("shutdown target");
    let offline_delete = source_app
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!(
                    "/api/projects/{}/chats/{chat_id}/queue/{queued_input_id}",
                    project.project_id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(offline_delete.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        relative_file_paths(&controller_command_root),
        controller_files_before_controls,
        "offline Project controls must not persist a delayed command"
    );
    let target_after_offline_failure = target_state
        .conversation_authority
        .control_view(&address, &serde_json::json!({}))
        .await;
    assert_eq!(target_after_offline_failure.queue.entries.len(), 1);
    assert_eq!(
        target_after_offline_failure.queue.entries[0].input_id,
        queued_input_id
    );

    source.runtime.shutdown().await.expect("shutdown source");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_project_model_reasoning_and_options_use_resource_authority() {
    let source_dir = TempDir::new().expect("source state");
    let source_state = AppState::from_tempdir(source_dir.path().to_path_buf(), ServerProfile::Test);
    let target_dir = TempDir::new().expect("target state");
    let target_state = AppState::from_tempdir(target_dir.path().to_path_buf(), ServerProfile::Test);
    let mut reasoning_model = fixture_provider_model("reasoning-model");
    reasoning_model.thinking_param_style = Some("effort".to_string());
    reasoning_model.reasoning_effort_options =
        vec!["low".to_string(), "medium".to_string(), "high".to_string()];
    seed_fixture_provider(
        &target_state,
        vec![fixture_provider_model("plain-model"), reasoning_model],
    )
    .await;

    let mut source_running = source_state
        .conversation_input_intake
        .authority_running_config("default")
        .await
        .expect("source running config");
    source_running["max_iters"] = serde_json::json!(3);
    source_state
        .conversation_input_intake
        .update_authority_running_config("default", "source-options-fixture", source_running)
        .await
        .expect("set source running config");
    let mut target_running = target_state
        .conversation_input_intake
        .authority_running_config("default")
        .await
        .expect("target running config");
    target_running["max_iters"] = serde_json::json!(17);
    target_state
        .conversation_input_intake
        .update_authority_running_config("default", "target-options-fixture", target_running)
        .await
        .expect("set target running config");

    let network_secret = [104; 32];
    install_test_device(&target_state, 106);
    let target_device_id = hex::encode(
        DeviceNetworkIdentity::new(iroh::SecretKey::from_bytes(&[106; 32])).local_device_id(),
    );
    let target_repo = target_dir.path().join("project-model-options");
    std::fs::create_dir_all(&target_repo).unwrap();
    let project = target_state
        .project_catalog
        .create_bound(rua_project::ProjectCreateRequest {
            resource_device_id: target_device_id.clone(),
            resource_root: target_repo.to_string_lossy().to_string(),
            display_name: Some("Project Model Options".to_string()),
        })
        .unwrap()
        .project;
    source_state
        .project_catalog
        .apply_descriptor(project.clone())
        .unwrap();
    let chat_id = "00000000-0000-4000-8000-000000000430";
    let address = ChatAddress::project(project.project_id.clone(), chat_id);
    let target_scope = rua_core::ChatScopeStore::resolve(&target_state.rua_root, address.clone());
    seed_project_chat_state(target_scope.root(), &address, &target_device_id).await;
    let store = WorkspaceSessionStore::new(target_scope.root());
    let header = store.load_chat_header(chat_id).await.unwrap().unwrap();
    let mut configuration = header.extra["hook_configuration"].clone();
    let old_epoch = configuration["epoch"].as_str().unwrap().to_string();
    configuration["epoch"] = serde_json::json!(format!("configuration-{}", uuid::Uuid::new_v4()));
    configuration["running_config"]["max_iters"] = serde_json::json!(17);
    store
        .replace_chat_configuration(chat_id, &old_epoch, configuration)
        .await
        .unwrap();
    let mut first_viewer = target_state
        .active_stream_broadcast_hub
        .subscribe_chat(&address, &BTreeMap::new())
        .await
        .rx
        .expect("first Project model viewer");
    let mut second_viewer = target_state
        .active_stream_broadcast_hub
        .subscribe_chat(&address, &BTreeMap::new())
        .await
        .rx
        .expect("second Project model viewer");

    let source = build_runtime(
        105,
        network_secret,
        vec![peer_entry(106)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let target = build_runtime(
        106,
        network_secret,
        vec![peer_entry(105)],
        Some(full_command_handler(&target_state, &target_device_id)),
        Vec::new(),
        test_config(),
    )
    .await;
    install_connectable_peer(&source, &target);
    *source_state.device_network.lock().await = Some(source.runtime.clone());
    let source_scope = rua_core::ChatScopeStore::resolve(&source_state.rua_root, address.clone());
    assert!(
        WorkspaceSessionStore::new(source_scope.root())
            .load_chat_header(chat_id)
            .await
            .unwrap()
            .is_none(),
        "controller has no Project Chat Header"
    );
    let source_app = rua_server::router::build_app(source_state.clone(), None);
    let chat_uri = format!("/api/projects/{}/chats/{chat_id}", project.project_id);

    let model_response = source_app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri(&chat_uri)
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "model": {
                            "provider_id": "fixture",
                            "model": "reasoning-model",
                            "thinking_strength": "auto",
                        },
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(model_response.status(), StatusCode::OK);
    let model_body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(model_response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(model_body["model"]["model"], "reasoning-model");
    assert_eq!(model_body["model"]["thinking_strength"], "auto");
    let first_model_control = read_control_for_reason(&mut first_viewer, "model_updated").await;
    let second_model_control = read_control_for_reason(&mut second_viewer, "model_updated").await;
    assert_eq!(
        first_model_control["control"]["revision"],
        second_model_control["control"]["revision"]
    );
    let model_revision = first_model_control["control"]["revision"]
        .as_u64()
        .expect("model revision");

    let reasoning_response = source_app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri(&chat_uri)
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "model": {
                            "provider_id": "fixture",
                            "model": "reasoning-model",
                            "thinking_strength": "high",
                        },
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reasoning_response.status(), StatusCode::OK);
    let reasoning_body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(reasoning_response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(reasoning_body["model"]["thinking_strength"], "high");
    let first_reasoning_control = read_control_for_reason(&mut first_viewer, "model_updated").await;
    let second_reasoning_control =
        read_control_for_reason(&mut second_viewer, "model_updated").await;
    assert_eq!(
        first_reasoning_control["control"]["revision"],
        second_reasoning_control["control"]["revision"]
    );
    let reasoning_revision = first_reasoning_control["control"]["revision"]
        .as_u64()
        .expect("reasoning revision");
    assert!(reasoning_revision > model_revision);
    assert_eq!(
        first_reasoning_control["control"]["model"]["thinking_strength"],
        "high"
    );

    let invalid_reasoning = source_app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri(&chat_uri)
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "model": {
                            "provider_id": "fixture",
                            "model": "reasoning-model",
                            "thinking_strength": "ultra",
                        },
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid_reasoning.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        WorkspaceSessionStore::new(target_scope.root())
            .load_chat_header(chat_id)
            .await
            .unwrap()
            .unwrap()
            .extra["model"]["thinking_strength"],
        "high"
    );

    let options_response = source_app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("{chat_uri}/running-config"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(options_response.status(), StatusCode::OK);
    let mut options_body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(options_response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(options_body["max_iters"], 17);

    options_body["max_iters"] = serde_json::json!(19);
    let updated_options = options_body;
    let update_options_response = source_app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("{chat_uri}/running-config"))
                .header("content-type", "application/json")
                .body(Body::from(updated_options.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(update_options_response.status(), StatusCode::OK);
    let first_options_control =
        read_control_for_reason(&mut first_viewer, "running_config_updated").await;
    let second_options_control =
        read_control_for_reason(&mut second_viewer, "running_config_updated").await;
    assert_eq!(
        first_options_control["control"]["revision"],
        second_options_control["control"]["revision"]
    );
    let options_revision = first_options_control["control"]["revision"]
        .as_u64()
        .expect("options revision");
    assert!(options_revision > reasoning_revision);
    assert_eq!(
        first_options_control["control"]["running_config"]["max_iters"],
        19
    );
    assert_eq!(
        target_state
            .conversation_input_intake
            .authority_running_config("default")
            .await
            .unwrap()["max_iters"],
        17,
        "Project Chat options must not mutate target Agent defaults"
    );
    assert_eq!(
        WorkspaceSessionStore::new(target_scope.root())
            .load_chat_running_config(chat_id)
            .await
            .unwrap()
            .unwrap()
            .max_iters,
        19
    );
    assert_eq!(
        source_state
            .conversation_input_intake
            .authority_running_config("default")
            .await
            .unwrap()["max_iters"],
        3,
        "controller Agent defaults must not impersonate Project options"
    );

    let invalid_options = source_app
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("{chat_uri}/running-config"))
                .header("content-type", "application/json")
                .body(Body::from({
                    let mut invalid = updated_options;
                    invalid["llm_max_retries"] = serde_json::json!(0);
                    invalid.to_string()
                }))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid_options.status(), StatusCode::CONFLICT);
    assert_eq!(
        target_state
            .conversation_input_intake
            .authority_running_config("default")
            .await
            .unwrap()["max_iters"],
        17
    );
    assert!(
        rua_server::services::chat_index::get(source_scope.root(), chat_id)
            .await
            .unwrap()
            .is_none(),
        "model and options controls do not require controller Chat metadata"
    );

    source.runtime.shutdown().await.expect("shutdown source");
    target.runtime.shutdown().await.expect("shutdown target");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_project_approval_and_acp_use_resource_authority_without_controller_storage() {
    let source_dir = TempDir::new().expect("source state");
    let source_state = AppState::from_tempdir(source_dir.path().to_path_buf(), ServerProfile::Test);
    let target_dir = TempDir::new().expect("target state");
    let target_state = AppState::from_tempdir(target_dir.path().to_path_buf(), ServerProfile::Test);
    let network_secret = [107; 32];
    let target_sync_identity: rua_sync::identity::DeviceIdentity =
        serde_json::from_value(serde_json::json!({
            "secret_bytes": vec![109_u8; 32],
        }))
        .expect("fixed target Device identity");
    *target_state.sync_device.write().expect("sync Device lock") =
        Some(Arc::new(target_sync_identity));
    let target_device_id = hex::encode(
        DeviceNetworkIdentity::new(iroh::SecretKey::from_bytes(&[109; 32])).local_device_id(),
    );
    assert_eq!(
        rua_server::services::device_context::current_sync_device_id(&target_state.sync_device)
            .as_deref(),
        Some(target_device_id.as_str())
    );
    let target_repo = target_dir.path().join("project-approval-acp");
    std::fs::create_dir_all(&target_repo).unwrap();
    let project = target_state
        .project_catalog
        .create_bound(rua_project::ProjectCreateRequest {
            resource_device_id: target_device_id.clone(),
            resource_root: target_repo.to_string_lossy().to_string(),
            display_name: Some("Project Approval ACP".to_string()),
        })
        .unwrap()
        .project;
    source_state
        .project_catalog
        .apply_descriptor(project.clone())
        .unwrap();
    let chat_id = "00000000-0000-4000-8000-000000000431";
    let address = ChatAddress::project(project.project_id.clone(), chat_id);
    let target_scope = rua_core::ChatScopeStore::resolve(&target_state.rua_root, address.clone());
    seed_project_chat_state(target_scope.root(), &address, &target_device_id).await;
    let placement =
        rua_server::services::conversation_authority_target::load_chat_authority_placement_target(
            target_scope.root(),
            chat_id,
        )
        .await
        .unwrap();

    let existing_handle = target_state
        .acp_chat_registry
        .get("default")
        .expect("target ACP runtime handle");
    // Session discovery must use a local fixture, not an installed codex-acp.
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../rua-acp/tests/fixtures/mock_acp_agent.py");
    rua_config::atomic_write_json(
        &target_repo.join("agent.json"),
        &serde_json::json!({"acp": {"agents": {"codex": {
            "enabled": true,
            "command": if cfg!(windows) { "python" } else { "python3" },
            "args": [fixture],
            "trusted": true,
        }}}}),
    )
    .unwrap();
    let acp_manager = Arc::new(AcpChatSessionManager::new(Duration::from_secs(300)));
    target_state.acp_chat_registry.register(
        "default".to_string(),
        Arc::new(AcpChatRuntimeHandle {
            service: Arc::clone(&existing_handle.service),
            manager: Arc::clone(&acp_manager),
            controller: Arc::new(TestAcpController {
                manager: Arc::clone(&acp_manager),
                workspace_dir: target_repo.clone(),
            }),
            workspace_dir: target_repo.clone(),
        }),
    );

    let source = build_runtime(
        108,
        network_secret,
        vec![peer_entry(109)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let target = build_runtime(
        109,
        network_secret,
        vec![peer_entry(108)],
        Some(full_command_handler(&target_state, &target_device_id)),
        Vec::new(),
        test_config(),
    )
    .await;
    install_connectable_peer(&source, &target);
    *source_state.device_network.lock().await = Some(source.runtime.clone());
    let source_scope = rua_core::ChatScopeStore::resolve(&source_state.rua_root, address.clone());
    assert!(
        WorkspaceSessionStore::new(source_scope.root())
            .load_chat_header(chat_id)
            .await
            .unwrap()
            .is_none(),
        "controller has no Project Chat Header"
    );
    let source_app = rua_server::router::build_app(source_state.clone(), None);
    let chat_uri = format!("/api/projects/{}/chats/{chat_id}", project.project_id);

    let approval_authority = target_state.security.approval_authority();
    for (index, name) in ["run-once", "allow-chat", "allow-agent", "reject"]
        .into_iter()
        .enumerate()
    {
        let approval = approval_authority
            .create_request(ApprovalRequestInput {
                agent_id: "default".to_string(),
                owner: rua_security::ApprovalOwner::chat(address.clone()),
                turn_id: format!("turn-{name}"),
                tool_call_id: format!("call-{name}"),
                tool_name: "bash".to_string(),
                trigger: ApprovalTrigger::approval_mode("approval required"),
                tool_call: ToolCallSnapshot {
                    display: format!("bash {name}"),
                    args: serde_json::json!({"command": name}),
                },
            })
            .await;
        let decision = match name {
            "allow-chat" => ConversationApprovalDecision::AllowChat {
                decided_by: "test".to_string(),
                matcher: approval
                    .reuse_options
                    .iter()
                    .find(|option| option.scope == rua_security::ApprovalReuseScope::Chat)
                    .expect("Chat reuse option")
                    .matcher
                    .clone(),
            },
            "allow-agent" => ConversationApprovalDecision::AllowAgent {
                decided_by: "test".to_string(),
                matcher: approval
                    .reuse_options
                    .iter()
                    .find(|option| option.scope == rua_security::ApprovalReuseScope::Agent)
                    .expect("Agent reuse option")
                    .matcher
                    .clone(),
            },
            "reject" => ConversationApprovalDecision::Reject {
                decided_by: "test".to_string(),
            },
            _ => ConversationApprovalDecision::RunOnce {
                decided_by: "test".to_string(),
            },
        };
        if index == 0 {
            let snapshot = source_app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::GET)
                        .uri(format!("{chat_uri}/approvals/snapshot"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(snapshot.status(), StatusCode::OK);
            let snapshot: serde_json::Value = serde_json::from_slice(
                &axum::body::to_bytes(snapshot.into_body(), usize::MAX)
                    .await
                    .unwrap(),
            )
            .unwrap();
            assert!(snapshot["pending"]
                .as_array()
                .is_some_and(|pending| pending.iter().any(|item| item["id"] == approval.id)));
        }
        let decided = source_app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("{chat_uri}/approvals/{}/decide", approval.id))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&decision).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(decided.status(), StatusCode::OK, "{name} decision");
    }
    assert_eq!(approval_authority.list_chat_rules(&address).await.len(), 1);
    assert_eq!(
        approval_authority.list_agent_rules("default").await.len(),
        1
    );
    assert!(source_state
        .security
        .approval_authority()
        .list_chat_rules(&address)
        .await
        .is_empty());
    assert!(source_state
        .security
        .approval_authority()
        .list_agent_rules("default")
        .await
        .is_empty());

    let runners = source_app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("{chat_uri}/acp/runners"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(runners.status(), StatusCode::OK);
    let runners: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(runners.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(runners
        .as_array()
        .is_some_and(|runners| runners.iter().any(|runner| runner["key"] == "codex")));

    let sessions = source_app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "{chat_uri}/acp/sessions?runner=codex&cwd=%2Fcontroller%2Fwrong"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let sessions_status = sessions.status();
    let sessions_body = axum::body::to_bytes(sessions.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        sessions_status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&sessions_body)
    );
    let sessions: serde_json::Value = serde_json::from_slice(&sessions_body).unwrap();
    assert_eq!(sessions[0]["session_id"], "fixture-listed-session");
    assert_eq!(sessions[0]["cwd"], target_repo.to_string_lossy().as_ref());

    let entered = source_app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("{chat_uri}/acp/enter"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "agent_id": "default",
                        "runner": "codex",
                        "cwd": "/controller/wrong",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let entered_status = entered.status();
    let entered_body = axum::body::to_bytes(entered.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        entered_status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&entered_body)
    );
    let entered: serde_json::Value = serde_json::from_slice(&entered_body).unwrap();
    assert_eq!(entered["status"]["active"], true);
    assert_eq!(
        entered["status"]["cwd"],
        target_repo.to_string_lossy().as_ref()
    );
    assert_eq!(
        entered["status"]["workspace_dir"],
        target_repo.to_string_lossy().as_ref()
    );
    let switch_state = rua_server::services::project_chat::project_chat_agent_state(
        target_scope.root(),
        &address,
        true,
        &target_state.conversation_authority,
        approval_authority,
    )
    .await
    .unwrap();
    assert!(switch_state.blockers.contains(
        &rua_server::services::project_chat::ProjectAgentSwitchBlocker::ActiveAcpBinding
    ));

    let status = source_app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("{chat_uri}/acp/status"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::OK);

    let protocol_request: rua_acp::permissions::RequestPermissionRequest =
        serde_json::from_value(serde_json::json!({
            "sessionId": chat_id,
            "toolCall": {"toolCallId": "project-permission-tool"},
            "options": [{
                "optionId": "allow_once",
                "name": "Allow once",
                "kind": "allow_once"
            }]
        }))
        .unwrap();
    let (permission, resolution_rx) = target_state
        .acp_permission_relay
        .create_request(
            rua_acp::permissions::AcpPermissionRequestContext {
                agent_id: "default".to_string(),
                chat_id: chat_id.to_string(),
                chat_address: Some(address.clone()),
                runner: "codex".to_string(),
                task: None,
            },
            &protocol_request,
            None,
        )
        .await;
    let permission_snapshot = source_app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("{chat_uri}/acp/permissions"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(permission_snapshot.status(), StatusCode::OK);
    let permission_decided = source_app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "{chat_uri}/acp/permissions/{}/decide",
                    permission.id
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"option_id": "allow_once"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let permission_status = permission_decided.status();
    let permission_body = axum::body::to_bytes(permission_decided.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        permission_status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&permission_body)
    );
    assert!(matches!(
        resolution_rx.await.unwrap(),
        rua_acp::permissions::PermissionRelayResolution::Selected(_)
    ));

    let exited = source_app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("{chat_uri}/acp/exit"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(exited.status(), StatusCode::OK);
    assert!(acp_manager.get(chat_id).await.is_none());

    let resumed = source_app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("{chat_uri}/acp/enter"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "agent_id": "default",
                        "runner": "codex",
                        "cwd": "/controller/wrong",
                        "resume_session_id": "runner-session-existing",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resumed.status(), StatusCode::OK);
    let resumed: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(resumed.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(resumed["history_preview"], "history preview");
    assert_eq!(
        resumed["status"]["cwd"],
        target_repo.to_string_lossy().as_ref()
    );
    let reexited = source_app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("{chat_uri}/acp/exit"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reexited.status(), StatusCode::OK);

    let lazy_chat_id = "00000000-0000-4000-8000-000000000433";
    let lazy_address = ChatAddress::project(project.project_id.clone(), lazy_chat_id);
    let lazy_source_scope =
        rua_core::ChatScopeStore::resolve(&source_state.rua_root, lazy_address.clone());
    let lazy_target_scope =
        rua_core::ChatScopeStore::resolve(&target_state.rua_root, lazy_address.clone());
    let lazy_chat_uri = format!("/api/projects/{}/chats/{lazy_chat_id}", project.project_id);
    let lazy_entered = source_app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("{lazy_chat_uri}/acp/enter"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "agent_id": "default",
                        "runner": "codex",
                        "cwd": "/controller/wrong",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(lazy_entered.status(), StatusCode::NOT_FOUND);
    assert!(
        WorkspaceSessionStore::new(lazy_target_scope.root())
            .load_chat_header(lazy_chat_id)
            .await
            .unwrap()
            .is_none(),
        "ACP control must not materialize a Chat before its first accepted input"
    );
    assert!(
        WorkspaceSessionStore::new(lazy_source_scope.root())
            .load_chat_header(lazy_chat_id)
            .await
            .unwrap()
            .is_none(),
        "first ACP Enter does not create controller Project Chat storage"
    );
    let lazy_exited = source_app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("{lazy_chat_uri}/acp/exit"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(lazy_exited.status(), StatusCode::NOT_FOUND);

    target_state
        .agent_service
        .create_agent(rua_server::endpoints::dto::AgentCreate {
            id: Some("code".to_string()),
            name: "Code Agent".to_string(),
            description: String::new(),
            workspace_dir: None,
            template_id: None,
            skill_names: None,
            extra: serde_json::Map::new(),
        })
        .await
        .unwrap();
    let code_agent = target_state
        .agent_service
        .get_agent("code")
        .await
        .unwrap()
        .expect("Code Agent");
    let switched = rua_server::services::project_chat::switch_project_chat_agent(
        target_scope.root(),
        &address,
        "default",
        &code_agent,
        &target_device_id,
        &target_state.conversation_authority,
        approval_authority,
    )
    .await
    .unwrap();
    assert_eq!(switched.selection.agent_id, "code");
    assert_eq!(approval_authority.list_chat_rules(&address).await.len(), 1);
    assert_eq!(
        approval_authority.list_agent_rules("default").await.len(),
        1
    );
    assert!(approval_authority.list_agent_rules("code").await.is_empty());

    let pending_offline = approval_authority
        .create_request(ApprovalRequestInput {
            agent_id: "code".to_string(),
            owner: rua_security::ApprovalOwner::chat(address.clone()),
            turn_id: "turn-offline".to_string(),
            tool_call_id: "call-offline".to_string(),
            tool_name: "bash".to_string(),
            trigger: ApprovalTrigger::approval_mode("approval required"),
            tool_call: ToolCallSnapshot {
                display: "bash offline".to_string(),
                args: serde_json::json!({"command": "offline"}),
            },
        })
        .await;
    let stale_acp = deliver_acp_chat_command_via_session(
        source.runtime.session_manager(),
        &AcpChatDeviceCommand {
            command_id: "project-acp-stale-epoch".to_string(),
            agent_id: "code".to_string(),
            address: address.clone(),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            authority_epoch: epoch(0),
            action: AcpChatDeviceAction::Status,
        },
        Duration::from_secs(5),
    )
    .await
    .expect_err("stale Project ACP Epoch");
    assert!(matches!(
        stale_acp,
        DeviceCommandDeliveryError::Rejected { ref reason, .. }
            if reason == "stale_authority_epoch"
    ));
    assert_eq!(
        placement.authority_epoch,
        rua_server::services::conversation_authority_target::load_chat_authority_placement_target(
            target_scope.root(),
            chat_id,
        )
        .await
        .unwrap()
        .authority_epoch
    );

    let controller_command_root = source_scope.root().join("device_commands");
    let controller_files_before_offline = relative_file_paths(&controller_command_root);
    target.runtime.shutdown().await.expect("shutdown target");
    let offline_decision = source_app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "{chat_uri}/approvals/{}/decide",
                    pending_offline.id
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&ConversationApprovalDecision::RunOnce {
                        decided_by: "test".to_string(),
                    })
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(offline_decision.status(), StatusCode::SERVICE_UNAVAILABLE);
    let offline_status = source_app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("{chat_uri}/acp/status"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(offline_status.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(approval_authority
        .request(&pending_offline.id)
        .await
        .is_some_and(|request| {
            request.status == rua_security::approval_authority::ApprovalStatus::Pending
        }));
    assert_eq!(
        relative_file_paths(&controller_command_root),
        controller_files_before_offline
    );

    source.runtime.shutdown().await.expect("shutdown source");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn collaboration_device_command_is_idempotent_epoch_gated_and_rebuilds_project_resource() {
    let source_dir = TempDir::new().expect("source state");
    let source_state = AppState::from_tempdir(source_dir.path().to_path_buf(), ServerProfile::Test);
    let target_dir = TempDir::new().expect("target state");
    let target_state = AppState::from_tempdir(target_dir.path().to_path_buf(), ServerProfile::Test);
    let network_secret = [117; 32];
    let target_sync_identity: rua_sync::identity::DeviceIdentity =
        serde_json::from_value(serde_json::json!({ "secret_bytes": vec![119_u8; 32] })).unwrap();
    *target_state.sync_device.write().expect("sync Device lock") =
        Some(Arc::new(target_sync_identity));
    let target_device_id = hex::encode(
        DeviceNetworkIdentity::new(iroh::SecretKey::from_bytes(&[119; 32])).local_device_id(),
    );
    let target_repo = target_dir.path().join("collaboration-resource");
    std::fs::create_dir_all(&target_repo).unwrap();
    let project = target_state
        .project_catalog
        .create_bound(rua_project::ProjectCreateRequest {
            resource_device_id: target_device_id.clone(),
            resource_root: target_repo.to_string_lossy().to_string(),
            display_name: Some("Collaboration Resource".into()),
        })
        .unwrap()
        .project;
    source_state
        .project_catalog
        .apply_descriptor(project.clone())
        .unwrap();
    let chat_id = "00000000-0000-4000-8000-000000000511";
    let address = ChatAddress::project(project.project_id.clone(), chat_id);
    let target_scope = rua_core::ChatScopeStore::resolve(&target_state.rua_root, address.clone());
    seed_project_chat_state(target_scope.root(), &address, &target_device_id).await;
    let configuration = rua_server::services::product_hooks::ChatHookConfiguration {
        epoch: "project-source-configuration".into(),
        running_config: rua_config::ChatRunningConfig::default(),
        security_config: rua_config::types::security::SecurityConfig::default(),
        agent_assignment: None,
        prompt_file_content: None,
        tool_config_snapshot: Default::default(),
        capability_configuration: None,
        selected_agent_id: "default".into(),
        model: None,
        thinking: None,
        collaboration_mode: serde_json::json!({"max_in_flight": 4}),
        harness_configuration: None,
        authority_device_id: Some(target_device_id.clone()),
        definition_revisions: Vec::new(),
        effective_hooks: Vec::new(),
        warnings: Vec::new(),
        execution_requirements: Vec::new(),
        extra: serde_json::Map::new(),
    };
    configuration.validate().unwrap();
    let mut header = WorkspaceSessionStore::new(target_scope.root())
        .load_chat_header(chat_id)
        .await
        .unwrap()
        .unwrap();
    let old_epoch = header.extra["hook_configuration"]["epoch"]
        .as_str()
        .unwrap();
    WorkspaceSessionStore::new(target_scope.root())
        .replace_chat_configuration(
            chat_id,
            old_epoch,
            serde_json::to_value(&configuration).unwrap(),
        )
        .await
        .unwrap();
    header.extra.insert(
        "hook_configuration".into(),
        serde_json::to_value(&configuration).unwrap(),
    );
    WorkspaceSessionStore::new(target_scope.root())
        .save_chat_header(&header)
        .await
        .unwrap();
    target_state
        .product_hooks
        .adopt_chat_configuration(&address, configuration.clone())
        .await;
    let configuration_epoch = configuration.epoch;
    let placement =
        rua_server::services::conversation_authority_target::load_chat_authority_placement_target(
            target_scope.root(),
            chat_id,
        )
        .await
        .unwrap();

    let source = build_runtime(
        118,
        network_secret,
        vec![peer_entry(119)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    *source_state.device_network.lock().await = Some(source.runtime.clone());
    let manager = source.runtime.session_manager().clone();
    let caller = rua_server::services::collaboration::CollaborationCaller {
        address: address.clone(),
        turn_id: "source-turn".into(),
        tool_call_id: "remote-create-call".into(),
        configuration_epoch,
        working_dir: PathBuf::from("/controller/path/must/not/be-used"),
    };
    let command = CollaborationDeviceCommand {
        command_id: "remote-collaboration-create".into(),
        source_device_id: hex::encode(
            DeviceNetworkIdentity::new(iroh::SecretKey::from_bytes(&[118; 32])).local_device_id(),
        ),
        target_device_id: target_device_id.clone(),
        route_address: address.clone(),
        authority_epoch: placement.authority_epoch,
        action: CollaborationDeviceAction::CreateAgent {
            caller,
            agent_id: "default".into(),
            message: None,
            yield_seconds: 0,
        },
    };
    let offline = deliver_collaboration_device_command_via_session(
        &manager,
        &command,
        Duration::from_millis(250),
    )
    .await;
    assert!(
        offline.is_err(),
        "offline collaboration must fail explicitly"
    );
    assert!(WorkspaceSessionStore::new(target_scope.root())
        .load_collaboration_targets(chat_id)
        .await
        .unwrap()
        .is_empty());

    let target = build_runtime(
        119,
        network_secret,
        vec![peer_entry(118)],
        Some(full_command_handler(&target_state, &target_device_id)),
        Vec::new(),
        test_config(),
    )
    .await;
    install_connectable_peer(&source, &target);
    let first = deliver_collaboration_device_command_via_session(
        &manager,
        &command,
        Duration::from_secs(10),
    )
    .await
    .unwrap();
    let second = deliver_collaboration_device_command_via_session(
        &manager,
        &command,
        Duration::from_secs(10),
    )
    .await
    .unwrap();
    assert_eq!(first, second);
    let CollaborationDeviceResponse::Reply {
        reply,
        authority_device_id,
        authority_epoch,
    } = first
    else {
        panic!("unexpected collaboration response");
    };
    assert_eq!(authority_device_id, target_device_id);
    assert_eq!(authority_epoch, placement.authority_epoch);
    assert_eq!(reply.state, "created");
    let child_scope = rua_core::ChatScopeStore::resolve(&target_state.rua_root, reply.chat.clone());
    let child_header = WorkspaceSessionStore::new(child_scope.root())
        .load_chat_header(reply.chat.chat_id().as_str())
        .await
        .unwrap()
        .unwrap();
    let descriptor: rua_session::collaboration::CollaborationDescriptor =
        serde_json::from_value(child_header.extra["collaboration"].clone()).unwrap();
    assert_eq!(descriptor.working_dir, target_repo);

    let child_placement =
        rua_server::services::conversation_authority_target::load_chat_authority_placement_target(
            child_scope.root(),
            reply.chat.chat_id().as_str(),
        )
        .await
        .unwrap();
    let remote_caller = match &command.action {
        CollaborationDeviceAction::CreateAgent { caller, .. } => caller.clone(),
        _ => unreachable!(),
    };
    let send = deliver_collaboration_device_command_via_session(
        &manager,
        &CollaborationDeviceCommand {
            command_id: "remote-collaboration-send".into(),
            source_device_id: command.source_device_id.clone(),
            target_device_id: target_device_id.clone(),
            route_address: reply.chat.clone(),
            authority_epoch: child_placement.authority_epoch,
            action: CollaborationDeviceAction::Send {
                caller: remote_caller.clone(),
                target: reply.chat.clone(),
                message: "continue remotely".into(),
                yield_seconds: 0,
            },
        },
        Duration::from_secs(10),
    )
    .await
    .unwrap();
    let CollaborationDeviceResponse::Reply {
        reply: sent,
        authority_device_id,
        authority_epoch,
    } = send
    else {
        panic!("unexpected send response");
    };
    assert_eq!(authority_device_id, target_device_id);
    assert_eq!(authority_epoch, child_placement.authority_epoch);
    assert!(sent.request_id.is_some());

    let result_store = WorkspaceSessionStore::new(child_scope.root());
    let result_request_id = uuid::Uuid::new_v4().to_string();
    let full_output = "远端结果".repeat(5000);
    result_store
        .save_collaboration_request(&rua_session::collaboration::CollaborationRequest {
            request_id: result_request_id.clone(),
            source: remote_caller.address.clone(),
            target: reply.chat.clone(),
            origin_turn_id: remote_caller.turn_id.clone(),
            origin_tool_call_id: "remote-page-fixture".into(),
            input_digest: "fixture".into(),
            created_at: chrono::Utc::now().to_rfc3339(),
            input_id: None,
            turn_id: None,
            outcome: Some(rua_session::collaboration::CollaborationOutcome {
                result_id: "remote-page-result".into(),
                status: "completed".into(),
                output: full_output.clone(),
                partial: false,
                extra: Default::default(),
            }),
            delivery: Default::default(),
            extra: Default::default(),
        })
        .await
        .unwrap();
    let delivery_before = result_store
        .load_collaboration_request(reply.chat.chat_id().as_str(), &result_request_id)
        .await
        .unwrap()
        .unwrap()
        .delivery;
    let mut offset = 0;
    let mut recovered = String::new();
    loop {
        let response = deliver_collaboration_device_command_via_session(
            &manager,
            &CollaborationDeviceCommand {
                command_id: format!("remote-result-page-{offset}"),
                source_device_id: command.source_device_id.clone(),
                target_device_id: target_device_id.clone(),
                route_address: reply.chat.clone(),
                authority_epoch: child_placement.authority_epoch,
                action: CollaborationDeviceAction::Read {
                    caller: remote_caller.clone(),
                    target: reply.chat.clone(),
                    yield_seconds: 0,
                    result: Some(rua_session::collaboration::CollaborationResultCursor {
                        request_id: result_request_id.clone(),
                        result_id: "remote-page-result".into(),
                        offset,
                        extra: Default::default(),
                    }),
                },
            },
            Duration::from_secs(10),
        )
        .await
        .unwrap();
        let CollaborationDeviceResponse::Reply {
            reply: page_reply, ..
        } = response
        else {
            panic!("result reply");
        };
        let outcome = page_reply.outcome.unwrap();
        let page = rua_session::collaboration::collaboration_result_page(
            &result_request_id,
            &outcome.result_id,
            &outcome.status,
            &outcome.output,
            outcome.partial,
            offset,
        )
        .unwrap();
        recovered.push_str(page["output"].as_str().unwrap());
        if page["has_more"] == false {
            break;
        }
        offset = page["next"]["offset"].as_u64().unwrap() as usize;
    }
    assert_eq!(recovered, full_output);
    assert_eq!(
        result_store
            .load_collaboration_request(reply.chat.chat_id().as_str(), &result_request_id)
            .await
            .unwrap()
            .unwrap()
            .delivery,
        delivery_before
    );

    let read = deliver_collaboration_device_command_via_session(
        &manager,
        &CollaborationDeviceCommand {
            command_id: "remote-collaboration-read".into(),
            source_device_id: command.source_device_id.clone(),
            target_device_id: target_device_id.clone(),
            route_address: reply.chat.clone(),
            authority_epoch: child_placement.authority_epoch,
            action: CollaborationDeviceAction::Read {
                caller: remote_caller.clone(),
                target: reply.chat.clone(),
                result: None,
                yield_seconds: 0,
            },
        },
        Duration::from_secs(10),
    )
    .await
    .unwrap();
    assert!(matches!(
        read,
        CollaborationDeviceResponse::Reply {
            authority_device_id,
            authority_epoch,
            ..
        } if authority_device_id == target_device_id
            && authority_epoch == child_placement.authority_epoch
    ));

    let stopped = deliver_collaboration_device_command_via_session(
        &manager,
        &CollaborationDeviceCommand {
            command_id: "remote-collaboration-stop".into(),
            source_device_id: command.source_device_id.clone(),
            target_device_id: target_device_id.clone(),
            route_address: reply.chat.clone(),
            authority_epoch: child_placement.authority_epoch,
            action: CollaborationDeviceAction::StopTurn {
                caller: remote_caller,
                target: reply.chat.clone(),
                turn_id: "00000000-0000-4000-8000-000000000599".into(),
                turn_authority_epoch: child_placement.authority_epoch,
            },
        },
        Duration::from_secs(10),
    )
    .await
    .unwrap();
    assert!(matches!(
        stopped,
        CollaborationDeviceResponse::Stopped {
            reply,
            authority_device_id,
            authority_epoch,
            ..
        } if authority_device_id == target_device_id
            && authority_epoch == child_placement.authority_epoch
            && !reply.stopped
    ));

    let mut conflicting = command.clone();
    if let CollaborationDeviceAction::CreateAgent { agent_id, .. } = &mut conflicting.action {
        *agent_id = "other".into();
    }
    assert!(matches!(
        deliver_collaboration_device_command_via_session(
            &manager,
            &conflicting,
            Duration::from_secs(10)
        )
        .await,
        Err(DeviceCommandDeliveryError::Rejected { ref reason, .. })
            if reason == "duplicate_command_conflict"
    ));
    let mut stale = command;
    stale.command_id = "remote-collaboration-stale".into();
    stale.authority_epoch =
        ConversationAuthorityEpoch::from_parts(u64::MAX, u32::MAX, [u8::MAX; 8]);
    let stale_result =
        deliver_collaboration_device_command_via_session(&manager, &stale, Duration::from_secs(10))
            .await;
    assert!(
        matches!(
            stale_result,
            Err(DeviceCommandDeliveryError::Rejected { ref reason, .. })
                if reason == "conversation_authority_redirect"
        ),
        "unexpected stale authority result: {stale_result:?}"
    );
    source.runtime.shutdown().await.unwrap();
    target.runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_project_agent_switch_is_live_for_two_viewers_without_controller_storage() {
    let first_dir = TempDir::new().expect("first controller state");
    let first_state = AppState::from_tempdir(first_dir.path().to_path_buf(), ServerProfile::Test);
    let second_dir = TempDir::new().expect("second controller state");
    let second_state = AppState::from_tempdir(second_dir.path().to_path_buf(), ServerProfile::Test);
    let target_dir = TempDir::new().expect("target state");
    let target_state = AppState::from_tempdir(target_dir.path().to_path_buf(), ServerProfile::Test);
    seed_fixture_provider(
        &target_state,
        vec![fixture_provider_model("project-switch-model")],
    )
    .await;
    target_state
        .provider_manager
        .store_default_model(DefaultModel {
            provider_id: "fixture".to_string(),
            model: "project-switch-model".to_string(),
            source: None,
            extra: Default::default(),
        })
        .await;
    target_state
        .agent_service
        .create_agent(rua_server::endpoints::dto::AgentCreate {
            id: Some("code".to_string()),
            name: "Code Agent".to_string(),
            description: String::new(),
            workspace_dir: None,
            template_id: None,
            skill_names: None,
            extra: serde_json::Map::new(),
        })
        .await
        .unwrap();
    let captured = CapturingAgentRuntime::default();
    target_state
        .registry
        .get_or_create("code")
        .await
        .expect("Code Agent runtime")
        .set_react_runtime(Arc::new(captured.clone()))
        .await;

    let network_secret = [110; 32];
    let target_device_id = hex::encode(
        DeviceNetworkIdentity::new(iroh::SecretKey::from_bytes(&[112; 32])).local_device_id(),
    );
    let target_repo = target_dir.path().join("project-agent-switch");
    std::fs::create_dir_all(&target_repo).unwrap();
    let project = target_state
        .project_catalog
        .create_bound(rua_project::ProjectCreateRequest {
            resource_device_id: target_device_id.clone(),
            resource_root: target_repo.to_string_lossy().to_string(),
            display_name: Some("Project Agent Switch".to_string()),
        })
        .unwrap()
        .project;
    first_state
        .project_catalog
        .apply_descriptor(project.clone())
        .unwrap();
    second_state
        .project_catalog
        .apply_descriptor(project.clone())
        .unwrap();
    let chat_id = "00000000-0000-4000-8000-000000000432";
    let address = ChatAddress::project(project.project_id.clone(), chat_id);
    let target_scope = rua_core::ChatScopeStore::resolve(&target_state.rua_root, address.clone());
    seed_project_chat_state(target_scope.root(), &address, &target_device_id).await;

    let first = build_runtime(
        111,
        network_secret,
        vec![peer_entry(112)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let target = build_runtime(
        112,
        network_secret,
        vec![peer_entry(111), peer_entry(113)],
        Some(full_command_handler(&target_state, &target_device_id)),
        Vec::new(),
        test_config(),
    )
    .await;
    let second = build_runtime(
        113,
        network_secret,
        vec![peer_entry(112)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    install_connectable_peer(&first, &target);
    install_connectable_peer(&second, &target);
    target
        .runtime
        .session_manager()
        .set_live_activity_handler(Arc::new(
            ActiveStreamBroadcastDeviceSessionHandler::new(Arc::clone(
                &target_state.active_stream_broadcast_hub,
            ))
            .with_conversation_control_initial_state(
                Arc::clone(&target_state.conversation_authority),
                target_state.conversation_input_intake.clone(),
                target_device_id.clone(),
            ),
        ));
    *first_state.device_network.lock().await = Some(first.runtime.clone());
    *second_state.device_network.lock().await = Some(second.runtime.clone());
    for state in [&first_state, &second_state] {
        let scope = rua_core::ChatScopeStore::resolve(&state.rua_root, address.clone());
        assert!(
            WorkspaceSessionStore::new(scope.root())
                .load_chat_header(chat_id)
                .await
                .unwrap()
                .is_none(),
            "controller has no Project Chat Header"
        );
    }

    let first_app = rua_server::router::build_app(first_state.clone(), None);
    let first_live = first_app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/api/projects/{}/chats/{chat_id}/console/live",
                    project.project_id
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "viewer_id": "project-switch-first",
                        "agent_id": "default",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first_live.status(), StatusCode::OK);
    let mut first_viewer = first_live.into_body().into_data_stream();
    let first_initial = read_project_live_control(&mut first_viewer, "viewer_attached").await;

    let second_app = rua_server::router::build_app(second_state.clone(), None);
    let second_live = second_app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/api/projects/{}/chats/{chat_id}/console/live",
                    project.project_id
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "viewer_id": "project-switch-second",
                        "agent_id": "default",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second_live.status(), StatusCode::OK);
    let mut second_viewer = second_live.into_body().into_data_stream();
    let second_initial = read_project_live_control(&mut second_viewer, "viewer_attached").await;
    assert_eq!(
        first_initial["control"]["project_chat_agent_state"]["selection"]["agent_id"],
        "default"
    );
    assert_eq!(
        first_initial["control"]["project_chat_agent_state"],
        second_initial["control"]["project_chat_agent_state"]
    );

    let switched = first_app
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri(format!(
                    "/api/projects/{}/chats/{chat_id}/agent-selection",
                    project.project_id
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "expected_agent_id": "default",
                        "agent_id": "code",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let switched_status = switched.status();
    let switched_bytes = axum::body::to_bytes(switched.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        switched_status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&switched_bytes)
    );
    let switched: serde_json::Value = serde_json::from_slice(&switched_bytes).unwrap();
    assert_eq!(switched["selection"]["agent_id"], "code");
    assert_eq!(switched["selection"]["revision"], 2);

    let first_updated =
        read_project_live_control(&mut first_viewer, "project_agent_selection_updated").await;
    let second_updated =
        read_project_live_control(&mut second_viewer, "project_agent_selection_updated").await;
    assert_eq!(
        first_updated["control"]["project_chat_agent_state"],
        second_updated["control"]["project_chat_agent_state"]
    );
    assert_eq!(
        first_updated["control"]["project_chat_agent_state"]["selection"]["agent_id"],
        "code"
    );
    assert_eq!(
        first_updated["control"]["project_chat_agent_state"]["selected_agent_available"],
        true
    );
    assert_eq!(
        first_updated["control"]["project_chat_agent_state"]["blockers"],
        serde_json::json!([])
    );
    assert!(
        first_updated["control"]["revision"].as_u64().unwrap()
            > first_initial["control"]["revision"].as_u64().unwrap()
    );

    let outcome = second_state
        .conversation_input_intake
        .submit_project_delivery(
            &project.project_id,
            "code",
            ConsoleChatRequest {
                message: "run with the switched Agent".to_string(),
                chat_id: Some(chat_id.to_string()),
                model: None,
                is_reconnect: false,
                attachments: Vec::new(),
                input: None,
                messages: None,
                extra: serde_json::Map::new(),
                channel_kind: Some(rua_channel::core::renderer::ChannelKind::Chat),
            },
        )
        .await;
    assert!(matches!(
        outcome,
        rua_server::services::conversation_input_intake::ConversationInputDeliveryOutcome::Delivered {
            transport: rua_server::services::conversation_input_intake::ConversationInputDeliveryTransport::RemoteAuthority,
            accepted: true,
            ..
        }
    ));
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if !captured.requests.lock().await.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("switched Agent request");
    let requests = captured.requests.lock().await.clone();
    assert_eq!(requests[0].agent_id.as_str(), "code");

    drop(first_viewer);
    drop(second_viewer);
    first.runtime.shutdown().await.expect("shutdown first");
    second.runtime.shutdown().await.expect("shutdown second");
    target.runtime.shutdown().await.expect("shutdown target");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_project_variant_and_rollback_are_live_without_controller_storage() {
    let source_dir = TempDir::new().expect("controller state");
    let source_state = AppState::from_tempdir(source_dir.path().to_path_buf(), ServerProfile::Test);
    let target_dir = TempDir::new().expect("target state");
    let target_state = AppState::from_tempdir(target_dir.path().to_path_buf(), ServerProfile::Test);
    let network_secret = [120; 32];
    let target_device_id = hex::encode(
        DeviceNetworkIdentity::new(iroh::SecretKey::from_bytes(&[122; 32])).local_device_id(),
    );
    let target_repo = target_dir.path().join("project-history-mutation");
    std::fs::create_dir_all(&target_repo).unwrap();
    let project = target_state
        .project_catalog
        .create_bound(rua_project::ProjectCreateRequest {
            resource_device_id: target_device_id.clone(),
            resource_root: target_repo.to_string_lossy().to_string(),
            display_name: Some("Project History Mutation".to_string()),
        })
        .unwrap()
        .project;
    source_state
        .project_catalog
        .apply_descriptor(project.clone())
        .unwrap();
    let chat_id = "00000000-0000-4000-8000-000000000433";
    let address = ChatAddress::project(project.project_id.clone(), chat_id);
    let target_scope = rua_core::ChatScopeStore::resolve(&target_state.rua_root, address.clone());
    seed_project_chat_state(target_scope.root(), &address, &target_device_id).await;
    let target_store = WorkspaceSessionStore::new(target_scope.root());
    target_store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: "turn-1".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-08-01T00:00:00Z".to_string(),
                    assistant_started_at: Some("2026-08-01T00:00:01Z".to_string()),
                    assistant_completed_at: Some("2026-08-01T00:00:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("variant-1".to_string()),
                messages: vec![
                    NewConversationMessage {
                        message_id: "user-1".to_string(),
                        role: "user".to_string(),
                        created_at: "2026-08-01T00:00:00Z".to_string(),
                        status: "completed".to_string(),
                        content: serde_json::json!([{"type": "text", "text": "inspect"}]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                    NewConversationMessage {
                        message_id: "variant-1".to_string(),
                        role: "assistant".to_string(),
                        created_at: "2026-08-01T00:00:02Z".to_string(),
                        status: "completed".to_string(),
                        content: serde_json::json!([{"type": "text", "text": "first answer"}]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                ],
            },
        )
        .await
        .unwrap();
    target_store
        .append_messages_to_turn_as_new_variant(
            chat_id,
            "turn-1",
            TurnLifecycle {
                user_submitted_at: "2026-08-01T00:00:00Z".to_string(),
                assistant_started_at: Some("2026-08-01T00:00:03Z".to_string()),
                assistant_completed_at: Some("2026-08-01T00:00:04Z".to_string()),
                status: "completed".to_string(),
            },
            Some("variant-2".to_string()),
            vec![NewConversationMessage {
                message_id: "variant-2".to_string(),
                role: "assistant".to_string(),
                created_at: "2026-08-01T00:00:04Z".to_string(),
                status: "completed".to_string(),
                content: serde_json::json!([{"type": "text", "text": "selected remotely"}]),
                provider: None,
                tool: None,
                metadata: serde_json::Map::new(),
            }],
        )
        .await
        .unwrap();
    assert!(target_store
        .set_turn_selected_variant(chat_id, "turn-1", "variant-1".to_string())
        .await
        .unwrap());
    let source = build_runtime(
        121,
        network_secret,
        vec![peer_entry(122)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let target = build_runtime(
        122,
        network_secret,
        vec![peer_entry(121)],
        Some(full_command_handler(&target_state, &target_device_id)),
        Vec::new(),
        test_config(),
    )
    .await;
    install_connectable_peer(&source, &target);
    target
        .runtime
        .session_manager()
        .set_live_activity_handler(Arc::new(
            ActiveStreamBroadcastDeviceSessionHandler::new(Arc::clone(
                &target_state.active_stream_broadcast_hub,
            ))
            .with_conversation_control_initial_state(
                Arc::clone(&target_state.conversation_authority),
                target_state.conversation_input_intake.clone(),
                target_device_id.clone(),
            ),
        ));
    *source_state.device_network.lock().await = Some(source.runtime.clone());
    let source_scope = rua_core::ChatScopeStore::resolve(&source_state.rua_root, address.clone());
    assert!(
        WorkspaceSessionStore::new(source_scope.root())
            .load_chat_header(chat_id)
            .await
            .unwrap()
            .is_none(),
        "controller has no Project Chat Header"
    );

    let app = rua_server::router::build_app(source_state.clone(), None);
    let attach = |viewer_id: &'static str| {
        app.clone().oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/api/projects/{}/chats/{chat_id}/console/live",
                    project.project_id
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "viewer_id": viewer_id,
                        "agent_id": "default",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
    };
    let first_live = attach("history-mutation-first").await.unwrap();
    let second_live = attach("history-mutation-second").await.unwrap();
    assert_eq!(first_live.status(), StatusCode::OK);
    assert_eq!(second_live.status(), StatusCode::OK);
    let mut first_viewer = first_live.into_body().into_data_stream();
    let mut second_viewer = second_live.into_body().into_data_stream();
    let _ = read_project_live_control(&mut first_viewer, "viewer_attached").await;
    let _ = read_project_live_control(&mut second_viewer, "viewer_attached").await;

    let selected = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!(
                    "/api/projects/{}/chats/{chat_id}",
                    project.project_id
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "turn_id": "turn-1",
                        "selected_variant_id": "variant-2",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let selected_status = selected.status();
    let selected_body = axum::body::to_bytes(selected.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        selected_status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&selected_body)
    );
    let first_selected = read_project_history_mutation(&mut first_viewer, "variant_selected").await;
    let second_selected =
        read_project_history_mutation(&mut second_viewer, "variant_selected").await;
    assert_eq!(first_selected, second_selected);
    assert_eq!(first_selected["turn_id"], "turn-1");
    assert_eq!(first_selected["selected_variant_id"], "variant-2");

    target_store
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: "turn-2".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-08-01T00:01:00Z".to_string(),
                    assistant_started_at: Some("2026-08-01T00:01:01Z".to_string()),
                    assistant_completed_at: Some("2026-08-01T00:01:02Z".to_string()),
                    status: "completed".to_string(),
                },
                selected_variant_id: Some("assistant-2".to_string()),
                messages: vec![
                    NewConversationMessage {
                        message_id: "user-2".to_string(),
                        role: "user".to_string(),
                        created_at: "2026-08-01T00:01:00Z".to_string(),
                        status: "completed".to_string(),
                        content: serde_json::json!([{"type": "text", "text": "later"}]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                    NewConversationMessage {
                        message_id: "assistant-2".to_string(),
                        role: "assistant".to_string(),
                        created_at: "2026-08-01T00:01:02Z".to_string(),
                        status: "completed".to_string(),
                        content: serde_json::json!([{"type": "text", "text": "later answer"}]),
                        provider: None,
                        tool: None,
                        metadata: serde_json::Map::new(),
                    },
                ],
            },
        )
        .await
        .unwrap();

    let rolled_back = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/api/projects/{}/chats/{chat_id}/rollback",
                    project.project_id
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "before_turn_id": "turn-2" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rolled_back.status(), StatusCode::OK);
    let first_rollback = read_project_history_mutation(&mut first_viewer, "rolled_back").await;
    let second_rollback = read_project_history_mutation(&mut second_viewer, "rolled_back").await;
    assert_eq!(first_rollback, second_rollback);
    assert_eq!(first_rollback["before_turn_id"], "turn-2");
    assert_eq!(first_rollback["selected_turn_id"], "turn-1");
    assert_eq!(first_rollback["selected_variant_id"], "variant-2");
    assert_eq!(
        target_store
            .read_turn_range(chat_id, 1, usize::MAX)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        WorkspaceSessionStore::new(source_scope.root())
            .load_chat_header(chat_id)
            .await
            .unwrap()
            .is_none(),
        "foreground controls do not require controller Project storage"
    );

    drop(first_viewer);
    drop(second_viewer);
    source.runtime.shutdown().await.expect("shutdown source");
    target.runtime.shutdown().await.expect("shutdown target");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn removed_remote_project_background_tool_route_does_not_transfer_execution() {
    let source_dir = TempDir::new().expect("controller state");
    let source_state = AppState::from_tempdir(source_dir.path().to_path_buf(), ServerProfile::Test);
    let target_dir = TempDir::new().expect("target state");
    let target_state = AppState::from_tempdir(target_dir.path().to_path_buf(), ServerProfile::Test);
    let network_secret = [123; 32];
    let target_sync_identity: rua_sync::identity::DeviceIdentity =
        serde_json::from_value(serde_json::json!({
            "secret_bytes": vec![125_u8; 32],
        }))
        .expect("fixed target Device identity");
    *target_state.sync_device.write().expect("sync Device lock") =
        Some(Arc::new(target_sync_identity));
    let target_device_id = hex::encode(
        DeviceNetworkIdentity::new(iroh::SecretKey::from_bytes(&[125; 32])).local_device_id(),
    );
    assert_eq!(
        rua_server::services::device_context::current_sync_device_id(&target_state.sync_device)
            .as_deref(),
        Some(target_device_id.as_str())
    );
    let target_repo = target_dir.path().join("project-background-authority");
    std::fs::create_dir_all(&target_repo).unwrap();
    let project = target_state
        .project_catalog
        .create_bound(rua_project::ProjectCreateRequest {
            resource_device_id: target_device_id.clone(),
            resource_root: target_repo.to_string_lossy().to_string(),
            display_name: Some("Project Background Authority".to_string()),
        })
        .unwrap()
        .project;
    source_state
        .project_catalog
        .apply_descriptor(project.clone())
        .unwrap();
    let chat_id = "00000000-0000-4000-8000-000000000434";
    let tool_call_id = "remote-project-background-tool";
    let address = ChatAddress::project(project.project_id.clone(), chat_id);
    let target_scope = rua_core::ChatScopeStore::resolve(&target_state.rua_root, address.clone());
    seed_project_chat_state(target_scope.root(), &address, &target_device_id).await;

    let source = build_runtime(
        124,
        network_secret,
        vec![peer_entry(125)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let target = build_runtime(
        125,
        network_secret,
        vec![peer_entry(124)],
        Some(full_command_handler(&target_state, &target_device_id)),
        Vec::new(),
        test_config(),
    )
    .await;
    install_connectable_peer(&source, &target);
    *source_state.device_network.lock().await = Some(source.runtime.clone());
    let source_scope = rua_core::ChatScopeStore::resolve(&source_state.rua_root, address.clone());
    assert!(
        WorkspaceSessionStore::new(source_scope.root())
            .load_chat_header(chat_id)
            .await
            .unwrap()
            .is_none(),
        "controller has no Project Chat Header"
    );

    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let mut toolkit = AgentToolkit::new();
    toolkit.register(
        ToolDefinition {
            name: "remote_project_blocking_tool".to_string(),
            description: "blocks until moved to Project Background Tool Task".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            strict: None,
            annotations: ToolAnnotations::default(),
            metadata: serde_json::Map::new(),
        },
        Arc::new(RemoteProjectBlockingTool {
            started: tokio::sync::Mutex::new(Some(started_tx)),
            release: tokio::sync::Mutex::new(Some(release_rx)),
        }),
    );
    let call = ToolCall::new(
        tool_call_id,
        "remote_project_blocking_tool",
        serde_json::json!({}),
    );
    let mut tool_ctx = rua_core::tool::ToolContext {
        agent_id: "default".to_string(),
        session_id: chat_id.to_string(),
        working_dir: target_repo,
        task_runtime: Some(Arc::new(
            rua_tool::extended::task_tools::TaskRuntimeHandle::new(Arc::clone(
                &target_state.task_runtime,
            )),
        )),
        ..Default::default()
    };
    tool_ctx
        .metadata
        .insert("agent_id".to_string(), serde_json::json!("default"));
    tool_ctx
        .metadata
        .insert("turn_id".to_string(), serde_json::json!("turn-background"));
    tool_ctx.metadata.insert(
        "security_chat_address".to_string(),
        serde_json::to_value(&address).unwrap(),
    );
    let hook: Arc<dyn react_agent::AgentHook> = Arc::new(react_agent::NoopHook);
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
    let cancel = CancellationToken::new();
    let foreground = tokio::spawn(async move {
        react_agent::react_loop::execute_single_tool(
            &call,
            &toolkit,
            &hook,
            &tool_ctx,
            &react_agent::ToolDisclosureConfig::default(),
            0,
            &cancel,
            Duration::from_secs(30),
            &event_tx,
        )
        .await
        .expect("foreground tool returns synthetic result")
    });
    started_rx.await.expect("resource tool started");

    let app = rua_server::router::build_app(source_state.clone(), None);
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/api/projects/{}/chats/{chat_id}/console/background-tool",
                    project.project_id
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "tool_call_id": tool_call_id }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!body.is_empty());

    release_tx.send(()).expect("release resource tool");
    let result = foreground.await.expect("foreground join");
    assert_eq!(result.text_content(), "remote Project background result");
    assert!(!result.metadata.contains_key("background_task_id"));
    let owner = rua_task::TaskOwner::from_address(address.clone());
    assert!(
        target_state
            .task_runtime
            .list(&owner, rua_task::TaskListFilter::default())
            .await
            .unwrap()
            .is_empty(),
        "removed route must not create a Project Task"
    );
    assert!(
        source_state
            .task_runtime
            .list(&owner, rua_task::TaskListFilter::default())
            .await
            .unwrap()
            .is_empty(),
        "controller must not create a Project Task"
    );
    source.runtime.shutdown().await.expect("shutdown source");
    target.runtime.shutdown().await.expect("shutdown target");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_project_first_submit_materializes_on_resource_device() {
    let source_dir = TempDir::new().expect("source state");
    let mut source_state =
        AppState::from_tempdir(source_dir.path().to_path_buf(), ServerProfile::Test);
    let second_dir = TempDir::new().expect("second viewer state");
    let second_state = AppState::from_tempdir(second_dir.path().to_path_buf(), ServerProfile::Test);
    let target_dir = TempDir::new().expect("target state");
    let target_state = AppState::from_tempdir(target_dir.path().to_path_buf(), ServerProfile::Test);
    seed_fixture_provider(
        &target_state,
        vec![
            fixture_provider_model("project-control-model"),
            fixture_provider_model("offline-project-model"),
        ],
    )
    .await;
    target_state
        .provider_manager
        .store_default_model(DefaultModel {
            provider_id: "fixture".to_string(),
            model: "offline-project-model".to_string(),
            source: None,
            extra: Default::default(),
        })
        .await;
    let network_secret = [91; 32];
    let target_device_id = hex::encode(
        DeviceNetworkIdentity::new(iroh::SecretKey::from_bytes(&[93; 32])).local_device_id(),
    );
    let target_repo = target_dir.path().join("remote-project");
    std::fs::create_dir_all(&target_repo).unwrap();
    let created = target_state
        .project_catalog
        .create_bound(rua_project::ProjectCreateRequest {
            resource_device_id: target_device_id.clone(),
            resource_root: target_repo.to_string_lossy().to_string(),
            display_name: Some("Remote Project".to_string()),
        })
        .unwrap();
    source_state
        .project_catalog
        .apply_descriptor(created.project.clone())
        .unwrap();
    second_state
        .project_catalog
        .apply_descriptor(created.project.clone())
        .unwrap();

    let target_handler = full_command_handler(&target_state, &target_device_id);
    let source = build_runtime(
        92,
        network_secret,
        vec![peer_entry(93)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let target = build_runtime(
        93,
        network_secret,
        vec![peer_entry(92), peer_entry(94)],
        Some(target_handler),
        Vec::new(),
        test_config(),
    )
    .await;
    let second = build_runtime(
        94,
        network_secret,
        vec![peer_entry(93)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    install_connectable_peer(&source, &target);
    install_connectable_peer(&second, &target);
    target
        .runtime
        .session_manager()
        .set_live_activity_handler(Arc::new(
            ActiveStreamBroadcastDeviceSessionHandler::new(Arc::clone(
                &target_state.active_stream_broadcast_hub,
            ))
            .with_conversation_control_initial_state(
                Arc::clone(&target_state.conversation_authority),
                target_state.conversation_input_intake.clone(),
                target_device_id.clone(),
            ),
        ));
    *source_state.device_network.lock().await = Some(source.runtime.clone());
    *second_state.device_network.lock().await = Some(second.runtime.clone());

    let chat_id = "00000000-0000-4000-8000-000000000404";
    let mut first_submit_extra = serde_json::Map::new();
    first_submit_extra.insert(
        "chat_model_slot".to_string(),
        serde_json::json!({
            "provider_id": "fixture",
            "model": "project-control-model",
            "thinking_strength": "auto",
        }),
    );
    let source_sync_guard = source_state.sync_node.lock().await;
    assert!(
        source_sync_guard.is_none(),
        "Issue 01 starts with Sync unavailable"
    );
    let outcome = tokio::time::timeout(
        Duration::from_secs(5),
        source_state
            .conversation_input_intake
            .submit_project_delivery(
                &created.project.project_id,
                "default",
                ConsoleChatRequest {
                    message: "Inspect the remote Project".to_string(),
                    chat_id: Some(chat_id.to_string()),
                    model: None,
                    is_reconnect: false,
                    attachments: Vec::new(),
                    input: None,
                    messages: None,
                    extra: first_submit_extra,
                    channel_kind: Some(rua_channel::core::renderer::ChannelKind::Chat),
                },
            ),
    )
    .await
    .expect("Project Submit acknowledgement must not await priority Sync");
    drop(source_sync_guard);
    let (submission_id, input_id, device_delivery_id) = match outcome {
        rua_server::services::conversation_input_intake::ConversationInputDeliveryOutcome::Delivered {
            transport: rua_server::services::conversation_input_intake::ConversationInputDeliveryTransport::RemoteAuthority,
            accepted: true,
            submission_id: Some(submission_id),
            input_id: Some(input_id),
            device_delivery_id: Some(device_delivery_id),
            ..
        } => (submission_id, input_id, device_delivery_id),
        outcome => panic!("unexpected Project Submit outcome: {outcome:?}"),
    };

    let address = ChatAddress::project(created.project.project_id.clone(), chat_id);
    let source_scope = rua_core::ChatScopeStore::resolve(&source_state.rua_root, address.clone());
    assert!(
        rua_server::services::chat_index::get(source_scope.root(), chat_id)
            .await
            .unwrap()
            .is_none(),
        "foreground Project Live must not depend on synchronized source storage"
    );
    assert!(
        WorkspaceSessionStore::new(source_scope.root())
            .load_chat_header(chat_id)
            .await
            .unwrap()
            .is_none(),
        "source Project Chat Header must still be absent"
    );
    assert!(
        !source_scope.evergreen_chat_root().exists(),
        "source Evergreen Chat storage must still be absent"
    );
    let target_scope = rua_core::ChatScopeStore::resolve(&target_state.rua_root, address.clone());
    let target_store = WorkspaceSessionStore::new(target_scope.root());
    let header = target_store
        .load_chat_header(chat_id)
        .await
        .unwrap()
        .expect("resource Device Project Chat header");
    assert_eq!(header.agent_id, "default");
    assert_eq!(header.extra["model"]["provider_id"], "fixture");
    assert_eq!(header.extra["model"]["model"], "project-control-model");
    assert_eq!(header.turn_index.latest_turn, 1);
    assert!(header.message_log.latest_sequence >= 1);
    let turns = target_store
        .read_turn_range(chat_id, 1, usize::MAX)
        .await
        .unwrap();
    assert_eq!(turns.len(), 1);
    let messages = target_store
        .read_turn_messages(chat_id, &turns[0].turn_id)
        .await
        .unwrap();
    let stored_user = messages
        .iter()
        .find(|message| message.role == "user")
        .expect("resource Device persisted accepted user boundary");
    assert!(stored_user
        .content
        .to_string()
        .contains("Inspect the remote Project"));
    assert_eq!(stored_user.metadata["submission_id"], submission_id);
    assert_eq!(stored_user.metadata["input_id"], input_id);
    assert_eq!(
        stored_user.metadata["device_delivery_id"],
        device_delivery_id
    );
    let placement =
        rua_server::services::conversation_authority_target::load_chat_authority_placement_target(
            target_scope.root(),
            chat_id,
        )
        .await
        .unwrap();
    target_state
        .active_stream_broadcast_hub
        .publish_conversation_event(
            "project-user-before-sync",
            &address,
            placement.authority_epoch,
            serde_json::json!({
                "type": "user_message",
                "message": {
                    "id": stored_user.message_id,
                    "role": stored_user.role,
                    "turn_id": turns[0].turn_id,
                    "content": stored_user.content,
                    "metadata": stored_user.metadata,
                },
            })
            .to_string(),
        )
        .await;
    target_state
        .active_stream_broadcast_hub
        .publish_conversation_event(
            "project-live-before-sync",
            &address,
            placement.authority_epoch,
            serde_json::json!({
                "type": "text_delta",
                "delta": "live-before-sync",
            })
            .to_string(),
        )
        .await;
    let (history_sync_tx, mut history_sync_rx) =
        tokio::sync::mpsc::unbounded_channel::<ConversationHistorySyncRequest>();
    let restore_sync = Arc::new(Notify::new());
    let restore_sync_for_worker = Arc::clone(&restore_sync);
    let target_scope_root = target_scope.root().to_path_buf();
    let source_scope_root = source_scope.root().to_path_buf();
    let target_device_id_for_sync = target_device_id.clone();
    let history_sync_worker = tokio::spawn(async move {
        let request = history_sync_rx
            .recv()
            .await
            .expect("Project Live requests stable history Sync");
        assert_eq!(request.source_device_id, target_device_id_for_sync);
        restore_sync_for_worker.notified().await;
        copy_tree(&target_scope_root, &source_scope_root)
            .expect("restore Project Chat stable history");
        let _ = request.result_tx.send(Ok(()));
    });
    source_state.console_facade = Arc::new(
        ConsoleChatFacadeImpl::new(
            source_state.conversation_input_intake.clone(),
            Arc::clone(&source_state.streams),
            Arc::clone(&source_state.registry),
            Arc::clone(&source_state.sync_device),
            Arc::clone(&source_state.sync_node),
            Arc::clone(&source_state.device_network),
            source_state.rua_root.clone(),
        )
        .with_conversation_authority(Arc::clone(&source_state.conversation_authority))
        .with_project_catalog(Arc::clone(&source_state.project_catalog))
        .with_history_sync(history_sync_tx)
        .with_chat_activity_wake_hub(Arc::clone(&source_state.chat_activity_wake_hub))
        .with_acp_chat_registry(Arc::clone(&source_state.acp_chat_registry))
        .with_active_stream_broadcast_hub(Arc::clone(&source_state.active_stream_broadcast_hub)),
    );
    let app = rua_server::router::build_app(source_state.clone(), None);
    let live_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/api/projects/{}/chats/{chat_id}/console/live",
                    created.project.project_id
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "viewer_id": "project-viewer-before-sync",
                        "agent_id": "default",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        live_response.status(),
        StatusCode::OK,
        "Project Live HTTP must not require source-local Chat storage"
    );
    let mut viewer = live_response.into_body().into_data_stream();
    let (initial_control, user_boundary, live_delta) =
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut initial_control = None;
            let mut user_boundary = None;
            let mut live_delta = None;
            let mut buffered = String::new();
            while initial_control.is_none() || user_boundary.is_none() || live_delta.is_none() {
                let chunk = viewer
                    .next()
                    .await
                    .expect("Project live viewer remains open")
                    .expect("Project live payload is infallible");
                buffered.push_str(std::str::from_utf8(&chunk).expect("Project live UTF-8"));
                while let Some(newline) = buffered.find('\n') {
                    let line = buffered[..newline].trim().to_string();
                    buffered.drain(..=newline);
                    let Some(payload) = parse_project_live_sse_line(&line) else {
                        continue;
                    };
                    match payload.get("type").and_then(serde_json::Value::as_str) {
                        Some("conversation_control") => initial_control = Some(payload),
                        Some("user_message")
                            if payload
                                .pointer("/message/metadata/submission_id")
                                .and_then(serde_json::Value::as_str)
                                == Some(submission_id.as_str()) =>
                        {
                            assert!(
                                initial_control.is_some(),
                                "initial Control View must precede accepted user output"
                            );
                            user_boundary = Some(payload);
                        }
                        Some("text_delta") if payload["delta"] == "live-before-sync" => {
                            assert!(
                                initial_control.is_some(),
                                "initial Control View must precede assistant output"
                            );
                            live_delta = Some(payload);
                        }
                        Some("stream_replay") => {
                            if let Some(events) =
                                payload.get("events").and_then(serde_json::Value::as_array)
                            {
                                if events.iter().any(|event| {
                                    matches!(
                                        event.get("type").and_then(serde_json::Value::as_str),
                                        Some("user_message" | "text_delta")
                                    )
                                }) {
                                    assert!(
                                        initial_control.is_some(),
                                        "initial Control View must precede replay output"
                                    );
                                }
                                user_boundary = user_boundary.or_else(|| {
                                    events
                                        .iter()
                                        .find(|event| {
                                            event["type"] == "user_message"
                                                && event
                                                    .pointer("/message/metadata/submission_id")
                                                    .and_then(serde_json::Value::as_str)
                                                    == Some(submission_id.as_str())
                                        })
                                        .cloned()
                                });
                                live_delta = live_delta.or_else(|| {
                                    events
                                        .iter()
                                        .find(|event| {
                                            event["type"] == "text_delta"
                                                && event["delta"] == "live-before-sync"
                                        })
                                        .cloned()
                                });
                            }
                        }
                        Some("error") => panic!("Project Live attachment failed: {payload}"),
                        _ => {}
                    }
                }
            }
            (
                initial_control.unwrap(),
                user_boundary.unwrap(),
                live_delta.unwrap(),
            )
        })
        .await
        .expect("Project Live must attach before source Sync");
    assert_eq!(
        initial_control["control"]["placement"]["authority_device_id"],
        target_device_id
    );
    assert_eq!(
        initial_control["control"]["placement"]["authority_epoch"],
        placement.authority_epoch.to_string()
    );
    assert_eq!(initial_control["control"]["placement"]["switchable"], false);
    assert_eq!(
        initial_control["control"]["allowed_controls"]["switch_authority"],
        false
    );
    assert_eq!(
        user_boundary["message"]["metadata"]["submission_id"],
        submission_id
    );
    assert_eq!(user_boundary["message"]["metadata"]["input_id"], input_id);
    assert_eq!(
        user_boundary["message"]["metadata"]["device_delivery_id"],
        device_delivery_id
    );
    assert_eq!(live_delta["delta"], "live-before-sync");

    let second_scope = rua_core::ChatScopeStore::resolve(&second_state.rua_root, address.clone());
    assert!(
        WorkspaceSessionStore::new(second_scope.root())
            .load_chat_header(chat_id)
            .await
            .unwrap()
            .is_none(),
        "second viewer has no Project Chat Header before Live"
    );
    let second_app = rua_server::router::build_app(second_state.clone(), None);
    let second_live = second_app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/api/projects/{}/chats/{chat_id}/console/live",
                    created.project.project_id
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "viewer_id": "project-second-viewer-before-sync",
                        "agent_id": "default",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second_live.status(), StatusCode::OK);
    let mut second_viewer = second_live.into_body().into_data_stream();
    let (second_control, second_replay) = read_project_live_snapshot(&mut second_viewer).await;
    for field in ["authority_device_id", "authority_epoch", "switchable"] {
        assert_eq!(
            second_control["control"]["placement"][field],
            initial_control["control"]["placement"][field],
            "both Project viewers must observe the same stable authority field {field}"
        );
    }
    let second_replay_events = second_replay["events"].as_array().unwrap();
    assert!(second_replay_events.iter().any(|event| {
        event["type"] == "user_message"
            && event["message"]["metadata"]["submission_id"] == submission_id
    }));
    assert!(second_replay_events
        .iter()
        .any(|event| event["type"] == "text_delta" && event["delta"] == "live-before-sync"));
    assert!(
        WorkspaceSessionStore::new(second_scope.root())
            .load_chat_header(chat_id)
            .await
            .unwrap()
            .is_none(),
        "second viewer Live does not require stable Project storage"
    );

    assert!(target_scope.chat_index_path().is_file());
    assert!(!target_repo.join("sessions").exists());
    assert!(source_state.project_bindings.list().unwrap().is_empty());

    target_state
        .agent_service
        .create_agent(rua_server::endpoints::dto::AgentCreate {
            id: Some("code".to_string()),
            name: "Code Agent".to_string(),
            description: String::new(),
            workspace_dir: None,
            template_id: None,
            skill_names: None,
            extra: serde_json::Map::new(),
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if target_state
                .conversation_authority
                .active_turn(&ChatAddress::project(
                    created.project.project_id.clone(),
                    chat_id,
                ))
                .await
                .is_none()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("resource Project Chat did not become idle");

    let published_manifest = load_conversation_manifest(target_scope.root(), &address)
        .await
        .expect("load completed Project Chat manifest")
        .expect("completed Project Chat manifest exists");
    DurablePublicationWatermarkStore::new(target_state.rua_root.join("sync"))
        .expect("open target watermark store")
        .advance(&address, &published_manifest)
        .await
        .expect("record fixture publication watermark");
    let hydration_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/api/projects/{}/chats/{chat_id}/console/live",
                    created.project.project_id
                ))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "viewer_id": "project-viewer-after-publication",
                        "agent_id": "default",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .expect("reattach after Project history publication");
    assert_eq!(hydration_response.status(), StatusCode::OK);
    let mut hydration_viewer = hydration_response.into_body().into_data_stream();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(150),
            read_project_history_watermark(&mut hydration_viewer),
        )
        .await
        .is_err(),
        "history watermark must not be emitted before canonical apply completes"
    );
    assert!(
        WorkspaceSessionStore::new(source_scope.root())
            .load_chat_header(chat_id)
            .await
            .unwrap()
            .is_none(),
        "source durable History must remain at its prior prefix before apply"
    );
    restore_sync.notify_one();
    tokio::time::timeout(Duration::from_secs(10), history_sync_worker)
        .await
        .expect("Project history Sync request was not issued after publication")
        .expect("Project history Sync worker");
    assert!(
        rua_server::services::chat_index::get(source_scope.root(), chat_id)
            .await
            .unwrap()
            .is_some(),
        "restored Sync must materialize the source Chat Index"
    );
    let source_store = WorkspaceSessionStore::new(source_scope.root());
    let source_turns = source_store
        .read_turn_range(chat_id, 1, usize::MAX)
        .await
        .unwrap();
    let stable_target_turns = target_store
        .read_turn_range(chat_id, 1, usize::MAX)
        .await
        .unwrap();
    assert_eq!(source_turns.len(), stable_target_turns.len());
    let source_messages = source_store
        .read_turn_messages(chat_id, &source_turns[0].turn_id)
        .await
        .unwrap();
    let stable_target_messages = target_store
        .read_turn_messages(chat_id, &stable_target_turns[0].turn_id)
        .await
        .unwrap();
    assert_eq!(
        source_messages
            .iter()
            .map(|message| &message.message_id)
            .collect::<Vec<_>>(),
        stable_target_messages
            .iter()
            .map(|message| &message.message_id)
            .collect::<Vec<_>>(),
        "stable history must replace live projection without duplicate messages"
    );
    let reached = tokio::time::timeout(
        Duration::from_secs(5),
        read_project_history_watermark(&mut hydration_viewer),
    )
    .await
    .expect("history watermark follows canonical apply");
    assert_eq!(reached["address"], serde_json::to_value(&address).unwrap());
    assert_eq!(
        reached["durable_sequence"],
        published_manifest.latest_sequence
    );
    assert_eq!(reached["manifest_hash"], published_manifest.hash().unwrap());
    let sibling_chat_id = "00000000-0000-4000-8000-000000000425";
    let sibling_outcome = source_state
        .conversation_input_intake
        .submit_project_delivery(
            &created.project.project_id,
            "default",
            ConsoleChatRequest {
                message: "Run independently".to_string(),
                chat_id: Some(sibling_chat_id.to_string()),
                model: None,
                is_reconnect: false,
                attachments: Vec::new(),
                input: None,
                messages: None,
                extra: serde_json::Map::new(),
                channel_kind: Some(rua_channel::core::renderer::ChannelKind::Chat),
            },
        )
        .await;
    assert!(matches!(
        sibling_outcome,
        rua_server::services::conversation_input_intake::ConversationInputDeliveryOutcome::Delivered {
            transport: rua_server::services::conversation_input_intake::ConversationInputDeliveryTransport::RemoteAuthority,
            accepted: true,
            ..
        }
    ));
    let sibling_address = ChatAddress::project(created.project.project_id.clone(), sibling_chat_id);
    tokio::time::timeout(Duration::from_secs(10), async {
        while target_state
            .conversation_authority
            .active_turn(&sibling_address)
            .await
            .is_some()
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("sibling Project Chat did not become idle");
    let sibling_scope =
        rua_core::ChatScopeStore::resolve(&target_state.rua_root, sibling_address.clone());
    let sibling_header = WorkspaceSessionStore::new(sibling_scope.root())
        .load_chat_header(sibling_chat_id)
        .await
        .unwrap()
        .expect("sibling Project Chat header");
    assert_eq!(sibling_header.extra["model"]["provider_id"], "fixture");
    assert_eq!(
        sibling_header.extra["model"]["model"],
        "offline-project-model"
    );
    let sibling_placement =
        rua_server::services::conversation_authority_target::load_chat_authority_placement_target(
            sibling_scope.root(),
            sibling_chat_id,
        )
        .await
        .unwrap();
    target_state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: sibling_address.clone(),
            driver: ConversationDriverKind::Rua,
            authority_device_id: sibling_placement.device_id,
            authority_epoch: sibling_placement.authority_epoch,
            driver_detail: None,
        })
        .await;
    let switched = deliver_project_device_command_via_session(
        source.runtime.session_manager(),
        &ProjectDeviceCommand {
            command_id: "project-agent-switch-remote".to_string(),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            action: ProjectDeviceAction::SwitchChatAgent {
                project_id: created.project.project_id.clone(),
                chat_id: chat_id.to_string(),
                expected_agent_id: "default".to_string(),
                agent_id: "code".to_string(),
            },
        },
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert!(matches!(
        switched,
        ProjectDeviceResponse::ChatAgentSelection {
            state: rua_server::services::project_chat::ProjectChatAgentState {
                selection:
                    rua_server::services::project_chat::ProjectChatAgentSelection {
                        agent_id,
                        revision: 2,
                        ..
                    },
                ..
            }
        } if agent_id == "code"
    ));
    let switched_header = target_store
        .load_chat_header(chat_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(switched_header.extra["selected_agent_id"], "code");
    assert!(target_state
        .conversation_authority
        .active_turn(&sibling_address)
        .await
        .is_some());
    let first_placement =
        rua_server::services::conversation_authority_target::load_chat_authority_placement_target(
            target_scope.root(),
            chat_id,
        )
        .await
        .unwrap();
    let first_address = ChatAddress::project(created.project.project_id.clone(), chat_id);
    target_state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: first_address.clone(),
            driver: ConversationDriverKind::Rua,
            authority_device_id: first_placement.device_id,
            authority_epoch: first_placement.authority_epoch,
            driver_detail: None,
        })
        .await;
    assert!(target_state
        .conversation_authority
        .active_turn(&first_address)
        .await
        .is_some());
    assert!(target_state
        .conversation_authority
        .active_turn(&sibling_address)
        .await
        .is_some());
    target_state
        .conversation_authority
        .finish_active_turn(&first_address)
        .await;
    assert!(target_state
        .conversation_authority
        .active_turn(&sibling_address)
        .await
        .is_some());
    target_state
        .conversation_authority
        .finish_active_turn(&sibling_address)
        .await;

    let updated = deliver_project_device_command_via_session(
        source.runtime.session_manager(),
        &ProjectDeviceCommand {
            command_id: "project-chat-update-remote".to_string(),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            action: ProjectDeviceAction::UpdateChat {
                project_id: created.project.project_id.clone(),
                chat_id: chat_id.to_string(),
                update: rua_server::services::project_chat::ProjectChatUpdate {
                    name: Some("Remote renamed".to_string()),
                    pinned: Some(true),
                    tag: None,
                    pin_order: Some(0),
                    model: None,
                    collaboration_mode: None,
                    archived: None,
                },
            },
        },
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert!(matches!(
        updated,
        ProjectDeviceResponse::ChatUpdated { chat }
            if chat["name"] == "Remote renamed" && chat["pinned"] == true
    ));

    let control_address = ChatAddress::project(created.project.project_id.clone(), chat_id);
    let control_placement =
        rua_server::services::conversation_authority_target::load_chat_authority_placement_target(
            target_scope.root(),
            chat_id,
        )
        .await
        .unwrap();
    let workspace = deliver_conversation_control_via_session(
        source.runtime.session_manager(),
        &ConversationControlDeviceCommand {
            command_id: "project-resource-policy-remote".to_string(),
            agent_id: "code".to_string(),
            address: control_address.clone(),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            authority_epoch: control_placement.authority_epoch,
            action: ConversationControlDeviceAction::ViewResourcePolicy,
        },
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert!(matches!(
        workspace,
        ConversationControlDeviceResponse::ResourcePolicy { workspace }
            if workspace.policy.profile
                == rua_security::ProjectResourceProfile::ResourceWrite
                && workspace.policy.cwd == target_repo.to_string_lossy()
                && workspace.policy.dedicated_operations_root_confined
    ));
    let model = deliver_conversation_control_via_session(
        source.runtime.session_manager(),
        &ConversationControlDeviceCommand {
            command_id: "project-model-control-remote".to_string(),
            agent_id: "code".to_string(),
            address: control_address.clone(),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            authority_epoch: control_placement.authority_epoch,
            action: ConversationControlDeviceAction::UpdateModel {
                model: serde_json::json!({
                    "provider_id": "fixture",
                    "model": "project-control-model",
                    "thinking_strength": "auto",
                }),
            },
        },
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert!(matches!(
        model,
        ConversationControlDeviceResponse::ModelUpdated { model, .. }
            if model["provider_id"] == "fixture"
                && model["model"] == "project-control-model"
    ));
    assert_eq!(
        target_store
            .load_chat_header(chat_id)
            .await
            .unwrap()
            .unwrap()
            .extra["model"]["model"],
        "project-control-model"
    );

    let running = deliver_conversation_control_via_session(
        source.runtime.session_manager(),
        &ConversationControlDeviceCommand {
            command_id: "project-options-view-remote".to_string(),
            agent_id: "code".to_string(),
            address: control_address.clone(),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            authority_epoch: control_placement.authority_epoch,
            action: ConversationControlDeviceAction::ViewRunningConfig,
        },
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    let ConversationControlDeviceResponse::RunningConfig { running } = running else {
        panic!("unexpected Project options view response");
    };
    let updated_running = deliver_conversation_control_via_session(
        source.runtime.session_manager(),
        &ConversationControlDeviceCommand {
            command_id: "project-options-update-remote".to_string(),
            agent_id: "code".to_string(),
            address: control_address.clone(),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            authority_epoch: control_placement.authority_epoch,
            action: ConversationControlDeviceAction::UpdateRunningConfig {
                running: running.clone(),
            },
        },
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert!(matches!(
        updated_running,
        ConversationControlDeviceResponse::RunningConfigUpdated {
            running: confirmed,
            ..
        } if confirmed == running
    ));

    let acp_status = deliver_acp_chat_command_via_session(
        source.runtime.session_manager(),
        &AcpChatDeviceCommand {
            command_id: "project-acp-status-remote".to_string(),
            agent_id: "code".to_string(),
            address: control_address.clone(),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            authority_epoch: control_placement.authority_epoch,
            action: AcpChatDeviceAction::Status,
        },
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert!(matches!(
        acp_status,
        AcpChatDeviceResponse::Status {
            status: AcpChatDeviceStatus {
                active: false,
                workspace_dir,
                ..
            }
        } if workspace_dir == target_repo.to_string_lossy()
    ));

    let approval = target_state
        .security
        .approval_authority()
        .create_request(ApprovalRequestInput {
            agent_id: "code".to_string(),
            owner: rua_security::ApprovalOwner::chat(control_address.clone()),
            turn_id: "turn-project-control".to_string(),
            tool_call_id: "call-project-control".to_string(),
            tool_name: "bash".to_string(),
            trigger: ApprovalTrigger::approval_mode("approval required"),
            tool_call: ToolCallSnapshot {
                display: "bash".to_string(),
                args: serde_json::json!({"command": "pwd"}),
            },
        })
        .await;
    let approval_snapshot = deliver_conversation_control_via_session(
        source.runtime.session_manager(),
        &ConversationControlDeviceCommand {
            command_id: "project-approval-view-remote".to_string(),
            agent_id: "code".to_string(),
            address: control_address.clone(),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            authority_epoch: control_placement.authority_epoch,
            action: ConversationControlDeviceAction::ApprovalSnapshot,
        },
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert!(matches!(
        approval_snapshot,
        ConversationControlDeviceResponse::ApprovalSnapshot { snapshot }
            if snapshot["pending"][0]["id"] == approval.id
    ));
    let approval_decision = deliver_conversation_control_via_session(
        source.runtime.session_manager(),
        &ConversationControlDeviceCommand {
            command_id: "project-approval-decide-remote".to_string(),
            agent_id: "code".to_string(),
            address: control_address.clone(),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            authority_epoch: control_placement.authority_epoch,
            action: ConversationControlDeviceAction::DecideApproval {
                approval_id: approval.id.clone(),
                decision: ConversationApprovalDecision::RunOnce {
                    decided_by: "test".to_string(),
                },
            },
        },
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert!(matches!(
        approval_decision,
        ConversationControlDeviceResponse::ApprovalDecided { request }
            if request["id"] == approval.id && request["status"] == "approved"
    ));

    target_state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: control_address.clone(),
            driver: ConversationDriverKind::Rua,
            authority_device_id: control_placement.device_id.clone(),
            authority_epoch: control_placement.authority_epoch,
            driver_detail: None,
        })
        .await;
    let control_key = StreamKey::new(
        CONSOLE_CHANNEL_ID,
        control_address.clone(),
        Priority::Normal,
    );
    let (control_producer_id, mut control_cancel_rx) = target_state
        .streams
        .register_active_keyed(&control_key)
        .await;
    let queued = target_state
        .conversation_authority
        .enqueue_input(
            &control_address,
            ConversationInputPayload {
                message: "remote queued".to_string(),
                model_override: None,
                input: None,
                messages: None,
                extra: serde_json::Map::new(),
                attachments: Vec::new(),
                enqueued_at: Instant::now(),
            },
            Some(source.device_id.clone()),
        )
        .await
        .unwrap();
    let edited = deliver_conversation_control_via_session(
        source.runtime.session_manager(),
        &ConversationControlDeviceCommand {
            command_id: "project-queue-edit-remote".to_string(),
            agent_id: "code".to_string(),
            address: control_address.clone(),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            authority_epoch: control_placement.authority_epoch,
            action: ConversationControlDeviceAction::EditQueuedInput {
                input_id: queued.input_id.clone(),
                message: "remote edited".to_string(),
            },
        },
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert!(matches!(
        edited,
        ConversationControlDeviceResponse::Edited { entry, .. }
            if entry.input_id == queued.input_id && entry.text == "remote edited"
    ));
    let authority = Arc::clone(&target_state.conversation_authority);
    let streams = Arc::clone(&target_state.streams);
    let finish_control = control_address.clone();
    let finish_control_key = control_key.clone();
    let finish_control_task = tokio::spawn(async move {
        control_cancel_rx.changed().await.expect("stop signal");
        authority.finish_active_turn(&finish_control).await;
        streams
            .mark_completed(&finish_control_key, control_producer_id)
            .await;
    });
    let stopped = deliver_remote_chat_control_via_session(
        source.runtime.session_manager(),
        &RemoteChatControlCommand {
            command_id: "project-stop-remote".to_string(),
            agent_id: "code".to_string(),
            address: control_address.clone(),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            authority_epoch: control_placement.authority_epoch,
            device_delivery_id: None,
            await_terminal: true,
            action: RemoteChatControlAction::StopCurrentTurn,
        },
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert!(stopped.applied);
    finish_control_task.await.expect("stop completion task");

    let offline_chat_id = "00000000-0000-4000-8000-000000000424";
    let offline_outcome = source_state
        .conversation_input_intake
        .submit_project_delivery(
            &created.project.project_id,
            "default",
            ConsoleChatRequest {
                message: "Materialize before disconnect".to_string(),
                chat_id: Some(offline_chat_id.to_string()),
                model: None,
                is_reconnect: false,
                attachments: Vec::new(),
                input: None,
                messages: None,
                extra: serde_json::Map::new(),
                channel_kind: Some(rua_channel::core::renderer::ChannelKind::Chat),
            },
        )
        .await;
    assert!(matches!(
        offline_outcome,
        rua_server::services::conversation_input_intake::ConversationInputDeliveryOutcome::Delivered {
            accepted: true,
            ..
        }
    ));
    let offline_address = ChatAddress::project(created.project.project_id.clone(), offline_chat_id);
    tokio::time::timeout(Duration::from_secs(10), async {
        while target_state
            .conversation_authority
            .active_turn(&offline_address)
            .await
            .is_some()
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("offline Project Chat did not become idle");
    let offline_scope =
        rua_core::ChatScopeStore::resolve(&target_state.rua_root, offline_address.clone());
    let offline_store = WorkspaceSessionStore::new(offline_scope.root());
    let offline_header_before = offline_store
        .load_chat_header(offline_chat_id)
        .await
        .unwrap()
        .unwrap();
    let offline_turn_count_before = offline_store
        .read_turn_range(offline_chat_id, 1, usize::MAX)
        .await
        .unwrap()
        .len();
    let offline_model_before = offline_header_before.extra.get("model").cloned();
    let offline_placement =
        rua_server::services::conversation_authority_target::load_chat_authority_placement_target(
            offline_scope.root(),
            offline_chat_id,
        )
        .await
        .unwrap();
    std::fs::remove_dir_all(&target_repo).unwrap();
    let path_missing_submit = source_state
        .conversation_input_intake
        .submit_project_delivery(
            &created.project.project_id,
            "default",
            ConsoleChatRequest {
                message: "must not survive path loss".to_string(),
                chat_id: Some(offline_chat_id.to_string()),
                model: None,
                is_reconnect: false,
                attachments: Vec::new(),
                input: None,
                messages: None,
                extra: serde_json::Map::new(),
                channel_kind: Some(rua_channel::core::renderer::ChannelKind::Chat),
            },
        )
        .await;
    assert!(!matches!(
        path_missing_submit,
        rua_server::services::conversation_input_intake::ConversationInputDeliveryOutcome::Delivered {
            accepted: true,
            ..
        }
    ));
    assert_eq!(
        offline_store
            .read_turn_range(offline_chat_id, 1, usize::MAX)
            .await
            .unwrap()
            .len(),
        offline_turn_count_before
    );
    assert!(target_state
        .conversation_authority
        .queue_snapshot(&offline_address)
        .await
        .is_empty());
    std::fs::create_dir_all(&target_repo).unwrap();
    assert_eq!(
        offline_store
            .read_turn_range(offline_chat_id, 1, usize::MAX)
            .await
            .unwrap()
            .len(),
        offline_turn_count_before
    );
    let offline_source_scope =
        rua_core::ChatScopeStore::resolve(&source_state.rua_root, offline_address.clone());
    copy_tree(offline_scope.root(), offline_source_scope.root())
        .expect("synchronize stable offline Project Chat history");
    target.runtime.shutdown().await.unwrap();
    let stable_offline_history = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/api/projects/{}/chats/{offline_chat_id}",
                    created.project.project_id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        stable_offline_history.status(),
        StatusCode::OK,
        "stable Project history remains readable after resource Device shutdown"
    );
    let offline_submit = source_state
        .conversation_input_intake
        .submit_project_delivery(
            &created.project.project_id,
            "default",
            ConsoleChatRequest {
                message: "must not survive disconnect".to_string(),
                chat_id: Some(offline_chat_id.to_string()),
                model: None,
                is_reconnect: false,
                attachments: Vec::new(),
                input: None,
                messages: None,
                extra: serde_json::Map::from_iter([(
                    "submission_id".to_string(),
                    serde_json::json!("offline-project-unconfirmed-submission"),
                )]),
                channel_kind: Some(rua_channel::core::renderer::ChannelKind::Chat),
            },
        )
        .await;
    let rua_server::services::conversation_input_intake::ConversationInputDeliveryOutcome::Unconfirmed {
        chat_id: unconfirmed_chat_id,
        submission_id,
        device_delivery_id,
        target_device_id,
        device_delivery_sequence,
        ..
    } = offline_submit else {
        panic!("an absent remote acknowledgement must retain the submitted identifiers: {offline_submit:?}");
    };
    assert_eq!(unconfirmed_chat_id, offline_chat_id);
    assert_eq!(submission_id, "offline-project-unconfirmed-submission");
    assert!(device_delivery_id.starts_with("device-delivery-"));
    assert_eq!(target_device_id, target.device_id);
    assert!(device_delivery_sequence > 0);
    assert_eq!(
        offline_store
            .read_turn_range(offline_chat_id, 1, usize::MAX)
            .await
            .unwrap()
            .len(),
        offline_turn_count_before
    );
    assert!(target_state
        .conversation_authority
        .queue_snapshot(&offline_address)
        .await
        .is_empty());
    assert!(
        rua_server::services::chat_index::get(offline_source_scope.root(), offline_chat_id)
            .await
            .unwrap()
            .is_some()
    );
    let offline_control = deliver_conversation_control_via_session(
        source.runtime.session_manager(),
        &ConversationControlDeviceCommand {
            command_id: "project-model-control-offline".to_string(),
            agent_id: "default".to_string(),
            address: offline_address.clone(),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            authority_epoch: offline_placement.authority_epoch,
            action: ConversationControlDeviceAction::UpdateModel {
                model: serde_json::json!({
                    "provider_id": "fixture",
                    "model": "offline-project-model",
                    "thinking_strength": "auto",
                }),
            },
        },
        Duration::from_secs(2),
    )
    .await
    .expect_err("offline Project control must fail");
    assert_eq!(offline_control.failure_kind(), "device_offline");
    assert_eq!(
        offline_store
            .load_chat_header(offline_chat_id)
            .await
            .unwrap()
            .unwrap()
            .extra
            .get("model")
            .cloned(),
        offline_model_before
    );
    assert_eq!(
        offline_store
            .read_turn_range(offline_chat_id, 1, usize::MAX)
            .await
            .unwrap()
            .len(),
        offline_turn_count_before
    );

    drop(second_viewer);
    second.runtime.shutdown().await.unwrap();
    source.runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_project_turns_read_fresh_instructions_only_from_the_resource_root() {
    const FIRST_MARKER: &str = "RESOURCE_PROJECT_INSTRUCTIONS_FIRST_12A7";
    const SECOND_MARKER: &str = "RESOURCE_PROJECT_INSTRUCTIONS_SECOND_12B8";
    const CONTROLLER_MARKER: &str = "CONTROLLER_INSTRUCTIONS_MUST_NOT_APPEAR_12C9";
    const PROJECT_SKILL_MARKER: &str = "RESOURCE_PROJECT_SKILL_BODY_13B2";

    let source_dir = TempDir::new().expect("source state");
    let source_state = AppState::from_tempdir(source_dir.path().join("data"), ServerProfile::Test);
    std::fs::write(
        source_state.rua_root.join("AGENTS.md"),
        CONTROLLER_MARKER.as_bytes(),
    )
    .expect("controller instructions");
    let target_dir = TempDir::new().expect("target state");
    let target_state = AppState::from_tempdir(target_dir.path().join("data"), ServerProfile::Test);
    let repository = TempDir::new().expect("resource repository");
    let target_repo = repository.path().join("project");
    std::fs::create_dir_all(&target_repo).expect("resource Project root");
    std::fs::write(target_repo.join("AGENTS.md"), FIRST_MARKER.as_bytes())
        .expect("first Project Instructions");
    std::fs::create_dir_all(target_repo.join(".agents/skills/resource-only"))
        .expect("Project Skill directory");
    std::fs::write(
        target_repo.join(".agents/skills/resource-only/SKILL.md"),
        format!(
            "---\nname: resource-only\ndescription: Resource-only Project Skill\n---\n\n{PROJECT_SKILL_MARKER}"
        ),
    )
    .expect("Project Skill");

    let network_secret = [101; 32];
    let target_device_id = hex::encode(
        DeviceNetworkIdentity::new(iroh::SecretKey::from_bytes(&[103; 32])).local_device_id(),
    );
    let created = target_state
        .project_catalog
        .create_bound(rua_project::ProjectCreateRequest {
            resource_device_id: target_device_id.clone(),
            resource_root: target_repo.to_string_lossy().to_string(),
            display_name: Some("Instruction Project".to_string()),
        })
        .expect("create bound Project");
    source_state
        .project_catalog
        .apply_descriptor(created.project.clone())
        .expect("replicate Project descriptor");

    let captured = CapturingAgentRuntime::default();
    target_state
        .registry
        .get_or_create("default")
        .await
        .expect("default Agent runtime")
        .set_react_runtime(Arc::new(captured.clone()))
        .await;

    let target_handler = full_command_handler(&target_state, &target_device_id);
    let source = build_runtime(
        102,
        network_secret,
        vec![peer_entry(103)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let target = build_runtime(
        103,
        network_secret,
        vec![peer_entry(102)],
        Some(target_handler),
        Vec::new(),
        test_config(),
    )
    .await;
    install_connectable_peer(&source, &target);
    *source_state.device_network.lock().await = Some(source.runtime.clone());

    let chat_id = "00000000-0000-4000-8000-000000000429";
    for (message, expected_count) in [("first", 1_usize), ("second", 2_usize)] {
        let outcome = source_state
            .conversation_input_intake
            .submit_project_delivery(
                &created.project.project_id,
                "default",
                ConsoleChatRequest {
                    message: message.to_string(),
                    chat_id: Some(chat_id.to_string()),
                    model: None,
                    is_reconnect: false,
                    attachments: Vec::new(),
                    input: None,
                    messages: None,
                    extra: serde_json::Map::new(),
                    channel_kind: Some(rua_channel::core::renderer::ChannelKind::Chat),
                },
            )
            .await;
        if !matches!(
            outcome,
            rua_server::services::conversation_input_intake::ConversationInputDeliveryOutcome::Delivered {
                transport: rua_server::services::conversation_input_intake::ConversationInputDeliveryTransport::RemoteAuthority,
                accepted: true,
                ..
            }
        ) {
            panic!("Project Turn delivery failed: {outcome:?}");
        }

        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let request_count = captured.requests.lock().await.len();
                let idle = target_state
                    .conversation_authority
                    .active_turn(&ChatAddress::project(
                        created.project.project_id.clone(),
                        chat_id,
                    ))
                    .await
                    .is_none();
                if request_count >= expected_count && idle {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("resource Project Turn did not finish");

        if expected_count == 1 {
            let target_scope = rua_core::ChatScopeStore::resolve(
                &target_state.rua_root,
                ChatAddress::project(created.project.project_id.clone(), chat_id),
            );
            let synced_chat = rua_server::services::chat_index::get(target_scope.root(), chat_id)
                .await
                .expect("read resource Chat Index")
                .expect("resource Chat Index entry");
            let source_scope = rua_core::ChatScopeStore::resolve(
                &source_state.rua_root,
                ChatAddress::project(created.project.project_id.clone(), chat_id),
            );
            rua_server::services::chat_index::upsert(source_scope.root(), synced_chat)
                .await
                .expect("simulate synchronized Project Chat metadata");
            std::fs::write(target_repo.join("AGENTS.md"), SECOND_MARKER.as_bytes())
                .expect("updated Project Instructions");
        }
    }

    let requests = captured.requests.lock().await.clone();
    assert_eq!(requests.len(), 2);
    for (request, expected_marker, rejected_marker) in [
        (&requests[0], FIRST_MARKER, SECOND_MARKER),
        (&requests[1], SECOND_MARKER, FIRST_MARKER),
    ] {
        assert_eq!(request.working_dir.as_deref(), Some(target_repo.as_path()));
        assert_eq!(
            request
                .metadata
                .get(rua_agent::hooks::PROJECT_SKILLS_ROOT_METADATA_KEY)
                .and_then(serde_json::Value::as_str),
            Some(target_repo.to_string_lossy().as_ref())
        );
        assert_eq!(
            request.conversation_store_dir.as_deref(),
            Some(
                target_state
                    .rua_root
                    .join("projects")
                    .join(created.project.project_id.as_str())
                    .as_path()
            )
        );
        let instructions = request
            .metadata
            .get(rua_agent::hooks::PROJECT_INSTRUCTIONS_METADATA_KEY)
            .and_then(serde_json::Value::as_str)
            .expect("Project Instructions runtime metadata");
        assert!(instructions.contains(expected_marker));
        assert!(!instructions.contains(rejected_marker));
        assert!(!instructions.contains(CONTROLLER_MARKER));
    }

    let descriptor = serde_json::to_string(
        &target_state
            .project_catalog
            .get(&created.project.project_id)
            .expect("read Project descriptor")
            .expect("Project descriptor"),
    )
    .expect("serialize Project descriptor");
    assert!(!descriptor.contains(FIRST_MARKER));
    assert!(!descriptor.contains(SECOND_MARKER));
    for entry in walkdir::WalkDir::new(&target_state.rua_root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
    {
        let bytes = std::fs::read(entry.path()).expect("read Rua-owned data file");
        assert!(
            !bytes
                .windows(FIRST_MARKER.len())
                .any(|window| window == FIRST_MARKER.as_bytes())
                && !bytes
                    .windows(SECOND_MARKER.len())
                    .any(|window| window == SECOND_MARKER.as_bytes())
                && !bytes
                    .windows(PROJECT_SKILL_MARKER.len())
                    .any(|window| window == PROJECT_SKILL_MARKER.as_bytes()),
            "Project repository context leaked into Rua-owned storage: {}",
            entry.path().display()
        );
    }
    assert!(!target_repo.join("CONTEXT.md").exists());
    assert!(!target_repo.join("docs/adr").exists());

    source.runtime.shutdown().await.expect("source shutdown");
    target.runtime.shutdown().await.expect("target shutdown");
}

fn full_command_handler(state: &AppState, target_device_id: &str) -> Arc<DeviceCommandHandler> {
    Arc::new(
        command_handler(state, target_device_id).with_project_runtime(
            Arc::clone(&state.project_catalog),
            Arc::clone(&state.project_bindings),
            rua_server::sync_publication::SyncPublicationRuntime::from_app_state(state),
        ),
    )
}

fn install_test_device(state: &AppState, seed: u8) {
    let identity = serde_json::from_value::<rua_sync::identity::DeviceIdentity>(
        serde_json::json!({"secret_bytes":vec![seed;32]}),
    )
    .unwrap();
    *state.sync_device.write().unwrap() = Some(Arc::new(identity));
}

struct TestAcpController {
    manager: Arc<AcpChatSessionManager>,
    workspace_dir: std::path::PathBuf,
}

#[async_trait]
impl AcpChatController for TestAcpController {
    async fn enter(&self, chat_id: &str, runner: &str, cwd: Option<&str>) -> String {
        self.manager
            .register_with_runner_session(
                chat_id.to_string(),
                runner.to_string(),
                Some("test-runner-session".to_string()),
                cwd.map(std::path::PathBuf::from)
                    .unwrap_or_else(|| self.workspace_dir.clone()),
            )
            .await;
        "entered".to_string()
    }

    async fn exit(&self, chat_id: &str) -> String {
        self.manager.unregister(chat_id).await;
        "exited".to_string()
    }

    async fn list_sessions(&self, _chat_id: &str, _runner: &str, _cwd_arg: Option<&str>) -> String {
        "[]".to_string()
    }

    async fn resume(
        &self,
        chat_id: &str,
        runner: &str,
        _acp_session_id: &str,
        cwd: Option<&str>,
    ) -> String {
        self.enter(chat_id, runner, cwd).await
    }

    async fn resume_with_history_preview(
        &self,
        chat_id: &str,
        runner: &str,
        acp_session_id: &str,
        cwd: Option<&str>,
    ) -> AcpChatResumeResult {
        AcpChatResumeResult {
            message: self.resume(chat_id, runner, acp_session_id, cwd).await,
            history_preview: Some("history preview".to_string()),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repeated_control_view_reads_reuse_one_device_session_generation() {
    let state_dir = TempDir::new().expect("state tempdir");
    let state = AppState::from_tempdir(state_dir.path().to_path_buf(), ServerProfile::Test);
    let mut available_model = fixture_provider_model("available");
    available_model.thinking_param_style = Some("effort".to_string());
    available_model.reasoning_effort_options =
        vec!["low".to_string(), "medium".to_string(), "high".to_string()];
    seed_fixture_provider(&state, vec![available_model]).await;
    let network_secret = [21; 32];
    install_test_device(&state, 22);
    let target_device_id = hex::encode(
        DeviceNetworkIdentity::new(iroh::SecretKey::from_bytes(&[22; 32])).local_device_id(),
    );
    let chat_id = "00000000-0000-0000-0000-000000009906";
    let workspace_dir = state.rua_root.join("workspaces").join("default");
    seed_authority_placement(&workspace_dir, chat_id, &target_device_id).await;
    seed_chat_header(&workspace_dir, chat_id, &target_device_id).await;
    let key = StreamKey::new(
        CONSOLE_CHANNEL_ID,
        ChatAddress::agent("default", chat_id),
        Priority::Normal,
    );
    let (_producer_id, _cancel_rx) = state.streams.register_active_keyed(&key).await;
    let active_turn = state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent("default".to_string(), chat_id.to_string()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: target_device_id.clone(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: None,
        })
        .await;
    let queued = state
        .conversation_authority
        .enqueue_input(
            &ChatAddress::agent("default", chat_id),
            ConversationInputPayload {
                message: "queued on target".to_string(),
                model_override: None,
                input: None,
                messages: None,
                extra: serde_json::Map::new(),
                attachments: Vec::new(),
                enqueued_at: std::time::Instant::now(),
            },
            Some("source-device".to_string()),
        )
        .await
        .expect("target queue input");
    let target_handler = full_command_handler(&state, &target_device_id);
    let source = build_runtime(
        23,
        network_secret,
        vec![peer_entry(22)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let target = build_runtime(
        22,
        network_secret,
        vec![peer_entry(23)],
        Some(target_handler),
        Vec::new(),
        test_config(),
    )
    .await;
    install_connectable_peer(&source, &target);
    assert_eq!(target.device_id, target_device_id);

    let store = rua_session::WorkspaceSessionStore::new(&workspace_dir);
    store
        .set_collaboration_wake_suppressed(chat_id, true)
        .await
        .unwrap();
    let note = rua_session::RuntimeNote {
        runtime_note_id: "remote-background-result".into(),
        created_at: chrono::Utc::now().to_rfc3339(),
        payload: rua_session::RuntimeNotePayload::TaskAttemptTerminal(
            rua_session::TaskAttemptTerminalRuntimeNote {
                task_id: "remote-task".into(),
                attempt_no: 1,
                terminal_status: "completed".into(),
                output: "result".into(),
                runner_name: "fixture".into(),
                tool_call_id: None,
                full_result_ref: None,
                artifacts: serde_json::json!([]),
                device: serde_json::json!({}),
            },
        ),
    };
    let delivery = deliver_conversation_control_via_session(
        source.runtime.session_manager(),
        &ConversationControlDeviceCommand {
            command_id: "stopped-background-result".into(),
            agent_id: "default".into(),
            address: ChatAddress::agent("default", chat_id),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            authority_epoch: active_turn.authority_epoch,
            action: ConversationControlDeviceAction::DeliverBackgroundResult {
                note: serde_json::to_value(note).unwrap(),
            },
        },
        Duration::from_secs(5),
    )
    .await;
    assert!(
        delivery.is_err(),
        "remote result must not bypass source Stop"
    );
    assert_eq!(
        state
            .conversation_authority
            .active_turn(&ChatAddress::agent("default", chat_id))
            .await
            .unwrap()
            .turn_id,
        active_turn.turn_id
    );
    store
        .set_collaboration_wake_suppressed(chat_id, false)
        .await
        .unwrap();

    for command_id in ["control-view-1", "control-view-2"] {
        let response = deliver_conversation_control_via_session(
            source.runtime.session_manager(),
            &ConversationControlDeviceCommand {
                command_id: command_id.to_string(),
                agent_id: "default".to_string(),
            address: rua_core::ChatAddress::agent("default".to_string(), chat_id.to_string()),
                source_device_id: source.device_id.clone(),
                target_device_id: target.device_id.clone(),
                authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
                action: ConversationControlDeviceAction::View,
            },
            Duration::from_secs(5),
        )
        .await
        .expect("deliver Control View");
        let ConversationControlDeviceResponse::View { control, .. } = response else {
            panic!("unexpected Control View response");
        };
        assert!(control.active_turn.is_some());
        assert_eq!(control.queue.entries.len(), 1);
        assert_eq!(control.queue.entries[0].input_id, queued.input_id);
    }

    let edited = deliver_conversation_control_via_session(
        source.runtime.session_manager(),
        &ConversationControlDeviceCommand {
            command_id: "control-edit-1".to_string(),
            agent_id: "default".to_string(),
            address: rua_core::ChatAddress::agent("default".to_string(), chat_id.to_string()),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            action: ConversationControlDeviceAction::EditQueuedInput {
                input_id: queued.input_id.clone(),
                message: "edited on target".to_string(),
            },
        },
        Duration::from_secs(5),
    )
    .await
    .expect("edit target queue");
    assert!(matches!(
        edited,
        ConversationControlDeviceResponse::Edited { ref entry, .. }
            if entry.text == "edited on target"
    ));
    let delete_command = ConversationControlDeviceCommand {
        command_id: "control-delete-1".to_string(),
        agent_id: "default".to_string(),
            address: rua_core::ChatAddress::agent("default".to_string(), chat_id.to_string()),
        source_device_id: source.device_id.clone(),
        target_device_id: target.device_id.clone(),
        authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
        action: ConversationControlDeviceAction::DeleteQueuedInput {
            input_id: queued.input_id.clone(),
        },
    };
    let deleted = deliver_conversation_control_via_session(
        source.runtime.session_manager(),
        &delete_command,
        Duration::from_secs(5),
    )
    .await
    .expect("delete target queue entry");
    let duplicate_delete = deliver_conversation_control_via_session(
        source.runtime.session_manager(),
        &delete_command,
        Duration::from_secs(5),
    )
    .await
    .expect("duplicate delete returns cached result");
    assert_eq!(duplicate_delete, deleted);

    let now = chrono::Utc::now();
    let remote_input = RemoteConversationInputRequest {
        address: rua_core::ChatAddress::agent("default".to_string(), chat_id.to_string()),
        agent_id: "default".to_string(),
        source_device_id: source.device_id.clone(),
        target_device_id: target.device_id.clone(),
        authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
        base_manifest_hash:
            "sha256:1111111111111111111111111111111111111111111111111111111111111111".to_string(),
        submission_id: "session-submission-1".to_string(),
        device_delivery_id: "session-delivery-1".to_string(),
        device_delivery_sequence: 1,
        text: "queued through Device Session".to_string(),
        input: None,
        media_requirements: Vec::new(),
        created_at: now.to_rfc3339(),
        delivery_deadline_at: (now + chrono::Duration::seconds(30)).to_rfc3339(),
        metadata: serde_json::Map::new(),
    };
    let first_ack = deliver_remote_conversation_input_via_session(
        source.runtime.session_manager(),
        &remote_input,
        Duration::from_secs(5),
    )
    .await
    .expect("deliver remote input");
    let duplicate_ack = deliver_remote_conversation_input_via_session(
        source.runtime.session_manager(),
        &remote_input,
        Duration::from_secs(5),
    )
    .await
    .expect("retry remote input");
    assert_eq!(duplicate_ack, first_ack);
    let authoritative_control = first_ack
        .control
        .as_ref()
        .expect("remote input ACK carries Authority Control");
    match first_ack.mode {
        ConversationInputAcceptanceMode::Started => {
            assert!(authoritative_control.active_turn.is_some());
        }
        ConversationInputAcceptanceMode::Queued => {
            assert!(authoritative_control
                .queue
                .entries
                .iter()
                .any(|entry| entry.input_id == first_ack.input_id));
        }
    }
    let send_now_command = ConversationControlDeviceCommand {
        command_id: "control-send-now-1".to_string(),
        agent_id: "default".to_string(),
            address: rua_core::ChatAddress::agent("default".to_string(), chat_id.to_string()),
        source_device_id: source.device_id.clone(),
        target_device_id: target.device_id.clone(),
        authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
        action: ConversationControlDeviceAction::SendNow {
            input_id: first_ack.input_id.clone(),
            control_command_id: "send-now-idempotency-1".to_string(),
        },
    };
    let send_now = deliver_conversation_control_via_session(
        source.runtime.session_manager(),
        &send_now_command,
        Duration::from_secs(5),
    )
    .await
    .expect("send queued input now");
    let duplicate_send_now = deliver_conversation_control_via_session(
        source.runtime.session_manager(),
        &send_now_command,
        Duration::from_secs(5),
    )
    .await
    .expect("duplicate Send Now returns cached result");
    assert_eq!(duplicate_send_now, send_now);
    assert!(matches!(
        send_now,
        ConversationControlDeviceResponse::SendNow { applied: true, .. }
    ));

    let model = serde_json::json!({
        "provider_id": "fixture",
        "model": "available",
        "thinking_strength": "high",
        "thinking_param_style": "effort",
        "reasoning_effort_options": ["low", "medium", "high"]
    });
    let model_response = deliver_conversation_control_via_session(
        source.runtime.session_manager(),
        &ConversationControlDeviceCommand {
            command_id: "control-model-1".to_string(),
            agent_id: "default".to_string(),
            address: rua_core::ChatAddress::agent("default".to_string(), chat_id.to_string()),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            action: ConversationControlDeviceAction::UpdateModel {
                model: model.clone(),
            },
        },
        Duration::from_secs(5),
    )
    .await
    .expect("update target model");
    assert!(matches!(
        model_response,
        ConversationControlDeviceResponse::ModelUpdated {
            model: ref confirmed,
            ..
        } if confirmed == &model
    ));
    let header = WorkspaceSessionStore::new(&workspace_dir)
        .load_chat_header(chat_id)
        .await
        .expect("load target header")
        .expect("target header");
    assert_eq!(header.extra.get("model"), Some(&model));
    let active_after_model_update = state
        .conversation_authority
        .active_turn(&ChatAddress::agent("default", chat_id))
        .await
        .expect("current target Turn remains active");
    assert_eq!(active_after_model_update.turn_id, active_turn.turn_id);
    assert!(
        active_after_model_update.driver_detail.is_none(),
        "model preference must take effect on a later claimed Turn"
    );

    let manifest = deliver_conversation_manifest_command_via_session(
        source.runtime.session_manager(),
        &ConversationManifestDeviceCommand {
            command_id: "manifest-1".to_string(),
            agent_id: "default".to_string(),
            address: rua_core::ChatAddress::agent("default".to_string(), chat_id.to_string()),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            required_manifest: None,
        },
        Duration::from_secs(5),
    )
    .await
    .expect("read target published manifest");
    assert_eq!(
        manifest.manifest, None,
        "disk state without publication evidence must not become a hydration target"
    );
    let address = ChatAddress::agent("default", chat_id);
    let published_manifest = load_conversation_manifest(&workspace_dir, &address)
        .await
        .expect("load published manifest")
        .expect("published manifest exists");
    DurablePublicationWatermarkStore::new(state.rua_root.join("sync"))
        .expect("open watermark store")
        .advance(&address, &published_manifest)
        .await
        .expect("record published watermark");
    WorkspaceSessionStore::new(&workspace_dir)
        .append_conversation_turn(
            chat_id,
            NewConversationTurn {
                turn_id: "unpublished-turn".to_string(),
                lifecycle: TurnLifecycle {
                    user_submitted_at: "2026-08-11T12:00:00Z".to_string(),
                    assistant_started_at: None,
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id: None,
                messages: vec![NewConversationMessage {
                    message_id: "unpublished-message".to_string(),
                    role: "user".to_string(),
                    created_at: "2026-08-11T12:00:00Z".to_string(),
                    status: "completed".to_string(),
                    content: serde_json::json!([{"type": "text", "text": "not published"}]),
                    provider: None,
                    tool: None,
                    metadata: serde_json::Map::new(),
                }],
            },
        )
        .await
        .expect("append unpublished disk state");
    let disk_manifest = load_conversation_manifest(&workspace_dir, &address)
        .await
        .expect("load newer disk manifest")
        .expect("newer disk manifest exists");
    assert!(disk_manifest.latest_sequence > published_manifest.latest_sequence);

    let advertised = deliver_conversation_manifest_command_via_session(
        source.runtime.session_manager(),
        &ConversationManifestDeviceCommand {
            command_id: "manifest-2".to_string(),
            agent_id: "default".to_string(),
            address,
            source_device_id: source.device_id.clone(),
            target_device_id: target_device_id.clone(),
            required_manifest: None,
        },
        Duration::from_secs(5),
    )
    .await
    .expect("read target published manifest");
    assert_eq!(advertised.manifest, Some(published_manifest));

    let acp_status = deliver_acp_chat_command_via_session(
        source.runtime.session_manager(),
        &AcpChatDeviceCommand {
            command_id: "acp-status-1".to_string(),
            agent_id: "default".to_string(),
            address: rua_core::ChatAddress::agent("default".to_string(), chat_id.to_string()),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            action: AcpChatDeviceAction::Status,
        },
        Duration::from_secs(5),
    )
    .await
    .expect("read target ACP status");
    assert!(matches!(acp_status, AcpChatDeviceResponse::Status { .. }));
    let permission_snapshot = deliver_acp_chat_command_via_session(
        source.runtime.session_manager(),
        &AcpChatDeviceCommand {
            command_id: "acp-permissions-1".to_string(),
            agent_id: "default".to_string(),
            address: rua_core::ChatAddress::agent("default".to_string(), chat_id.to_string()),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            action: AcpChatDeviceAction::PermissionSnapshot,
        },
        Duration::from_secs(5),
    )
    .await
    .expect("read target ACP permissions");
    assert!(matches!(
        permission_snapshot,
        AcpChatDeviceResponse::PermissionSnapshot { ref snapshot }
            if snapshot.pending.is_empty()
    ));
    let permission_error = deliver_acp_chat_command_via_session(
        source.runtime.session_manager(),
        &AcpChatDeviceCommand {
            command_id: "acp-permission-decide-1".to_string(),
            agent_id: "default".to_string(),
            address: rua_core::ChatAddress::agent("default".to_string(), chat_id.to_string()),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            action: AcpChatDeviceAction::DecidePermission {
                request_id: "missing-request".to_string(),
                option_id: "allow_once".to_string(),
            },
        },
        Duration::from_secs(5),
    )
    .await
    .expect_err("missing target permission");
    assert!(permission_error.to_string().contains("not_found"));

    let stop = deliver_remote_chat_control_via_session(
        source.runtime.session_manager(),
        &RemoteChatControlCommand {
            command_id: "stop-without-wait-1".to_string(),
            agent_id: "default".to_string(),
            address: rua_core::ChatAddress::agent("default".to_string(), chat_id.to_string()),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            device_delivery_id: None,
            await_terminal: false,
            action: RemoteChatControlAction::StopCurrentTurn,
        },
        Duration::from_secs(5),
    )
    .await
    .expect("stop target turn");
    assert!(stop.applied);
    assert_eq!(
        state
            .conversation_authority
            .control_view(
                &ChatAddress::agent("default", chat_id),
                &serde_json::json!({})
            )
            .await
            .queue
            .entries
            .len(),
        1,
        "duplicate input must not create a second queue entry"
    );

    let diagnostics = source.runtime.session_manager().diagnostics();
    assert_eq!(diagnostics.physical_dial_attempts, 1);
    assert_eq!(diagnostics.sessions.len(), 1);
    assert!(diagnostics.sessions[0].generation.is_some());
    assert_eq!(
        diagnostics.sessions[0].state,
        rua_device_network::DeviceSessionState::Connected,
        "feature-level rejection must not become Device offline"
    );
    assert_eq!(
        target
            .runtime
            .session_manager()
            .diagnostics()
            .accepted_connections,
        1
    );

    source.runtime.shutdown().await.expect("shutdown source");
    target.runtime.shutdown().await.expect("shutdown target");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_acp_lifecycle_persists_binding_on_authority_device() {
    let target_state_dir = TempDir::new().expect("target state tempdir");
    let target_state =
        AppState::from_tempdir(target_state_dir.path().to_path_buf(), ServerProfile::Test);
    let target_runtime = target_state
        .registry
        .get_or_create("default")
        .await
        .expect("target workspace runtime");
    let target_workspace = target_runtime.workspace_dir();
    let existing_handle = target_state
        .acp_chat_registry
        .get("default")
        .expect("target ACP runtime handle");
    let manager = Arc::new(AcpChatSessionManager::new(Duration::from_secs(300)));
    target_state.acp_chat_registry.register(
        "default".to_string(),
        Arc::new(AcpChatRuntimeHandle {
            service: Arc::clone(&existing_handle.service),
            manager: Arc::clone(&manager),
            controller: Arc::new(TestAcpController {
                manager: Arc::clone(&manager),
                workspace_dir: target_workspace.clone(),
            }),
            workspace_dir: target_workspace.clone(),
        }),
    );

    let network_secret = [24; 32];
    let target_device_id = hex::encode(
        DeviceNetworkIdentity::new(iroh::SecretKey::from_bytes(&[26; 32])).local_device_id(),
    );
    let chat_id = "00000000-0000-0000-0000-000000009907";
    seed_authority_placement(&target_workspace, chat_id, &target_device_id).await;
    seed_chat_header(&target_workspace, chat_id, &target_device_id).await;

    let source = build_runtime(
        25,
        network_secret,
        vec![peer_entry(26)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let target = build_runtime(
        26,
        network_secret,
        vec![peer_entry(25)],
        Some(full_command_handler(&target_state, &target_device_id)),
        Vec::new(),
        test_config(),
    )
    .await;
    install_connectable_peer(&source, &target);

    let entered = deliver_acp_chat_command_via_session(
        source.runtime.session_manager(),
        &AcpChatDeviceCommand {
            command_id: "remote-acp-enter".to_string(),
            agent_id: "default".to_string(),
            address: rua_core::ChatAddress::agent("default".to_string(), chat_id.to_string()),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            authority_epoch: epoch(1),
            action: AcpChatDeviceAction::Enter {
                runner: "codex".to_string(),
                cwd: Some("/tmp".to_string()),
                resume_session_id: None,
            },
        },
        Duration::from_secs(5),
    )
    .await
    .expect("remote ACP enter");
    assert!(matches!(
        entered,
        AcpChatDeviceResponse::Entered {
            ok: true,
            ref status,
            ..
        } if status.target_device_id == target_device_id
    ));
    let entered_chat = rua_server::services::chat_index::get(&target_workspace, chat_id)
        .await
        .expect("read target Chat Index")
        .expect("target Chat Index row");
    assert_eq!(
        entered_chat.meta["acp_binding"]["device_id"],
        target_device_id
    );
    assert_eq!(entered_chat.meta["acp_binding"]["runner"], "codex");
    assert_eq!(
        entered_chat.meta["acp_binding"]["runner_session_id"],
        "test-runner-session"
    );

    let exited = deliver_acp_chat_command_via_session(
        source.runtime.session_manager(),
        &AcpChatDeviceCommand {
            command_id: "remote-acp-exit".to_string(),
            agent_id: "default".to_string(),
            address: rua_core::ChatAddress::agent("default".to_string(), chat_id.to_string()),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            authority_epoch: epoch(1),
            action: AcpChatDeviceAction::Exit,
        },
        Duration::from_secs(5),
    )
    .await
    .expect("remote ACP exit");
    assert!(matches!(
        exited,
        AcpChatDeviceResponse::Exited { ok: true, .. }
    ));
    assert!(manager.get(chat_id).await.is_none());
    let exited_chat = rua_server::services::chat_index::get(&target_workspace, chat_id)
        .await
        .expect("read target Chat Index")
        .expect("target Chat Index row");
    assert!(exited_chat.meta.get("acp_binding").is_none());

    source.runtime.shutdown().await.expect("shutdown source");
    target.runtime.shutdown().await.expect("shutdown target");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_live_sends_initial_control_before_replay() {
    let source_state_dir = TempDir::new().expect("source state tempdir");
    let source_state =
        AppState::from_tempdir(source_state_dir.path().to_path_buf(), ServerProfile::Test);
    let target_state_dir = TempDir::new().expect("target state tempdir");
    let target_state =
        AppState::from_tempdir(target_state_dir.path().to_path_buf(), ServerProfile::Test);
    let network_secret = [52; 32];
    let target_device_id = hex::encode(
        DeviceNetworkIdentity::new(iroh::SecretKey::from_bytes(&[54; 32])).local_device_id(),
    );
    let chat_id = "00000000-0000-0000-0000-000000009919";
    let source_workspace = source_state.rua_root.join("workspaces").join("default");
    let target_workspace = target_state.rua_root.join("workspaces").join("default");
    seed_authority_placement(&source_workspace, chat_id, &target_device_id).await;
    seed_authority_placement(&target_workspace, chat_id, &target_device_id).await;

    let live_hub = Arc::new(ActiveStreamBroadcastHub::new());
    let target_handler = full_command_handler(&target_state, &target_device_id);
    let source = build_runtime(
        53,
        network_secret,
        vec![peer_entry(54)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let target = build_runtime(
        54,
        network_secret,
        vec![peer_entry(53)],
        Some(target_handler),
        Vec::new(),
        test_config(),
    )
    .await;
    install_connectable_peer(&source, &target);
    assert_eq!(target.device_id, target_device_id);
    target
        .runtime
        .session_manager()
        .set_live_activity_handler(Arc::new(
            ActiveStreamBroadcastDeviceSessionHandler::new(Arc::clone(&live_hub))
                .with_conversation_control_initial_state(
                    Arc::clone(&target_state.conversation_authority),
                    target_state.conversation_input_intake.clone(),
                    target_device_id.clone(),
                ),
        ));
    live_hub
        .publish_conversation_event(
            "remote-live-stream",
            &ChatAddress::agent("default", chat_id),
            epoch(1),
            serde_json::json!({"type": "text_delta", "delta": "already produced"}).to_string(),
        )
        .await;
    *source_state.device_network.lock().await = Some(source.runtime.clone());

    let activity = ConversationLiveActivity::new(
        Arc::clone(&source_state.streams),
        Arc::clone(&source_state.registry),
        Arc::clone(&source_state.sync_device),
        Arc::clone(&source_state.sync_node),
        Arc::clone(&source_state.device_network),
        source_state.rua_root.clone(),
    )
    .with_conversation_authority(Arc::clone(&source_state.conversation_authority));
    let mut viewer = activity
        .subscribe_viewer(
            &ChatAddress::agent("default", chat_id),
            "default",
            "viewer-a",
            None,
        )
        .await;

    let first = tokio::time::timeout(Duration::from_secs(5), viewer.next())
        .await
        .expect("initial remote control timeout")
        .expect("remote viewer remains open")
        .expect("live payload is infallible");
    let first: serde_json::Value =
        serde_json::from_str(&first).expect("initial remote control JSON");
    assert_eq!(first["type"], "conversation_control");
    assert_eq!(first["reason"], "viewer_attached");
    assert_eq!(
        first["control"]["placement"]["authority_device_id"],
        target_device_id
    );

    let replay = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let payload = viewer
                .next()
                .await
                .expect("remote viewer remains open")
                .expect("live payload is infallible");
            let payload: serde_json::Value =
                serde_json::from_str(&payload).expect("remote live JSON");
            if payload["type"] == "stream_replay" {
                break payload;
            }
        }
    })
    .await
    .expect("remote replay timeout");
    assert_eq!(replay["complete"], true);
    assert!(
        replay["events"]
            .as_array()
            .is_some_and(|events| events.iter().any(|event| {
                event["type"] == "text_delta" && event["delta"] == "already produced"
            })),
        "remote replay must follow the initial Control View"
    );

    drop(viewer);
    source.runtime.shutdown().await.expect("shutdown source");
    target.runtime.shutdown().await.expect("shutdown target");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_chat_live_recovers_when_another_chat_connects_the_shared_session() {
    let source_state_dir = TempDir::new().expect("source state tempdir");
    let source_state =
        AppState::from_tempdir(source_state_dir.path().to_path_buf(), ServerProfile::Test);
    let target_state_dir = TempDir::new().expect("target state tempdir");
    let target_state =
        AppState::from_tempdir(target_state_dir.path().to_path_buf(), ServerProfile::Test);
    let network_secret = [58; 32];
    let target_device_id = hex::encode(
        DeviceNetworkIdentity::new(iroh::SecretKey::from_bytes(&[60; 32])).local_device_id(),
    );
    let chat_a = "00000000-0000-0000-0000-000000009921";
    let chat_b = "00000000-0000-0000-0000-000000009922";
    let source_workspace = source_state.rua_root.join("workspaces").join("default");
    let target_workspace = target_state.rua_root.join("workspaces").join("default");
    for chat_id in [chat_a, chat_b] {
        seed_authority_placement(&source_workspace, chat_id, &target_device_id).await;
        seed_authority_placement(&target_workspace, chat_id, &target_device_id).await;
    }

    let live_hub = Arc::new(ActiveStreamBroadcastHub::new());
    let target_handler = full_command_handler(&target_state, &target_device_id);
    let source = build_runtime(
        59,
        network_secret,
        vec![peer_entry(60)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let target = build_runtime(
        60,
        network_secret,
        vec![peer_entry(59)],
        Some(target_handler),
        Vec::new(),
        test_config(),
    )
    .await;
    target
        .runtime
        .session_manager()
        .set_live_activity_handler(Arc::new(
            ActiveStreamBroadcastDeviceSessionHandler::new(Arc::clone(&live_hub))
                .with_conversation_control_initial_state(
                    Arc::clone(&target_state.conversation_authority),
                    target_state.conversation_input_intake.clone(),
                    target_device_id.clone(),
                ),
        ));
    *source_state.device_network.lock().await = Some(source.runtime.clone());

    let activity = ConversationLiveActivity::new(
        Arc::clone(&source_state.streams),
        Arc::clone(&source_state.registry),
        Arc::clone(&source_state.sync_device),
        Arc::clone(&source_state.sync_node),
        Arc::clone(&source_state.device_network),
        source_state.rua_root.clone(),
    )
    .with_conversation_authority(Arc::clone(&source_state.conversation_authority));
    let mut viewer_a = activity
        .subscribe_viewer(
            &ChatAddress::agent("default", chat_a),
            "default",
            "viewer-a",
            None,
        )
        .await;

    let unavailable = tokio::time::timeout(Duration::from_secs(5), viewer_a.next())
        .await
        .expect("Chat A attachment failure timeout")
        .expect("Chat A viewer remains open")
        .expect("live payload is infallible");
    let unavailable: serde_json::Value =
        serde_json::from_str(&unavailable).expect("Chat A attachment error JSON");
    assert_eq!(unavailable["type"], "error");
    let attempts_after_failure = source
        .runtime
        .session_manager()
        .diagnostics()
        .physical_dial_attempts;
    assert!(attempts_after_failure > 0);

    install_connectable_peer(&source, &target);
    let mut viewer_b = activity
        .subscribe_viewer(
            &ChatAddress::agent("default", chat_b),
            "default",
            "viewer-b",
            None,
        )
        .await;
    let control_b = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let payload = viewer_b
                .next()
                .await
                .expect("Chat B viewer remains open")
                .expect("live payload is infallible");
            let payload: serde_json::Value =
                serde_json::from_str(&payload).expect("Chat B live JSON");
            if payload["type"] == "conversation_control" {
                break payload;
            }
        }
    })
    .await
    .expect("Chat B initial Control View timeout");
    assert_eq!(
        control_b["control"]["placement"]["authority_device_id"],
        target_device_id
    );
    let attempts_after_chat_b = source
        .runtime
        .session_manager()
        .diagnostics()
        .physical_dial_attempts;
    assert!(
        attempts_after_chat_b > attempts_after_failure,
        "Chat B must establish the previously unavailable Session"
    );

    let control_a = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let payload = viewer_a
                .next()
                .await
                .expect("Chat A viewer remains open")
                .expect("live payload is infallible");
            let payload: serde_json::Value =
                serde_json::from_str(&payload).expect("Chat A recovery JSON");
            if payload["type"] == "conversation_control" {
                break payload;
            }
        }
    })
    .await
    .expect("Chat A must recover from the shared Session event");
    assert_eq!(
        control_a["control"]["placement"]["authority_device_id"],
        target_device_id
    );

    let source_diagnostics = source.runtime.session_manager().diagnostics();
    assert_eq!(
        source_diagnostics.physical_dial_attempts, attempts_after_chat_b,
        "Chat A recovery must reuse the Session established by Chat B"
    );
    assert_eq!(source_diagnostics.sessions.len(), 1);
    assert_eq!(
        target
            .runtime
            .session_manager()
            .diagnostics()
            .accepted_connections,
        1
    );

    drop(viewer_a);
    drop(viewer_b);
    source.runtime.shutdown().await.expect("shutdown source");
    target.runtime.shutdown().await.expect("shutdown target");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn synced_placement_wake_reconnects_remote_live_without_retry_timer() {
    let source_state_dir = TempDir::new().expect("source state tempdir");
    let source_state =
        AppState::from_tempdir(source_state_dir.path().to_path_buf(), ServerProfile::Test);
    let target_state_dir = TempDir::new().expect("target state tempdir");
    let target_state =
        AppState::from_tempdir(target_state_dir.path().to_path_buf(), ServerProfile::Test);
    let wake_hub = ChatActivityWakeHub::new();
    let live_hub = Arc::new(ActiveStreamBroadcastHub::new());
    let network_secret = [61; 32];
    let old_authority_device_id = "cccccccccccccccc";
    let target_device_id = hex::encode(
        DeviceNetworkIdentity::new(iroh::SecretKey::from_bytes(&[63; 32])).local_device_id(),
    );
    let chat_id = "00000000-0000-0000-0000-000000009924";
    let source_workspace = source_state.rua_root.join("workspaces").join("default");
    let target_workspace = target_state.rua_root.join("workspaces").join("default");
    seed_authority_placement(&source_workspace, chat_id, old_authority_device_id).await;
    seed_authority_placement_at_epoch(&target_workspace, chat_id, &target_device_id, epoch(2))
        .await;

    let target_handler = full_command_handler(&target_state, &target_device_id);
    let source = build_runtime(
        62,
        network_secret,
        vec![peer_entry(63)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let target = build_runtime(
        63,
        network_secret,
        vec![peer_entry(62)],
        Some(target_handler),
        Vec::new(),
        test_config(),
    )
    .await;
    install_connectable_peer(&source, &target);
    target
        .runtime
        .session_manager()
        .set_live_activity_handler(Arc::new(
            ActiveStreamBroadcastDeviceSessionHandler::new(Arc::clone(&live_hub))
                .with_conversation_control_initial_state(
                    Arc::clone(&target_state.conversation_authority),
                    target_state.conversation_input_intake.clone(),
                    target_device_id.clone(),
                ),
        ));
    live_hub
        .publish_conversation_event(
            "placement-recovery-stream",
            &ChatAddress::agent("default", chat_id),
            epoch(2),
            serde_json::json!({"type": "text_delta", "delta": "new authority output"}).to_string(),
        )
        .await;
    *source_state.device_network.lock().await = Some(source.runtime.clone());

    let activity = ConversationLiveActivity::new(
        Arc::clone(&source_state.streams),
        Arc::clone(&source_state.registry),
        Arc::clone(&source_state.sync_device),
        Arc::clone(&source_state.sync_node),
        Arc::clone(&source_state.device_network),
        source_state.rua_root.clone(),
    )
    .with_conversation_authority(Arc::clone(&source_state.conversation_authority))
    .with_chat_activity_wake_hub(Arc::clone(&wake_hub));
    let mut viewer = activity
        .subscribe_viewer(
            &ChatAddress::agent("default", chat_id),
            "default",
            "viewer-a",
            None,
        )
        .await;

    let unavailable = tokio::time::timeout(Duration::from_secs(2), viewer.next())
        .await
        .expect("stale placement failure timeout")
        .expect("viewer remains open")
        .expect("live payload is infallible");
    let unavailable: serde_json::Value =
        serde_json::from_str(&unavailable).expect("stale placement error JSON");
    assert_eq!(unavailable["type"], "error");

    seed_authority_placement_at_epoch(&source_workspace, chat_id, &target_device_id, epoch(2))
        .await;
    wake_hub.publish(ChatLiveWake::placement_changed(
        ChatAddress::agent("default", chat_id),
        target_device_id.clone(),
        epoch(2),
        format!("sha256:{}", "0".repeat(64)),
    ));

    let control = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let payload = viewer
                .next()
                .await
                .expect("viewer remains open")
                .expect("live payload is infallible");
            let payload: serde_json::Value =
                serde_json::from_str(&payload).expect("placement recovery JSON");
            if payload["type"] == "conversation_control" {
                break payload;
            }
        }
    })
    .await
    .expect("placement wake must not wait for a retry timer");
    assert_eq!(
        control["control"]["placement"]["authority_device_id"],
        target_device_id
    );
    assert_eq!(
        control["control"]["placement"]["authority_epoch"],
        serde_json::json!(epoch(2))
    );

    let replay = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let payload = viewer
                .next()
                .await
                .expect("viewer remains open")
                .expect("live payload is infallible");
            let payload: serde_json::Value =
                serde_json::from_str(&payload).expect("placement recovery replay JSON");
            if payload["type"] == "stream_replay" {
                break payload;
            }
        }
    })
    .await
    .expect("new authority replay timeout");
    assert!(replay["events"]
        .as_array()
        .is_some_and(|events| events.iter().any(|event| {
            event["type"] == "text_delta" && event["delta"] == "new authority output"
        })));
    assert_eq!(
        source
            .runtime
            .session_manager()
            .diagnostics()
            .physical_dial_attempts,
        1,
        "stale placement recovery must not probe every paired Device"
    );

    drop(viewer);
    source.runtime.shutdown().await.expect("shutdown source");
    target.runtime.shutdown().await.expect("shutdown target");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn higher_epoch_wake_interrupts_only_the_old_live_route() {
    let source_state_dir = TempDir::new().expect("source state tempdir");
    let source_state =
        AppState::from_tempdir(source_state_dir.path().to_path_buf(), ServerProfile::Test);
    let old_state_dir = TempDir::new().expect("old authority state tempdir");
    let old_state = AppState::from_tempdir(old_state_dir.path().to_path_buf(), ServerProfile::Test);
    let new_state_dir = TempDir::new().expect("new authority state tempdir");
    let new_state = AppState::from_tempdir(new_state_dir.path().to_path_buf(), ServerProfile::Test);
    let wake_hub = ChatActivityWakeHub::new();
    let old_live_hub = Arc::new(ActiveStreamBroadcastHub::new());
    let new_live_hub = Arc::new(ActiveStreamBroadcastHub::new());
    let network_secret = [64; 32];
    let old_device_id = hex::encode(
        DeviceNetworkIdentity::new(iroh::SecretKey::from_bytes(&[66; 32])).local_device_id(),
    );
    let new_device_id = hex::encode(
        DeviceNetworkIdentity::new(iroh::SecretKey::from_bytes(&[67; 32])).local_device_id(),
    );
    let chat_id = "00000000-0000-0000-0000-000000009925";
    let source_workspace = source_state.rua_root.join("workspaces").join("default");
    let old_workspace = old_state.rua_root.join("workspaces").join("default");
    let new_workspace = new_state.rua_root.join("workspaces").join("default");
    seed_authority_placement_at_epoch(&source_workspace, chat_id, &old_device_id, epoch(1)).await;
    seed_authority_placement_at_epoch(&old_workspace, chat_id, &old_device_id, epoch(1)).await;
    seed_authority_placement_at_epoch(&new_workspace, chat_id, &new_device_id, epoch(2)).await;

    let source = build_runtime(
        65,
        network_secret,
        vec![peer_entry(66), peer_entry(67)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let old = build_runtime(
        66,
        network_secret,
        vec![peer_entry(65)],
        Some(full_command_handler(&old_state, &old_device_id)),
        Vec::new(),
        test_config(),
    )
    .await;
    let new = build_runtime(
        67,
        network_secret,
        vec![peer_entry(65)],
        Some(full_command_handler(&new_state, &new_device_id)),
        Vec::new(),
        test_config(),
    )
    .await;
    install_connectable_peers(&source, &[&old, &new]);
    old.runtime
        .session_manager()
        .set_live_activity_handler(Arc::new(
            ActiveStreamBroadcastDeviceSessionHandler::new(Arc::clone(&old_live_hub))
                .with_conversation_control_initial_state(
                    Arc::clone(&old_state.conversation_authority),
                    old_state.conversation_input_intake.clone(),
                    old_device_id.clone(),
                ),
        ));
    new.runtime
        .session_manager()
        .set_live_activity_handler(Arc::new(
            ActiveStreamBroadcastDeviceSessionHandler::new(Arc::clone(&new_live_hub))
                .with_conversation_control_initial_state(
                    Arc::clone(&new_state.conversation_authority),
                    new_state.conversation_input_intake.clone(),
                    new_device_id.clone(),
                ),
        ));
    old_live_hub
        .publish_conversation_event(
            "old-authority-stream",
            &ChatAddress::agent("default", chat_id),
            epoch(1),
            serde_json::json!({"type": "text_delta", "delta": "old"}).to_string(),
        )
        .await;
    new_live_hub
        .publish_conversation_event(
            "new-authority-stream",
            &ChatAddress::agent("default", chat_id),
            epoch(2),
            serde_json::json!({"type": "text_delta", "delta": "new"}).to_string(),
        )
        .await;
    *source_state.device_network.lock().await = Some(source.runtime.clone());

    let activity = ConversationLiveActivity::new(
        Arc::clone(&source_state.streams),
        Arc::clone(&source_state.registry),
        Arc::clone(&source_state.sync_device),
        Arc::clone(&source_state.sync_node),
        Arc::clone(&source_state.device_network),
        source_state.rua_root.clone(),
    )
    .with_conversation_authority(Arc::clone(&source_state.conversation_authority))
    .with_chat_activity_wake_hub(Arc::clone(&wake_hub));
    let mut viewer = activity
        .subscribe_viewer(
            &ChatAddress::agent("default", chat_id),
            "default",
            "viewer-a",
            None,
        )
        .await;

    let old_control = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let payload = viewer
                .next()
                .await
                .expect("viewer remains open")
                .expect("live payload is infallible");
            let payload: serde_json::Value =
                serde_json::from_str(&payload).expect("old authority live JSON");
            if payload["type"] == "conversation_control"
                && payload["control"]["placement"]["authority_epoch"]
                    == "0000000000000001000000000000000000000000"
            {
                break payload;
            }
        }
    })
    .await
    .expect("old authority control timeout");
    assert_eq!(
        old_control["control"]["placement"]["authority_device_id"],
        old_device_id
    );
    let old_generation = source
        .runtime
        .session_manager()
        .diagnostics()
        .sessions
        .iter()
        .find(|session| session.peer_device_id == old.device_id)
        .and_then(|session| session.generation)
        .expect("old authority session generation");

    seed_authority_placement_at_epoch(&source_workspace, chat_id, &new_device_id, epoch(2)).await;
    wake_hub.publish(ChatLiveWake::placement_changed(
        ChatAddress::agent("default", chat_id),
        new_device_id.clone(),
        epoch(2),
        format!("sha256:{}", "0".repeat(64)),
    ));

    let new_control = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let payload = viewer
                .next()
                .await
                .expect("viewer remains open")
                .expect("live payload is infallible");
            let payload: serde_json::Value =
                serde_json::from_str(&payload).expect("new authority live JSON");
            if payload["type"] == "conversation_control"
                && payload["control"]["placement"]["authority_epoch"]
                    == "0000000000000002000000000000000000000000"
            {
                break payload;
            }
        }
    })
    .await
    .expect("higher Epoch wake must interrupt the old logical route");
    assert_eq!(
        new_control["control"]["placement"]["authority_device_id"],
        new_device_id
    );

    let diagnostics = source.runtime.session_manager().diagnostics();
    assert_eq!(
        diagnostics
            .sessions
            .iter()
            .find(|session| session.peer_device_id == old.device_id)
            .and_then(|session| session.generation),
        Some(old_generation),
        "switching Live authority must not replace the old shared Device Session"
    );
    assert!(
        diagnostics
            .sessions
            .iter()
            .any(|session| session.peer_device_id == new.device_id),
        "new authority must reuse its peer-scoped Device Session"
    );

    drop(viewer);
    source.runtime.shutdown().await.expect("shutdown source");
    old.runtime
        .shutdown()
        .await
        .expect("shutdown old authority");
    new.runtime
        .shutdown()
        .await
        .expect("shutdown new authority");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn target_prepares_history_and_required_media_before_adopting_authority() {
    let source_state_dir = TempDir::new().expect("source state tempdir");
    let source_state =
        AppState::from_tempdir(source_state_dir.path().to_path_buf(), ServerProfile::Test);
    let target_state_dir = TempDir::new().expect("target state tempdir");
    let target_state =
        AppState::from_tempdir(target_state_dir.path().to_path_buf(), ServerProfile::Test);
    seed_fixture_provider(&target_state, vec![fixture_provider_model("available")]).await;

    let network_secret = [55; 32];
    let target_device_id = hex::encode(
        DeviceNetworkIdentity::new(iroh::SecretKey::from_bytes(&[57; 32])).local_device_id(),
    );
    let source_device_id = hex::encode(
        DeviceNetworkIdentity::new(iroh::SecretKey::from_bytes(&[56; 32])).local_device_id(),
    );
    let old_authority_device_id = source_device_id.as_str();
    let chat_id = "00000000-0000-0000-0000-000000009920";
    let empty_chat_id = "00000000-0000-0000-0000-000000009921";
    let source_workspace = source_state.rua_root.join("workspaces").join("default");
    let target_workspace = target_state.rua_root.join("workspaces").join("default");
    seed_adoptable_chat(
        &source_workspace,
        chat_id,
        old_authority_device_id,
        "available",
    )
    .await;
    let required_media_id = "00000000-0000-4000-8000-000000009920";
    let source_chat_dir = source_workspace.join("sessions").join(chat_id);
    std::fs::write(
        source_chat_dir.join("required-media.json"),
        serde_json::to_vec(&serde_json::json!({
            "content": [{"media_ref": format!("media/{required_media_id}")}]
        }))
        .expect("required Media JSON"),
    )
    .expect("write required Media reference");
    let source_media_dir = source_workspace.join("media");
    std::fs::create_dir_all(&source_media_dir).expect("source Media dir");
    std::fs::write(
        source_media_dir.join(required_media_id),
        b"required authority media",
    )
    .expect("write required Media");
    seed_adoptable_empty_chat(
        &target_workspace,
        chat_id,
        old_authority_device_id,
        "available",
    )
    .await;
    seed_adoptable_empty_chat(
        &source_workspace,
        empty_chat_id,
        old_authority_device_id,
        "available",
    )
    .await;
    seed_adoptable_empty_chat(
        &target_workspace,
        empty_chat_id,
        old_authority_device_id,
        "available",
    )
    .await;
    let address = rua_core::ChatAddress::agent("default", chat_id);
    let empty_address = rua_core::ChatAddress::agent("default", empty_chat_id);
    let required_manifest = load_conversation_manifest(&source_workspace, &address)
        .await
        .expect("load source manifest")
        .expect("source manifest");
    let empty_manifest = load_conversation_manifest(&source_workspace, &empty_address)
        .await
        .expect("load empty source manifest")
        .expect("empty source manifest");

    let source_handler = full_command_handler(&source_state, &source_device_id);
    let target_handler = full_command_handler(&target_state, &target_device_id);
    let source = build_runtime(
        56,
        network_secret,
        vec![peer_entry(57)],
        Some(source_handler),
        Vec::new(),
        test_config(),
    )
    .await;
    let target = build_runtime(
        57,
        network_secret,
        vec![peer_entry(56)],
        Some(target_handler),
        Vec::new(),
        test_config(),
    )
    .await;
    install_connectable_peer(&source, &target);
    install_connectable_peer(&target, &source);
    assert_eq!(target.device_id, target_device_id);
    *source_state
        .sync_transfer_session_manager
        .write()
        .expect("source session manager cell") = Some(source.runtime.session_manager().clone());
    *target_state
        .sync_transfer_session_manager
        .write()
        .expect("target session manager cell") = Some(target.runtime.session_manager().clone());
    source
        .runtime
        .session_manager()
        .set_bulk_transfer_handler(Arc::new(
            SyncTransferBulkHandler::new(source.device_id.clone())
                .with_chat_media_root(source_state.rua_root.clone()),
        ));
    target
        .runtime
        .session_manager()
        .set_bulk_transfer_handler(Arc::new(SyncTransferBulkHandler::new(
            target.device_id.clone(),
        )));

    let mut control_events = target_state
        .conversation_authority
        .subscribe_global_control_events();
    let command = authority_adopt_command(
        "authority-adopt-success",
        &source.device_id,
        &target.device_id,
        chat_id,
        required_manifest.clone(),
    );
    target_state.sync_enabled.store(true, Ordering::Relaxed);
    let target_sync_guard = target_state.sync_node.lock().await;
    let response = tokio::time::timeout(
        Duration::from_secs(1),
        deliver_conversation_authority_adopt_via_session(
            source.runtime.session_manager(),
            &command,
            Duration::from_secs(5),
        ),
    )
    .await
    .expect("authority adoption acknowledgement must not await Sync publication")
    .expect("target adopts authority");
    drop(target_sync_guard);
    target_state.sync_enabled.store(false, Ordering::Relaxed);
    assert_eq!(response.placement.device_id, target.device_id);
    assert!(response.placement.authority_epoch > command.expected_authority_epoch);

    let control_event = tokio::time::timeout(Duration::from_secs(1), control_events.recv())
        .await
        .expect("target Live control event timeout")
        .expect("target Live control event");
    let control_event: serde_json::Value =
        serde_json::from_str(&control_event.payload).expect("control event JSON");
    assert_eq!(control_event["reason"], "authority_placement_updated");
    assert_eq!(
        control_event["control"]["placement"]["authority_device_id"],
        target.device_id
    );
    assert_eq!(
        control_event["control"]["placement"]["authority_epoch"],
        serde_json::json!(response.placement.authority_epoch)
    );

    let adopted =
        rua_server::services::conversation_authority_target::load_chat_authority_placement_target(
            &target_workspace,
            chat_id,
        )
        .await
        .expect("target adopted placement");
    assert_eq!(adopted.device_id, target.device_id);
    assert_eq!(adopted.authority_epoch, response.placement.authority_epoch);
    let target_manifest = load_conversation_manifest(&target_workspace, &address)
        .await
        .expect("load target manifest")
        .expect("target manifest");
    assert!(target_manifest.compare(&required_manifest).matches);
    assert_eq!(
        std::fs::read(target_workspace.join("media").join(required_media_id))
            .expect("Required Media transferred before adoption"),
        b"required authority media"
    );

    let empty_command = authority_adopt_command(
        "authority-adopt-empty-chat",
        &source.device_id,
        &target.device_id,
        empty_chat_id,
        empty_manifest.clone(),
    );
    let empty_response = deliver_conversation_authority_adopt_via_session(
        source.runtime.session_manager(),
        &empty_command,
        Duration::from_secs(5),
    )
    .await
    .expect("target adopts empty Chat without full Sync");
    assert_eq!(empty_response.placement.device_id, target.device_id);
    let target_empty_manifest = load_conversation_manifest(&target_workspace, &empty_address)
        .await
        .expect("load target empty manifest")
        .expect("target empty manifest");
    assert!(target_empty_manifest.compare(&empty_manifest).matches);

    let repeated = deliver_conversation_authority_adopt_via_session(
        source.runtime.session_manager(),
        &command,
        Duration::from_secs(5),
    )
    .await
    .expect("same adoption command is idempotent");
    assert_eq!(repeated.placement, response.placement);

    let mut conflicting = command.clone();
    conflicting.required_manifest.message_count += 1;
    let conflict = deliver_conversation_authority_adopt_via_session(
        source.runtime.session_manager(),
        &conflicting,
        Duration::from_secs(5),
    )
    .await
    .expect_err("same command id with different Manifest must fail");
    assert!(matches!(
        conflict,
        DeviceCommandDeliveryError::Rejected { ref reason, .. }
            if reason == "duplicate_command_conflict"
    ));

    source.runtime.shutdown().await.expect("shutdown source");
    target.runtime.shutdown().await.expect("shutdown target");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn supersession_clears_old_queue_and_redirects_attached_and_late_clients() {
    let controller_state_dir = TempDir::new().expect("controller state tempdir");
    let controller_state = AppState::from_tempdir(
        controller_state_dir.path().to_path_buf(),
        ServerProfile::Test,
    );
    let old_state_dir = TempDir::new().expect("old authority state tempdir");
    let old_state = AppState::from_tempdir(old_state_dir.path().to_path_buf(), ServerProfile::Test);
    let new_state_dir = TempDir::new().expect("new authority state tempdir");
    let new_state = AppState::from_tempdir(new_state_dir.path().to_path_buf(), ServerProfile::Test);
    seed_fixture_provider(&new_state, vec![fixture_provider_model("available")]).await;

    let network_secret = [75; 32];
    install_test_device(&controller_state, 76);
    install_test_device(&old_state, 77);
    install_test_device(&new_state, 78);
    let controller_id = hex::encode(
        DeviceNetworkIdentity::new(iroh::SecretKey::from_bytes(&[76; 32])).local_device_id(),
    );
    let old_device_id = hex::encode(
        DeviceNetworkIdentity::new(iroh::SecretKey::from_bytes(&[77; 32])).local_device_id(),
    );
    let new_device_id = hex::encode(
        DeviceNetworkIdentity::new(iroh::SecretKey::from_bytes(&[78; 32])).local_device_id(),
    );
    let chat_id = "00000000-0000-0000-0000-000000009926";
    let old_workspace = old_state.rua_root.join("workspaces").join("default");
    let new_workspace = new_state.rua_root.join("workspaces").join("default");
    let controller_workspace = controller_state.rua_root.join("workspaces").join("default");
    seed_authority_placement(&controller_workspace, chat_id, &old_device_id).await;
    seed_adoptable_chat(&old_workspace, chat_id, &old_device_id, "available").await;
    seed_adoptable_chat(&new_workspace, chat_id, &old_device_id, "available").await;
    let address = rua_core::ChatAddress::agent("default", chat_id);
    let required_manifest = load_conversation_manifest(&new_workspace, &address)
        .await
        .expect("load adoption manifest")
        .expect("adoption manifest");

    old_state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent("default".to_string(), chat_id.to_string()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: old_device_id.clone(),
            authority_epoch: epoch(1),
            driver_detail: None,
        })
        .await;
    let queued = old_state
        .conversation_authority
        .enqueue_input(
            &ChatAddress::agent("default", chat_id),
            ConversationInputPayload {
                message: "must be superseded".to_string(),
                model_override: None,
                input: None,
                messages: None,
                extra: serde_json::Map::new(),
                attachments: Vec::new(),
                enqueued_at: Instant::now(),
            },
            Some(controller_id.clone()),
        )
        .await
        .expect("old authority queue entry");
    old_state
        .conversation_authority
        .finish_active_turn(&ChatAddress::agent("default", chat_id))
        .await;

    let controller = build_runtime(
        76,
        network_secret,
        vec![peer_entry(77), peer_entry(78)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let old = build_runtime(
        77,
        network_secret,
        vec![peer_entry(76), peer_entry(78)],
        Some(full_command_handler(&old_state, &old_device_id)),
        Vec::new(),
        test_config(),
    )
    .await;
    let new = build_runtime(
        78,
        network_secret,
        vec![peer_entry(76), peer_entry(77)],
        Some(full_command_handler(&new_state, &new_device_id)),
        Vec::new(),
        test_config(),
    )
    .await;
    install_connectable_peers(&controller, &[&old, &new]);
    install_connectable_peers(&old, &[&controller, &new]);
    install_connectable_peers(&new, &[&controller, &old]);
    let controller_identity: rua_sync::identity::DeviceIdentity =
        serde_json::from_value(serde_json::json!({ "secret_bytes": vec![76u8; 32] }))
            .expect("controller sync Device identity");
    *controller_state
        .sync_device
        .write()
        .expect("controller sync Device identity cell") = Some(Arc::new(controller_identity));
    *controller_state.device_network.lock().await = Some(controller.runtime.clone());

    let old_live_hub = Arc::new(ActiveStreamBroadcastHub::new());
    old.runtime
        .session_manager()
        .set_live_activity_handler(Arc::new(
            ActiveStreamBroadcastDeviceSessionHandler::new(Arc::clone(&old_live_hub))
                .with_conversation_control_initial_state(
                    Arc::clone(&old_state.conversation_authority),
                    old_state.conversation_input_intake.clone(),
                    old_device_id.clone(),
                ),
        ));
    rua_server::services::conversation_live_activity::spawn_conversation_control_remote_bridge(
        Arc::clone(&old_state.conversation_authority),
        Arc::clone(&old_live_hub),
    );

    let (frame_tx, mut frame_rx) = tokio::sync::mpsc::unbounded_channel();
    let drop_next_redirect = Arc::new(AtomicUsize::new(0));
    let controller_manager = controller.runtime.session_manager().clone();
    let old_peer = old.device_id.clone();
    let drop_next_redirect_for_stream = Arc::clone(&drop_next_redirect);
    let attached_stream = tokio::spawn(async move {
        stream_active_broadcast_via_session(
            &controller_manager,
            &old_peer,
            ActiveStreamBroadcastSubscribeRequest {
                device_delivery_id: String::new(),
                owner: Some(ActiveStreamOwner::chat(ChatAddress::agent(
                    "default", chat_id,
                ))),
                after_seq_by_stream: BTreeMap::new(),
                activity_generation: None,
                conversation_activity_position: None,
                collapse_replay: true,
                initial_conversation_control: true,
                authority_epoch: Some(epoch(1)),
                viewer: None,
            },
            Duration::from_secs(5),
            tokio::sync::watch::channel(rua_core::RunControl::Running).1,
            move |frame| {
                let frame_tx = frame_tx.clone();
                let drop_next_redirect = Arc::clone(&drop_next_redirect_for_stream);
                async move {
                    if matches!(&frame, ActiveStreamBroadcastFrame::AuthorityRedirect { .. })
                        && drop_next_redirect.swap(0, Ordering::SeqCst) == 1
                    {
                        return;
                    }
                    let _ = frame_tx.send(frame);
                }
            },
        )
        .await
    });
    let initial = tokio::time::timeout(Duration::from_secs(5), frame_rx.recv())
        .await
        .expect("initial Control timeout")
        .expect("initial Control frame");
    assert!(matches!(initial, ActiveStreamBroadcastFrame::Event { .. }));

    let mut old_control_events = old_state
        .conversation_authority
        .subscribe_global_control_events();
    let command = authority_adopt_command(
        "authority-adopt-with-supersession",
        &controller.device_id,
        &new.device_id,
        chat_id,
        required_manifest,
    );
    let adopted = deliver_conversation_authority_adopt_via_session(
        controller.runtime.session_manager(),
        &command,
        Duration::from_secs(5),
    )
    .await
    .expect("new authority adoption");
    assert_eq!(adopted.placement.device_id, new_device_id);

    old_state
        .conversation_authority
        .apply_confirmed_authority_placement(&old_workspace, &address, adopted.placement.clone())
        .await
        .expect("initiating authority applies target acknowledgement");
    let acknowledged_control =
        tokio::time::timeout(Duration::from_secs(5), old_control_events.recv())
            .await
            .expect("acknowledged Control event timeout")
            .expect("acknowledged Control event");
    let acknowledged_control: serde_json::Value =
        serde_json::from_str(&acknowledged_control.payload).expect("acknowledged Control JSON");
    assert_eq!(
        acknowledged_control["reason"],
        "authority_placement_adopted"
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let frame = frame_rx.recv().await.expect("acknowledged Control frame");
            let ActiveStreamBroadcastFrame::Event { payload, .. } = frame else {
                continue;
            };
            let payload: serde_json::Value =
                serde_json::from_str(&payload).expect("acknowledged Control frame JSON");
            if payload["type"] == "conversation_control"
                && payload["reason"] == "authority_placement_adopted"
            {
                break;
            }
        }
    })
    .await
    .expect("acknowledged Control frame timeout");

    drop_next_redirect.store(1, Ordering::SeqCst);
    deliver_conversation_authority_supersede_via_session(
        new.runtime.session_manager(),
        &ConversationAuthoritySupersedeDeviceCommand {
            command_id: "authority-supersede-after-ack".to_string(),
            agent_id: "default".to_string(),
            address: address.clone(),
            source_device_id: new.device_id.clone(),
            target_device_id: old.device_id.clone(),
            previous_authority_epoch: epoch(1),
            replacement: adopted.placement.clone(),
        },
        Duration::from_secs(5),
    )
    .await
    .expect("new authority supersedes the acknowledged old authority");

    let old_redirect_event =
        tokio::time::timeout(Duration::from_secs(5), old_control_events.recv())
            .await
            .expect("old authority redirect event timeout")
            .expect("old authority redirect event");
    let old_redirect_event: serde_json::Value =
        serde_json::from_str(&old_redirect_event.payload).expect("redirect event JSON");
    assert_eq!(old_redirect_event["type"], "authority_redirect");
    assert_eq!(
        old_redirect_event["placement"]["device_id"],
        adopted.placement.device_id
    );
    assert_eq!(
        old_redirect_event["superseded_input_ids"],
        serde_json::json!([queued.input_id])
    );

    attached_stream
        .await
        .expect("attached stream task")
        .expect("attached stream ends cleanly on Redirect");
    assert_eq!(
        drop_next_redirect.load(Ordering::SeqCst),
        0,
        "the one-shot Authority Redirect frame must be emitted and deterministically dropped"
    );
    if let Ok(Some(frame)) = tokio::time::timeout(Duration::from_millis(100), frame_rx.recv()).await
    {
        panic!("the dropped Redirect must not reach the viewer: {frame:?}");
    }

    let old_placement =
        rua_server::services::conversation_authority_target::load_chat_authority_placement_target(
            &old_workspace,
            chat_id,
        )
        .await
        .expect("old authority stored replacement");
    assert_eq!(old_placement.device_id, new_device_id);
    assert_eq!(
        old_placement.authority_epoch,
        adopted.placement.authority_epoch
    );
    let old_control = old_state
        .conversation_authority
        .control_view(
            &ChatAddress::agent("default", chat_id),
            &rua_server::services::chat_index::get(&old_workspace, chat_id)
                .await
                .expect("old Chat Index")
                .expect("old Chat Index row")
                .meta,
        )
        .await;
    assert!(old_control.queue.entries.is_empty());

    new_state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent("default".to_string(), chat_id.to_string()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: new_device_id.clone(),
            authority_epoch: adopted.placement.authority_epoch,
            driver_detail: None,
        })
        .await;
    let redirected_queue_entry = new_state
        .conversation_authority
        .enqueue_input(
            &ChatAddress::agent("default", chat_id),
            ConversationInputPayload {
                message: "delete after Redirect".to_string(),
                model_override: None,
                input: None,
                messages: None,
                extra: serde_json::Map::new(),
                attachments: Vec::new(),
                enqueued_at: Instant::now(),
            },
            Some(controller_id.clone()),
        )
        .await
        .expect("new authority queue entry");
    let redirected_delete = ConversationControlDeviceCommand {
        command_id: "queue-delete-follows-one-redirect".to_string(),
        agent_id: "default".to_string(),
        address: rua_core::ChatAddress::agent("default".to_string(), chat_id.to_string()),
        source_device_id: controller.device_id.clone(),
        target_device_id: old.device_id.clone(),
        authority_epoch: epoch(1),
        action: ConversationControlDeviceAction::DeleteQueuedInput {
            input_id: redirected_queue_entry.input_id.clone(),
        },
    };
    let deleted =
        rua_server::services::conversation_control_dispatcher::dispatch_conversation_control(
            &controller_state.conversation_input_intake,
            redirected_delete.clone(),
        )
        .await
        .expect("queue delete follows authority Redirect");
    assert!(matches!(
        deleted,
        ConversationControlDeviceResponse::Deleted {
            ref deleted_input_id,
            ..
        } if deleted_input_id == &redirected_queue_entry.input_id
    ));
    let duplicate_at_new_authority = deliver_conversation_control_via_session(
        controller.runtime.session_manager(),
        &ConversationControlDeviceCommand {
            target_device_id: new.device_id.clone(),
            authority_epoch: adopted.placement.authority_epoch,
            ..redirected_delete
        },
        Duration::from_secs(5),
    )
    .await
    .expect("same Command ID returns target authority cached result");
    assert_eq!(duplicate_at_new_authority, deleted);
    new_state
        .conversation_authority
        .finish_active_turn(&ChatAddress::agent("default", chat_id))
        .await;

    seed_authority_placement_at_epoch(&controller_workspace, chat_id, &old_device_id, epoch(1))
        .await;
    let acp_redirect = deliver_acp_chat_command_via_session(
        controller.runtime.session_manager(),
        &AcpChatDeviceCommand {
            command_id: "stale-acp-status-after-supersession".to_string(),
            agent_id: "default".to_string(),
            address: rua_core::ChatAddress::agent("default".to_string(), chat_id.to_string()),
            source_device_id: controller.device_id.clone(),
            target_device_id: old.device_id.clone(),
            authority_epoch: epoch(1),
            action: AcpChatDeviceAction::Status,
        },
        Duration::from_secs(5),
    )
    .await
    .expect_err("old authority returns persistent ACP Redirect");
    assert!(matches!(
        acp_redirect,
        DeviceCommandDeliveryError::AuthorityRedirect(ref redirect)
            if redirect.placement == adopted.placement
    ));
    let redirected_acp_status = AcpChatDeviceCommand {
        command_id: "acp-status-follows-one-redirect".to_string(),
        agent_id: "default".to_string(),
        address: rua_core::ChatAddress::agent("default".to_string(), chat_id.to_string()),
        source_device_id: controller.device_id.clone(),
        target_device_id: old.device_id.clone(),
        authority_epoch: epoch(1),
        action: AcpChatDeviceAction::Status,
    };
    let acp_status = rua_server::services::conversation_control_dispatcher::dispatch_acp_control(
        &controller_state.conversation_input_intake,
        redirected_acp_status.clone(),
    )
    .await
    .expect("ACP status follows authority Redirect");
    assert!(matches!(acp_status, AcpChatDeviceResponse::Status { .. }));
    let duplicate_acp_status = deliver_acp_chat_command_via_session(
        controller.runtime.session_manager(),
        &AcpChatDeviceCommand {
            target_device_id: new.device_id.clone(),
            authority_epoch: adopted.placement.authority_epoch,
            ..redirected_acp_status
        },
        Duration::from_secs(5),
    )
    .await
    .expect("same ACP Command ID returns target authority cached result");
    assert_eq!(duplicate_acp_status, acp_status);

    new_state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent("default".to_string(), chat_id.to_string()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: new_device_id.clone(),
            authority_epoch: adopted.placement.authority_epoch,
            driver_detail: None,
        })
        .await;
    let submit_outcome = controller_state
        .console_facade
        .submit_chat(
            "default",
            ConsoleChatRequest {
                message: "follow the authority Redirect once".to_string(),
                chat_id: Some(chat_id.to_string()),
                model: None,
                is_reconnect: false,
                attachments: Vec::<AttachmentInfo>::new(),
                input: None,
                messages: None,
                extra: serde_json::Map::from_iter([(
                    "submission_id".to_string(),
                    serde_json::json!("redirected-submission"),
                )]),
                channel_kind: None,
            },
        )
        .await;
    assert!(submit_outcome.accepted);
    assert_eq!(submit_outcome.kind, ConsoleSubmitKind::Queued);
    assert_eq!(
        submit_outcome.target_device_id.as_deref(),
        Some(new_device_id.as_str())
    );
    let redirected_queue = new_state
        .conversation_authority
        .control_view(
            &ChatAddress::agent("default", chat_id),
            &rua_server::services::chat_index::get(&new_workspace, chat_id)
                .await
                .expect("new Chat Index")
                .expect("new Chat Index row")
                .meta,
        )
        .await;
    assert!(redirected_queue
        .queue
        .entries
        .iter()
        .any(|entry| entry.submission_id == "redirected-submission"));
    new_state
        .conversation_authority
        .finish_active_turn(&ChatAddress::agent("default", chat_id))
        .await;
    seed_authority_placement_at_epoch(&controller_workspace, chat_id, &old_device_id, epoch(1))
        .await;
    assert!(
        !controller_state
            .console_facade
            .stop_chat("default", chat_id)
            .await,
        "already-idle Stop should follow one Redirect and return an authoritative no-op"
    );
    let stop_observed_placement =
        rua_server::services::conversation_authority_target::load_chat_authority_placement_target(
            &controller_workspace,
            chat_id,
        )
        .await
        .expect("Stop stores verified redirect");
    assert_eq!(stop_observed_placement.device_id, new_device_id);
    assert_eq!(
        stop_observed_placement.authority_epoch,
        adopted.placement.authority_epoch
    );

    let stale_control = ConversationControlDeviceCommand {
        command_id: "stale-control-after-supersession".to_string(),
        agent_id: "default".to_string(),
        address: rua_core::ChatAddress::agent("default".to_string(), chat_id.to_string()),
        source_device_id: controller.device_id.clone(),
        target_device_id: old.device_id.clone(),
        authority_epoch: epoch(1),
        action: ConversationControlDeviceAction::View,
    };
    let control_error = deliver_conversation_control_via_session(
        controller.runtime.session_manager(),
        &stale_control,
        Duration::from_secs(5),
    )
    .await
    .expect_err("old authority returns persistent Control Redirect");
    let redirect = match control_error {
        DeviceCommandDeliveryError::AuthorityRedirect(redirect) => redirect,
        other => panic!("expected authority Redirect, got {other:?}"),
    };
    assert_eq!(redirect.placement, adopted.placement);
    let observed = controller_state
        .conversation_input_intake
        .observe_authority_redirect(epoch(1), &redirect)
        .await
        .expect("verify and observe redirected authority");
    assert_eq!(observed, adopted.placement);
    let controller_placement =
        rua_server::services::conversation_authority_target::load_chat_authority_placement_target(
            &controller_workspace,
            chat_id,
        )
        .await
        .expect("controller stores verified redirect");
    assert_eq!(controller_placement.device_id, new_device_id);
    assert_eq!(
        controller_placement.authority_epoch,
        adopted.placement.authority_epoch
    );
    let redirected_control = ConversationControlDeviceCommand {
        target_device_id: new.device_id.clone(),
        authority_epoch: adopted.placement.authority_epoch,
        ..stale_control.clone()
    };
    let redirected_control = deliver_conversation_control_via_session(
        controller.runtime.session_manager(),
        &redirected_control,
        Duration::from_secs(5),
    )
    .await
    .expect("same Control command id reaches redirected authority");
    assert!(matches!(
        redirected_control,
        ConversationControlDeviceResponse::View { .. }
    ));

    let mut stale_input = remote_input_request(&old.device_id);
    stale_input.agent_id = "default".to_string();
    stale_input.address = rua_core::ChatAddress::agent(stale_input.agent_id.clone(), chat_id);
    stale_input.source_device_id = controller.device_id.clone();
    stale_input.authority_epoch = epoch(1);
    let input_error = deliver_remote_conversation_input_via_session(
        controller.runtime.session_manager(),
        &stale_input,
        Duration::from_secs(5),
    )
    .await
    .expect_err("old authority returns persistent Submit Redirect");
    assert!(matches!(
        input_error,
        DeviceCommandDeliveryError::AuthorityRedirect(ref redirect)
            if redirect.placement == adopted.placement
    ));

    let stale_stop = RemoteChatControlCommand {
        command_id: "stale-stop-after-supersession".to_string(),
        agent_id: "default".to_string(),
        address: rua_core::ChatAddress::agent("default".to_string(), chat_id.to_string()),
        source_device_id: controller.device_id.clone(),
        target_device_id: old.device_id.clone(),
        authority_epoch: epoch(1),
        device_delivery_id: None,
        await_terminal: true,
        action: RemoteChatControlAction::StopCurrentTurn,
    };
    let stop_error = deliver_remote_chat_control_via_session(
        controller.runtime.session_manager(),
        &stale_stop,
        Duration::from_secs(5),
    )
    .await
    .expect_err("old authority returns persistent Stop Redirect");
    assert!(matches!(
        stop_error,
        DeviceCommandDeliveryError::AuthorityRedirect(ref redirect)
            if redirect.placement == adopted.placement
    ));
    let idle_stop = RemoteChatControlCommand {
        command_id: "idle-stop-on-current-authority".to_string(),
        target_device_id: new.device_id.clone(),
        authority_epoch: adopted.placement.authority_epoch,
        ..stale_stop.clone()
    };
    let idle_stop = deliver_remote_chat_control_via_session(
        controller.runtime.session_manager(),
        &idle_stop,
        Duration::from_secs(5),
    )
    .await
    .expect("already-idle Stop reaches current authority");
    assert!(!idle_stop.applied);
    assert!(idle_stop
        .detail
        .as_deref()
        .is_some_and(|detail| detail.contains("no active turn")));

    let request = ActiveStreamBroadcastSubscribeRequest {
        device_delivery_id: String::new(),
        owner: Some(ActiveStreamOwner::chat(ChatAddress::agent(
            "default", chat_id,
        ))),
        after_seq_by_stream: BTreeMap::new(),
        activity_generation: None,
        conversation_activity_position: None,
        collapse_replay: true,
        initial_conversation_control: true,
        authority_epoch: Some(epoch(1)),
        viewer: None,
    };
    let payload = serde_json::to_vec(&request).expect("late Live request");
    let mut late_stream = controller
        .runtime
        .session_manager()
        .open_live_activity_to(&old.device_id, &payload, Duration::from_secs(5))
        .await
        .expect("open late stale Live");
    let late_frame = late_stream
        .next_frame()
        .await
        .expect("late Redirect frame")
        .expect("late Redirect bytes");
    let late_frame: ActiveStreamBroadcastFrame =
        serde_json::from_slice(&late_frame).expect("decode late Redirect");
    assert!(matches!(
        late_frame,
        ActiveStreamBroadcastFrame::AuthorityRedirect { ref placement, .. }
            if placement == &adopted.placement
    ));

    controller
        .runtime
        .shutdown()
        .await
        .expect("shutdown controller");
    old.runtime
        .shutdown()
        .await
        .expect("shutdown old authority");
    new.runtime
        .shutdown()
        .await
        .expect("shutdown new authority");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn redirect_verification_rejects_a_second_redirect_without_updating_local_index() {
    let source_state_dir = TempDir::new().expect("source state tempdir");
    let source_state =
        AppState::from_tempdir(source_state_dir.path().to_path_buf(), ServerProfile::Test);
    let target_state_dir = TempDir::new().expect("target state tempdir");
    let target_state =
        AppState::from_tempdir(target_state_dir.path().to_path_buf(), ServerProfile::Test);
    let network_secret = [80; 32];
    let source_device_id = hex::encode(
        DeviceNetworkIdentity::new(iroh::SecretKey::from_bytes(&[81; 32])).local_device_id(),
    );
    let target_device_id = hex::encode(
        DeviceNetworkIdentity::new(iroh::SecretKey::from_bytes(&[82; 32])).local_device_id(),
    );
    let third_device_id = hex::encode(
        DeviceNetworkIdentity::new(iroh::SecretKey::from_bytes(&[83; 32])).local_device_id(),
    );
    let chat_id = "00000000-0000-0000-0000-000000009927";
    let source_workspace = source_state.rua_root.join("workspaces").join("default");
    let target_workspace = target_state.rua_root.join("workspaces").join("default");
    seed_authority_placement_at_epoch(&source_workspace, chat_id, "old-device", epoch(1)).await;
    seed_authority_placement_at_epoch(&target_workspace, chat_id, &third_device_id, epoch(3)).await;

    let source = build_runtime(
        81,
        network_secret,
        vec![peer_entry(82)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let target = build_runtime(
        82,
        network_secret,
        vec![peer_entry(81)],
        Some(full_command_handler(&target_state, &target_device_id)),
        Vec::new(),
        test_config(),
    )
    .await;
    install_connectable_peer(&source, &target);
    let source_identity: rua_sync::identity::DeviceIdentity =
        serde_json::from_value(serde_json::json!({ "secret_bytes": vec![81u8; 32] }))
            .expect("source sync Device identity");
    *source_state
        .sync_device
        .write()
        .expect("source sync Device identity cell") = Some(Arc::new(source_identity));
    *source_state.device_network.lock().await = Some(source.runtime.clone());

    let redirect = ConversationAuthorityRedirect {
        address: rua_core::ChatAddress::agent("default".to_string(), chat_id.to_string()),
        placement:
            rua_server::services::conversation_authority_placement::ConversationAuthorityPlacement {
                device_id: target_device_id,
                authority_epoch: epoch(2),
                base_manifest_hash:
                    "sha256:2222222222222222222222222222222222222222222222222222222222222222"
                        .to_string(),
            },
    };
    let error = source_state
        .conversation_input_intake
        .observe_authority_redirect(epoch(1), &redirect)
        .await
        .expect_err("a second Redirect must end the current operation");
    assert!(
        error.contains(&third_device_id),
        "unexpected error: {error}"
    );
    let unchanged =
        rua_server::services::conversation_authority_target::load_chat_authority_placement_target(
            &source_workspace,
            chat_id,
        )
        .await
        .expect("source placement remains readable");
    assert_eq!(unchanged.device_id, "old-device");
    assert_eq!(unchanged.authority_epoch, epoch(1));
    assert_eq!(source.device_id, source_device_id);

    source.runtime.shutdown().await.expect("shutdown source");
    target.runtime.shutdown().await.expect("shutdown target");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn target_rejects_adoption_for_model_active_turn_and_acp() {
    let source_state_dir = TempDir::new().expect("source state tempdir");
    let source_state =
        AppState::from_tempdir(source_state_dir.path().to_path_buf(), ServerProfile::Test);
    let target_state_dir = TempDir::new().expect("target state tempdir");
    let target_state =
        AppState::from_tempdir(target_state_dir.path().to_path_buf(), ServerProfile::Test);
    seed_fixture_provider(&target_state, vec![fixture_provider_model("available")]).await;

    let network_secret = [58; 32];
    let old_authority_device_id = "bbbbbbbbbbbbbbbb";
    let target_device_id = hex::encode(
        DeviceNetworkIdentity::new(iroh::SecretKey::from_bytes(&[60; 32])).local_device_id(),
    );
    let source_workspace = source_state.rua_root.join("workspaces").join("default");
    let target_workspace = target_state.rua_root.join("workspaces").join("default");
    let model_chat = "00000000-0000-0000-0000-000000009921";
    let active_chat = "00000000-0000-0000-0000-000000009922";
    let acp_chat = "00000000-0000-0000-0000-000000009923";

    for (chat_id, model_id) in [
        (model_chat, "missing"),
        (active_chat, "available"),
        (acp_chat, "available"),
    ] {
        seed_adoptable_chat(
            &source_workspace,
            chat_id,
            old_authority_device_id,
            model_id,
        )
        .await;
        seed_adoptable_chat(
            &target_workspace,
            chat_id,
            old_authority_device_id,
            model_id,
        )
        .await;
        let address = ChatAddress::agent("default", chat_id);
        let source_manifest = load_conversation_manifest(&source_workspace, &address)
            .await
            .expect("load source validation manifest")
            .expect("source validation manifest");
        let target_manifest = load_conversation_manifest(&target_workspace, &address)
            .await
            .expect("load target validation manifest")
            .expect("target validation manifest");
        assert!(
            target_manifest.compare(&source_manifest).matches,
            "validation fixture histories must match"
        );
    }
    target_state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent("default".to_string(), active_chat.to_string()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: old_authority_device_id.to_string(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: None,
        })
        .await;
    let store = WorkspaceSessionStore::new(&target_workspace);
    let mut acp_header = store
        .load_chat_header(acp_chat)
        .await
        .expect("load ACP Chat Session")
        .expect("ACP Chat Session");
    acp_header.extra.insert(
        "acp_binding".to_string(),
        serde_json::json!({
            "device_id": old_authority_device_id,
            "runner": "codex",
        }),
    );
    store
        .save_chat_header(&acp_header)
        .await
        .expect("seed target Session ACP binding");

    let target_handler = full_command_handler(&target_state, &target_device_id);
    let source = build_runtime(
        59,
        network_secret,
        vec![peer_entry(60)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let target = build_runtime(
        60,
        network_secret,
        vec![peer_entry(59)],
        Some(target_handler),
        Vec::new(),
        test_config(),
    )
    .await;
    install_connectable_peer(&source, &target);
    assert_eq!(target.device_id, target_device_id);

    for (chat_id, command_id, expected_reason) in [
        (model_chat, "authority-adopt-model", "model_unavailable"),
        (active_chat, "authority-adopt-active", "active_turn"),
        (acp_chat, "authority-adopt-acp", "acp_active"),
    ] {
        let address = rua_core::ChatAddress::agent("default", chat_id);
        let manifest = load_conversation_manifest(&source_workspace, &address)
            .await
            .expect("load source manifest")
            .expect("source manifest");
        let command = authority_adopt_command(
            command_id,
            &source.device_id,
            &target.device_id,
            chat_id,
            manifest,
        );
        let error = deliver_conversation_authority_adopt_via_session(
            source.runtime.session_manager(),
            &command,
            Duration::from_secs(5),
        )
        .await
        .expect_err("target validation must reject adoption");
        assert!(
            matches!(
                error,
                DeviceCommandDeliveryError::Rejected { ref reason, .. }
                    if reason == expected_reason
            ),
            "unexpected rejection for {chat_id}: {error}"
        );
    }

    source.runtime.shutdown().await.expect("shutdown source");
    target.runtime.shutdown().await.expect("shutdown target");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_model_control_is_normalized_and_persisted_on_authority() {
    let target_state_dir = TempDir::new().expect("target state tempdir");
    let target_state =
        AppState::from_tempdir(target_state_dir.path().to_path_buf(), ServerProfile::Test);
    let mut old_model = fixture_provider_model("old-model");
    old_model.thinking_param_style = Some("budget".to_string());
    old_model.thinking_budget_range = Some([1, 8_192]);
    let mut new_model = fixture_provider_model("new-model");
    new_model.thinking_param_style = Some("effort".to_string());
    new_model.reasoning_effort_options =
        vec!["low".to_string(), "medium".to_string(), "high".to_string()];
    seed_fixture_provider(&target_state, vec![old_model, new_model]).await;

    let network_secret = [68; 32];
    install_test_device(&target_state, 70);
    let target_device_id = hex::encode(
        DeviceNetworkIdentity::new(iroh::SecretKey::from_bytes(&[70; 32])).local_device_id(),
    );
    let chat_id = "00000000-0000-0000-0000-000000009926";
    let target_workspace = target_state.rua_root.join("workspaces").join("default");
    seed_adoptable_chat(&target_workspace, chat_id, &target_device_id, "old-model").await;
    target_state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent("default".to_string(), chat_id.to_string()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: target_device_id.clone(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: Some(serde_json::json!({
                "chat_model_slot": {
                    "provider_id": "fixture",
                    "model": "old-model",
                    "thinking_strength": "auto",
                }
            })),
        })
        .await;
    let mut control_events = target_state
        .conversation_authority
        .subscribe_global_control_events();

    let source = build_runtime(
        69,
        network_secret,
        vec![peer_entry(70)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let target = build_runtime(
        70,
        network_secret,
        vec![peer_entry(69)],
        Some(full_command_handler(&target_state, &target_device_id)),
        Vec::new(),
        test_config(),
    )
    .await;
    install_connectable_peer(&source, &target);

    target_state.sync_enabled.store(true, Ordering::Relaxed);
    let target_sync_guard = target_state.sync_node.lock().await;
    let response = tokio::time::timeout(
        Duration::from_secs(1),
        deliver_conversation_control_via_session(
            source.runtime.session_manager(),
            &ConversationControlDeviceCommand {
                command_id: "model-control-target-normalization".to_string(),
                agent_id: "default".to_string(),
                address: rua_core::ChatAddress::agent("default".to_string(), chat_id.to_string()),
                source_device_id: source.device_id.clone(),
                target_device_id: target.device_id.clone(),
                authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
                action: ConversationControlDeviceAction::UpdateModel {
                    model: serde_json::json!({
                        "provider_id": "fixture",
                        "model": "new-model",
                        "thinking_strength": "high",
                        "thinking_param_style": "budget",
                        "thinking_budget_range": [1, 1],
                        "reasoning_effort_options": ["forged"],
                    }),
                },
            },
            Duration::from_secs(5),
        ),
    )
    .await
    .expect("model control acknowledgement must not await Sync publication")
    .expect("remote model update");
    drop(target_sync_guard);
    target_state.sync_enabled.store(false, Ordering::Relaxed);
    let ConversationControlDeviceResponse::ModelUpdated { model, control, .. } = response else {
        panic!("unexpected model response");
    };
    assert_eq!(model["provider_id"], "fixture");
    assert_eq!(model["model"], "new-model");
    assert_eq!(model["thinking_strength"], "high");
    assert_eq!(model["thinking_param_style"], "effort");
    assert_eq!(
        model["reasoning_effort_options"],
        serde_json::json!(["low", "medium", "high"])
    );
    assert!(model.get("thinking_budget_range").is_none());
    assert_eq!(control.model, Some(model.clone()));
    assert_eq!(
        control
            .active_turn
            .as_ref()
            .and_then(|turn| turn.driver_detail.as_ref())
            .and_then(|detail| detail.pointer("/chat_model_slot/model"))
            .and_then(serde_json::Value::as_str),
        Some("old-model"),
        "the current Active Turn keeps its captured model"
    );

    let persisted = target_state
        .conversation_input_intake
        .authority_chat_model("default", chat_id)
        .await
        .expect("authority model projection");
    assert_eq!(persisted, model);
    let event = tokio::time::timeout(Duration::from_secs(1), control_events.recv())
        .await
        .expect("model control event timeout")
        .expect("model control event");
    let event: serde_json::Value =
        serde_json::from_str(&event.payload).expect("model control event JSON");
    assert_eq!(event["reason"], "model_updated");
    assert_eq!(event["control"]["model"], persisted);

    let unavailable = deliver_conversation_control_via_session(
        source.runtime.session_manager(),
        &ConversationControlDeviceCommand {
            command_id: "model-control-unavailable".to_string(),
            agent_id: "default".to_string(),
            address: rua_core::ChatAddress::agent("default".to_string(), chat_id.to_string()),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            action: ConversationControlDeviceAction::UpdateModel {
                model: serde_json::json!({
                    "provider_id": "fixture",
                    "model": "missing-model",
                    "thinking_strength": "auto",
                }),
            },
        },
        Duration::from_secs(5),
    )
    .await
    .expect_err("unavailable target model must fail");
    assert!(matches!(
        unavailable,
        DeviceCommandDeliveryError::Rejected { ref reason, .. }
            if reason == "chat_model_update_rejected"
    ));

    source.runtime.shutdown().await.expect("shutdown source");
    target.runtime.shutdown().await.expect("shutdown target");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_chat_model_and_authority_switch_matrix_accepts_input() {
    let source_dir = TempDir::new().unwrap();
    let target_dir = TempDir::new().unwrap();
    let source_state = AppState::from_tempdir(source_dir.path().to_path_buf(), ServerProfile::Test);
    let target_state = AppState::from_tempdir(target_dir.path().to_path_buf(), ServerProfile::Test);
    install_test_device(&source_state, 151);
    install_test_device(&target_state, 152);
    let captured = CapturingAgentRuntime::default();
    for state in [&source_state, &target_state] {
        state
            .registry
            .get_or_create("default")
            .await
            .unwrap()
            .set_react_runtime(Arc::new(captured.clone()))
            .await;
        seed_fixture_provider(
            state,
            vec![
                fixture_provider_model("model-a"),
                fixture_provider_model("model-b"),
            ],
        )
        .await;
        state
            .agent_service
            .get_agent("default")
            .await
            .unwrap()
            .expect("default Agent");
    }
    let source_id = hex::encode(
        DeviceNetworkIdentity::new(iroh::SecretKey::from_bytes(&[151; 32])).local_device_id(),
    );
    let target_id = hex::encode(
        DeviceNetworkIdentity::new(iroh::SecretKey::from_bytes(&[152; 32])).local_device_id(),
    );
    let source = build_runtime(
        151,
        [153; 32],
        vec![peer_entry(152)],
        Some(full_command_handler(&source_state, &source_id)),
        Vec::new(),
        test_config(),
    )
    .await;
    let target = build_runtime(
        152,
        [153; 32],
        vec![peer_entry(151)],
        Some(full_command_handler(&target_state, &target_id)),
        Vec::new(),
        test_config(),
    )
    .await;
    install_connectable_peer(&source, &target);
    install_connectable_peer(&target, &source);
    *source_state.device_network.lock().await = Some(source.runtime.clone());
    *target_state.device_network.lock().await = Some(target.runtime.clone());
    *source_state.sync_transfer_session_manager.write().unwrap() =
        Some(source.runtime.session_manager().clone());
    *target_state.sync_transfer_session_manager.write().unwrap() =
        Some(target.runtime.session_manager().clone());
    source
        .runtime
        .session_manager()
        .set_bulk_transfer_handler(Arc::new(
            SyncTransferBulkHandler::new(source.device_id.clone())
                .with_chat_media_root(source_state.rua_root.clone()),
        ));
    target
        .runtime
        .session_manager()
        .set_bulk_transfer_handler(Arc::new(
            SyncTransferBulkHandler::new(target.device_id.clone())
                .with_chat_media_root(target_state.rua_root.clone()),
        ));
    let app = axum::Router::new()
        .nest("/api", rua_server::endpoints::chats::agent_scoped_router())
        .layer(axum::middleware::from_fn_with_state(
            source_state.clone(),
            rua_server::middleware::agent_context_middleware,
        ))
        .with_state(source_state.clone());
    for materialized in [false, true] {
        for operations in [
            vec!["model", "device"],
            vec!["device", "model"],
            vec!["model"],
            vec!["device"],
        ] {
            let chat_id = uuid::Uuid::new_v4().to_string();
            let address = ChatAddress::agent("default", &chat_id);
            let mut selected_model = "model-a";
            let mut exists = materialized;
            if materialized {
                let workspace = source_state
                    .registry
                    .get_or_create("default")
                    .await
                    .unwrap()
                    .workspace_dir();
                seed_adoptable_chat(&workspace, &chat_id, &source_id, selected_model).await;
            }
            for operation in &operations {
                let (path, body) = if *operation == "model" {
                    selected_model = "model-b";
                    if !exists {
                        continue;
                    }
                    (
                        format!("/api/agents/default/chats/{chat_id}"),
                        serde_json::json!({"model":{"provider_id":"fixture","model":selected_model}}),
                    )
                } else {
                    if !exists {
                        continue;
                    }
                    (
                        format!("/api/agents/default/chats/{chat_id}/authority-placement"),
                        serde_json::json!({"device_id":target_id,"model":{"provider_id":"fixture","model":selected_model}}),
                    )
                };
                let response = app
                    .clone()
                    .oneshot(
                        Request::builder()
                            .method(Method::PUT)
                            .uri(path)
                            .header("content-type", "application/json")
                            .body(Body::from(serde_json::to_vec(&body).unwrap()))
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                let status = response.status();
                let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap();
                assert_eq!(status, StatusCode::OK, "materialized={materialized} operations={operations:?} operation={operation}: {}", String::from_utf8_lossy(&bytes));
                exists = true;
                if *operation == "device" {
                    use rua_sync::sync_registry::DomainSyncAdapter;
                    let target_workspace = target_state
                        .registry
                        .get_or_create("default")
                        .await
                        .unwrap()
                        .workspace_dir();
                    let header = WorkspaceSessionStore::new(&target_workspace)
                        .load_chat_header(&chat_id)
                        .await
                        .unwrap()
                        .unwrap();
                    let mut delayed = serde_json::to_value(&header).unwrap();
                    delayed["authority_placement"]["device_id"] = serde_json::json!(source_id);
                    delayed["authority_placement"]["authority_epoch"] = serde_json::json!(epoch(1));
                    let adapter = rua_sync::evergreen_chat_adapter::EvergreenChatSyncAdapter::new(
                        target_state.rua_root.clone(),
                        target_id.clone(),
                    );
                    let merge_dir = TempDir::new().unwrap();
                    let merge = rua_sync::merge_store::MergeStore::open(merge_dir.path()).unwrap();
                    let change =
                        rua_sync::evergreen_chat_adapter::EvergreenChatSyncAdapter::file_change(
                            format!("delayed-{chat_id}"),
                            &address,
                            "session",
                            delayed,
                            &source_id,
                            rua_sync::hlc::HlcTimestamp {
                                wall_ms: u64::MAX - 1,
                                counter: 0,
                                device_id: [151; 8],
                            },
                        );
                    adapter.apply_change(&change, &merge).await.unwrap();
                }
            }
            let remote = operations.contains(&"device");
            let authority_state = if remote { &target_state } else { &source_state };
            let workspace = authority_state
                .registry
                .get_or_create("default")
                .await
                .unwrap()
                .workspace_dir();
            let store = WorkspaceSessionStore::new(&workspace);
            if exists {
                let before = store.load_chat_header(&chat_id).await.unwrap().unwrap();
                assert_eq!(
                    rua_chat::model_slot_from_header(&before).unwrap()["model"],
                    selected_model
                );
            }
            let outcome = source_state
                .console_facade
                .submit_chat(
                    "default",
                    ConsoleChatRequest {
                        message: "matrix input".into(),
                        chat_id: Some(chat_id.clone()),
                        model: None,
                        is_reconnect: false,
                        attachments: vec![],
                        input: None,
                        messages: None,
                        extra: serde_json::Map::from_iter([
                            (
                                "target_device_id".into(),
                                serde_json::json!(if remote { &target_id } else { &source_id }),
                            ),
                            (
                                "chat_model_slot".into(),
                                serde_json::json!({"provider_id":"fixture","model":selected_model}),
                            ),
                            (
                                "submission_id".into(),
                                serde_json::json!(uuid::Uuid::new_v4().to_string()),
                            ),
                        ]),
                        channel_kind: None,
                    },
                )
                .await;
            assert!(
                outcome.accepted,
                "materialized={materialized} operations={operations:?}: {outcome:?}"
            );
            let placement = rua_server::services::conversation_authority_target::load_chat_authority_placement_target(&workspace, &chat_id).await.unwrap();
            let active = authority_state
                .conversation_authority
                .active_turn(&address)
                .await;
            if let Some(active) = active {
                assert_eq!(active.authority_epoch, placement.authority_epoch);
            }
            let after = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let header = store.load_chat_header(&chat_id).await.unwrap().unwrap();
                    if header.turn_index.latest_turn == if materialized { 2 } else { 1 } {
                        break header;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("accepted input persisted on authority");
            assert_eq!(
                placement.device_id,
                if remote {
                    target_id.clone()
                } else {
                    source_id.clone()
                }
            );
            assert_eq!(
                after.turn_index.latest_turn,
                if materialized { 2 } else { 1 },
                "accepted input must be persisted on the authority: {operations:?}"
            );
            if materialized || !remote {
                let controller_placement = rua_server::services::conversation_authority_target::load_optional_chat_authority_placement(&source_state.registry.get_or_create("default").await.unwrap().workspace_dir(), &chat_id).await.unwrap().unwrap();
                assert_eq!(controller_placement.device_id, placement.device_id);
                assert_eq!(
                    controller_placement.authority_epoch,
                    placement.authority_epoch
                );
            } else {
                assert!(WorkspaceSessionStore::new(
                    source_state
                        .registry
                        .get("default")
                        .await
                        .unwrap()
                        .workspace_dir()
                )
                .load_chat_header(&chat_id)
                .await
                .unwrap()
                .is_none());
            }
            assert_eq!(
                rua_chat::model_slot_from_header(&after).unwrap()["model"],
                selected_model
            );
        }
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        while captured.requests.lock().await.len() < 8 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("all accepted inputs reach the runtime");
    assert_eq!(captured.requests.lock().await.len(), 8);
    source.runtime.shutdown().await.unwrap();
    target.runtime.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_chat_options_update_authority_runtime_and_control_view() {
    let target_state_dir = TempDir::new().expect("target state tempdir");
    let target_state =
        AppState::from_tempdir(target_state_dir.path().to_path_buf(), ServerProfile::Test);
    let network_secret = [71; 32];
    install_test_device(&target_state, 73);
    let target_device_id = hex::encode(
        DeviceNetworkIdentity::new(iroh::SecretKey::from_bytes(&[73; 32])).local_device_id(),
    );
    let chat_id = "00000000-0000-0000-0000-000000009927";
    let target_workspace = target_state.rua_root.join("workspaces").join("default");
    seed_authority_placement(&target_workspace, chat_id, &target_device_id).await;
    seed_chat_header(&target_workspace, chat_id, &target_device_id).await;
    let runtime_before = target_state
        .registry
        .get_or_create("default")
        .await
        .expect("load target runtime before options update");
    let agent_defaults = target_state
        .conversation_input_intake
        .authority_running_config("default")
        .await
        .expect("target running config");
    let default_max_iters = agent_defaults["max_iters"].clone();
    let mut control_events = target_state
        .conversation_authority
        .subscribe_global_control_events();

    let source = build_runtime(
        72,
        network_secret,
        vec![peer_entry(73)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let target = build_runtime(
        73,
        network_secret,
        vec![peer_entry(72)],
        Some(full_command_handler(&target_state, &target_device_id)),
        Vec::new(),
        test_config(),
    )
    .await;
    install_connectable_peer(&source, &target);

    let read = deliver_conversation_control_via_session(
        source.runtime.session_manager(),
        &ConversationControlDeviceCommand {
            command_id: "running-config-read".to_string(),
            agent_id: "default".to_string(),
            address: rua_core::ChatAddress::agent("default".to_string(), chat_id.to_string()),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            action: ConversationControlDeviceAction::ViewRunningConfig,
        },
        Duration::from_secs(5),
    )
    .await
    .expect("read target running config");
    let ConversationControlDeviceResponse::RunningConfig {
        running: mut chat_running,
    } = read
    else {
        panic!("unexpected running config response");
    };
    assert_eq!(chat_running["max_iters"], default_max_iters);
    chat_running["max_iters"] = serde_json::json!(17);

    let updated = deliver_conversation_control_via_session(
        source.runtime.session_manager(),
        &ConversationControlDeviceCommand {
            command_id: "running-config-update".to_string(),
            agent_id: "default".to_string(),
            address: rua_core::ChatAddress::agent("default".to_string(), chat_id.to_string()),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            action: ConversationControlDeviceAction::UpdateRunningConfig {
                running: chat_running,
            },
        },
        Duration::from_secs(5),
    )
    .await
    .expect("update target running config");
    let ConversationControlDeviceResponse::RunningConfigUpdated {
        running: confirmed,
        control,
    } = updated
    else {
        panic!("unexpected running config response");
    };
    assert_eq!(confirmed["max_iters"], 17);
    assert_eq!(control.running_config, Some(confirmed.clone()));
    assert_eq!(
        target_state
            .conversation_input_intake
            .authority_running_config("default")
            .await
            .expect("read applied running config")["max_iters"],
        default_max_iters,
        "Chat options must not mutate Agent defaults"
    );
    assert_eq!(
        WorkspaceSessionStore::new(&target_workspace)
            .load_chat_running_config(chat_id)
            .await
            .unwrap()
            .unwrap()
            .max_iters,
        17
    );
    let runtime_after = target_state
        .registry
        .get_or_create("default")
        .await
        .expect("load target runtime after options update");
    assert!(
        Arc::ptr_eq(&runtime_before, &runtime_after),
        "Chat options must not reload the Agent runtime"
    );

    let event = tokio::time::timeout(Duration::from_secs(1), control_events.recv())
        .await
        .expect("running config control event timeout")
        .expect("running config control event");
    let event: serde_json::Value =
        serde_json::from_str(&event.payload).expect("running config event JSON");
    assert_eq!(event["reason"], "running_config_updated");
    assert_eq!(event["control"]["running_config"]["max_iters"], 17);

    let mut invalid = confirmed;
    invalid["llm_max_retries"] = serde_json::json!(0);
    let rejected = deliver_conversation_control_via_session(
        source.runtime.session_manager(),
        &ConversationControlDeviceCommand {
            command_id: "running-config-invalid".to_string(),
            agent_id: "default".to_string(),
            address: rua_core::ChatAddress::agent("default".to_string(), chat_id.to_string()),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            action: ConversationControlDeviceAction::UpdateRunningConfig {
                running: invalid,
            },
        },
        Duration::from_secs(5),
    )
    .await
    .expect_err("invalid target running config must fail");
    assert!(matches!(
        rejected,
        DeviceCommandDeliveryError::Rejected { ref reason, .. }
            if reason == "chat_running_config_update_rejected"
    ));
    assert_eq!(
        target_state
            .conversation_input_intake
            .authority_running_config("default")
            .await
            .expect("running config remains valid")["max_iters"],
        default_max_iters,
        "rejected update must not mutate Agent defaults"
    );

    source.runtime.shutdown().await.expect("shutdown source");
    target.runtime.shutdown().await.expect("shutdown target");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_waits_for_handoff_and_duplicate_command_does_not_stop_later_turn() {
    let state_dir = TempDir::new().expect("state tempdir");
    let state = AppState::from_tempdir(state_dir.path().to_path_buf(), ServerProfile::Test);
    let network_secret = [27; 32];
    let target_device_id = hex::encode(
        DeviceNetworkIdentity::new(iroh::SecretKey::from_bytes(&[28; 32])).local_device_id(),
    );
    let chat_id = "00000000-0000-0000-0000-000000009907";
    let workspace_dir = state.rua_root.join("workspaces").join("default");
    seed_authority_placement(&workspace_dir, chat_id, &target_device_id).await;
    seed_chat_header(&workspace_dir, chat_id, &target_device_id).await;
    let key = StreamKey::new(
        CONSOLE_CHANNEL_ID,
        ChatAddress::agent("default", chat_id),
        Priority::Normal,
    );
    let (producer_id, mut cancel_rx) = state.streams.register_active_keyed(&key).await;
    let active_turn = state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent("default".to_string(), chat_id.to_string()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: target_device_id.clone(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: None,
        })
        .await;
    let target_handler = full_command_handler(&state, &target_device_id);
    let source = build_runtime(
        29,
        network_secret,
        vec![peer_entry(28)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let target = build_runtime(
        28,
        network_secret,
        vec![peer_entry(29)],
        Some(target_handler),
        Vec::new(),
        test_config(),
    )
    .await;
    install_connectable_peer(&source, &target);

    let authority = Arc::clone(&state.conversation_authority);
    let streams = Arc::clone(&state.streams);
    let handoff_key = key.clone();
    let turn_id = active_turn.turn_id.clone();
    let handoff = tokio::spawn(async move {
        cancel_rx.changed().await.expect("stop signal");
        tokio::time::sleep(Duration::from_millis(100)).await;
        authority
            .complete_active_turn_and_drain_queue(
                streams.as_ref(),
                &handoff_key,
                &ChatAddress::agent("default", chat_id),
                &turn_id,
                "interrupted",
                Some(chrono::Utc::now().to_rfc3339()),
                None,
                false,
                |_| async {},
            )
            .await
            .expect("complete target handoff");
        streams.mark_completed(&handoff_key, producer_id).await;
    });

    let command = RemoteChatControlCommand {
        command_id: "stop-await-handoff-1".to_string(),
        agent_id: "default".to_string(),
            address: rua_core::ChatAddress::agent("default".to_string(), chat_id.to_string()),
        source_device_id: source.device_id.clone(),
        target_device_id: target.device_id.clone(),
        authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
        device_delivery_id: None,
        await_terminal: true,
        action: RemoteChatControlAction::StopCurrentTurn,
    };
    let started = Instant::now();
    let response = deliver_remote_chat_control_via_session(
        source.runtime.session_manager(),
        &command,
        Duration::from_secs(5),
    )
    .await
    .expect("confirmed target stop");
    assert!(response.applied);
    assert!(started.elapsed() >= Duration::from_millis(100));
    handoff.await.expect("handoff task");
    assert!(state
        .conversation_authority
        .active_turn(&ChatAddress::agent("default", chat_id))
        .await
        .is_none());

    let later_turn = state
        .conversation_authority
        .start_active_turn(ConversationActiveTurnStart {
            address: rua_core::ChatAddress::agent("default".to_string(), chat_id.to_string()),
            driver: ConversationDriverKind::Rua,
            authority_device_id: target_device_id,
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            driver_detail: None,
        })
        .await;
    let duplicate = deliver_remote_chat_control_via_session(
        source.runtime.session_manager(),
        &command,
        Duration::from_secs(5),
    )
    .await
    .expect("duplicate stop returns cached result");
    assert_eq!(duplicate, response);
    assert_eq!(
        state
            .conversation_authority
            .active_turn(&ChatAddress::agent("default", chat_id))
            .await
            .expect("later turn remains active")
            .turn_id,
        later_turn.turn_id
    );

    source.runtime.shutdown().await.expect("shutdown source");
    target.runtime.shutdown().await.expect("shutdown target");
}

#[derive(Debug, Default)]
struct CountingSessionHandler {
    calls: AtomicUsize,
}

#[async_trait]
impl DeviceSessionCommandHandler for CountingSessionHandler {
    async fn handle_command(&self, payload: &[u8]) -> Result<Vec<u8>, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(payload.to_vec())
    }
}

#[derive(Debug, Default)]
struct HangingSessionHandler;

#[async_trait]
impl DeviceSessionCommandHandler for HangingSessionHandler {
    async fn handle_command(&self, _payload: &[u8]) -> Result<Vec<u8>, String> {
        std::future::pending().await
    }
}

#[derive(Debug)]
struct TerminalAttachmentSessionHandler {
    payload: Vec<u8>,
    live_cursor: TerminalStreamCursor,
    chunk_attempts: Option<Arc<AtomicUsize>>,
}

#[async_trait]
impl DeviceSessionCommandHandler for TerminalAttachmentSessionHandler {
    async fn handle_command(&self, payload: &[u8]) -> Result<Vec<u8>, String> {
        let request: serde_json::Value =
            serde_json::from_slice(payload).map_err(|error| error.to_string())?;
        let action = request
            .pointer("/payload/action/action")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "terminal command action missing".to_string())?;
        let response = match action {
            "attach" => DeviceCommandResponse::Terminal {
                response: TerminalDeviceResponse::AttachedTransfer {
                    session: TerminalSessionSnapshot {
                        session_id: "terminal-session-transfer".to_string(),
                        cwd: "/remote/home".into(),
                        profile_id: "system".to_string(),
                        profile_label: "System".to_string(),
                        display_name: None,
                        viewer_count: 1,
                        input_lease_holder: None,
                        pty_geometry: TerminalPtyGeometry { cols: 80, rows: 24 },
                    },
                    snapshot_cursor: TerminalStreamCursor {
                        next_activity_sequence: 7,
                        next_output_offset: 100,
                    },
                    live_cursor: self.live_cursor,
                    transfer_id: "terminal-attachment-session-test".to_string(),
                    total_bytes: self.payload.len(),
                },
            },
            "attachment_chunk" => {
                if let Some(attempts) = &self.chunk_attempts {
                    let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                    if attempt == 0 {
                        return Err("injected attachment chunk failure".to_string());
                    }
                }
                let offset_bytes = request
                    .pointer("/payload/action/offset_bytes")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or_else(|| "attachment chunk offset missing".to_string())?
                    as usize;
                let next_offset_bytes = offset_bytes
                    .saturating_add(256 * 1024)
                    .min(self.payload.len());
                DeviceCommandResponse::Terminal {
                    response: TerminalDeviceResponse::AttachmentChunk {
                        transfer_id: "terminal-attachment-session-test".to_string(),
                        offset_bytes,
                        next_offset_bytes,
                        total_bytes: self.payload.len(),
                        data_base64: BASE64_STANDARD
                            .encode(&self.payload[offset_bytes..next_offset_bytes]),
                        done: next_offset_bytes == self.payload.len(),
                    },
                }
            }
            other => return Err(format!("unexpected terminal command action: {other}")),
        };
        serde_json::to_vec(&response).map_err(|error| error.to_string())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_live_switch_and_commands_reuse_one_device_session_generation() {
    let network_secret = [33; 32];
    let target_handler = Arc::new(CountingSessionHandler::default());
    let live_hub = Arc::new(ActiveStreamBroadcastHub::new());
    let source = build_runtime(
        34,
        network_secret,
        vec![peer_entry(35)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let target = build_runtime(
        35,
        network_secret,
        vec![peer_entry(34)],
        Some(target_handler.clone()),
        Vec::new(),
        test_config(),
    )
    .await;
    install_connectable_peer(&source, &target);
    target
        .runtime
        .session_manager()
        .set_live_activity_handler(Arc::new(ActiveStreamBroadcastDeviceSessionHandler::new(
            Arc::clone(&live_hub),
        )));

    live_hub
        .publish_conversation_event(
            "stream-a",
            &ChatAddress::agent("default", "chat-a"),
            epoch(1),
            serde_json::json!({"type": "text_delta", "delta": "A"}).to_string(),
        )
        .await;
    let (first_cancel_tx, first_cancel_rx) =
        tokio::sync::watch::channel(rua_core::RunControl::Running);
    let (first_frame_tx, mut first_frame_rx) = tokio::sync::mpsc::unbounded_channel();
    let first_manager = source.runtime.session_manager().clone();
    let first_target_device_id = target.device_id.clone();
    let first = tokio::spawn(async move {
        stream_active_broadcast_via_session(
            &first_manager,
            &first_target_device_id,
            ActiveStreamBroadcastSubscribeRequest {
                device_delivery_id: String::new(),
                owner: Some(ActiveStreamOwner::chat(ChatAddress::agent(
                    "default", "chat-a",
                ))),
                after_seq_by_stream: BTreeMap::new(),
                activity_generation: None,
                conversation_activity_position: None,
                collapse_replay: true,
                initial_conversation_control: false,
                authority_epoch: None,
                viewer: None,
            },
            Duration::from_secs(5),
            first_cancel_rx,
            move |frame| {
                let first_frame_tx = first_frame_tx.clone();
                async move {
                    let _ = first_frame_tx.send(frame);
                }
            },
        )
        .await
    });
    let first_frame = tokio::time::timeout(Duration::from_secs(5), first_frame_rx.recv())
        .await
        .expect("Chat A replay timeout")
        .expect("Chat A replay");
    assert!(matches!(
        first_frame,
        ActiveStreamBroadcastFrame::Event { ref payload, .. }
            if payload.contains("stream_replay")
    ));
    let generation = source.runtime.session_manager().diagnostics().sessions[0]
        .generation
        .expect("session generation");

    let echoed = source
        .runtime
        .session_manager()
        .command(
            &target.device_id,
            target.addr.clone(),
            b"command-during-live",
            Duration::from_secs(5),
        )
        .await
        .expect("Command over shared Session");
    assert_eq!(echoed, b"command-during-live");

    first_cancel_tx
        .send(rua_core::RunControl::Stop)
        .expect("cancel Chat A");
    assert!(matches!(
        first.await.expect("Chat A task"),
        Err(rua_server::services::active_stream_broadcast::ActiveStreamBroadcastClientError::Cancelled)
    ));

    live_hub
        .publish_conversation_event(
            "stream-b",
            &ChatAddress::agent("default", "chat-b"),
            epoch(1),
            serde_json::json!({"type": "text_delta", "delta": "B"}).to_string(),
        )
        .await;
    let (second_cancel_tx, second_cancel_rx) =
        tokio::sync::watch::channel(rua_core::RunControl::Running);
    let (second_frame_tx, mut second_frame_rx) = tokio::sync::mpsc::unbounded_channel();
    let second_manager = source.runtime.session_manager().clone();
    let second_target_device_id = target.device_id.clone();
    let second = tokio::spawn(async move {
        stream_active_broadcast_via_session(
            &second_manager,
            &second_target_device_id,
            ActiveStreamBroadcastSubscribeRequest {
                device_delivery_id: String::new(),
                owner: Some(ActiveStreamOwner::chat(ChatAddress::agent(
                    "default", "chat-b",
                ))),
                after_seq_by_stream: BTreeMap::new(),
                activity_generation: None,
                conversation_activity_position: None,
                collapse_replay: true,
                initial_conversation_control: false,
                authority_epoch: None,
                viewer: None,
            },
            Duration::from_secs(5),
            second_cancel_rx,
            move |frame| {
                let second_frame_tx = second_frame_tx.clone();
                async move {
                    let _ = second_frame_tx.send(frame);
                }
            },
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), second_frame_rx.recv())
        .await
        .expect("Chat B replay timeout")
        .expect("Chat B replay");

    let diagnostics = source.runtime.session_manager().diagnostics();
    assert_eq!(diagnostics.physical_dial_attempts, 1);
    assert_eq!(diagnostics.sessions[0].generation, Some(generation));
    assert_eq!(target_handler.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        target
            .runtime
            .session_manager()
            .diagnostics()
            .accepted_connections,
        1
    );

    second_cancel_tx
        .send(rua_core::RunControl::Stop)
        .expect("cancel Chat B");
    let _ = second.await.expect("Chat B task");
    source.runtime.shutdown().await.expect("shutdown source");
    target.runtime.shutdown().await.expect("shutdown target");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_commands_multiple_activity_streams_and_chat_reuse_one_session_generation() {
    let network_secret = [42; 32];
    let target_device_id =
        hex::encode(&iroh::SecretKey::from_bytes(&[44; 32]).public().as_bytes()[..8]);
    let terminal_dir = TempDir::new().expect("terminal dir");
    let live_hub = Arc::new(ActiveStreamBroadcastHub::new());
    let terminal_authority = Arc::new(
        TerminalSessionAuthority::new(
            TerminalAuthorityConfig {
                default_home: terminal_dir.path().to_path_buf(),
                detached_ttl: Duration::from_secs(600),
            },
            Arc::new(NativeTerminalProcessSpawner),
        )
        .with_active_stream_broadcast_hub(Arc::clone(&live_hub)),
    );
    let command_handler = Arc::new(
        DeviceCommandHandler::new(
            terminal_dir.path().join("device_commands"),
            target_device_id.clone(),
        )
        .with_terminal_authority(terminal_authority),
    );
    let source = build_runtime(
        43,
        network_secret,
        vec![peer_entry(44)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let target = build_runtime(
        44,
        network_secret,
        vec![peer_entry(43)],
        Some(command_handler),
        Vec::new(),
        test_config(),
    )
    .await;
    install_connectable_peer(&source, &target);
    target
        .runtime
        .session_manager()
        .set_live_activity_handler(Arc::new(ActiveStreamBroadcastDeviceSessionHandler::new(
            Arc::clone(&live_hub),
        )));

    let listed = deliver_terminal_device_command_via_session(
        source.runtime.session_manager(),
        &TerminalDeviceCommand {
            command_id: "terminal-list-via-session".to_string(),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            action: TerminalDeviceAction::ListSessions,
        },
        Duration::from_secs(5),
    )
    .await
    .expect("Terminal command over Device Session");
    assert!(matches!(
        listed,
        TerminalDeviceResponse::Sessions { ref sessions } if sessions.is_empty()
    ));
    let generation = source.runtime.session_manager().diagnostics().sessions[0]
        .generation
        .expect("session generation");

    for (stream_id, session_id, data) in [
        ("terminal-stream-a", "terminal-a", b"A".as_slice()),
        ("terminal-stream-b", "terminal-b", b"B".as_slice()),
    ] {
        live_hub
            .publish_terminal_event_with_replay_max_bytes(
                stream_id,
                session_id,
                terminal_activity_frame_json(&TerminalActivityFrame::Output {
                    activity_sequence: 0,
                    start_offset: 0,
                    end_offset: 1,
                    data: data.to_vec(),
                })
                .to_string(),
                1024,
            )
            .await;
    }
    live_hub
        .publish_conversation_event(
            "chat-stream",
            &ChatAddress::agent("default", "chat-a"),
            epoch(1),
            serde_json::json!({"type": "text_delta", "delta": "chat"}).to_string(),
        )
        .await;

    let mut streams = Vec::new();
    for owner in [
        ActiveStreamOwner::terminal("terminal-a"),
        ActiveStreamOwner::terminal("terminal-b"),
        ActiveStreamOwner::chat(ChatAddress::agent("default", "chat-a")),
    ] {
        let payload = serde_json::to_vec(&ActiveStreamBroadcastSubscribeRequest {
            device_delivery_id: String::new(),
            owner: Some(owner),
            after_seq_by_stream: BTreeMap::new(),
            activity_generation: None,
            conversation_activity_position: None,
            collapse_replay: false,
            initial_conversation_control: false,
            authority_epoch: None,
            viewer: None,
        })
        .expect("serialize Live Activity request");
        let mut stream = source
            .runtime
            .session_manager()
            .open_live_activity(
                &target.device_id,
                target.addr.clone(),
                &payload,
                Duration::from_secs(5),
            )
            .await
            .expect("Live Activity over Device Session");
        assert!(stream
            .next_frame()
            .await
            .expect("Live Activity frame")
            .is_some());
        streams.push(stream);
    }

    let diagnostics = source.runtime.session_manager().diagnostics();
    assert_eq!(diagnostics.physical_dial_attempts, 1);
    assert_eq!(diagnostics.sessions[0].generation, Some(generation));
    assert_eq!(diagnostics.sessions[0].active_logical_streams, 3);
    assert_eq!(
        target
            .runtime
            .session_manager()
            .diagnostics()
            .accepted_connections,
        1
    );

    drop(streams.remove(0));
    live_hub
        .publish_conversation_event(
            "chat-stream",
            &ChatAddress::agent("default", "chat-a"),
            epoch(1),
            serde_json::json!({"type": "text_delta", "delta": "chat-continues"}).to_string(),
        )
        .await;
    let chat_frame = streams
        .last_mut()
        .expect("Chat stream")
        .next_frame()
        .await
        .expect("Chat frame after Terminal stream close")
        .expect("Chat frame bytes");
    assert!(String::from_utf8_lossy(&chat_frame).contains("chat-continues"));
    let listed_after_terminal_close = deliver_terminal_device_command_via_session(
        source.runtime.session_manager(),
        &TerminalDeviceCommand {
            command_id: "terminal-list-after-stream-close".to_string(),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            action: TerminalDeviceAction::ListSessions,
        },
        Duration::from_secs(5),
    )
    .await
    .expect("Terminal command after logical stream close");
    assert!(matches!(
        listed_after_terminal_close,
        TerminalDeviceResponse::Sessions { ref sessions } if sessions.is_empty()
    ));
    let diagnostics = source.runtime.session_manager().diagnostics();
    assert_eq!(diagnostics.physical_dial_attempts, 1);
    assert_eq!(diagnostics.sessions[0].generation, Some(generation));

    drop(streams);
    source.runtime.shutdown().await.expect("shutdown source");
    target.runtime.shutdown().await.expect("shutdown target");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn terminal_activity_survives_source_device_network_runtime_replacement() {
    let network_secret = [52; 32];
    let live_hub = Arc::new(ActiveStreamBroadcastHub::new());
    let source_before = build_runtime(
        53,
        network_secret,
        vec![peer_entry(54)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let target = build_runtime(
        54,
        network_secret,
        vec![peer_entry(53)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    install_connectable_peer(&source_before, &target);
    target
        .runtime
        .session_manager()
        .set_live_activity_handler(Arc::new(ActiveStreamBroadcastDeviceSessionHandler::new(
            Arc::clone(&live_hub),
        )));
    let availability = Arc::new(
        rua_server::services::device_network_availability::DeviceNetworkAvailability::new(),
    );
    availability.mark_starting();
    let device_network = Arc::new(
        rua_server::services::device_session_access::DeviceNetworkAccess::new(
            None,
            Arc::clone(&availability),
        ),
    );
    let client = DeviceSessionTerminalRemoteActivityClient::new(Arc::clone(&device_network));
    let session_id = "terminal-runtime-replacement";
    let subscribing = client.subscribe(TerminalRemoteActivityRequest {
        source_device_id: source_before.device_id.clone(),
        target_device_id: target.device_id.clone(),
        session_id: session_id.to_string(),
        viewer_id: "browser-a".to_string(),
        retain_authority: true,
        cursor: TerminalStreamCursor {
            next_activity_sequence: 0,
            next_output_offset: 0,
        },
    });
    tokio::pin!(subscribing);
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut subscribing)
            .await
            .is_err()
    );
    *device_network.lock().await = Some(source_before.runtime.clone());
    availability.mark_ready();
    let mut activity = subscribing
        .await
        .expect("subscribe after foreground rebind");

    live_hub
        .publish_terminal_event_with_replay_max_bytes(
            &format!("terminal-activity:{session_id}"),
            session_id,
            terminal_activity_frame_json(&TerminalActivityFrame::Output {
                activity_sequence: 0,
                start_offset: 0,
                end_offset: 1,
                data: b"A".to_vec(),
            })
            .to_string(),
            1024,
        )
        .await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), activity.recv())
            .await
            .expect("first terminal frame timeout")
            .expect("first terminal frame"),
        TerminalActivityFrame::Output {
            activity_sequence: 0,
            start_offset: 0,
            end_offset: 1,
            data: b"A".to_vec(),
        }
    );

    *device_network.lock().await = None;
    source_before
        .runtime
        .shutdown()
        .await
        .expect("shutdown source runtime");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let source_after = build_runtime(
        53,
        network_secret,
        vec![peer_entry(54)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    install_connectable_peer(&source_after, &target);
    *device_network.lock().await = Some(source_after.runtime.clone());
    live_hub
        .publish_terminal_event_with_replay_max_bytes(
            &format!("terminal-activity:{session_id}"),
            session_id,
            terminal_activity_frame_json(&TerminalActivityFrame::Output {
                activity_sequence: 1,
                start_offset: 1,
                end_offset: 2,
                data: b"B".to_vec(),
            })
            .to_string(),
            1024,
        )
        .await;

    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), activity.recv())
            .await
            .expect("resumed terminal frame timeout")
            .expect("resumed terminal frame"),
        TerminalActivityFrame::Output {
            activity_sequence: 1,
            start_offset: 1,
            end_offset: 2,
            data: b"B".to_vec(),
        }
    );

    drop(activity);
    source_after
        .runtime
        .shutdown()
        .await
        .expect("shutdown replacement source runtime");
    target.runtime.shutdown().await.expect("shutdown target");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn terminal_activity_resumes_after_physical_device_session_replacement() {
    let network_secret = [55; 32];
    let live_hub = Arc::new(ActiveStreamBroadcastHub::new());
    let source = build_runtime(
        56,
        network_secret,
        vec![peer_entry(57)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let target = build_runtime(
        57,
        network_secret,
        vec![peer_entry(56)],
        Some(Arc::new(CountingSessionHandler::default())),
        Vec::new(),
        test_config(),
    )
    .await;
    install_connectable_peer(&source, &target);
    target
        .runtime
        .session_manager()
        .set_live_activity_handler(Arc::new(ActiveStreamBroadcastDeviceSessionHandler::new(
            Arc::clone(&live_hub),
        )));
    let device_network = Arc::new(
        rua_server::services::device_session_access::DeviceNetworkAccess::new(
            Some(source.runtime.clone()),
            Arc::new(
                rua_server::services::device_network_availability::DeviceNetworkAvailability::new(),
            ),
        ),
    );
    let client = DeviceSessionTerminalRemoteActivityClient::new(Arc::clone(&device_network));
    let session_id = "terminal-physical-session-replacement";
    let mut activity = client
        .subscribe(TerminalRemoteActivityRequest {
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            session_id: session_id.to_string(),
            viewer_id: "browser-a".to_string(),
            retain_authority: true,
            cursor: TerminalStreamCursor {
                next_activity_sequence: 0,
                next_output_offset: 0,
            },
        })
        .await
        .expect("subscribe terminal activity");

    live_hub
        .publish_terminal_event_with_replay_max_bytes(
            &format!("terminal-activity:{session_id}"),
            session_id,
            terminal_activity_frame_json(&TerminalActivityFrame::Output {
                activity_sequence: 0,
                start_offset: 0,
                end_offset: 1,
                data: b"A".to_vec(),
            })
            .to_string(),
            1024,
        )
        .await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), activity.recv())
            .await
            .expect("first terminal frame timeout")
            .expect("first terminal frame"),
        TerminalActivityFrame::Output {
            activity_sequence: 0,
            start_offset: 0,
            end_offset: 1,
            data: b"A".to_vec(),
        }
    );

    let replacement_source = build_runtime(
        56,
        network_secret,
        vec![peer_entry(57)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    install_connectable_peer(&replacement_source, &target);
    replacement_source
        .runtime
        .session_manager()
        .command(
            &target.device_id,
            target.addr.clone(),
            b"replace-physical-session",
            Duration::from_secs(5),
        )
        .await
        .expect("replacement connection command");

    live_hub
        .publish_terminal_event_with_replay_max_bytes(
            &format!("terminal-activity:{session_id}"),
            session_id,
            terminal_activity_frame_json(&TerminalActivityFrame::Output {
                activity_sequence: 1,
                start_offset: 1,
                end_offset: 2,
                data: b"B".to_vec(),
            })
            .to_string(),
            1024,
        )
        .await;

    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), activity.recv())
            .await
            .expect("resumed terminal frame timeout")
            .expect("resumed terminal frame"),
        TerminalActivityFrame::Output {
            activity_sequence: 1,
            start_offset: 1,
            end_offset: 2,
            data: b"B".to_vec(),
        }
    );

    drop(activity);
    replacement_source
        .runtime
        .shutdown()
        .await
        .expect("shutdown replacement source");
    source.runtime.shutdown().await.expect("shutdown source");
    target.runtime.shutdown().await.expect("shutdown target");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_attach_reassembles_oversized_snapshot_over_device_session() {
    let network_secret = [45; 32];
    let snapshot_data = format!("{}terminal-snapshot-end", "x".repeat(1024 * 1024));
    let payload = serde_json::to_vec(&serde_json::json!({
        "snapshot": {
            "data": snapshot_data,
            "cols": 80,
            "rows": 24,
            "cursor": {
                "next_activity_sequence": 7,
                "next_output_offset": 100,
            },
        },
        "tail": [],
    }))
    .expect("serialize Terminal attachment");
    assert!(payload.len() > 1024 * 1024);
    let source = build_runtime(
        46,
        network_secret,
        vec![peer_entry(47)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let target = build_runtime(
        47,
        network_secret,
        vec![peer_entry(46)],
        Some(Arc::new(TerminalAttachmentSessionHandler {
            payload,
            live_cursor: TerminalStreamCursor {
                next_activity_sequence: 7,
                next_output_offset: 100,
            },
            chunk_attempts: None,
        })),
        Vec::new(),
        test_config(),
    )
    .await;
    install_connectable_peer(&source, &target);

    let response = deliver_terminal_device_command_via_session(
        source.runtime.session_manager(),
        &TerminalDeviceCommand {
            command_id: "terminal-attach-via-session".to_string(),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            action: TerminalDeviceAction::Attach {
                session_id: "terminal-session-transfer".to_string(),
                browser_viewer_id: "browser-a".to_string(),
                scrollback_lines: 100_000,
                cursor: None,
            },
        },
        Duration::from_secs(5),
    )
    .await
    .expect("reassemble Terminal attachment over Device Session");

    match response {
        TerminalDeviceResponse::Attached {
            snapshot,
            tail,
            live_cursor,
            ..
        } => {
            assert!(snapshot.data.len() > 1024 * 1024);
            assert!(snapshot.data.ends_with("terminal-snapshot-end"));
            assert_eq!(snapshot.cursor, live_cursor);
            assert!(tail.is_empty());
        }
        other => panic!("unexpected Terminal response: {other:?}"),
    }

    assert_eq!(
        source
            .runtime
            .session_manager()
            .diagnostics()
            .physical_dial_attempts,
        1
    );
    source.runtime.shutdown().await.expect("shutdown source");
    target.runtime.shutdown().await.expect("shutdown target");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_attachment_chunk_retry_preserves_positioned_tail_over_device_session() {
    let network_secret = [48; 32];
    let expected_tail = vec![
        TerminalActivityFrame::Output {
            activity_sequence: 7,
            start_offset: 100,
            end_offset: 104,
            data: vec![0xff, 0x00, 0x01, 0x1b],
        },
        TerminalActivityFrame::PtyGeometryChanged {
            activity_sequence: 8,
            geometry: TerminalPtyGeometry {
                cols: 120,
                rows: 40,
            },
        },
    ];
    let payload = serde_json::to_vec(&serde_json::json!({
        "snapshot": {
            "data": "serialized-snapshot",
            "cols": 80,
            "rows": 24,
            "cursor": {
                "next_activity_sequence": 7,
                "next_output_offset": 100,
            },
        },
        "tail": expected_tail,
    }))
    .expect("serialize Terminal attachment");
    let chunk_attempts = Arc::new(AtomicUsize::new(0));
    let source = build_runtime(
        49,
        network_secret,
        vec![peer_entry(50)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let target = build_runtime(
        50,
        network_secret,
        vec![peer_entry(49)],
        Some(Arc::new(TerminalAttachmentSessionHandler {
            payload,
            live_cursor: TerminalStreamCursor {
                next_activity_sequence: 9,
                next_output_offset: 104,
            },
            chunk_attempts: Some(Arc::clone(&chunk_attempts)),
        })),
        Vec::new(),
        test_config(),
    )
    .await;
    install_connectable_peer(&source, &target);

    let response = deliver_terminal_device_command_via_session(
        source.runtime.session_manager(),
        &TerminalDeviceCommand {
            command_id: "terminal-attach-retry-via-session".to_string(),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            action: TerminalDeviceAction::Attach {
                session_id: "terminal-session-transfer".to_string(),
                browser_viewer_id: "browser-b".to_string(),
                scrollback_lines: 5_000,
                cursor: None,
            },
        },
        Duration::from_secs(5),
    )
    .await
    .expect("reassemble retried Terminal attachment");

    match response {
        TerminalDeviceResponse::Attached {
            snapshot,
            tail,
            live_cursor,
            ..
        } => {
            assert_eq!(snapshot.data, "serialized-snapshot");
            assert_eq!(
                snapshot.cursor,
                TerminalStreamCursor {
                    next_activity_sequence: 7,
                    next_output_offset: 100,
                }
            );
            assert_eq!(tail, expected_tail);
            assert_eq!(
                live_cursor,
                TerminalStreamCursor {
                    next_activity_sequence: 9,
                    next_output_offset: 104,
                }
            );
        }
        other => panic!("unexpected Terminal response: {other:?}"),
    }
    assert_eq!(chunk_attempts.load(Ordering::SeqCst), 2);
    assert_eq!(
        source
            .runtime
            .session_manager()
            .diagnostics()
            .physical_dial_attempts,
        1
    );

    source.runtime.shutdown().await.expect("shutdown source");
    target.runtime.shutdown().await.expect("shutdown target");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_live_reconnect_resumes_after_last_observed_sequence() {
    assert_chat_live_reconnect_resumes_after_last_observed_sequence(
        ChatAddress::agent("default", "chat-reconnect"),
        36,
        37,
        38,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_chat_live_reconnect_resumes_after_last_observed_sequence() {
    assert_chat_live_reconnect_resumes_after_last_observed_sequence(
        ChatAddress::project("project-reconnect", "chat-reconnect"),
        39,
        40,
        41,
    )
    .await;
}

async fn assert_chat_live_reconnect_resumes_after_last_observed_sequence(
    address: ChatAddress,
    network_secret_byte: u8,
    source_seed: u8,
    target_seed: u8,
) {
    let network_secret = [network_secret_byte; 32];
    let live_hub = Arc::new(ActiveStreamBroadcastHub::new());
    let source = build_runtime(
        source_seed,
        network_secret,
        vec![peer_entry(target_seed)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let target = build_runtime(
        target_seed,
        network_secret,
        vec![peer_entry(source_seed)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    install_connectable_peer(&source, &target);
    target
        .runtime
        .session_manager()
        .set_live_activity_handler(Arc::new(ActiveStreamBroadcastDeviceSessionHandler::new(
            Arc::clone(&live_hub),
        )));
    live_hub
        .publish_conversation_event(
            "stream-reconnect",
            &address,
            epoch(1),
            serde_json::json!({"type": "text_delta", "delta": "first"}).to_string(),
        )
        .await;

    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(rua_core::RunControl::Running);
    let (frame_tx, mut frame_rx) = tokio::sync::mpsc::unbounded_channel();
    let manager = source.runtime.session_manager().clone();
    let target_device_id = target.device_id.clone();
    let viewer_address = address.clone();
    let viewer = tokio::spawn(async move {
        stream_active_broadcast_via_session(
            &manager,
            &target_device_id,
            ActiveStreamBroadcastSubscribeRequest {
                device_delivery_id: String::new(),
                owner: Some(ActiveStreamOwner::chat(viewer_address)),
                after_seq_by_stream: BTreeMap::new(),
                activity_generation: None,
                conversation_activity_position: None,
                collapse_replay: false,
                initial_conversation_control: false,
                authority_epoch: None,
                viewer: None,
            },
            Duration::from_secs(5),
            cancel_rx,
            move |frame| {
                let frame_tx = frame_tx.clone();
                async move {
                    let _ = frame_tx.send(frame);
                }
            },
        )
        .await
    });

    let first = tokio::time::timeout(Duration::from_secs(5), frame_rx.recv())
        .await
        .expect("first frame timeout")
        .expect("first frame");
    assert!(matches!(
        first,
        ActiveStreamBroadcastFrame::Event {
            seq: 1,
            ref payload,
            ..
        } if payload.contains("first")
    ));

    source
        .runtime
        .session_manager()
        .disconnect_peer(&target.device_id)
        .await
        .expect("disconnect Session");
    live_hub
        .publish_conversation_event(
            "stream-reconnect",
            &address,
            epoch(1),
            serde_json::json!({"type": "text_delta", "delta": "second"}).to_string(),
        )
        .await;

    let second = tokio::time::timeout(Duration::from_secs(5), frame_rx.recv())
        .await
        .expect("resumed frame timeout")
        .expect("resumed frame");
    assert!(matches!(
        second,
        ActiveStreamBroadcastFrame::Event {
            seq: 2,
            ref payload,
            ..
        } if payload.contains("second")
    ));
    assert_eq!(
        source
            .runtime
            .session_manager()
            .diagnostics()
            .physical_dial_attempts,
        2
    );

    cancel_tx
        .send(rua_core::RunControl::Stop)
        .expect("cancel viewer");
    let _ = viewer.await.expect("viewer task");
    source.runtime.shutdown().await.expect("shutdown source");
    target.runtime.shutdown().await.expect("shutdown target");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn collapsed_chat_replay_preserves_stream_boundaries() {
    let network_secret = [46; 32];
    let live_hub = Arc::new(ActiveStreamBroadcastHub::new());
    let source = build_runtime(
        47,
        network_secret,
        vec![peer_entry(48)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let target = build_runtime(
        48,
        network_secret,
        vec![peer_entry(47)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    install_connectable_peer(&source, &target);
    target
        .runtime
        .session_manager()
        .set_live_activity_handler(Arc::new(ActiveStreamBroadcastDeviceSessionHandler::new(
            Arc::clone(&live_hub),
        )));
    for stream_id in ["stream-a", "stream-b"] {
        live_hub
            .publish_conversation_event(
                stream_id,
                &ChatAddress::agent("default", "chat-collapse"),
                epoch(1),
                serde_json::json!({"type": "text_delta", "delta": stream_id}).to_string(),
            )
            .await;
    }

    let payload = serde_json::to_vec(&ActiveStreamBroadcastSubscribeRequest {
        device_delivery_id: String::new(),
        owner: Some(ActiveStreamOwner::chat(ChatAddress::agent(
            "default",
            "chat-collapse",
        ))),
        after_seq_by_stream: BTreeMap::new(),
        activity_generation: None,
        conversation_activity_position: None,
        collapse_replay: true,
        initial_conversation_control: false,
        authority_epoch: None,
        viewer: None,
    })
    .expect("serialize Live Activity request");
    let mut stream = source
        .runtime
        .session_manager()
        .open_live_activity_to(&target.device_id, &payload, Duration::from_secs(5))
        .await
        .expect("open collapsed replay");

    let mut replay_stream_ids = BTreeSet::new();
    for _ in 0..2 {
        let bytes = tokio::time::timeout(Duration::from_secs(2), stream.next_frame())
            .await
            .expect("collapsed replay frame timeout")
            .expect("collapsed replay frame")
            .expect("collapsed replay stream remains open");
        let frame: ActiveStreamBroadcastFrame =
            serde_json::from_slice(&bytes).expect("decode collapsed replay frame");
        let ActiveStreamBroadcastFrame::Event {
            device_delivery_id, ..
        } = frame
        else {
            panic!("unexpected collapsed replay frame");
        };
        replay_stream_ids.insert(device_delivery_id);
    }
    assert_eq!(
        replay_stream_ids,
        BTreeSet::from(["stream-a".to_string(), "stream-b".to_string()])
    );

    source.runtime.shutdown().await.expect("shutdown source");
    target.runtime.shutdown().await.expect("shutdown target");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chat_live_wake_gossip_bridge_does_not_open_device_sessions() {
    let network_secret = [39; 32];
    let target = build_runtime(
        40,
        network_secret,
        vec![peer_entry(41)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let source = build_runtime(
        41,
        network_secret,
        vec![peer_entry_with_direct_addrs(40, direct_addrs(&target.addr))],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let activity_hub = Arc::new(ActiveStreamBroadcastHub::new());
    let wake_hub = ChatActivityWakeHub::new();
    let source_runtime = Arc::new(
        rua_server::services::device_session_access::DeviceNetworkAccess::new(
            Some(source.runtime.clone()),
            Arc::new(
                rua_server::services::device_network_availability::DeviceNetworkAvailability::new(),
            ),
        ),
    );
    let secret_cell: rua_server::services::chat_activity_wake::SyncNetworkSecretCell =
        Arc::new(std::sync::RwLock::new(Some(Arc::new(
            rua_sync::identity::NetworkSecret::from_bytes(&network_secret),
        ))));
    let (publisher_shutdown, publisher_shutdown_rx) = tokio::sync::watch::channel(false);
    spawn_chat_activity_gossip_publisher(
        Arc::clone(&activity_hub),
        source_runtime,
        Arc::clone(&secret_cell),
        publisher_shutdown_rx,
    );
    spawn_chat_activity_gossip_listener(
        target.runtime.clone(),
        Arc::clone(&wake_hub),
        Arc::clone(&secret_cell),
    );
    let mut wakes = wake_hub.subscribe();

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if source.runtime.gossip_bus().neighbor_count() > 0
                && target.runtime.gossip_bus().neighbor_count() > 0
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("Gossip peers should connect");
    tokio::time::sleep(Duration::from_millis(50)).await;
    activity_hub
        .publish_conversation_event(
            "stream-gossip",
            &ChatAddress::agent("default", "chat-gossip"),
            epoch(9),
            serde_json::json!({"type": "text_delta", "delta": "wake"}).to_string(),
        )
        .await;

    let wake = tokio::time::timeout(Duration::from_secs(5), wakes.recv())
        .await
        .expect("Chat wake timeout")
        .expect("Chat wake");
    assert_eq!(wake.address.chat_id().as_str(), "chat-gossip");
    assert_eq!(wake.authority_device_id, source.device_id);
    assert_eq!(wake.authority_epoch, epoch(9));
    let source_diagnostics = source.runtime.session_manager().diagnostics();
    let target_diagnostics = target.runtime.session_manager().diagnostics();
    assert_eq!(source_diagnostics.physical_dial_attempts, 0);
    assert_eq!(target_diagnostics.physical_dial_attempts, 0);
    assert_eq!(source_diagnostics.sessions[0].generation, None);
    assert_eq!(target_diagnostics.sessions[0].generation, None);
    assert_eq!(
        source_diagnostics.sessions[0].state,
        rua_device_network::DeviceSessionState::NotConnected
    );
    assert_eq!(
        target_diagnostics.sessions[0].state,
        rua_device_network::DeviceSessionState::NotConnected
    );

    publisher_shutdown
        .send(true)
        .expect("stop activity publisher");
    source.runtime.shutdown().await.expect("shutdown source");
    target.runtime.shutdown().await.expect("shutdown target");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authority_takeover_broadcasts_placement_changed_over_gossip() {
    let network_secret = [43; 32];
    let target = build_runtime(
        44,
        network_secret,
        vec![peer_entry(45)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let source = build_runtime(
        45,
        network_secret,
        vec![peer_entry_with_direct_addrs(44, direct_addrs(&target.addr))],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let wake_hub = ChatActivityWakeHub::new();
    let secret_cell: rua_server::services::chat_activity_wake::SyncNetworkSecretCell =
        Arc::new(std::sync::RwLock::new(Some(Arc::new(
            rua_sync::identity::NetworkSecret::from_bytes(&network_secret),
        ))));
    spawn_chat_activity_gossip_listener(
        target.runtime.clone(),
        Arc::clone(&wake_hub),
        Arc::clone(&secret_cell),
    );
    let mut wakes = wake_hub.subscribe();

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if source.runtime.gossip_bus().neighbor_count() > 0
                && target.runtime.gossip_bus().neighbor_count() > 0
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("Gossip peers should connect");
    tokio::time::sleep(Duration::from_millis(50)).await;

    let base_manifest_hash = format!("sha256:{}", "0".repeat(64));
    broadcast_placement_changed(
        &source.runtime,
        &secret_cell,
        ChatAddress::agent("default", "chat-takeover"),
        rua_server::services::conversation_authority_placement::ConversationAuthorityPlacement {
            device_id: source.device_id.clone(),
            authority_epoch: epoch(12),
            base_manifest_hash: base_manifest_hash.clone(),
        },
    )
    .await;

    let wake = tokio::time::timeout(Duration::from_secs(5), wakes.recv())
        .await
        .expect("placement wake timeout")
        .expect("placement wake");
    assert_eq!(wake.phase, "placement_changed");
    assert_eq!(wake.address.chat_id().as_str(), "chat-takeover");
    assert_eq!(wake.authority_device_id, source.device_id);
    assert_eq!(wake.authority_epoch, epoch(12));
    assert_eq!(wake.base_manifest_hash, Some(base_manifest_hash));

    source.runtime.shutdown().await.expect("shutdown source");
    target.runtime.shutdown().await.expect("shutdown target");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn timed_out_chat_command_is_bounded() {
    let network_secret = [30; 32];
    let source = build_runtime(
        31,
        network_secret,
        vec![peer_entry(32)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let target = build_runtime(
        32,
        network_secret,
        vec![peer_entry(31)],
        Some(Arc::new(HangingSessionHandler)),
        Vec::new(),
        test_config(),
    )
    .await;
    install_connectable_peer(&source, &target);

    let started = Instant::now();
    let error = deliver_conversation_control_via_session(
        source.runtime.session_manager(),
        &ConversationControlDeviceCommand {
            command_id: "control-view-timeout".to_string(),
            agent_id: "default".to_string(),
            address: rua_core::ChatAddress::agent("default".to_string(), "chat-a".to_string()),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            action: ConversationControlDeviceAction::View,
        },
        Duration::from_millis(100),
    )
    .await
    .expect_err("command must time out");

    assert!(matches!(
        error,
        DeviceCommandDeliveryError::Session(rua_device_network::DeviceSessionError::Timeout { .. })
    ));
    assert!(started.elapsed() < Duration::from_secs(2));

    source.runtime.shutdown().await.expect("shutdown source");
    target.runtime.shutdown().await.expect("shutdown target");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn version_incompatible_control_view_fails_explicitly_without_retry() {
    let network_secret = [24; 32];
    let session_handler = Arc::new(CountingSessionHandler::default());
    let source = build_runtime(
        25,
        network_secret,
        vec![peer_entry(26)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    let mut incompatible = test_config();
    incompatible.protocol_version = 4;
    let target = build_runtime(
        26,
        network_secret,
        vec![peer_entry(25)],
        Some(session_handler.clone()),
        Vec::new(),
        incompatible,
    )
    .await;
    install_connectable_peer(&source, &target);

    let error = deliver_conversation_control_via_session(
        source.runtime.session_manager(),
        &ConversationControlDeviceCommand {
            command_id: "control-view-version-mismatch".to_string(),
            agent_id: "default".to_string(),
            address: rua_core::ChatAddress::agent("default".to_string(), "chat-a".to_string()),
            source_device_id: source.device_id.clone(),
            target_device_id: target.device_id.clone(),
            authority_epoch: rua_server::services::conversation_authority_placement::ConversationAuthorityEpoch::from_parts(1, 0, [0; 8]),
            action: ConversationControlDeviceAction::View,
        },
        Duration::from_secs(5),
    )
    .await
    .expect_err("version mismatch");

    assert!(error.is_session_version_incompatible());
    assert_eq!(DEVICE_SESSION_PROTOCOL_VERSION, 18);
    assert_eq!(session_handler.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        source
            .runtime
            .session_manager()
            .diagnostics()
            .physical_dial_attempts,
        1
    );

    source.runtime.shutdown().await.expect("shutdown source");
    target.runtime.shutdown().await.expect("shutdown target");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn user_notification_crosses_device_session_and_is_presented_once() {
    let network_secret = [55; 32];
    let target_device_id = hex::encode(
        DeviceNetworkIdentity::new(iroh::SecretKey::from_bytes(&[57; 32])).local_device_id(),
    );
    let target_adapter = Arc::new(SessionNotificationAdapter::default());
    let target_service = Arc::new(UserNotificationService::new(
        target_device_id.clone(),
        Arc::clone(&target_adapter) as Arc<dyn LocalUserNotificationAdapter>,
        Arc::new(NeverNotificationTransport),
    ));
    let target_handler = Arc::new(
        DeviceCommandHandler::new(
            std::env::temp_dir().join("rua-user-notification-target"),
            target_device_id,
        )
        .with_user_notifications(target_service),
    );
    let target = build_runtime(
        57,
        network_secret,
        vec![peer_entry(56)],
        Some(target_handler),
        Vec::new(),
        test_config(),
    )
    .await;
    let source = build_runtime(
        56,
        network_secret,
        vec![peer_entry(57)],
        None,
        Vec::new(),
        test_config(),
    )
    .await;
    install_connectable_peer(&source, &target);

    let source_adapter = Arc::new(SessionNotificationAdapter::default());
    let session_manager = Arc::new(std::sync::RwLock::new(Some(
        source.runtime.session_manager().clone(),
    )));
    let source_service = UserNotificationService::new(
        source.device_id.clone(),
        Arc::clone(&source_adapter) as Arc<dyn LocalUserNotificationAdapter>,
        Arc::new(DeviceSessionUserNotificationTransport::new(
            session_manager,
            Duration::from_secs(5),
        )),
    );
    let expected = UserNotificationRequest {
        target_device_id: target.device_id.clone(),
        sender: UserNotificationSender {
            id: "agent:researcher".to_string(),
            name: "Research Agent".to_string(),
        },
        title: "Run complete".to_string(),
        body: Some("The requested analysis is ready.".to_string()),
        rua_uri: Some("rua://agents/researcher/chats/chat-42?turn=7#result".to_string()),
    };

    source_service.send(expected.clone()).await;

    assert_eq!(source_adapter.calls.load(Ordering::SeqCst), 0);
    assert_eq!(target_adapter.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        target_adapter
            .requests
            .lock()
            .expect("notification lock")
            .as_slice(),
        &[expected]
    );

    source.runtime.shutdown().await.expect("shutdown source");
    target.runtime.shutdown().await.expect("shutdown target");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shared_runtime_access_waits_for_readiness_and_wakes_all_consumers() {
    use rua_server::services::device_network_availability::DeviceNetworkAvailability;
    use rua_server::services::device_session_access::DeviceNetworkAccess;
    let availability = Arc::new(DeviceNetworkAvailability::new());
    availability.mark_starting();
    let access = Arc::new(DeviceNetworkAccess::new(None, Arc::clone(&availability)));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut waiters = Vec::new();
    for _ in 0..8 {
        let access = Arc::clone(&access);
        waiters.push(tokio::spawn(async move {
            access.session_manager(deadline).await
        }));
    }
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(waiters.iter().all(|waiter| !waiter.is_finished()));
    // One caller's deadline must not cancel initialization or the other consumers.
    assert!(access
        .session_manager(tokio::time::Instant::now() + Duration::from_millis(10))
        .await
        .err()
        .unwrap()
        .contains("readiness timed out"));
    let runtime = build_runtime(61, [62; 32], Vec::new(), None, Vec::new(), test_config()).await;
    *access.lock().await = Some(runtime.runtime.clone());
    availability.mark_ready();
    for waiter in waiters {
        let manager = waiter.await.unwrap().expect("shared runtime becomes ready");
        assert_eq!(manager.diagnostics().physical_dial_attempts, 0);
    }
    runtime.runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn shared_runtime_access_reports_failure_and_suspension_without_polling() {
    use rua_server::services::device_network_availability::DeviceNetworkAvailability;
    use rua_server::services::device_session_access::DeviceNetworkAccess;
    let availability = Arc::new(DeviceNetworkAvailability::new());
    let access = DeviceNetworkAccess::new(None, Arc::clone(&availability));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    assert!(access
        .session_manager(deadline)
        .await
        .err()
        .unwrap()
        .contains("suspended"));
    availability.mark_starting();
    let waiter = access.session_manager(deadline);
    tokio::pin!(waiter);
    assert!(tokio::time::timeout(Duration::from_millis(20), &mut waiter)
        .await
        .is_err());
    availability.mark_failed("bind failed");
    assert!(waiter.await.err().unwrap().contains("bind failed"));
}

struct InputConfirmationHandler {
    inner: Arc<DeviceCommandHandler>,
    before_acceptance: Duration,
    hold_reply: bool,
    accepted: Notify,
    release_reply: Notify,
    requests: Mutex<Vec<RemoteConversationInputRequest>>,
}

#[async_trait]
impl DeviceSessionCommandHandler for InputConfirmationHandler {
    async fn handle_command(&self, payload: &[u8]) -> Result<Vec<u8>, String> {
        let envelope: serde_json::Value =
            serde_json::from_slice(payload).map_err(|error| error.to_string())?;
        if envelope["command"] == "conversation_input" {
            let request = serde_json::from_value(envelope["payload"].clone())
                .map_err(|error| error.to_string())?;
            self.requests.lock().unwrap().push(request);
            tokio::time::sleep(self.before_acceptance).await;
            let response = self.inner.handle_command(payload).await?;
            if matches!(
                serde_json::from_slice::<DeviceCommandResponse>(&response),
                Ok(DeviceCommandResponse::Accepted(_))
            ) {
                self.accepted.notify_one();
                if self.hold_reply {
                    self.release_reply.notified().await;
                }
            }
            Ok(response)
        } else {
            self.inner.handle_command(payload).await
        }
    }
}

struct RemoteInputConfirmationFixture {
    _source_dir: TempDir,
    _target_dir: TempDir,
    source_state: AppState,
    target_state: AppState,
    source: TestRuntime,
    target: TestRuntime,
    handler: Arc<InputConfirmationHandler>,
    chat_id: String,
    submission_id: String,
}

impl RemoteInputConfirmationFixture {
    async fn new(before_acceptance: Duration, hold_reply: bool) -> Self {
        let source_dir = TempDir::new().unwrap();
        let target_dir = TempDir::new().unwrap();
        let source_state = AppState::from_tempdir(source_dir.path().into(), ServerProfile::Test);
        let target_state = AppState::from_tempdir(target_dir.path().into(), ServerProfile::Test);
        install_test_device(&source_state, 171);
        install_test_device(&target_state, 172);
        let captured = CapturingAgentRuntime::default();
        for state in [&source_state, &target_state] {
            state
                .registry
                .get_or_create("default")
                .await
                .unwrap()
                .set_react_runtime(Arc::new(captured.clone()))
                .await;
            seed_fixture_provider(state, vec![fixture_provider_model("model-a")]).await;
            state
                .agent_service
                .get_agent("default")
                .await
                .unwrap()
                .unwrap();
        }
        let target_id = hex::encode(
            DeviceNetworkIdentity::new(iroh::SecretKey::from_bytes(&[172; 32])).local_device_id(),
        );
        let handler = Arc::new(InputConfirmationHandler {
            inner: full_command_handler(&target_state, &target_id),
            before_acceptance,
            hold_reply,
            accepted: Notify::new(),
            release_reply: Notify::new(),
            requests: Mutex::new(Vec::new()),
        });
        let source = build_runtime(
            171,
            [173; 32],
            vec![peer_entry(172)],
            None,
            Vec::new(),
            test_config(),
        )
        .await;
        let target = build_runtime(
            172,
            [173; 32],
            vec![peer_entry(171)],
            Some(handler.clone()),
            Vec::new(),
            test_config(),
        )
        .await;
        install_connectable_peer(&source, &target);
        *source_state.device_network.lock().await = Some(source.runtime.clone());
        *target_state.device_network.lock().await = Some(target.runtime.clone());
        Self {
            _source_dir: source_dir,
            _target_dir: target_dir,
            source_state,
            target_state,
            source,
            target,
            handler,
            chat_id: uuid::Uuid::new_v4().to_string(),
            submission_id: uuid::Uuid::new_v4().to_string(),
        }
    }

    fn submit(&self) -> impl std::future::Future<Output = serde_json::Value> + Send + 'static {
        let app = rua_server::router::build_app(self.source_state.clone(), None);
        let chat_id = self.chat_id.clone();
        let body = serde_json::json!({
            "message": "one diagnostic input",
            "submission_id": self.submission_id,
            "target_device_id": self.target.device_id,
            "chat_model_slot": {"provider_id":"fixture","model":"model-a"},
        });
        async move {
            let response = app
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(format!(
                            "/api/agents/default/chats/{chat_id}/console/submit"
                        ))
                        .header("content-type", "application/json")
                        .body(Body::from(serde_json::to_vec(&body).unwrap()))
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = response.status();
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            assert_eq!(
                status,
                StatusCode::OK,
                "{}",
                String::from_utf8_lossy(&bytes)
            );
            serde_json::from_slice(&bytes).unwrap()
        }
    }

    async fn persisted_users(&self) -> Vec<rua_session::MessageLogEntry> {
        let workspace = self
            .target_state
            .registry
            .get_or_create("default")
            .await
            .unwrap()
            .workspace_dir();
        let store = WorkspaceSessionStore::new(workspace);
        let turns = store
            .read_turn_range(&self.chat_id, 1, usize::MAX)
            .await
            .unwrap();
        let mut users = Vec::new();
        for turn in turns {
            users.extend(
                store
                    .read_turn_messages(&self.chat_id, &turn.turn_id)
                    .await
                    .unwrap()
                    .into_iter()
                    .filter(|message| message.role == "user"),
            );
        }
        users
    }

    async fn shutdown(&self) {
        self.handler.release_reply.notify_one();
        self.source.runtime.shutdown().await.unwrap();
        self.target.runtime.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_input_confirmation_waits_for_slow_admission_without_replay() {
    let fixture = RemoteInputConfirmationFixture::new(Duration::from_secs(12), false).await;
    let outcome = fixture.submit().await;
    fixture.shutdown().await;
    assert_eq!(
        outcome["accepted"], true,
        "slow admission must retain its bounded input confirmation window: {outcome}"
    );
    assert_eq!(outcome["submission_id"], fixture.submission_id);
    assert_eq!(fixture.handler.requests.lock().unwrap().len(), 1);
    assert_eq!(fixture.persisted_users().await.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_input_confirmation_does_not_wait_for_post_commit_sync_lock() {
    let fixture = RemoteInputConfirmationFixture::new(Duration::ZERO, false).await;
    fixture
        .target_state
        .sync_enabled
        .store(true, Ordering::Relaxed);
    let sync_guard = fixture.target_state.sync_node.lock().await;
    let mut submitting = tokio::spawn(fixture.submit());
    tokio::time::timeout(Duration::from_secs(5), async {
        while fixture.persisted_users().await.is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the authority must durably accept before testing publication blockage");
    let before_sync = tokio::time::timeout(Duration::from_secs(2), &mut submitting).await;
    if before_sync.is_ok() {
        let runtime = fixture.target_state.registry.get("default").await.unwrap();
        assert!(runtime
            .task_tracker()
            .active_task_names()
            .await
            .iter()
            .any(|name| name == "Publish accepted remote input"));
        assert!(runtime.task_tracker().cancel_all().await > 0);
        assert!(
            runtime.task_tracker().drain(Duration::from_secs(1)).await,
            "Agent cancellation must remove a publication still blocked on Sync"
        );
    }
    drop(sync_guard);
    let acknowledged_before_sync = before_sync.is_ok();
    let outcome = match before_sync {
        Ok(value) => value.unwrap(),
        Err(_) => submitting.await.unwrap(),
    };
    fixture.shutdown().await;
    assert!(
        acknowledged_before_sync,
        "durable acceptance must be acknowledged without waiting for Sync: {outcome}"
    );
    assert_eq!(outcome["accepted"], true);
    assert_eq!(fixture.persisted_users().await.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_input_confirmation_lost_ack_is_unconfirmed_with_original_ids() {
    let fixture = RemoteInputConfirmationFixture::new(Duration::ZERO, true).await;
    let submitting = tokio::spawn(fixture.submit());
    tokio::time::timeout(Duration::from_secs(5), fixture.handler.accepted.notified())
        .await
        .expect("actual authority must commit and build Accepted before the reply is withheld");
    let users = fixture.persisted_users().await;
    assert_eq!(users.len(), 1);
    let outcome = tokio::time::timeout(Duration::from_secs(35), submitting)
        .await
        .expect("input confirmation remains bounded")
        .unwrap();
    let request = fixture.handler.requests.lock().unwrap()[0].clone();
    fixture.shutdown().await;
    assert_eq!(
        outcome["mode"], "unconfirmed",
        "a missing acceptance reply cannot prove rejection: {outcome}"
    );
    assert_eq!(outcome["accepted"], false);
    assert_eq!(outcome["failure_kind"], "delivery_unknown");
    assert_eq!(outcome["chat_id"], fixture.chat_id);
    assert_eq!(outcome["submission_id"], request.submission_id);
    assert_eq!(outcome["device_delivery_id"], request.device_delivery_id);
    assert_eq!(
        outcome["device_delivery_sequence"],
        request.device_delivery_sequence
    );
    assert_eq!(outcome["target_device_id"], request.target_device_id);
    assert!(outcome["input_id"].is_null());
    assert!(outcome["accepted_at"].is_null());
    assert_eq!(
        fixture.handler.requests.lock().unwrap().len(),
        1,
        "transport uncertainty must not replay the input"
    );
    assert_eq!(fixture.persisted_users().await.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_input_confirmation_publication_timeout_clears_tracker() {
    let fixture = RemoteInputConfirmationFixture::new(Duration::ZERO, false).await;
    fixture
        .target_state
        .sync_enabled
        .store(true, Ordering::Relaxed);
    let sync_guard = fixture.target_state.sync_node.lock().await;
    let outcome = tokio::time::timeout(Duration::from_secs(5), fixture.submit())
        .await
        .expect("publication cannot hold the committed input acknowledgement");
    assert_eq!(outcome["accepted"], true);
    let runtime = fixture.target_state.registry.get("default").await.unwrap();
    assert!(runtime
        .task_tracker()
        .active_task_names()
        .await
        .iter()
        .any(|name| name == "Publish accepted remote input"));
    assert!(
        runtime.task_tracker().drain(Duration::from_secs(32)).await,
        "publication expiry must remove its tracked task while Sync is still blocked"
    );
    drop(sync_guard);
    fixture.shutdown().await;
    assert_eq!(fixture.persisted_users().await.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_input_confirmation_same_prepared_attempt_reuses_cached_acceptance() {
    use rua_server::services::remote_conversation_input::{
        RemoteConversationInputDelivery, RemoteConversationInputParams,
    };
    let fixture = RemoteInputConfirmationFixture::new(Duration::ZERO, false).await;
    let delivery = RemoteConversationInputDelivery::new(
        Arc::clone(&fixture.source_state.sync_node),
        Arc::clone(&fixture.source_state.device_network),
        fixture.source_state.rua_root.clone(),
    );
    let address = ChatAddress::agent("default", &fixture.chat_id);
    let manifest =
        rua_server::services::conversation_manifest::initial_conversation_manifest_hash(&address)
            .unwrap();
    let request = ConsoleChatRequest {
        message: "one prepared attempt".to_string(),
        chat_id: Some(fixture.chat_id.clone()),
        model: None,
        is_reconnect: false,
        attachments: Vec::new(),
        input: None,
        messages: None,
        extra: serde_json::Map::from_iter([
            (
                "submission_id".to_string(),
                serde_json::json!(fixture.submission_id),
            ),
            (
                "chat_model_slot".to_string(),
                serde_json::json!({"provider_id":"fixture","model":"model-a"}),
            ),
        ]),
        channel_kind: None,
    };
    let prepared = delivery
        .build_remote_conversation_input_request(&RemoteConversationInputParams {
            address: &address,
            execution_agent_id: "default",
            request: &request,
            source_device_id: &fixture.source.device_id,
            target_device_id: &fixture.target.device_id,
            authority_epoch: epoch(1),
            base_manifest_hash: &manifest,
            workspace_dir: None,
            materialize_conversation: true,
        })
        .await
        .unwrap();
    let first = delivery
        .deliver_prepared_conversation_input(&prepared, "same attempt first observation")
        .await
        .unwrap();
    let second = delivery
        .deliver_prepared_conversation_input(&prepared, "same attempt repeated observation")
        .await;
    fixture.shutdown().await;
    let second = second.expect("the same prepared attempt must retain byte-identical wire payload and reuse its cached receipt");
    assert_eq!(first, second);
    assert_eq!(fixture.persisted_users().await.len(), 1);
    let requests = fixture.handler.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0], requests[1]);
}
