use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures::stream::StreamExt;
use rua_channel::core::console::SseOutputStream;
use rua_core::{ChatAddress, ConversationActivityPosition};

use crate::services::active_stream_broadcast::{
    collapse_replay_frames, stream_active_broadcast_via_session, stream_replay_payloads,
    ActiveStreamBroadcastClientError, ActiveStreamBroadcastFrame, ActiveStreamBroadcastHub,
    ActiveStreamBroadcastSubscribeRequest, ActiveStreamBroadcastSubscription,
    ActiveStreamBroadcastViewer, ActiveStreamOwner,
};
use crate::services::chat_activity_wake::ChatActivityWakeHub;
use crate::services::conversation_authority::ConversationAuthority;
use crate::services::conversation_authority_placement::{
    ConversationAuthorityEpoch, ConversationAuthorityPlacement,
};
use crate::services::conversation_authority_router::{
    ConversationAuthorityRouter, ConversationLiveActivityRoute,
    ConversationLiveActivityRouteRequest,
};
use crate::services::conversation_history_readiness::{
    ensure_conversation_history_ready, ConversationHistoryReadinessRequest,
    ConversationHistorySyncRequest,
};
use crate::services::conversation_manifest::ConversationManifest;
use crate::services::device_command_bus::{
    deliver_conversation_manifest_command_via_session, ConversationManifestDeviceCommand,
};
use crate::services::device_context::current_sync_device_id;
use crate::services::device_session_access::current_session_manager;
use crate::services::foreground_operation::ForegroundOperationKind;
use crate::streaming::{ActiveStreamRegistry, Priority, StreamKey};

type LiveActivityAuthorityMap = Arc<tokio::sync::RwLock<HashMap<ChatAddress, String>>>;
const REMOTE_LIVE_ATTACH_TIMEOUT: Duration = Duration::from_secs(10);

struct RemoteDiscoveryStream {
    target_device_id: String,
    stream: SseOutputStream,
}

#[derive(Clone)]
pub struct ConversationLiveActivity {
    streams: Arc<ActiveStreamRegistry>,
    registry: Arc<dyn rua_workspace::WorkspaceRegistry>,
    sync_device: Arc<std::sync::RwLock<Option<Arc<rua_sync::identity::DeviceIdentity>>>>,
    sync_node: Arc<tokio::sync::Mutex<Option<rua_sync::endpoint::SyncNode>>>,
    device_network: Arc<crate::services::device_session_access::DeviceNetworkAccess>,
    data_dir: PathBuf,
    project_catalog: Option<Arc<rua_project::ProjectCatalog>>,
    conversation_authority: Option<Arc<ConversationAuthority>>,
    history_sync_tx: Option<tokio::sync::mpsc::UnboundedSender<ConversationHistorySyncRequest>>,
    chat_activity_wake_hub: Option<Arc<ChatActivityWakeHub>>,
    active_stream_broadcast_hub: Option<Arc<ActiveStreamBroadcastHub>>,
    authorities: LiveActivityAuthorityMap,
    remote_wakeup: Arc<tokio::sync::Notify>,
    route_revision: tokio::sync::watch::Sender<u64>,
}

impl ConversationLiveActivity {
    pub fn new(
        streams: Arc<ActiveStreamRegistry>,
        registry: Arc<dyn rua_workspace::WorkspaceRegistry>,
        sync_device: Arc<std::sync::RwLock<Option<Arc<rua_sync::identity::DeviceIdentity>>>>,
        sync_node: Arc<tokio::sync::Mutex<Option<rua_sync::endpoint::SyncNode>>>,
        device_network: Arc<crate::services::device_session_access::DeviceNetworkAccess>,
        data_dir: PathBuf,
    ) -> Self {
        let (route_revision, _) = tokio::sync::watch::channel(0);
        Self {
            streams,
            registry,
            sync_device,
            sync_node,
            device_network,
            data_dir,
            project_catalog: None,
            conversation_authority: None,
            history_sync_tx: None,
            chat_activity_wake_hub: None,
            active_stream_broadcast_hub: None,
            authorities: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            remote_wakeup: Arc::new(tokio::sync::Notify::new()),
            route_revision,
        }
    }

    pub fn with_conversation_authority(mut self, authority: Arc<ConversationAuthority>) -> Self {
        self.conversation_authority = Some(authority);
        self
    }

    pub fn with_project_catalog(
        mut self,
        project_catalog: Arc<rua_project::ProjectCatalog>,
    ) -> Self {
        self.project_catalog = Some(project_catalog);
        self
    }

    pub fn with_history_sync(
        mut self,
        tx: tokio::sync::mpsc::UnboundedSender<ConversationHistorySyncRequest>,
    ) -> Self {
        self.history_sync_tx = Some(tx);
        self
    }

    pub fn with_chat_activity_wake_hub(mut self, wake_hub: Arc<ChatActivityWakeHub>) -> Self {
        self.chat_activity_wake_hub = Some(wake_hub);
        self
    }

    pub fn with_active_stream_broadcast_hub(mut self, hub: Arc<ActiveStreamBroadcastHub>) -> Self {
        self.active_stream_broadcast_hub = Some(hub);
        self
    }

    pub async fn active_authority(&self, address: &ChatAddress) -> Option<String> {
        self.authorities.read().await.get(address).cloned()
    }

