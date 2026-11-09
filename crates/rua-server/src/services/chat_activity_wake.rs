use std::sync::{Arc, RwLock};

use rua_core::ChatAddress;
use rua_sync::crypto::{symmetric_decrypt, symmetric_encrypt, SyncKey};
use rua_sync::identity::NetworkSecret;
use serde::{Deserialize, Serialize};

use crate::services::active_stream_broadcast::{
    ActiveStreamBroadcastFrame, ActiveStreamBroadcastHub,
};
use crate::services::conversation_authority_placement::ConversationAuthorityEpoch;
use crate::services::conversation_navigation_activity::{
    ConversationNavigationActivityHub, NavigationActiveTurn, NavigationActivityChange,
    NavigationActivityFact,
};

const CHAT_LIVE_WAKE_KIND: &str = "chat_live_wake";
const CHAT_LIVE_WAKE_VERSION: u16 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatLiveWake {
    #[serde(rename = "type")]
    pub kind: String,
    pub version: u16,
    pub address: ChatAddress,
    pub stream_id: String,
    pub phase: String,
    pub authority_device_id: String,
    pub authority_epoch: ConversationAuthorityEpoch,

    #[serde(default)]
    pub base_manifest_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<NavigationActivityFact>,
}

impl ChatLiveWake {
    pub fn placement_changed(
        address: ChatAddress,
        authority_device_id: impl Into<String>,
        authority_epoch: ConversationAuthorityEpoch,
        base_manifest_hash: impl Into<String>,
    ) -> Self {
        Self {
            kind: CHAT_LIVE_WAKE_KIND.to_string(),
            version: CHAT_LIVE_WAKE_VERSION,
            stream_id: format!("placement-sync-{address}-{authority_epoch}"),
            address,
            phase: "placement_changed".to_string(),
            authority_device_id: authority_device_id.into(),
            authority_epoch,
            base_manifest_hash: Some(base_manifest_hash.into()),
            activity: None,
        }
    }

    pub fn from_frame(
        frame: ActiveStreamBroadcastFrame,
        authority_device_id: String,
    ) -> Option<Self> {
        if let ActiveStreamBroadcastFrame::Event {
            device_delivery_id,
            owner,
            authority_epoch: Some(authority_epoch),
            payload,
            ..
        } = &frame
        {
            let value: serde_json::Value = serde_json::from_str(payload).ok()?;
            let (change, authority_device_id) = match value.get("type")?.as_str()? {
                "conversation_control" => {
                    let control = value.get("control")?;
                    let active = control.get("active_turn").filter(|turn| !turn.is_null());
                    let active_turn = active.and_then(|turn| {
                        matches!(
                            turn.get("phase")?.as_str()?,
                            "preparing"
                                | "streaming"
                                | "waiting_approval"
                                | "running_tool"
                                | "committing"
                                | "stopping"
                        )
                        .then(|| {
                            serde_json::from_value::<NavigationActiveTurn>(turn.clone()).ok()
                        })?
                    });
                    let device = control
                        .get("authority_device_id")
                        .and_then(serde_json::Value::as_str)
                        .or_else(|| active?.get("authority_device_id")?.as_str())
                        .unwrap_or(&authority_device_id)
                        .to_string();
                    (
                        NavigationActivityChange::Control {
                            revision: control.get("revision")?.as_u64()?,
                            active_turn,
                        },
                        device,
                    )
                }
                "turn_terminal" => (
                    NavigationActivityChange::Terminal {
                        turn_id: value.get("turn_id")?.as_str()?.to_string(),
                        status: value.get("status")?.as_str()?.to_string(),
                        completed_at: value
                            .get("completed_at")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string),
                    },
                    value
                        .get("authority_device_id")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(&authority_device_id)
                        .to_string(),
                ),
                _ => return None,
            };
            return Some(Self {
                kind: CHAT_LIVE_WAKE_KIND.to_string(),
                version: CHAT_LIVE_WAKE_VERSION,
                address: owner.as_chat_address()?.clone(),
                stream_id: device_delivery_id.clone(),
                phase: "activity".to_string(),
                authority_device_id,
                authority_epoch: *authority_epoch,
                base_manifest_hash: None,
                activity: Some(NavigationActivityFact {
                    generation: value.get("activity_generation")?.as_str()?.to_string(),
                    sequence: value.get("activity_sequence")?.as_u64()?,
                    change,
                }),
            });
        }
        let (stream_id, address, phase, authority_epoch) = match frame {
            ActiveStreamBroadcastFrame::Started {
                device_delivery_id,
                owner,
                authority_epoch: Some(authority_epoch),
                ..
            } => (
                device_delivery_id,
                owner.as_chat_address()?.clone(),
                "started",
                authority_epoch,
            ),
            ActiveStreamBroadcastFrame::Done {
                device_delivery_id,
                owner,
                authority_epoch: Some(authority_epoch),
                ..
            } => (
                device_delivery_id,
                owner.as_chat_address()?.clone(),
                "done",
                authority_epoch,
            ),
            ActiveStreamBroadcastFrame::Error {
                device_delivery_id,
                owner: Some(owner),
                authority_epoch: Some(authority_epoch),
                ..
            } => (
                device_delivery_id,
                owner.as_chat_address()?.clone(),
                "error",
                authority_epoch,
            ),
            _ => return None,
        };
        Some(Self {
            kind: CHAT_LIVE_WAKE_KIND.to_string(),
            version: CHAT_LIVE_WAKE_VERSION,
            address,
            stream_id,
            phase: phase.to_string(),
            authority_device_id,
            authority_epoch,
            base_manifest_hash: None,
            activity: None,
        })
    }

    pub fn is_supported(&self) -> bool {
        self.kind == CHAT_LIVE_WAKE_KIND && self.version == CHAT_LIVE_WAKE_VERSION
    }
}