    pub async fn subscribe_viewer(
        &self,
        address: &ChatAddress,
        execution_agent_id: &str,
        viewer_id: &str,
        position: Option<ConversationActivityPosition>,
    ) -> SseOutputStream {
        let chat_id = address.chat_id().as_str();
        let (out_tx, out_rx) =
            tokio::sync::mpsc::channel::<std::result::Result<String, Infallible>>(64);
        let key = StreamKey::new(
            crate::services::console_chat::CONSOLE_CHANNEL_ID,
            address.clone(),
            Priority::Normal,
        );
        if let Some(hub) = self.active_stream_broadcast_hub.as_ref() {
            let subscription = match position.as_ref() {
                Some(position) => {
                    hub.subscribe_chat_from_activity_position(address, position)
                        .await
                }
                None => hub.subscribe_chat(address, &BTreeMap::new()).await,
            };
            if let Some(authority) = self.conversation_authority.as_ref() {
                if self
                    .local_is_current_authority(execution_agent_id, address)
                    .await
                {
                    if let Some(scope_root) = self.scope_root(execution_agent_id, address).await {
                        if let Ok(Some(meta)) =
                            crate::services::conversation_authority_target::load_chat_control_meta(
                                &scope_root,
                                chat_id,
                            )
                            .await
                        {
                            let control = authority.control_view(&key.address, &meta).await;
                            let payload = hub.conversation_activity_snapshot(
                                conversation_control_payload(address, "viewer_attached", control),
                            );
                            let _ = out_tx.send(Ok(payload)).await;
                        }
                    }
                }
            }
            spawn_local_broadcast_forwarder(
                self.clone(),
                execution_agent_id.to_string(),
                address.clone(),
                subscription,
                out_tx.clone(),
            );
        } else {
            if let Some(authority) = self.conversation_authority.as_ref() {
                if self
                    .local_is_current_authority(execution_agent_id, address)
                    .await
                {
                    if let Some(scope_root) = self.scope_root(execution_agent_id, address).await {
                        if let Ok(Some(meta)) =
                            crate::services::conversation_authority_target::load_chat_control_meta(
                                &scope_root,
                                chat_id,
                            )
                            .await
                        {
                            let control = authority.control_view(&key.address, &meta).await;
                            let _ = out_tx
                                .send(Ok(conversation_control_payload(
                                    address,
                                    "viewer_attached",
                                    control,
                                )))
                                .await;
                        }
                    }
                }
                let rx = authority.subscribe_control_events(&key.address).await;
                spawn_control_forwarder(
                    rx,
                    out_tx.clone(),
                    Arc::clone(&self.remote_wakeup),
                    self.route_revision.clone(),
                );
            }
            spawn_local_future_forwarder(
                self.clone(),
                execution_agent_id.to_string(),
                address.clone(),
                key,
                out_tx.clone(),
            );
        }
        if let Some(wake_hub) = self.chat_activity_wake_hub.as_ref() {
            spawn_chat_wake_forwarder(
                wake_hub.subscribe(),
                address.clone(),
                out_tx.clone(),
                Arc::clone(&self.remote_wakeup),
                self.route_revision.clone(),
            );
        }
        spawn_remote_discovery_forwarder(
            self.clone(),
            execution_agent_id.to_string(),
            address.clone(),
            viewer_id.to_string(),
            position,
            out_tx.clone(),
        );

        with_live_activity_keepalive(Box::pin(
            futures::stream::unfold(out_rx, |mut rx| async move {
                rx.recv().await.map(|item| (item, rx))
            })
            .fuse(),
        ))
    }

    async fn local_is_current_authority(
        &self,
        execution_agent_id: &str,
        address: &ChatAddress,
    ) -> bool {
        let Some(scope_root) = self.scope_root(execution_agent_id, address).await else {
            return false;
        };
        let Some(local_device_id) = current_sync_device_id(&self.sync_device) else {
            return false;
        };
        crate::services::conversation_authority_target::load_chat_authority_placement_target(
            &scope_root,
            address.chat_id().as_str(),
        )
        .await
        .is_ok_and(|placement| placement.device_id == local_device_id)
    }

    async fn remote_discovery_stream(
        &self,
        execution_agent_id: &str,
        address: &ChatAddress,
        viewer_id: &str,
        position: Option<ConversationActivityPosition>,
        out_tx: &tokio::sync::mpsc::Sender<std::result::Result<String, Infallible>>,
    ) -> Option<RemoteDiscoveryStream> {
        let workspace_dir = match self.scope_root(execution_agent_id, address).await {
            Some(scope_root) => scope_root,
            None => {
                tracing::warn!(
                    execution_agent_id,
                    chat_address = %address,
                    "failed to resolve workspace while discovering remote live activity"
                );
                return None;
            }
        };
        let local_chat =
            match crate::services::conversation_authority_target::load_chat_control_meta(
                &workspace_dir,
                address.chat_id().as_str(),
            )
            .await
            {
                Ok(meta) => meta,
                Err(error) => {
                    tracing::warn!(
                        execution_agent_id,
                        chat_address = %address,
                        error = %error,
                        "failed to inspect Chat while discovering remote live activity"
                    );
                    return None;
                }
            };
        let local_device_id = current_sync_device_id(&self.sync_device);
        let (target_device_id, source_device_id, authority_epoch) = match local_chat {
            Some(_) => {
                let route = ConversationAuthorityRouter
                    .resolve_live_activity_route(ConversationLiveActivityRouteRequest {
                        workspace_dir: Some(&workspace_dir),
                        address,
                        local_device_id: local_device_id.as_deref(),
                    })
                    .await;
                match route {
                    ConversationLiveActivityRoute::LocalAuthority { .. } => return None,
                    ConversationLiveActivityRoute::RemoteAuthority {
                        source_device_id,
                        target_device_id,
                        authority_epoch,
                    } => (target_device_id, source_device_id, Some(authority_epoch)),
                    ConversationLiveActivityRoute::Rejected { detail, .. } => {
                        let _ = out_tx
                            .send(Ok(serde_json::json!({
                                "type": "error",
                                "message": detail,
                            })
                            .to_string()))
                            .await;
                        return None;
                    }
                }
            }
            None => {
                let project_id = address.project_id()?;
                let Some(project_catalog) = self.project_catalog.as_ref() else {
                    let _ = out_tx
                        .send(Ok(serde_json::json!({
                            "type": "error",
                            "message": "Project Catalog is unavailable",
                        })
                        .to_string()))
                        .await;
                    return None;
                };
                let project = match project_catalog.get(project_id) {
                    Ok(Some(project)) => project,
                    Ok(None) => {
                        let _ = out_tx
                            .send(Ok(serde_json::json!({
                                "type": "error",
                                "message": format!("Project not found: {project_id}"),
                            })
                            .to_string()))
                            .await;
                        return None;
                    }
                    Err(error) => {
                        let _ = out_tx
                            .send(Ok(serde_json::json!({
                                "type": "error",
                                "message": error.to_string(),
                            })
                            .to_string()))
                            .await;
                        return None;
                    }
                };
                if project.lifecycle != rua_project::ProjectLifecycle::Active {
                    let _ = out_tx
                        .send(Ok(serde_json::json!({
                            "type": "error",
                            "message": "Archived Project cannot provide Chat activity",
                        })
                        .to_string()))
                        .await;
                    return None;
                }
                let Some(source_device_id) = local_device_id else {
                    let _ = out_tx
                        .send(Ok(serde_json::json!({
                            "type": "error",
                            "message": "local Device identity is unavailable",
                        })
                        .to_string()))
                        .await;
                    return None;
                };
                if project.resource_device_id == source_device_id {
                    return None;
                }
                (project.resource_device_id, source_device_id, None)
            }
        };
        spawn_remote_history_hydration(RemoteHistoryHydrationParams {
            device_network: Arc::clone(&self.device_network),
            data_dir: self.data_dir.clone(),
            execution_agent_id: execution_agent_id.to_string(),
            address: address.clone(),
            source_device_id,
            authority_device_id: target_device_id.clone(),
            history_sync_tx: self.history_sync_tx.clone(),
            out_tx: out_tx.clone(),
        });

        Some(RemoteDiscoveryStream {
            target_device_id: target_device_id.clone(),
            stream: active_stream_broadcast_live_stream(ActiveStreamBroadcastLiveStreamParams {
                registry: Arc::clone(&self.registry),
                conversation_authority: self.conversation_authority.clone(),
                sync_device: Arc::clone(&self.sync_device),
                sync_node: Arc::clone(&self.sync_node),
                device_network: Arc::clone(&self.device_network),
                data_dir: self.data_dir.clone(),
                address: address.clone(),
                execution_agent_id: execution_agent_id.to_string(),
                target_device_ids: vec![target_device_id],
                viewer_id: viewer_id.to_string(),
                conversation_activity_position: position,
                authority_epoch,
                connect_timeout: REMOTE_LIVE_ATTACH_TIMEOUT,
                live_activity_authorities: Some(Arc::clone(&self.authorities)),
                workspace_dir,
                route_revision: self.route_revision.clone(),
            })
            .await,
        })
    }

    async fn scope_root(&self, execution_agent_id: &str, address: &ChatAddress) -> Option<PathBuf> {
        if address.project_id().is_some() {
            return Some(
                rua_core::ChatScopeStore::resolve(&self.data_dir, address.clone())
                    .root()
                    .to_path_buf(),
            );
        }
        self.registry
            .get_or_create(execution_agent_id)
            .await
            .ok()
            .map(|runtime| runtime.workspace_dir())
    }
}

fn conversation_control_payload(
    address: &ChatAddress,
    reason: &str,
    control: crate::services::conversation_authority::ConversationControlView,
) -> String {
    match address.agent_id() {
        Some(agent_id) => serde_json::json!({
            "type": "conversation_control",
            "agent_id": agent_id,
            "chat_id": address.chat_id(),
            "reason": reason,
            "control": control,
        }),
        None => serde_json::json!({
            "type": "conversation_control",
            "address": address,
            "reason": reason,
            "control": control,
        }),
    }
    .to_string()
}

fn live_authority_redirect_payload(
    address: &ChatAddress,
    placement: &ConversationAuthorityPlacement,
) -> String {
    match address.agent_id() {
        Some(agent_id) => serde_json::json!({
            "type": "authority_redirect",
            "agent_id": agent_id,
            "chat_id": address.chat_id(),
            "placement": placement,
        }),
        None => serde_json::json!({
            "type": "authority_redirect",
            "address": address,
            "placement": placement,
        }),
    }
    .to_string()
}