#[derive(Clone)]
pub struct ChatActivityWakeHub {
    tx: tokio::sync::broadcast::Sender<ChatLiveWake>,
    pub navigation: Arc<ConversationNavigationActivityHub>,
}

impl std::fmt::Debug for ChatActivityWakeHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatActivityWakeHub").finish()
    }
}

impl Default for ChatActivityWakeHub {
    fn default() -> Self {
        let (tx, _) = tokio::sync::broadcast::channel(128);
        Self {
            tx,
            navigation: Arc::new(ConversationNavigationActivityHub::default()),
        }
    }
}

impl ChatActivityWakeHub {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<ChatLiveWake> {
        self.tx.subscribe()
    }

    pub fn publish(&self, wake: ChatLiveWake) {
        self.navigation.observe(&wake);
        let _ = self.tx.send(wake);
    }
}

pub type SyncNetworkSecretCell = Arc<RwLock<Option<Arc<NetworkSecret>>>>;

fn current_sync_key(secret: &SyncNetworkSecretCell) -> Option<SyncKey> {
    secret.read().ok()?.as_ref().map(|ns| ns.derive_sync_key())
}

fn seal_wake(wake: &ChatLiveWake, key: &SyncKey) -> Option<Vec<u8>> {
    let plaintext = serde_json::to_vec(wake).ok()?;
    symmetric_encrypt(&plaintext, key).ok()
}

fn open_wake(payload: &[u8], key: &SyncKey) -> Option<ChatLiveWake> {
    let plaintext = symmetric_decrypt(payload, key).ok()?;
    serde_json::from_slice::<ChatLiveWake>(&plaintext).ok()
}