pub fn spawn_conversation_control_remote_bridge(
    authority: Arc<ConversationAuthority>,
    hub: Arc<ActiveStreamBroadcastHub>,
) {
    tokio::spawn(async move {
        let mut rx = authority.subscribe_global_control_events();
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if let Some(placement) = authority_redirect_placement(&event.payload) {
                        hub.publish_authority_redirect(&event.address, placement)
                            .await;
                        continue;
                    }
                    let stream_id =
                        format!("control:{event_address}", event_address = event.address);
                    if let Some(authority_epoch) = control_event_authority_epoch(&event.payload) {
                        hub.publish_conversation_event(
                            &stream_id,
                            &event.address,
                            authority_epoch,
                            event.payload,
                        )
                        .await;
                    } else {
                        tracing::error!(
                            chat_address = %event.address,
                            "Conversation Authority published Control without Authority Epoch"
                        );
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                    tracing::warn!(
                        lagged = count,
                        "conversation control remote bridge lagged; later getChat.control can repair"
                    );
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    });
}

async fn send_live_payload(
    out_tx: &tokio::sync::mpsc::Sender<std::result::Result<String, Infallible>>,
    payload: String,
) -> bool {
    out_tx.send(Ok(payload)).await.is_ok()
}

struct ActiveStreamBroadcastLiveStreamParams {
    registry: Arc<dyn rua_workspace::WorkspaceRegistry>,
    conversation_authority: Option<Arc<ConversationAuthority>>,
    sync_device: Arc<std::sync::RwLock<Option<Arc<rua_sync::identity::DeviceIdentity>>>>,
    sync_node: Arc<tokio::sync::Mutex<Option<rua_sync::endpoint::SyncNode>>>,
    device_network: Arc<crate::services::device_session_access::DeviceNetworkAccess>,
    data_dir: PathBuf,
    address: ChatAddress,
    execution_agent_id: String,
    target_device_ids: Vec<String>,
    viewer_id: String,
    conversation_activity_position: Option<ConversationActivityPosition>,
    authority_epoch: Option<ConversationAuthorityEpoch>,
    connect_timeout: Duration,
    live_activity_authorities: Option<LiveActivityAuthorityMap>,
    workspace_dir: PathBuf,
    route_revision: tokio::sync::watch::Sender<u64>,
}

async fn active_stream_broadcast_live_stream(
    params: ActiveStreamBroadcastLiveStreamParams,
) -> SseOutputStream {
    let ActiveStreamBroadcastLiveStreamParams {
        registry,
        conversation_authority,
        sync_device,
        sync_node,
        device_network,
        data_dir,
        address,
        execution_agent_id,
        target_device_ids,
        viewer_id,
        conversation_activity_position,
        authority_epoch,
        connect_timeout,
        live_activity_authorities,
        workspace_dir,
        route_revision,
    } = params;
    let bridge_address = address;
    let bridge_execution_agent_id = execution_agent_id;
    let (out_tx, out_rx) =
        tokio::sync::mpsc::channel::<std::result::Result<String, Infallible>>(64);
    tokio::spawn(async move {
        let target_device_ids_for_log = target_device_ids.clone();
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(rua_core::RunControl::Running);
        let mut joins = tokio::task::JoinSet::new();

        for target_device_id in target_device_ids {
            let sync_node = Arc::clone(&sync_node);
            let device_network = Arc::clone(&device_network);
            let registry = Arc::clone(&registry);
            let conversation_authority = conversation_authority.clone();
            let sync_device = Arc::clone(&sync_device);
            let data_dir = data_dir.clone();
            let out_tx_for_frames = out_tx.clone();
            let request_address = bridge_address.clone();
            let request_execution_agent_id = bridge_execution_agent_id.clone();
            let workspace_dir = workspace_dir.clone();
            let authorities_for_frames = live_activity_authorities.clone();
            let cancel_rx = cancel_rx.clone();
            let expected_authority_epoch = authority_epoch;
            let conversation_activity_position = conversation_activity_position.clone();
            let verified_authority_epoch =
                Arc::new(tokio::sync::RwLock::new(expected_authority_epoch));
            let viewer = current_sync_device_id(&sync_device).map(|source_device_id| {
                ActiveStreamBroadcastViewer {
                    source_device_id,
                    viewer_id: viewer_id.clone(),
                    kind: ForegroundOperationKind::ChatViewer,
                }
            });
            let route_revision = route_revision.clone();
            joins.spawn(async move {
                match current_session_manager(&device_network).await {
                    Ok(session_manager) => {
                        let request = ActiveStreamBroadcastSubscribeRequest {
                            device_delivery_id: String::new(),
                            owner: Some(ActiveStreamOwner::chat(request_address.clone())),
                            after_seq_by_stream: BTreeMap::new(),
                            activity_generation: None,
                            conversation_activity_position,
                            collapse_replay: true,
                            initial_conversation_control: true,
                            authority_epoch: expected_authority_epoch,
                            viewer,
                        };
                        let terminal_sync_node = Arc::clone(&sync_node);
                        let terminal_device_network = Arc::clone(&device_network);
                        let terminal_data_dir = data_dir.clone();
                        let session_peer_device_id = target_device_id.clone();
                        stream_active_broadcast_via_session(
                            &session_manager,
                            &session_peer_device_id,
                            request,
                            connect_timeout,
                            cancel_rx,
                            move |frame| {
                                let out_tx = out_tx_for_frames.clone();
                                let authorities = authorities_for_frames.clone();
                                let authority_address = request_address.clone();
                                let authority_execution_agent_id =
                                    request_execution_agent_id.clone();
                                let authority_device_id = target_device_id.clone();
                                let sync_node_for_terminal = Arc::clone(&terminal_sync_node);
                                let device_network_for_terminal =
                                    Arc::clone(&terminal_device_network);
                                let data_dir_for_terminal = terminal_data_dir.clone();
                                let workspace_dir_for_gap = workspace_dir.clone();
                                let redirect_registry = Arc::clone(&registry);
                                let redirect_authority = conversation_authority.clone();
                                let redirect_device_network = Arc::clone(&device_network);
                                let redirect_sync_device = Arc::clone(&sync_device);
                                let redirect_route_revision = route_revision.clone();
                                let verified_authority_epoch =
                                    Arc::clone(&verified_authority_epoch);
                                async move {
                                    if let ActiveStreamBroadcastFrame::AuthorityRedirect {
                                        placement,
                                        ..
                                    } = &frame
                                    {
                                        if authority_address.project_id().is_some() {
                                            let _ = send_live_payload(
                                                &out_tx,
                                                serde_json::json!({
                                                    "type": "error",
                                                    "message": "Project Chat authority is fixed to its resource Device",
                                                })
                                                .to_string(),
                                            )
                                            .await;
                                            return;
                                        }
                                        let redirect = crate::services::device_command_bus::ConversationAuthorityRedirect {
                                            address: authority_address.clone(),
                                            placement: placement.clone(),
                                        };
                                        let payload = live_authority_redirect_payload(
                                            &authority_address,
                                            placement,
                                        );
                                        let _ = send_live_payload(&out_tx, payload).await;
                                        let result = match (
                                            redirect_authority.as_ref(),
                                            expected_authority_epoch,
                                        ) {
                                            (Some(authority), Some(expected_authority_epoch)) => crate::services::conversation_authority_router::verify_and_observe_authority_redirect(
                                                &redirect_registry,
                                                authority,
                                                &redirect_device_network,
                                                &redirect_sync_device,
                                                expected_authority_epoch,
                                                &redirect,
                                            )
                                            .await,
                                            (None, _) => Err("Conversation Authority is unavailable".to_string()),
                                            (_, None) => Err(
                                                "Project Chat authority is fixed to its resource Device"
                                                    .to_string(),
                                            ),
                                        };
                                        match result {
                                            Ok(_) => increment_route_revision(&redirect_route_revision),
                                            Err(error) => {
                                                let _ = send_live_payload(
                                                    &out_tx,
                                                    serde_json::json!({
                                                        "type": "error",
                                                        "message": format!(
                                                            "authority Redirect verification failed: {error}"
                                                        ),
                                                    })
                                                    .to_string(),
                                                )
                                                .await;
                                            }
                                        }
                                        return;
                                    }
                                    if expected_authority_epoch.is_none() {
                                        if let Some(authority_epoch) =
                                            project_initial_control_authority_epoch(
                                                &frame,
                                                &authority_address,
                                                &authority_device_id,
                                            )
                                        {
                                            verified_authority_epoch
                                                .write()
                                                .await
                                                .replace(authority_epoch);
                                        }
                                    }
                                    let Some(verified_authority_epoch) =
                                        *verified_authority_epoch.read().await
                                    else {
                                        tracing::warn!(
                                            chat_address = %authority_address,
                                            authority_device_id,
                                            "ignored Project live activity before verified initial Control View"
                                        );
                                        return;
                                    };
                                    if let ActiveStreamBroadcastFrame::Event { payload, .. } =
                                        &frame
                                    {
                                        mark_source_pending_terminal_seen_from_payload(
                                            Arc::clone(&sync_node_for_terminal),
                                            Arc::clone(&device_network_for_terminal),
                                            data_dir_for_terminal.clone(),
                                            &authority_address,
                                            payload,
                                        )
                                        .await;
                                    }
                                    if let ActiveStreamBroadcastFrame::Gap {
                                        device_delivery_id,
                                        ..
                                    } = &frame
                                    {
                                        if !device_delivery_id.is_empty() {
                                            recover_terminal_after_replay_gap(
                                                Arc::clone(&sync_node_for_terminal),
                                                Arc::clone(&device_network_for_terminal),
                                                data_dir_for_terminal.clone(),
                                                workspace_dir_for_gap.clone(),
                                                &authority_address,
                                                &authority_execution_agent_id,
                                                &authority_device_id,
                                                device_delivery_id,
                                                &out_tx,
                                            )
                                            .await;
                                        }
                                    }
                                    let _ = forward_live_activity_frame(
                                        frame,
                                        &out_tx,
                                        LiveActivityFrameContext {
                                            address: &authority_address,
                                            execution_agent_id: &authority_execution_agent_id,
                                            authority_device_id: Some(&authority_device_id),
                                            expected_authority_epoch: verified_authority_epoch,
                                            authorities: authorities.as_ref(),
                                        },
                                        |_| {},
                                    )
                                    .await;
                                }
                            },
                        )
                        .await
                    }
                    Err(error) => Err(ActiveStreamBroadcastClientError::Connect(error)),
                }
            });
        }

        let mut last_error: Option<ActiveStreamBroadcastClientError> = None;
        loop {
            tokio::select! {
                _ = out_tx.closed() => {
                    let _ = cancel_tx.send(rua_core::RunControl::Stop);
                    while joins.join_next().await.is_some() {}
                    break;
                }
                joined = joins.join_next() => {
                        let Some(joined) = joined else {
                            if let Some(error) = last_error.as_ref() {
                                tracing::warn!(
                                    target_device_ids = ?target_device_ids_for_log,
                                    error = %error,
                                    "active stream broadcast unavailable; waiting for final snapshot sync"
                                );
                                let _ = send_live_payload(
                                    &out_tx,
                                    serde_json::json!({
                                        "type": "error",
                                        "message": format!(
                                            "remote Conversation Live Activity unavailable: {error}"
                                        ),
                                    })
                                    .to_string(),
                                )
                                .await;
                            }
                        break;
                    };
                    match joined {
                        Ok(Ok(())) | Ok(Err(ActiveStreamBroadcastClientError::Cancelled)) => {}
                        Ok(Err(error)) => last_error = Some(error),
                        Err(error) => {
                            last_error = Some(ActiveStreamBroadcastClientError::Connect(error.to_string()));
                        }
                    }
                }
            }
        }
    });

    Box::pin(
        futures::stream::unfold(out_rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        })
        .fuse(),
    )
}