pub async fn broadcast_placement_changed(
    runtime: &rua_device_network::DeviceNetworkRuntime,
    sync_network_secret: &SyncNetworkSecretCell,
    address: ChatAddress,
    placement: crate::services::conversation_authority_placement::ConversationAuthorityPlacement,
) {
    let wake = ChatLiveWake::placement_changed(
        address,
        placement.device_id,
        placement.authority_epoch,
        placement.base_manifest_hash,
    );
    let Some(sync_key) = current_sync_key(sync_network_secret) else {
        return;
    };
    let Some(payload) = seal_wake(&wake, &sync_key) else {
        return;
    };
    if let Err(error) = runtime.gossip_bus().publish(payload).await {
        tracing::debug!(error = %error, "placement-changed Gossip broadcast failed");
    }
}

pub fn spawn_chat_placement_convergence(
    wake_hub: Arc<ChatActivityWakeHub>,
    conversation_authority: Arc<crate::services::conversation_authority::ConversationAuthority>,
    registry: Arc<dyn rua_workspace::WorkspaceRegistry>,
) {
    tokio::spawn(async move {
        let mut wakes = wake_hub.subscribe();
        loop {
            let wake = match wakes.recv().await {
                Ok(wake) => wake,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            };
            if wake.phase != "placement_changed" {
                continue;
            }
            let Some(agent_id) = wake.address.agent_id() else {
                continue;
            };
            let Ok(runtime) = registry.get_or_create(agent_id.as_str()).await else {
                continue;
            };
            let workspace_dir = runtime.workspace_dir();
            converge_placement_from_wake(&conversation_authority, &workspace_dir, &wake).await;
        }
    });
}

pub(crate) async fn converge_placement_from_wake(
    conversation_authority: &crate::services::conversation_authority::ConversationAuthority,
    workspace_dir: &std::path::Path,
    wake: &ChatLiveWake,
) -> bool {
    use crate::services::conversation_authority_placement::ConversationAuthorityPlacement;
    if wake.phase != "placement_changed" {
        return false;
    }
    let Some(base_manifest_hash) = wake.base_manifest_hash.clone() else {
        return false;
    };
    let Some(current) =
        crate::services::conversation_authority_target::load_optional_chat_authority_placement(
            workspace_dir,
            wake.address.chat_id().as_str(),
        )
        .await
        .ok()
        .flatten()
    else {
        return false;
    };
    if wake.authority_epoch <= current.authority_epoch {
        return false;
    }
    let placement = ConversationAuthorityPlacement {
        device_id: wake.authority_device_id.clone(),
        authority_epoch: wake.authority_epoch,
        base_manifest_hash,
    };
    match conversation_authority
        .apply_confirmed_authority_placement(workspace_dir, &wake.address, placement)
        .await
    {
        Ok(_) => true,
        Err(error) => {
            tracing::debug!(error = %error, "gossip placement convergence apply failed");
            false
        }
    }
}

pub fn spawn_chat_activity_gossip_publisher(
    activity_hub: Arc<ActiveStreamBroadcastHub>,
    device_network: Arc<crate::services::device_session_access::DeviceNetworkAccess>,
    sync_network_secret: SyncNetworkSecretCell,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    tokio::spawn(async move {
        let mut subscription = activity_hub.subscribe_all().await;
        let Some(mut rx) = subscription.rx.take() else {
            return;
        };
        loop {
            if *shutdown.borrow() {
                return;
            }
            let frame = tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() { return; }
                    continue;
                }
                frame = rx.recv() => match frame { Some(frame) => frame, None => return },
            };
            let runtime = device_network.lock().await.as_ref().cloned();
            let Some(runtime) = runtime else {
                continue;
            };
            let authority_device_id = hex::encode(runtime.identity().local_device_id());
            let Some(wake) = ChatLiveWake::from_frame(frame, authority_device_id) else {
                continue;
            };
            let Some(sync_key) = current_sync_key(&sync_network_secret) else {
                continue;
            };
            let Some(payload) = seal_wake(&wake, &sync_key) else {
                continue;
            };
            if let Err(error) = runtime.gossip_bus().publish(payload).await {
                tracing::debug!(error = %error, "Chat Live wake Gossip publish failed");
            }
        }
    });
}

pub fn spawn_chat_activity_gossip_listener(
    runtime: rua_device_network::DeviceNetworkRuntime,
    wake_hub: Arc<ChatActivityWakeHub>,
    sync_network_secret: SyncNetworkSecretCell,
) {
    tokio::spawn(async move {
        let local_device_id = hex::encode(runtime.identity().local_device_id());
        let mut incoming = runtime.gossip_bus().subscribe();
        let mut shutdown = runtime.gossip_bus().shutdown_receiver();
        loop {
            let payload = tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return;
                    }
                    continue;
                }
                payload = incoming.recv() => {
                    match payload {
                        Ok(payload) => payload,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    }
                }
            };
            let Some(sync_key) = current_sync_key(&sync_network_secret) else {
                continue;
            };
            let Some(wake) = open_wake(&payload, &sync_key) else {
                continue;
            };
            if wake.is_supported() && wake.authority_device_id != local_device_id {
                wake_hub.publish(wake);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_wake() -> ChatLiveWake {
        ChatLiveWake::placement_changed(
            ChatAddress::agent("default", "chat-x"),
            "aabbccddaabbccdd",
            ConversationAuthorityEpoch::from_parts(1_000, 7, [1u8; 8]),
            format!("sha256:{}", "0".repeat(64)),
        )
    }

    #[test]
    fn seal_then_open_round_trips() {
        let key = SyncKey::from_bytes([7u8; 32]);
        let wake = sample_wake();
        let sealed = seal_wake(&wake, &key).expect("seal");

        assert!(!sealed.windows(6).any(|w| w == b"chat-x"));
        let opened = open_wake(&sealed, &key).expect("open");
        assert_eq!(opened, wake);
    }

    #[test]
    fn open_rejects_wrong_key() {
        let sealed = seal_wake(&sample_wake(), &SyncKey::from_bytes([7u8; 32])).expect("seal");
        assert!(open_wake(&sealed, &SyncKey::from_bytes([8u8; 32])).is_none());
    }

    #[test]
    fn open_rejects_tampered_ciphertext() {
        let key = SyncKey::from_bytes([7u8; 32]);
        let mut sealed = seal_wake(&sample_wake(), &key).expect("seal");

        let last = sealed.len() - 1;
        sealed[last] ^= 0xff;
        assert!(open_wake(&sealed, &key).is_none());
    }

    fn test_epoch(wall_ms: u64) -> ConversationAuthorityEpoch {
        ConversationAuthorityEpoch::from_parts(wall_ms, 0, [0u8; 8])
    }

    async fn seed_chat_placement(
        workspace_dir: &std::path::Path,
        chat_id: &str,
        device_id: &str,
        authority_epoch: ConversationAuthorityEpoch,
    ) {
        let now = chrono::Utc::now();
        crate::services::chat_index::upsert(
            workspace_dir,
            crate::services::chat_index::ChatIndexEntry {
                id: chat_id.to_string(),
                name: "fixture".to_string(),
                user_id: "default".to_string(),
                channel: "console".to_string(),
                created_at: now,
                updated_at: now,
                meta: serde_json::json!({
                    "authority_placement": {
                        "device_id": device_id,
                        "authority_epoch": authority_epoch,
                        "base_manifest_hash": format!("sha256:{}", "1".repeat(64)),
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
        .expect("seed chat index");

        let mut header = rua_session::ChatSessionHeader::new(rua_session::ChatMetadata {
            chat_id: chat_id.to_string(),
            agent_id: "default".to_string(),
            title: "fixture".to_string(),
            created_at: now.to_rfc3339(),
            last_activity_at: now.to_rfc3339(),
            pinned: false,
            archived: false,
            preview: None,
        });
        header.extra.insert(
            "authority_placement".to_string(),
            serde_json::json!({
                "device_id": device_id,
                "authority_epoch": authority_epoch,
                "base_manifest_hash": format!("sha256:{}", "1".repeat(64)),
            }),
        );
        rua_chat::apply_agent_scope_defaults(&mut header, "default");
        rua_session::WorkspaceSessionStore::new(workspace_dir)
            .save_chat_header(&header)
            .await
            .expect("seed Chat Session");
    }

    fn placement_wake(
        chat_id: &str,
        device_id: &str,
        authority_epoch: ConversationAuthorityEpoch,
    ) -> ChatLiveWake {
        ChatLiveWake::placement_changed(
            ChatAddress::agent("default", chat_id),
            device_id,
            authority_epoch,
            format!("sha256:{}", "2".repeat(64)),
        )
    }

    async fn read_placement(
        workspace_dir: &std::path::Path,
        chat_id: &str,
    ) -> crate::services::conversation_authority_placement::ConversationAuthorityPlacement {
        crate::services::conversation_authority_target::load_optional_chat_authority_placement(
            workspace_dir,
            chat_id,
        )
        .await
        .expect("read placement")
        .expect("placement present")
    }

    #[tokio::test]
    async fn converge_applies_newer_placement_locally() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ws = dir.path();
        seed_chat_placement(ws, "chat-c", "old-device", test_epoch(1)).await;
        let authority = crate::services::conversation_authority::ConversationAuthority::new();
        let wake = placement_wake("chat-c", "new-device", test_epoch(2));
        assert!(converge_placement_from_wake(&authority, ws, &wake).await);
        let placement = read_placement(ws, "chat-c").await;
        assert_eq!(placement.device_id, "new-device");
        assert_eq!(placement.authority_epoch, test_epoch(2));
        assert_eq!(
            placement.base_manifest_hash,
            format!("sha256:{}", "2".repeat(64))
        );
    }

    #[tokio::test]
    async fn converge_rejects_stale_or_equal_epoch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ws = dir.path();
        seed_chat_placement(ws, "chat-c", "device-a", test_epoch(5)).await;
        let authority = crate::services::conversation_authority::ConversationAuthority::new();
        assert!(
            !converge_placement_from_wake(
                &authority,
                ws,
                &placement_wake("chat-c", "device-b", test_epoch(5))
            )
            .await
        );
        assert!(
            !converge_placement_from_wake(
                &authority,
                ws,
                &placement_wake("chat-c", "device-b", test_epoch(3))
            )
            .await
        );
        assert_eq!(read_placement(ws, "chat-c").await.device_id, "device-a");
    }

    #[tokio::test]
    async fn converge_skips_conversation_without_local_record() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ws = dir.path();
        let authority = crate::services::conversation_authority::ConversationAuthority::new();
        let wake = placement_wake("unknown-chat", "new-device", test_epoch(9));
        assert!(!converge_placement_from_wake(&authority, ws, &wake).await);
        assert!(crate::services::chat_index::get(ws, "unknown-chat")
            .await
            .expect("read index")
            .is_none());
    }

    #[tokio::test]
    async fn converge_skips_index_only_legacy_placement() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ws = dir.path();
        let now = chrono::Utc::now();
        crate::services::chat_index::upsert(
            ws,
            crate::services::chat_index::ChatIndexEntry {
                id: "legacy-chat".to_string(),
                name: "legacy".to_string(),
                user_id: "default".to_string(),
                channel: "console".to_string(),
                created_at: now,
                updated_at: now,
                meta: serde_json::json!({
                    "authority_placement": {
                        "device_id": "old-device",
                        "authority_epoch": test_epoch(1),
                        "base_manifest_hash": format!("sha256:{}", "1".repeat(64)),
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
        .expect("seed legacy Chat Index");

        let authority = crate::services::conversation_authority::ConversationAuthority::new();
        assert!(
            !converge_placement_from_wake(
                &authority,
                ws,
                &placement_wake("legacy-chat", "new-device", test_epoch(2))
            )
            .await
        );
        assert!(rua_session::WorkspaceSessionStore::new(ws)
            .load_chat_header("legacy-chat")
            .await
            .expect("read Chat Session")
            .is_none());
    }
}