fn project_initial_control_authority_epoch(
    frame: &ActiveStreamBroadcastFrame,
    expected_address: &ChatAddress,
    expected_device_id: &str,
) -> Option<ConversationAuthorityEpoch> {
    expected_address.project_id()?;
    let ActiveStreamBroadcastFrame::Event {
        owner,
        authority_epoch: Some(frame_authority_epoch),
        payload,
        ..
    } = frame
    else {
        return None;
    };
    if owner.as_chat_address() != Some(expected_address) {
        return None;
    }
    let payload = serde_json::from_str::<serde_json::Value>(payload).ok()?;
    if payload.get("type").and_then(serde_json::Value::as_str) != Some("conversation_control")
        || payload.get("reason").and_then(serde_json::Value::as_str) != Some("viewer_attached")
    {
        return None;
    }
    let payload_address: ChatAddress =
        serde_json::from_value(payload.get("address")?.clone()).ok()?;
    if &payload_address != expected_address {
        return None;
    }
    let placement = payload.get("control")?.get("placement")?;
    if placement
        .get("authority_device_id")
        .and_then(serde_json::Value::as_str)
        != Some(expected_device_id)
    {
        return None;
    }
    let control_epoch = placement
        .get("authority_epoch")?
        .as_str()
        .and_then(|value| ConversationAuthorityEpoch::parse(value).ok())?;
    (control_epoch == *frame_authority_epoch).then_some(control_epoch)
}

struct RemoteHistoryHydrationParams {
    device_network: Arc<crate::services::device_session_access::DeviceNetworkAccess>,
    data_dir: PathBuf,
    execution_agent_id: String,
    address: ChatAddress,
    source_device_id: String,
    authority_device_id: String,
    history_sync_tx: Option<tokio::sync::mpsc::UnboundedSender<ConversationHistorySyncRequest>>,
    out_tx: tokio::sync::mpsc::Sender<std::result::Result<String, Infallible>>,
}

struct LiveActivityFrameContext<'a> {
    address: &'a ChatAddress,
    execution_agent_id: &'a str,
    authority_device_id: Option<&'a str>,
    expected_authority_epoch: ConversationAuthorityEpoch,
    authorities: Option<&'a LiveActivityAuthorityMap>,
}

fn spawn_remote_history_hydration(params: RemoteHistoryHydrationParams) {
    tokio::spawn(async move {
        let Some(history_sync_tx) = params.history_sync_tx.as_ref() else {
            tracing::warn!(
                execution_agent_id = %params.execution_agent_id,
                chat_address = %params.address,
                authority_device_id = %params.authority_device_id,
                "conversation history hydration skipped because sync runtime is unavailable"
            );
            return;
        };
        let requirement = match fetch_remote_history_requirement(
            &params.device_network,
            &params.execution_agent_id,
            &params.address,
            &params.source_device_id,
            &params.authority_device_id,
        )
        .await
        {
            Ok(requirement) => requirement,
            Err(error) => {
                tracing::warn!(
                    execution_agent_id = %params.execution_agent_id,
                    chat_address = %params.address,
                    authority_device_id = %params.authority_device_id,
                    error = %error,
                    "failed to fetch remote conversation history requirement"
                );
                return;
            }
        };
        let Some(required_manifest) = requirement.as_ref() else {
            return;
        };
        if let Err(error) = ensure_conversation_history_ready(ConversationHistoryReadinessRequest {
            data_dir: &params.data_dir,
            address: &params.address,
            source_device_id: &params.authority_device_id,
            requirement: required_manifest,
            sync_tx: Some(history_sync_tx),
        })
        .await
        {
            tracing::warn!(
                execution_agent_id = %params.execution_agent_id,
                chat_address = %params.address,
                authority_device_id = %params.authority_device_id,
                failure_kind = error.kind(),
                error = %error,
                "failed to hydrate remote conversation history"
            );
            return;
        }
        let payload = match history_watermark_reached_payload(
            &params.address,
            &params.authority_device_id,
            required_manifest,
        ) {
            Ok(payload) => payload.to_string(),
            Err(error) => {
                tracing::warn!(
                    chat_address = %params.address,
                    error = %error,
                    "failed to identify reached conversation history watermark"
                );
                return;
            }
        };
        let _ = send_live_payload(&params.out_tx, payload).await;
    });
}

pub fn history_watermark_reached_payload(
    address: &ChatAddress,
    authority_device_id: &str,
    manifest: &ConversationManifest,
) -> rua_core::Result<serde_json::Value> {
    let manifest_hash = manifest.hash()?;
    Ok(match address.agent_id() {
        Some(agent_id) => serde_json::json!({
            "type": "history_watermark_reached",
            "agent_id": agent_id,
            "chat_id": address.chat_id(),
            "authority_device_id": authority_device_id,
            "durable_sequence": manifest.latest_sequence,
            "manifest_hash": manifest_hash,
        }),
        None => serde_json::json!({
            "type": "history_watermark_reached",
            "address": address,
            "authority_device_id": authority_device_id,
            "durable_sequence": manifest.latest_sequence,
            "manifest_hash": manifest_hash,
        }),
    })
}

async fn fetch_remote_history_requirement(
    device_network: &Arc<crate::services::device_session_access::DeviceNetworkAccess>,
    execution_agent_id: &str,
    address: &ChatAddress,
    source_device_id: &str,
    authority_device_id: &str,
) -> std::result::Result<Option<ConversationManifest>, String> {
    let session_manager = current_session_manager(device_network).await?;
    let command = ConversationManifestDeviceCommand {
        command_id: format!("conversation-manifest-{}", uuid::Uuid::new_v4()),
        agent_id: execution_agent_id.to_string(),
        address: address.clone(),
        source_device_id: source_device_id.to_string(),
        target_device_id: authority_device_id.to_string(),
        required_manifest: None,
    };
    let response = deliver_conversation_manifest_command_via_session(
        &session_manager,
        &command,
        Duration::from_secs(10),
    )
    .await
    .map_err(|error| error.to_string())?;
    Ok(response.manifest)
}

async fn forward_live_activity_frame(
    frame: ActiveStreamBroadcastFrame,
    out_tx: &tokio::sync::mpsc::Sender<std::result::Result<String, Infallible>>,
    context: LiveActivityFrameContext<'_>,
    mut on_completion: impl FnMut(bool),
) -> bool {
    let frame_address = match &frame {
        ActiveStreamBroadcastFrame::Started { owner, .. }
        | ActiveStreamBroadcastFrame::Event { owner, .. }
        | ActiveStreamBroadcastFrame::Done { owner, .. } => owner.as_chat_address(),
        ActiveStreamBroadcastFrame::Error { owner, .. } => {
            owner.as_ref().and_then(ActiveStreamOwner::as_chat_address)
        }
        ActiveStreamBroadcastFrame::AuthorityRedirect { address, .. } => Some(address),
        ActiveStreamBroadcastFrame::Gap { .. } => None,
    };
    if frame_address.is_some_and(|address| address != context.address) {
        tracing::warn!(
            expected_chat_address = %context.address,
            frame_chat_address = ?frame_address,
            "ignored live activity frame for another Chat Address"
        );
        return true;
    }
    let frame_authority_epoch = match &frame {
        ActiveStreamBroadcastFrame::Started {
            authority_epoch, ..
        }
        | ActiveStreamBroadcastFrame::Event {
            authority_epoch, ..
        }
        | ActiveStreamBroadcastFrame::Done {
            authority_epoch, ..
        }
        | ActiveStreamBroadcastFrame::Error {
            authority_epoch, ..
        } => *authority_epoch,
        ActiveStreamBroadcastFrame::AuthorityRedirect { .. } => {
            return true;
        }
        ActiveStreamBroadcastFrame::Gap { .. } => Some(context.expected_authority_epoch),
    };
    if frame_authority_epoch != Some(context.expected_authority_epoch) {
        tracing::warn!(
            expected_authority_epoch = %context.expected_authority_epoch,
            frame_authority_epoch = ?frame_authority_epoch,
            execution_agent_id = context.execution_agent_id,
            chat_address = %context.address,
            "ignored live activity frame from stale authority Epoch"
        );
        return true;
    }
    if let (Some(authorities), Some(authority_device_id)) =
        (context.authorities, context.authority_device_id)
    {
        authorities
            .write()
            .await
            .insert(context.address.clone(), authority_device_id.to_string());
    }

    match frame {
        ActiveStreamBroadcastFrame::Started { .. } => true,
        ActiveStreamBroadcastFrame::Event { payload, .. } => {
            let completed = sse_payload_completes_response(&payload);
            on_completion(completed);
            let sent = send_live_payload(out_tx, payload).await;
            if completed {
                clear_live_activity_authority(
                    context.authorities,
                    context.address,
                    context.authority_device_id,
                )
                .await;
            }
            sent
        }
        ActiveStreamBroadcastFrame::Error { detail, .. } => {
            let payload = serde_json::json!({
                "type": "error",
                "message": detail,
            })
            .to_string();
            let sent = send_live_payload(out_tx, payload).await;
            clear_live_activity_authority(
                context.authorities,
                context.address,
                context.authority_device_id,
            )
            .await;
            sent
        }
        ActiveStreamBroadcastFrame::AuthorityRedirect { .. } => true,
        ActiveStreamBroadcastFrame::Gap {
            device_delivery_id,
            requested_after_seq,
            first_available_seq,
        } => {
            tracing::warn!(
                device_delivery_id,
                requested_after_seq,
                first_available_seq,
                "active stream broadcast replay gap; falling back to final snapshot sync"
            );
            true
        }
        ActiveStreamBroadcastFrame::Done { .. } => {
            clear_live_activity_authority(
                context.authorities,
                context.address,
                context.authority_device_id,
            )
            .await;
            true
        }
    }
}

fn control_event_authority_epoch(payload: &str) -> Option<ConversationAuthorityEpoch> {
    serde_json::from_str::<serde_json::Value>(payload)
        .ok()?
        .get("control")?
        .get("placement")?
        .get("authority_epoch")?
        .as_str()
        .and_then(|value| ConversationAuthorityEpoch::parse(value).ok())
}

fn authority_redirect_placement(
    payload: &str,
) -> Option<crate::services::conversation_authority_placement::ConversationAuthorityPlacement> {
    let value = serde_json::from_str::<serde_json::Value>(payload).ok()?;
    if value.get("type").and_then(serde_json::Value::as_str) != Some("authority_redirect") {
        return None;
    }
    serde_json::from_value(value.get("placement")?.clone()).ok()
}

async fn mark_source_pending_terminal_seen_from_payload(
    sync_node: Arc<tokio::sync::Mutex<Option<rua_sync::endpoint::SyncNode>>>,
    device_network: Arc<crate::services::device_session_access::DeviceNetworkAccess>,
    data_dir: PathBuf,
    address: &ChatAddress,
    payload: &str,
) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
        return;
    };
    if value.get("type").and_then(|value| value.as_str()) != Some("turn_terminal") {
        return;
    }
    let Some(device_delivery_id) = value
        .get("device_delivery_id")
        .and_then(|value| value.as_str())
    else {
        return;
    };
    let Some(status) = value.get("status").and_then(|value| value.as_str()) else {
        return;
    };
    if let Err(error) =
        crate::services::remote_conversation_input::RemoteConversationInputDelivery::new(
            sync_node,
            device_network,
            data_dir,
        )
        .mark_source_pending_terminal_seen(address, device_delivery_id, status)
        .await
    {
        tracing::warn!(
            chat_address = %address,
            device_delivery_id,
            error = %error,
            "failed to mark source pending device delivery terminal from live activity"
        );
    }
}

#[allow(clippy::too_many_arguments)]
async fn recover_terminal_after_replay_gap(
    sync_node: Arc<tokio::sync::Mutex<Option<rua_sync::endpoint::SyncNode>>>,
    device_network: Arc<crate::services::device_session_access::DeviceNetworkAccess>,
    data_dir: PathBuf,
    workspace_dir: PathBuf,
    address: &ChatAddress,
    execution_agent_id: &str,
    authority_device_id: &str,
    device_delivery_id: &str,
    out_tx: &tokio::sync::mpsc::Sender<std::result::Result<String, Infallible>>,
) {
    let _ = crate::sync_cycle::run_sync_cycle_to_device_id(
        &sync_node,
        &data_dir.join("sync"),
        authority_device_id,
    )
    .await;
    let payload = match durable_remote_turn_terminal_payload(
        &workspace_dir,
        address,
        execution_agent_id,
        authority_device_id,
        device_delivery_id,
    )
    .await
    {
        Ok(Some(payload)) => payload,
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(
                chat_address = %address,
                device_delivery_id,
                error = %error,
                "failed to recover remote turn terminal fact after replay gap"
            );
            return;
        }
    };
    mark_source_pending_terminal_seen_from_payload(
        sync_node,
        device_network,
        data_dir,
        address,
        &payload,
    )
    .await;
    let _ = send_live_payload(out_tx, payload).await;
}

async fn durable_remote_turn_terminal_payload(
    workspace_dir: &Path,
    address: &ChatAddress,
    execution_agent_id: &str,
    authority_device_id: &str,
    device_delivery_id: &str,
) -> std::result::Result<Option<String>, String> {
    let chat_id = address.chat_id().as_str();
    let store = rua_session::WorkspaceSessionStore::new(workspace_dir);
    let turns = store
        .read_turn_range(chat_id, 1, usize::MAX)
        .await
        .map_err(|error| error.to_string())?;
    for turn in turns {
        if !rua_session::TurnLifecycleStatus::parse(&turn.lifecycle.status)
            .is_syncable_conversation_state()
        {
            continue;
        }
        let messages = store
            .read_turn_messages(chat_id, &turn.turn_id)
            .await
            .map_err(|error| error.to_string())?;
        let belongs_to_device_delivery = messages.iter().any(|message| {
            message
                .metadata
                .get("device_delivery_id")
                .and_then(|value| value.as_str())
                == Some(device_delivery_id)
        });
        if !belongs_to_device_delivery {
            continue;
        }
        let payload = match address.agent_id() {
            Some(agent_id) => serde_json::json!({
                "type": "turn_terminal",
                "agent_id": agent_id,
                "chat_id": address.chat_id(),
                "turn_id": turn.turn_id,
                "status": turn.lifecycle.status,
                "completed_at": turn.lifecycle.assistant_completed_at,
                "authority_device_id": authority_device_id,
                "authority_epoch": turn.metadata.get("authority_epoch").cloned(),
                "device_delivery_id": device_delivery_id,
            }),
            None => serde_json::json!({
                "type": "turn_terminal",
                "address": address,
                "execution_agent_id": execution_agent_id,
                "turn_id": turn.turn_id,
                "status": turn.lifecycle.status,
                "completed_at": turn.lifecycle.assistant_completed_at,
                "authority_device_id": authority_device_id,
                "authority_epoch": turn.metadata.get("authority_epoch").cloned(),
                "device_delivery_id": device_delivery_id,
            }),
        };
        return Ok(Some(payload.to_string()));
    }
    Ok(None)
}

async fn clear_live_activity_authority(
    live_activity_authorities: Option<&LiveActivityAuthorityMap>,
    address: &ChatAddress,
    authority_device_id: Option<&str>,
) {
    let (Some(authorities), Some(authority_device_id)) =
        (live_activity_authorities, authority_device_id)
    else {
        return;
    };
    let mut guard = authorities.write().await;
    if guard.get(address).map(String::as_str) == Some(authority_device_id) {
        guard.remove(address);
    }
}

fn with_live_activity_keepalive(inner: SseOutputStream) -> SseOutputStream {
    let interval = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(15),
        Duration::from_secs(15),
    );
    Box::pin(
        futures::stream::unfold((inner, interval), |(mut inner, mut interval)| async move {
            tokio::select! {
                item = inner.next() => item.map(|item| (item, (inner, interval))),
                _ = interval.tick() => Some((Ok(": keepalive\n\n".to_string()), (inner, interval))),
            }
        })
        .fuse(),
    )
}

fn spawn_local_future_forwarder(
    activity: ConversationLiveActivity,
    execution_agent_id: String,
    address: ChatAddress,
    key: StreamKey,
    out_tx: tokio::sync::mpsc::Sender<std::result::Result<String, Infallible>>,
) {
    tokio::spawn(async move {
        loop {
            let (buffer, mut rx) = tokio::select! {
                _ = out_tx.closed() => return,
                subscription = activity.streams.subscribe_keyed_or_wait(&key) => subscription,
            };
            for event in stream_replay_payloads(buffer) {
                if activity
                    .local_is_current_authority(&execution_agent_id, &address)
                    .await
                    && !send_live_payload(&out_tx, event).await
                {
                    return;
                }
            }
            loop {
                let event = tokio::select! {
                    _ = out_tx.closed() => return,
                    event = rx.recv() => event,
                };
                let Some(event) = event else {
                    break;
                };
                if activity
                    .local_is_current_authority(&execution_agent_id, &address)
                    .await
                    && !send_live_payload(&out_tx, event).await
                {
                    return;
                }
            }
        }
    });
}

fn spawn_local_broadcast_forwarder(
    activity: ConversationLiveActivity,
    execution_agent_id: String,
    address: ChatAddress,
    subscription: ActiveStreamBroadcastSubscription,
    out_tx: tokio::sync::mpsc::Sender<std::result::Result<String, Infallible>>,
) {
    tokio::spawn(async move {
        let ActiveStreamBroadcastSubscription { replay, mut rx } = subscription;
        if !forward_local_broadcast_replay(
            &activity,
            &execution_agent_id,
            &address,
            replay,
            &out_tx,
        )
        .await
        {
            return;
        }
        let Some(rx) = rx.as_mut() else {
            return;
        };
        loop {
            let frame = tokio::select! {
                _ = out_tx.closed() => return,
                frame = rx.recv() => frame,
            };
            let Some(frame) = frame else {
                return;
            };
            if !forward_local_broadcast_frame(
                &activity,
                &execution_agent_id,
                &address,
                frame,
                &out_tx,
            )
            .await
            {
                return;
            }
        }
    });
}

async fn forward_local_broadcast_replay(
    activity: &ConversationLiveActivity,
    execution_agent_id: &str,
    address: &ChatAddress,
    replay: Vec<ActiveStreamBroadcastFrame>,
    out_tx: &tokio::sync::mpsc::Sender<std::result::Result<String, Infallible>>,
) -> bool {
    let mut deferred_gaps = Vec::new();
    for frame in collapse_replay_frames(replay) {
        if matches!(frame, ActiveStreamBroadcastFrame::Gap { .. }) {
            deferred_gaps.push(frame);
            continue;
        }
        if !forward_local_broadcast_frame(activity, execution_agent_id, address, frame, out_tx)
            .await
        {
            return false;
        }
    }
    for frame in deferred_gaps {
        if !forward_local_broadcast_frame(activity, execution_agent_id, address, frame, out_tx)
            .await
        {
            return false;
        }
    }
    true
}

async fn forward_local_broadcast_frame(
    activity: &ConversationLiveActivity,
    execution_agent_id: &str,
    address: &ChatAddress,
    frame: ActiveStreamBroadcastFrame,
    out_tx: &tokio::sync::mpsc::Sender<std::result::Result<String, Infallible>>,
) -> bool {
    if !activity
        .local_is_current_authority(execution_agent_id, address)
        .await
    {
        return true;
    }
    match frame {
        ActiveStreamBroadcastFrame::Event { payload, .. } => {
            send_live_payload(out_tx, payload).await
        }
        ActiveStreamBroadcastFrame::Gap {
            requested_after_seq,
            first_available_seq,
            ..
        } => {
            send_live_payload(
                out_tx,
                history_refresh_required_payload(address, requested_after_seq, first_available_seq)
                    .to_string(),
            )
            .await
        }
        ActiveStreamBroadcastFrame::Started { .. }
        | ActiveStreamBroadcastFrame::Done { .. }
        | ActiveStreamBroadcastFrame::Error { .. }
        | ActiveStreamBroadcastFrame::AuthorityRedirect { .. } => true,
    }
}

pub fn history_refresh_required_payload(
    address: &ChatAddress,
    requested_after_sequence: u64,
    first_available_sequence: u64,
) -> serde_json::Value {
    serde_json::json!({
        "type": "history_refresh_required",
        "reason": "live_activity_gap",
        "chat_id": address.chat_id(),
        "requested_after_sequence": requested_after_sequence,
        "first_available_sequence": first_available_sequence,
    })
}

fn spawn_control_forwarder(
    mut rx: tokio::sync::broadcast::Receiver<String>,
    out_tx: tokio::sync::mpsc::Sender<std::result::Result<String, Infallible>>,
    remote_wakeup: Arc<tokio::sync::Notify>,
    route_revision: tokio::sync::watch::Sender<u64>,
) {
    tokio::spawn(async move {
        loop {
            let payload = tokio::select! {
                _ = out_tx.closed() => return,
                payload = rx.recv() => payload,
            };
            match payload {
                Ok(payload) => {
                    remote_wakeup.notify_waiters();
                    increment_route_revision(&route_revision);
                    if !send_live_payload(&out_tx, payload).await {
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    });
}

fn spawn_chat_wake_forwarder(
    mut rx: tokio::sync::broadcast::Receiver<crate::services::chat_activity_wake::ChatLiveWake>,
    address: ChatAddress,
    out_tx: tokio::sync::mpsc::Sender<std::result::Result<String, Infallible>>,
    remote_wakeup: Arc<tokio::sync::Notify>,
    route_revision: tokio::sync::watch::Sender<u64>,
) {
    tokio::spawn(async move {
        loop {
            let wake = tokio::select! {
                _ = out_tx.closed() => return,
                wake = rx.recv() => wake,
            };
            match wake {
                Ok(wake) if wake.address == address => {
                    remote_wakeup.notify_waiters();
                    if wake.phase == "placement_changed" {
                        increment_route_revision(&route_revision);
                    }
                }
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    });
}

fn spawn_remote_discovery_forwarder(
    activity: ConversationLiveActivity,
    execution_agent_id: String,
    address: ChatAddress,
    viewer_id: String,
    position: Option<ConversationActivityPosition>,
    out_tx: tokio::sync::mpsc::Sender<std::result::Result<String, Infallible>>,
) {
    tokio::spawn(async move {
        let mut route_revision = activity.route_revision.subscribe();
        let mut runtime_revision = activity.device_network.subscribe_availability();
        loop {
            // Establish the lifecycle cursor before attachment, including failures
            // which race installation of the next resident runtime.
            runtime_revision.borrow_and_update();
            let Some(RemoteDiscoveryStream {
                target_device_id,
                mut stream,
            }) = activity
                .remote_discovery_stream(
                    &execution_agent_id,
                    &address,
                    &viewer_id,
                    position.clone(),
                    &out_tx,
                )
                .await
            else {
                tokio::select! {
                    _ = out_tx.closed() => return,
                    _ = activity.remote_wakeup.notified() => {}
                    changed = route_revision.changed() => {
                        if changed.is_err() {
                            return;
                        }
                    }
                }
                continue;
            };

            let mut route_changed = false;
            loop {
                let item = tokio::select! {
                    _ = out_tx.closed() => return,
                    changed = route_revision.changed() => {
                        if changed.is_err() {
                            return;
                        }
                        route_changed = true;
                        break;
                    }
                    item = stream.next() => item,
                };
                let Some(item) = item else {
                    break;
                };
                let Ok(payload) = item;
                if !send_live_payload(&out_tx, payload).await {
                    return;
                }
            }
            if route_changed {
                continue;
            }
            if !wait_for_remote_attachment_retry(
                &activity,
                &target_device_id,
                &out_tx,
                &mut route_revision,
                &mut runtime_revision,
            )
            .await
            {
                return;
            }
        }
    });
}

async fn wait_for_remote_attachment_retry(
    activity: &ConversationLiveActivity,
    target_device_id: &str,
    out_tx: &tokio::sync::mpsc::Sender<std::result::Result<String, Infallible>>,
    route_revision: &mut tokio::sync::watch::Receiver<u64>,
    runtime_revision: &mut tokio::sync::watch::Receiver<
        crate::services::device_network_availability::DeviceNetworkAvailabilitySnapshot,
    >,
) -> bool {
    let mut session_subscription = current_session_manager(&activity.device_network)
        .await
        .ok()
        .map(|manager| {
            let mut revision = manager.subscribe_connectivity_changes();
            revision.borrow_and_update();
            (manager, revision)
        });

    loop {
        enum RetryEvent {
            ViewerClosed,
            ExplicitWake,
            RouteChanged(Result<(), tokio::sync::watch::error::RecvError>),
            SessionChanged(Result<(), tokio::sync::watch::error::RecvError>),
            RuntimeChanged(Result<(), tokio::sync::watch::error::RecvError>),
        }

        let event = if let Some((_, revision)) = session_subscription.as_mut() {
            tokio::select! {
                _ = out_tx.closed() => RetryEvent::ViewerClosed,
                _ = activity.remote_wakeup.notified() => RetryEvent::ExplicitWake,
                changed = runtime_revision.changed() => RetryEvent::RuntimeChanged(changed),
                changed = route_revision.changed() => RetryEvent::RouteChanged(changed),
                changed = revision.changed() => RetryEvent::SessionChanged(changed),
            }
        } else {
            tokio::select! {
                _ = out_tx.closed() => RetryEvent::ViewerClosed,
                _ = activity.remote_wakeup.notified() => RetryEvent::ExplicitWake,
                changed = runtime_revision.changed() => RetryEvent::RuntimeChanged(changed),
                changed = route_revision.changed() => RetryEvent::RouteChanged(changed),
            }
        };

        match event {
            RetryEvent::ViewerClosed => return false,
            RetryEvent::RuntimeChanged(Err(_)) => return false,
            RetryEvent::RuntimeChanged(Ok(())) => {
                use crate::services::device_network_availability::DeviceNetworkAvailabilityState;
                if matches!(
                    runtime_revision.borrow_and_update().state,
                    DeviceNetworkAvailabilityState::Starting
                        | DeviceNetworkAvailabilityState::Ready
                ) {
                    return true;
                }
            }
            RetryEvent::ExplicitWake => return true,
            RetryEvent::RouteChanged(changed) => return changed.is_ok(),
            RetryEvent::SessionChanged(Ok(())) => {
                let connected = session_subscription.as_ref().is_some_and(|(manager, _)| {
                    matches!(
                        manager.connectivity_state(target_device_id),
                        Ok(rua_device_network::DeviceConnectivityState::Connected)
                    )
                });
                if connected {
                    return true;
                }
            }
            RetryEvent::SessionChanged(Err(_)) => {
                session_subscription = current_session_manager(&activity.device_network)
                    .await
                    .ok()
                    .map(|manager| {
                        let revision = manager.subscribe_connectivity_changes();
                        (manager, revision)
                    });
                if session_subscription.is_some() {
                    return true;
                }
            }
        }
    }
}

fn increment_route_revision(route_revision: &tokio::sync::watch::Sender<u64>) {
    let next = route_revision.borrow().saturating_add(1);
    route_revision.send_replace(next);
}

fn sse_payload_completes_response(payload: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
        return false;
    };
    value.get("type").and_then(|value| value.as_str()) == Some("turn_terminal")
}
