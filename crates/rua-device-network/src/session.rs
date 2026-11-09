use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use hmac::{Hmac, Mac};
use iroh::endpoint::{Connection, VarInt};
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::{Endpoint, EndpointAddr, EndpointId, TransportAddr};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{watch, Notify};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use crate::peer_directory::{
    DevicePeerAddressResolver, DevicePeerDirectory, DevicePeerDirectoryError,
};
use crate::runtime::{BoundDeviceNetworkMembership, DeviceNetworkIdentity};

pub const DEVICE_SESSION_ALPN: &[u8] = b"rua-device-session/1";
pub const DEVICE_SESSION_PROTOCOL_VERSION: u16 = 18;

const COMMAND_CAPABILITY: &str = "command";
const LIVE_ACTIVITY_CAPABILITY: &str = "live_activity";
const BULK_TRANSFER_CAPABILITY: &str = "bulk_transfer";
const HANDSHAKE_FRAME_LIMIT: usize = 64 * 1024;
const COMMAND_FRAME_LIMIT: usize = 1024 * 1024;
const LIVE_ACTIVITY_FRAME_LIMIT: usize = 1024 * 1024;
const BULK_TRANSFER_FRAME_LIMIT: usize = 2 * 1024 * 1024;
const PROBE_STREAM_KIND: u8 = 0;
const COMMAND_STREAM_KIND: u8 = 1;
const LIVE_ACTIVITY_STREAM_KIND: u8 = 2;
const BULK_TRANSFER_STREAM_KIND: u8 = 3;
const PROBE_RESPONSE_OK: u8 = 0;
const COMMAND_RESPONSE_OK: u8 = 0;
const COMMAND_RESPONSE_ERROR: u8 = 1;
const LIVE_ACTIVITY_RESPONSE_OK: u8 = 0;
const LIVE_ACTIVITY_RESPONSE_ERROR: u8 = 1;
const BULK_TRANSFER_RESPONSE_OK: u8 = 0;
const BULK_TRANSFER_RESPONSE_ERROR: u8 = 1;
const PROBE_STREAM_PRIORITY: i32 = 3;
const COMMAND_STREAM_PRIORITY: i32 = 2;
const LIVE_ACTIVITY_STREAM_PRIORITY: i32 = 1;
const BULK_TRANSFER_STREAM_PRIORITY: i32 = -1;
const SESSION_CLOSE_CODE: VarInt = VarInt::from_u32(1);

type DeviceId = [u8; 8];
type HmacSha256 = Hmac<Sha256>;

#[async_trait]
pub trait DeviceSessionCommandHandler: Send + Sync + 'static {
    async fn handle_command(&self, payload: &[u8]) -> Result<Vec<u8>, String>;
}

pub type DeviceSessionFrameStream = Pin<Box<dyn Stream<Item = Vec<u8>> + Send + 'static>>;

#[async_trait]
pub trait DeviceSessionLiveActivityHandler: Send + Sync + 'static {
    async fn subscribe_live_activity(
        &self,
        payload: &[u8],
    ) -> Result<DeviceSessionFrameStream, String>;
}

#[async_trait]
pub trait DeviceSessionBulkTransferHandler: Send + Sync + 'static {
    async fn handle_bulk_transfer(&self, stream: DeviceSessionBulkStream) -> Result<(), String>;
}

#[derive(Debug, Clone)]
pub struct DeviceSessionConfig {
    pub protocol_version: u16,
    pub capabilities: BTreeSet<String>,
    pub connect_timeout: Duration,
    pub handshake_timeout: Duration,
    pub network_change_probe_timeout: Duration,
    pub reconnect_attempts: usize,
    pub replacement_drain_timeout: Duration,
}

impl Default for DeviceSessionConfig {
    fn default() -> Self {
        Self {
            protocol_version: DEVICE_SESSION_PROTOCOL_VERSION,
            capabilities: BTreeSet::from([
                COMMAND_CAPABILITY.to_string(),
                LIVE_ACTIVITY_CAPABILITY.to_string(),
                BULK_TRANSFER_CAPABILITY.to_string(),
            ]),
            connect_timeout: Duration::from_secs(30),
            handshake_timeout: Duration::from_secs(30),
            network_change_probe_timeout: Duration::from_secs(1),
            reconnect_attempts: 1,
            replacement_drain_timeout: Duration::from_millis(500),
        }
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum DeviceSessionError {
    #[error("invalid Device id: {0}")]
    InvalidDeviceId(String),
    #[error("Device {0} is not paired")]
    PeerNotPaired(String),
    #[error("paired Device endpoint mismatch for {device_id}")]
    PeerEndpointMismatch { device_id: String },
    #[error("Device Session membership proof failed")]
    MembershipRejected,
    #[error("Device Session version incompatible: local={local_version}, remote={remote_version}")]
    VersionIncompatible {
        local_version: u16,
        remote_version: u16,
    },
    #[error("Device Session lacks required capability: {0}")]
    CapabilityMissing(String),
    #[error("Device Session command handler is unavailable")]
    CommandUnavailable,
    #[error("Device Session command rejected: {0}")]
    CommandRejected(String),
    #[error("Device Session Live Activity handler is unavailable")]
    LiveActivityUnavailable,
    #[error("Device Session Live Activity rejected: {0}")]
    LiveActivityRejected(String),
    #[error("Device Session Bulk Transfer handler is unavailable")]
    BulkTransferUnavailable,
    #[error("Device Session Bulk Transfer rejected: {0}")]
    BulkTransferRejected(String),
    #[error("Device Session frame exceeds {limit} bytes")]
    FrameTooLarge { limit: usize },
    #[error("Device Session timed out during {stage} (remaining budget: {budget_ms} ms)")]
    Timeout { stage: &'static str, budget_ms: u64 },
    #[error("Device Session connect failed: {0}")]
    Connect(String),
    #[error("Device Session closed: {0}")]
    Closed(String),
    #[error("Device Session IO failed: {0}")]
    Io(String),
    #[error("Device Session handshake failed: {0}")]
    Handshake(String),
    #[error("paired Device directory failed: {0}")]
    PeerDirectory(#[from] DevicePeerDirectoryError),
    #[error("Device Session was replaced before opening a stream")]
    Replaced,
}

impl DeviceSessionError {
    pub fn is_version_incompatible(&self) -> bool {
        matches!(self, Self::VersionIncompatible { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceConnectivityState {
    Connected,
    Connecting,
    NotConnected,
}

pub type DeviceSessionState = DeviceConnectivityState;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceConnectivityProjection {
    pub peer_device_id: String,
    pub peer_endpoint_id: String,
    pub state: DeviceConnectivityState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceSessionOrigin {
    Incoming,
    Outgoing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceSessionDiagnostics {
    pub peer_device_id: String,
    pub peer_endpoint_id: String,
    pub generation: Option<u64>,
    pub state: DeviceSessionState,
    pub origin: Option<DeviceSessionOrigin>,
    pub path_class: String,
    pub active_logical_streams: usize,
    pub established_at_ms: Option<u64>,
    pub last_used_at_ms: Option<u64>,
    pub state_changed_at_ms: Option<u64>,
    pub last_reconnect_at_ms: Option<u64>,
    pub rtt_ms: Option<u64>,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub reconnects: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceSessionManagerDiagnostics {
    pub physical_dial_attempts: u64,
    pub accepted_connections: u64,
    pub network_change_notifications: u64,
    pub network_change_probe_failures: u64,
    pub session_replacements: u64,
    pub sessions: Vec<DeviceSessionDiagnostics>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceSessionNetworkChangeReport {
    pub probed_sessions: usize,
    pub invalidated_sessions: usize,
}

pub struct DeviceSessionLiveStream {
    manager: DeviceSessionManager,
    session: Arc<DeviceSessionConnection>,
    recv: iroh::endpoint::RecvStream,
    _stream_guard: ActiveStreamGuard,
}

pub struct DeviceSessionBulkStream {
    session: Arc<DeviceSessionConnection>,
    send: iroh::endpoint::SendStream,
    recv: iroh::endpoint::RecvStream,
    _stream_guard: ActiveStreamGuard,
}

impl std::fmt::Debug for DeviceSessionBulkStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceSessionBulkStream")
            .field("peer_device_id", &hex::encode(self.session.peer_device_id))
            .field("generation", &self.session.generation)
            .finish()
    }
}

impl DeviceSessionBulkStream {
    pub fn peer_device_id(&self) -> String {
        hex::encode(self.session.peer_device_id)
    }

    pub fn generation(&self) -> u64 {
        self.session.generation
    }

    pub fn send_mut(&mut self) -> &mut iroh::endpoint::SendStream {
        &mut self.send
    }

    pub fn recv_mut(&mut self) -> &mut iroh::endpoint::RecvStream {
        &mut self.recv
    }

    pub fn split_mut(
        &mut self,
    ) -> (
        &mut iroh::endpoint::SendStream,
        &mut iroh::endpoint::RecvStream,
    ) {
        (&mut self.send, &mut self.recv)
    }

    pub fn finish(&mut self) -> Result<(), DeviceSessionError> {
        self.send
            .finish()
            .map_err(|error| DeviceSessionError::Closed(error.to_string()))
    }
}

impl std::fmt::Debug for DeviceSessionLiveStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceSessionLiveStream")
            .field("peer_device_id", &hex::encode(self.session.peer_device_id))
            .field("generation", &self.session.generation)
            .finish()
    }
}

impl DeviceSessionLiveStream {
    pub async fn next_frame(&mut self) -> Result<Option<Vec<u8>>, DeviceSessionError> {
        match read_optional_frame(&mut self.recv, LIVE_ACTIVITY_FRAME_LIMIT).await {
            Ok(frame) => Ok(frame),
            Err(error) => {
                if self.session.connection.close_reason().is_some() {
                    self.manager.invalidate_if_generation(
                        self.session.peer_device_id,
                        self.session.generation,
                        &error,
                    );
                }
                Err(error)
            }
        }
    }
}

#[derive(Debug, Clone)]
struct PeerStatus {
    endpoint_id: EndpointId,
    state: DeviceSessionState,
    reconnects: u64,
    state_changed_at_ms: u64,
    last_reconnect_at_ms: Option<u64>,
    version_incompatible: Option<(u16, u16)>,
    last_error: Option<String>,
}

#[derive(Clone)]
pub struct DeviceSessionManager {
    inner: Arc<DeviceSessionManagerInner>,
}

impl std::fmt::Debug for DeviceSessionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceSessionManager")
            .field(
                "local_device_id",
                &hex::encode(self.inner.identity.local_device_id()),
            )
            .field("diagnostics", &self.diagnostics())
            .finish()
    }
}

struct DeviceSessionManagerInner {
    endpoint: Endpoint,
    identity: DeviceNetworkIdentity,
    membership: BoundDeviceNetworkMembership,
    peer_directory: DevicePeerDirectory,
    peer_resolver: DevicePeerAddressResolver,
    command_handler: Option<Arc<dyn DeviceSessionCommandHandler>>,
    live_activity_handler: RwLock<Option<Arc<dyn DeviceSessionLiveActivityHandler>>>,
    bulk_transfer_handler: RwLock<Option<Arc<dyn DeviceSessionBulkTransferHandler>>>,
    config: DeviceSessionConfig,
    sessions: Mutex<HashMap<DeviceId, Arc<DeviceSessionConnection>>>,
    attempts: Mutex<HashMap<DeviceId, Arc<SessionAttempt>>>,
    cancellation: CancellationToken,
    tasks: TaskTracker,
    peer_status: Mutex<HashMap<DeviceId, PeerStatus>>,
    generation: AtomicU64,
    physical_dial_attempts: AtomicU64,
    accepted_connections: AtomicU64,
    network_change_notifications: AtomicU64,
    network_change_probe_failures: AtomicU64,
    session_replacements: AtomicU64,
    connectivity_revision: watch::Sender<u64>,
    shutdown: AtomicBool,
}

// An attempt belongs to the manager, never to whichever caller arrived first.
struct SessionAttempt {
    endpoint_id: EndpointId,
    interest: AtomicUsize,
    cancellation: CancellationToken,
    outcome: watch::Sender<Option<Result<Arc<DeviceSessionConnection>, DeviceSessionError>>>,
}

/// Retains connection demand independently of a short snapshot request.
/// Dropping the last interest cancels pending establishment and recovery.
pub struct DeviceSessionInterest {
    manager: DeviceSessionManager,
    peer: DeviceId,
    attempt: Arc<SessionAttempt>,
}

impl Drop for DeviceSessionInterest {
    fn drop(&mut self) {
        let mut attempts = lock(&self.manager.inner.attempts);
        if self.attempt.interest.fetch_sub(1, Ordering::SeqCst) != 1 {
            return;
        }
        // An authenticated incoming connection may already have admitted all
        // callers. Finish the bounded candidate arbitration before retiring its
        // outgoing counterpart: the other endpoint may still be selecting it.
        let arbitrating = self.attempt.outcome.borrow().is_none()
            && self.manager.active_session(self.peer).is_some();
        if arbitrating {
            return;
        }
        self.attempt.cancellation.cancel();
        if attempts
            .get(&self.peer)
            .is_some_and(|current| Arc::ptr_eq(current, &self.attempt))
        {
            attempts.remove(&self.peer);
            if self.attempt.outcome.borrow().is_none() {
                self.manager.set_peer_state(
                    self.peer,
                    self.attempt.endpoint_id,
                    DeviceSessionState::NotConnected,
                    None,
                    false,
                );
            }
        }
    }
}

struct DeviceSessionConnection {
    peer_device_id: DeviceId,
    peer_endpoint_id: EndpointId,
    generation: u64,
    origin: DeviceSessionOrigin,
    connection: Connection,
    active_streams: AtomicUsize,
    draining: AtomicBool,
    stream_idle: Notify,
    established_at_ms: u64,
    last_used_at_ms: AtomicU64,
}

impl std::fmt::Debug for DeviceSessionConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceSessionConnection")
            .field("peer_device_id", &hex::encode(self.peer_device_id))
            .field("peer_endpoint_id", &self.peer_endpoint_id)
            .field("generation", &self.generation)
            .field("origin", &self.origin)
            .field(
                "active_streams",
                &self.active_streams.load(Ordering::SeqCst),
            )
            .field("draining", &self.draining.load(Ordering::SeqCst))
            .finish()
    }
}

impl DeviceSessionConnection {
    fn begin_stream(self: &Arc<Self>) -> Result<ActiveStreamGuard, DeviceSessionError> {
        if self.draining.load(Ordering::SeqCst) {
            return Err(DeviceSessionError::Replaced);
        }
        if self.active_streams.fetch_add(1, Ordering::SeqCst) >= 128 {
            self.active_streams.fetch_sub(1, Ordering::SeqCst);
            return Err(DeviceSessionError::Closed(
                "Device Session stream capacity exhausted".into(),
            ));
        }
        if self.draining.load(Ordering::SeqCst) {
            if self.active_streams.fetch_sub(1, Ordering::SeqCst) == 1 {
                self.stream_idle.notify_waiters();
            }
            return Err(DeviceSessionError::Replaced);
        }
        self.last_used_at_ms.store(now_ms(), Ordering::SeqCst);
        Ok(ActiveStreamGuard {
            session: Arc::clone(self),
        })
    }

    fn begin_incoming_stream(self: &Arc<Self>) -> Result<ActiveStreamGuard, DeviceSessionError> {
        if self.active_streams.fetch_add(1, Ordering::SeqCst) >= 128 {
            self.active_streams.fetch_sub(1, Ordering::SeqCst);
            return Err(DeviceSessionError::Closed(
                "Device Session stream capacity exhausted".into(),
            ));
        }
        self.last_used_at_ms.store(now_ms(), Ordering::SeqCst);
        Ok(ActiveStreamGuard {
            session: self.clone(),
        })
    }

    fn path_diagnostics(&self) -> (String, Option<u64>) {
        self.connection
            .paths()
            .iter()
            .find(|path| path.is_selected())
            .map(|path| {
                if path.is_ip() {
                    ("direct".to_string(), Some(path.rtt().as_millis() as u64))
                } else if path.is_relay() {
                    ("relay".to_string(), Some(path.rtt().as_millis() as u64))
                } else {
                    ("other".to_string(), Some(path.rtt().as_millis() as u64))
                }
            })
            .unwrap_or_else(|| ("unknown".to_string(), None))
    }

    fn traffic_bytes(&self) -> (u64, u64) {
        let stats = self.connection.stats();
        (stats.udp_tx.bytes, stats.udp_rx.bytes)
    }

    fn selected_transport_addr(&self) -> Option<TransportAddr> {
        self.connection
            .paths()
            .iter()
            .find(|path| path.is_selected())
            .map(|path| path.remote_addr().clone())
    }
}

struct ActiveStreamGuard {
    session: Arc<DeviceSessionConnection>,
}

impl Drop for ActiveStreamGuard {
    fn drop(&mut self) {
        if self.session.active_streams.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.session.stream_idle.notify_waiters();
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct ClientHello {
    version: u16,
    device_id: DeviceId,
    network_id: [u8; 32],
    capabilities: BTreeSet<String>,
    nonce: [u8; 32],
    proof: [u8; 32],
}

#[derive(Debug, Serialize, Deserialize)]
struct ServerHello {
    accepted: bool,
    version: u16,
    device_id: DeviceId,
    network_id: [u8; 32],
    capabilities: BTreeSet<String>,
    nonce: [u8; 32],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    proof: Option<[u8; 32]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

impl DeviceSessionManager {
    pub(crate) fn new(
        endpoint: Endpoint,
        identity: DeviceNetworkIdentity,
        membership: BoundDeviceNetworkMembership,
        peer_directory: DevicePeerDirectory,
        peer_resolver: DevicePeerAddressResolver,
        command_handler: Option<Arc<dyn DeviceSessionCommandHandler>>,
        config: DeviceSessionConfig,
    ) -> Self {
        let (connectivity_revision, _) = watch::channel(0);
        Self {
            inner: Arc::new(DeviceSessionManagerInner {
                endpoint,
                identity,
                membership,
                peer_directory,
                peer_resolver,
                command_handler,
                live_activity_handler: RwLock::new(None),
                bulk_transfer_handler: RwLock::new(None),
                config,
                sessions: Mutex::new(HashMap::new()),
                attempts: Mutex::new(HashMap::new()),
                cancellation: CancellationToken::new(),
                tasks: TaskTracker::new(),
                peer_status: Mutex::new(HashMap::new()),
                generation: AtomicU64::new(0),
                physical_dial_attempts: AtomicU64::new(0),
                accepted_connections: AtomicU64::new(0),
                network_change_notifications: AtomicU64::new(0),
                network_change_probe_failures: AtomicU64::new(0),
                session_replacements: AtomicU64::new(0),
                connectivity_revision,
                shutdown: AtomicBool::new(false),
            }),
        }
    }

    pub fn diagnostics(&self) -> DeviceSessionManagerDiagnostics {
        let sessions = lock(&self.inner.sessions).clone();
        let statuses = lock(&self.inner.peer_status).clone();
        let connectivity = self.connectivity_from(&sessions, &statuses);
        let mut entries = Vec::with_capacity(connectivity.len());
        for projection in connectivity {
            let peer_device_id = parse_device_id(&projection.peer_device_id)
                .expect("connectivity projection contains a valid Device id");
            let session = sessions.get(&peer_device_id);
            let status = statuses.get(&peer_device_id);
            let (path_class, rtt_ms) = session
                .map(|session| session.path_diagnostics())
                .unwrap_or_else(|| ("unknown".to_string(), None));
            let (tx_bytes, rx_bytes) = session
                .map(|session| session.traffic_bytes())
                .unwrap_or_default();
            entries.push(DeviceSessionDiagnostics {
                peer_device_id: projection.peer_device_id,
                peer_endpoint_id: projection.peer_endpoint_id,
                generation: session.map(|session| session.generation),
                state: projection.state,
                origin: session.map(|session| session.origin),
                path_class,
                active_logical_streams: session
                    .map(|session| session.active_streams.load(Ordering::SeqCst))
                    .unwrap_or_default(),
                established_at_ms: session.map(|session| session.established_at_ms),
                last_used_at_ms: session
                    .map(|session| session.last_used_at_ms.load(Ordering::SeqCst)),
                state_changed_at_ms: status.map(|status| status.state_changed_at_ms),
                last_reconnect_at_ms: status.and_then(|status| status.last_reconnect_at_ms),
                rtt_ms,
                tx_bytes,
                rx_bytes,
                reconnects: status.map(|status| status.reconnects).unwrap_or_default(),
                last_error: status.and_then(|status| status.last_error.clone()),
            });
        }
        DeviceSessionManagerDiagnostics {
            physical_dial_attempts: self.inner.physical_dial_attempts.load(Ordering::SeqCst),
            accepted_connections: self.inner.accepted_connections.load(Ordering::SeqCst),
            network_change_notifications: self
                .inner
                .network_change_notifications
                .load(Ordering::SeqCst),
            network_change_probe_failures: self
                .inner
                .network_change_probe_failures
                .load(Ordering::SeqCst),
            session_replacements: self.inner.session_replacements.load(Ordering::SeqCst),
            sessions: entries,
        }
    }

    pub fn same_instance(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    pub fn connectivity(&self) -> Vec<DeviceConnectivityProjection> {
        let sessions = lock(&self.inner.sessions).clone();
        let statuses = lock(&self.inner.peer_status).clone();
        self.connectivity_from(&sessions, &statuses)
    }

    pub fn connectivity_state(
        &self,
        peer_device_id: &str,
    ) -> Result<DeviceConnectivityState, DeviceSessionError> {
        let peer_device_id = parse_device_id(peer_device_id)?;
        let sessions = lock(&self.inner.sessions);
        let statuses = lock(&self.inner.peer_status);
        Ok(connectivity_state_for(
            sessions.get(&peer_device_id),
            statuses.get(&peer_device_id),
        ))
    }

    pub fn subscribe_connectivity_changes(&self) -> watch::Receiver<u64> {
        self.inner.connectivity_revision.subscribe()
    }

    pub async fn notify_network_change(&self) -> DeviceSessionNetworkChangeReport {
        self.inner
            .network_change_notifications
            .fetch_add(1, Ordering::SeqCst);
        tracing::info!("Device Session host network-change notification");
        self.inner.endpoint.network_change().await;

        let sessions = lock(&self.inner.sessions)
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let probed_sessions = sessions.len();
        let probe_timeout = self.inner.config.network_change_probe_timeout;
        let results = futures::future::join_all(sessions.into_iter().map(|session| {
            let manager = self.clone();
            async move {
                let deadline = Instant::now() + probe_timeout;
                let result = timeout_at(
                    deadline,
                    "network-change probe",
                    manager.execute_probe(&session),
                )
                .await;
                match result {
                    Ok(Ok(())) => false,
                    Ok(Err(error)) | Err(error) => {
                        manager.invalidate_if_generation(
                            session.peer_device_id,
                            session.generation,
                            &error,
                        );
                        true
                    }
                }
            }
        }))
        .await;
        let invalidated_sessions = results
            .into_iter()
            .filter(|invalidated| *invalidated)
            .count();
        self.inner
            .network_change_probe_failures
            .fetch_add(invalidated_sessions as u64, Ordering::SeqCst);
        DeviceSessionNetworkChangeReport {
            probed_sessions,
            invalidated_sessions,
        }
    }

    fn connectivity_from(
        &self,
        sessions: &HashMap<DeviceId, Arc<DeviceSessionConnection>>,
        statuses: &HashMap<DeviceId, PeerStatus>,
    ) -> Vec<DeviceConnectivityProjection> {
        let mut peers = BTreeMap::<DeviceId, EndpointId>::new();
        match self.inner.peer_directory.read() {
            Ok(directory) => {
                for device in directory.devices {
                    if device.device_id == self.inner.identity.local_device_id() {
                        continue;
                    }
                    match EndpointId::from_bytes(&device.public_key) {
                        Ok(endpoint_id) => {
                            peers.insert(device.device_id, endpoint_id);
                        }
                        Err(error) => {
                            tracing::warn!(
                                peer_device_id = %hex::encode(device.device_id),
                                error = %error,
                                "ignored paired Device with invalid endpoint identity"
                            );
                        }
                    }
                }
            }
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "failed to read paired Device directory for connectivity projection"
                );
                for (peer_device_id, session) in sessions {
                    peers.insert(*peer_device_id, session.peer_endpoint_id);
                }
                for (peer_device_id, status) in statuses {
                    peers.entry(*peer_device_id).or_insert(status.endpoint_id);
                }
            }
        }
        peers
            .into_iter()
            .map(
                |(peer_device_id, peer_endpoint_id)| DeviceConnectivityProjection {
                    peer_device_id: hex::encode(peer_device_id),
                    peer_endpoint_id: peer_endpoint_id.to_string(),
                    state: connectivity_state_for(
                        sessions.get(&peer_device_id),
                        statuses.get(&peer_device_id),
                    ),
                },
            )
            .collect()
    }

    fn version_incompatible_error(&self, peer_device_id: DeviceId) -> Option<DeviceSessionError> {
        let statuses = lock(&self.inner.peer_status);
        let (local_version, remote_version) = statuses.get(&peer_device_id)?.version_incompatible?;
        Some(DeviceSessionError::VersionIncompatible {
            local_version,
            remote_version,
        })
    }

    pub fn set_live_activity_handler(&self, handler: Arc<dyn DeviceSessionLiveActivityHandler>) {
        *write_lock(&self.inner.live_activity_handler) = Some(handler);
    }

    pub fn set_bulk_transfer_handler(&self, handler: Arc<dyn DeviceSessionBulkTransferHandler>) {
        *write_lock(&self.inner.bulk_transfer_handler) = Some(handler);
    }

    /// `timeout` bounds admission and the logical operation together.
    pub async fn command_to(
        &self,
        peer_device_id: &str,
        payload: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>, DeviceSessionError> {
        let parsed_device_id = parse_device_id(peer_device_id)?;
        let deadline = Instant::now() + timeout;
        let peer_addr = timeout_at(
            deadline,
            "peer resolution",
            self.peer_addr(parsed_device_id),
        )
        .await??;
        let timeout = deadline.saturating_duration_since(Instant::now());
        self.command(peer_device_id, peer_addr, payload, timeout)
            .await
    }

    /// `timeout` bounds admission and the logical operation together.
    pub async fn command_once_to(
        &self,
        peer_device_id: &str,
        payload: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>, DeviceSessionError> {
        let parsed_device_id = parse_device_id(peer_device_id)?;
        let deadline = Instant::now() + timeout;
        let peer_addr = timeout_at(
            deadline,
            "peer resolution",
            self.peer_addr(parsed_device_id),
        )
        .await??;
        let timeout = deadline.saturating_duration_since(Instant::now());
        self.command_with_reconnect_attempts(peer_device_id, peer_addr, payload, timeout, 0)
            .await
    }

    pub async fn command(
        &self,
        peer_device_id: &str,
        peer_addr: EndpointAddr,
        payload: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>, DeviceSessionError> {
        self.command_with_reconnect_attempts(
            peer_device_id,
            peer_addr,
            payload,
            timeout,
            self.inner.config.reconnect_attempts,
        )
        .await
    }

    async fn command_with_reconnect_attempts(
        &self,
        peer_device_id: &str,
        peer_addr: EndpointAddr,
        payload: &[u8],
        timeout: Duration,
        _reconnect_attempts: usize,
    ) -> Result<Vec<u8>, DeviceSessionError> {
        let peer = parse_device_id(peer_device_id)?;
        if let Some(error) = self.version_incompatible_error(peer) {
            return Err(error);
        }
        let deadline = Instant::now() + timeout;
        loop {
            let session = self
                .get_or_connect(peer, peer_addr.clone(), deadline)
                .await?;
            match timeout_at(
                deadline,
                "command exchange",
                self.execute_command(&session, payload),
            )
            .await
            {
                Ok(Ok(response)) => return Ok(response),
                // begin_stream rejected this generation BEFORE opening a stream:
                // reacquisition is safe and is not replay of a sent command.
                Ok(Err(DeviceSessionError::Replaced)) => continue,
                Ok(Err(error)) => {
                    if session.connection.close_reason().is_some() {
                        self.invalidate_if_generation(peer, session.generation, &error);
                    }
                    return Err(error);
                }
                Err(error) => {
                    let manager = self.clone();
                    self.inner.tasks.spawn(async move {
                        tokio::select! {
                            _ = manager.inner.cancellation.cancelled() => {},
                            _ = manager.preserve_responsive_generation_after_timeout(&session) => {},
                        }
                    });
                    return Err(error);
                }
            }
        }
    }

    /// `timeout` bounds admission and the logical operation together.
    pub async fn open_live_activity_to(
        &self,
        peer_device_id: &str,
        payload: &[u8],
        timeout: Duration,
    ) -> Result<DeviceSessionLiveStream, DeviceSessionError> {
        let parsed_device_id = parse_device_id(peer_device_id)?;
        let deadline = Instant::now() + timeout;
        let peer_addr = timeout_at(
            deadline,
            "peer resolution",
            self.peer_addr(parsed_device_id),
        )
        .await??;
        let timeout = deadline.saturating_duration_since(Instant::now());
        self.open_live_activity(peer_device_id, peer_addr, payload, timeout)
            .await
    }

    pub async fn open_live_activity(
        &self,
        peer_device_id: &str,
        peer_addr: EndpointAddr,
        payload: &[u8],
        timeout: Duration,
    ) -> Result<DeviceSessionLiveStream, DeviceSessionError> {
        let peer_device_id = parse_device_id(peer_device_id)?;
        if let Some(error) = self.version_incompatible_error(peer_device_id) {
            return Err(error);
        }
        let deadline = Instant::now() + timeout;
        let mut last_error = None;
        for attempt in 0..=self.inner.config.reconnect_attempts {
            let session = self
                .get_or_connect(peer_device_id, peer_addr.clone(), deadline)
                .await?;
            match timeout_at(
                deadline,
                "Live Activity subscription",
                self.execute_live_activity(&session, payload),
            )
            .await
            {
                Ok(Ok(stream)) => {
                    self.set_peer_state(
                        peer_device_id,
                        peer_addr.id,
                        DeviceSessionState::Connected,
                        None,
                        false,
                    );
                    return Ok(stream);
                }
                Ok(Err(error @ DeviceSessionError::LiveActivityRejected(_)))
                | Ok(Err(error @ DeviceSessionError::FrameTooLarge { .. })) => {
                    return Err(error);
                }
                Err(error) => {
                    last_error = Some(error.to_string());
                    if self
                        .preserve_responsive_generation_after_timeout(&session)
                        .await
                    {
                        return Err(error);
                    }
                    if attempt == self.inner.config.reconnect_attempts || Instant::now() >= deadline
                    {
                        return Err(error);
                    }
                }
                Ok(Err(error)) => {
                    last_error = Some(error.to_string());
                    self.invalidate_if_generation(peer_device_id, session.generation, &error);
                    if attempt == self.inner.config.reconnect_attempts || Instant::now() >= deadline
                    {
                        return Err(error);
                    }
                }
            }
        }
        Err(DeviceSessionError::Closed(
            last_error.unwrap_or_else(|| "Live Activity failed".to_string()),
        ))
    }

    /// `timeout` bounds admission and the logical operation together.
    pub async fn open_bulk_transfer_to(
        &self,
        peer_device_id: &str,
        timeout: Duration,
    ) -> Result<DeviceSessionBulkStream, DeviceSessionError> {
        let parsed_device_id = parse_device_id(peer_device_id)?;
        let deadline = Instant::now() + timeout;
        let peer_addr = timeout_at(
            deadline,
            "peer resolution",
            self.peer_addr(parsed_device_id),
        )
        .await??;
        let timeout = deadline.saturating_duration_since(Instant::now());
        self.open_bulk_transfer(peer_device_id, peer_addr, timeout)
            .await
    }

    pub async fn open_bulk_transfer(
        &self,
        peer_device_id: &str,
        peer_addr: EndpointAddr,
        timeout: Duration,
    ) -> Result<DeviceSessionBulkStream, DeviceSessionError> {
        let peer_device_id = parse_device_id(peer_device_id)?;
        if let Some(error) = self.version_incompatible_error(peer_device_id) {
            return Err(error);
        }
        let deadline = Instant::now() + timeout;
        let mut last_error = None;
        for attempt in 0..=self.inner.config.reconnect_attempts {
            let session = self
                .get_or_connect(peer_device_id, peer_addr.clone(), deadline)
                .await?;
            match timeout_at(
                deadline,
                "Bulk Transfer stream opening",
                self.execute_bulk_transfer(&session),
            )
            .await
            {
                Ok(Ok(stream)) => {
                    self.set_peer_state(
                        peer_device_id,
                        peer_addr.id,
                        DeviceSessionState::Connected,
                        None,
                        false,
                    );
                    return Ok(stream);
                }
                Ok(Err(error @ DeviceSessionError::BulkTransferRejected(_)))
                | Ok(Err(error @ DeviceSessionError::FrameTooLarge { .. })) => {
                    return Err(error);
                }
                Err(error) => {
                    last_error = Some(error.to_string());
                    if self
                        .preserve_responsive_generation_after_timeout(&session)
                        .await
                    {
                        return Err(error);
                    }
                    if attempt == self.inner.config.reconnect_attempts || Instant::now() >= deadline
                    {
                        return Err(error);
                    }
                }
                Ok(Err(error)) => {
                    last_error = Some(error.to_string());
                    self.invalidate_if_generation(peer_device_id, session.generation, &error);
                    if attempt == self.inner.config.reconnect_attempts || Instant::now() >= deadline
                    {
                        return Err(error);
                    }
                }
            }
        }
        Err(DeviceSessionError::Closed(
            last_error.unwrap_or_else(|| "Bulk Transfer failed".to_string()),
        ))
    }

    async fn peer_addr(
        &self,
        peer_device_id: DeviceId,
    ) -> Result<EndpointAddr, DeviceSessionError> {
        let proven_addr = self
            .active_session(peer_device_id)
            .and_then(|session| session.selected_transport_addr());
        self.inner
            .peer_resolver
            .resolve_by_device_id(&peer_device_id, proven_addr)
            .await?
            .ok_or_else(|| DeviceSessionError::PeerNotPaired(hex::encode(peer_device_id)))
    }

    pub(crate) fn proven_transport_addr(&self, endpoint_id: EndpointId) -> Option<TransportAddr> {
        lock(&self.inner.sessions)
            .values()
            .find(|session| {
                session.peer_endpoint_id == endpoint_id
                    && !session.draining.load(Ordering::SeqCst)
                    && session.connection.close_reason().is_none()
            })
            .and_then(|session| session.selected_transport_addr())
    }

    pub async fn disconnect_peer(&self, peer_device_id: &str) -> Result<(), DeviceSessionError> {
        let peer_device_id = parse_device_id(peer_device_id)?;
        let mut attempts = lock(&self.inner.attempts);
        let removed_session = lock(&self.inner.sessions).remove(&peer_device_id);
        if let Some(session) = removed_session.as_ref() {
            session.draining.store(true, Ordering::SeqCst);
            session
                .connection
                .close(SESSION_CLOSE_CODE, b"Device Session disconnected");
        }
        let removed_status = lock(&self.inner.peer_status).remove(&peer_device_id);
        if let Some(attempt) = attempts.remove(&peer_device_id) {
            attempt.cancellation.cancel();
        }
        if removed_session.is_some()
            || removed_status
                .as_ref()
                .is_some_and(|status| status.state != DeviceSessionState::NotConnected)
        {
            self.notify_connectivity_changed();
        }
        Ok(())
    }

    pub fn reconcile_peer_directory(&self) -> Result<Vec<String>, DeviceSessionError> {
        let peers = self.inner.peer_directory.read()?;
        let allowed = peers
            .devices
            .into_iter()
            .map(|peer| (peer.device_id, peer.public_key))
            .collect::<HashMap<_, _>>();
        let mut invalidated = BTreeSet::new();
        let mut attempts = lock(&self.inner.attempts);

        lock(&self.inner.sessions).retain(|device_id, session| {
            let valid = allowed
                .get(device_id)
                .is_some_and(|public_key| public_key == session.peer_endpoint_id.as_bytes());
            if !valid {
                invalidated.insert(hex::encode(device_id));
                session.draining.store(true, Ordering::SeqCst);
                session.connection.close(
                    SESSION_CLOSE_CODE,
                    b"paired Device removed or identity changed",
                );
            }
            valid
        });
        lock(&self.inner.peer_status).retain(|device_id, status| {
            let valid = allowed
                .get(device_id)
                .is_some_and(|public_key| public_key == status.endpoint_id.as_bytes());
            if !valid {
                invalidated.insert(hex::encode(device_id));
            }
            valid
        });
        attempts.retain(|device_id, attempt| {
            let valid = allowed
                .get(device_id)
                .is_some_and(|key| key == attempt.endpoint_id.as_bytes());
            if !valid {
                attempt.cancellation.cancel();
                invalidated.insert(hex::encode(device_id));
            }
            valid
        });

        let invalidated = invalidated.into_iter().collect::<Vec<_>>();
        if !invalidated.is_empty() {
            self.notify_connectivity_changed();
        }
        Ok(invalidated)
    }

    pub async fn shutdown(&self) {
        if self.inner.shutdown.swap(true, Ordering::SeqCst) {
            return;
        }
        self.inner.cancellation.cancel();
        self.inner.tasks.close();
        lock(&self.inner.attempts).clear();
        let sessions = lock(&self.inner.sessions)
            .drain()
            .map(|(_, session)| session)
            .collect::<Vec<_>>();
        let had_sessions = !sessions.is_empty();
        for session in sessions {
            session.draining.store(true, Ordering::SeqCst);
            session
                .connection
                .close(SESSION_CLOSE_CODE, b"Device Session Manager shutdown");
        }
        if had_sessions {
            self.notify_connectivity_changed();
        }
        self.inner.tasks.wait().await;
    }

    pub(crate) async fn accept_incoming(
        &self,
        connection: Connection,
    ) -> Result<(), DeviceSessionError> {
        self.inner
            .accepted_connections
            .fetch_add(1, Ordering::SeqCst);
        let handshake = tokio::select! {
            _ = self.inner.cancellation.cancelled() => {
                connection.close(SESSION_CLOSE_CODE, b"Device Session Manager shutdown");
                return Ok(());
            }
            result = timeout_at(Instant::now() + self.inner.config.handshake_timeout,
                "incoming authentication", self.accept_handshake(&connection)) => result?,
        };
        let Some(peer_device_id) = handshake? else {
            return Ok(());
        };
        let installed = {
            let _attempts = lock(&self.inner.attempts);
            if self.inner.shutdown.load(Ordering::SeqCst) {
                connection.close(SESSION_CLOSE_CODE, b"Device Session Manager shutdown");
                return Ok(());
            }
            self.validate_peer_addr(peer_device_id, &EndpointAddr::new(connection.remote_id()))?;
            self.install_candidate(peer_device_id, connection, DeviceSessionOrigin::Incoming)
        };
        if !installed.candidate_selected {
            return Ok(());
        }
        self.serve_connection(installed.active).await;
        Ok(())
    }

    /// A view can retain demand while its individual RPC budgets expire.
    pub async fn retain_peer(
        &self,
        peer_device_id: &str,
    ) -> Result<DeviceSessionInterest, DeviceSessionError> {
        let peer = parse_device_id(peer_device_id)?;
        let addr = self.peer_addr(peer).await?;
        self.acquire_interest(peer, addr)
    }

    fn acquire_interest(
        &self,
        peer: DeviceId,
        addr: EndpointAddr,
    ) -> Result<DeviceSessionInterest, DeviceSessionError> {
        self.validate_peer_addr(peer, &addr)?;
        let mut attempts = lock(&self.inner.attempts);
        if self.inner.shutdown.load(Ordering::SeqCst) {
            return Err(DeviceSessionError::Closed(
                "Device Session Manager is shut down".into(),
            ));
        }
        if let Some(error) = self.version_incompatible_error(peer) {
            return Err(error);
        }
        if attempts.len() >= 256 && !attempts.contains_key(&peer) {
            return Err(DeviceSessionError::Closed(
                "Device Session admission capacity exhausted".into(),
            ));
        }
        let attempt = attempts
            .entry(peer)
            .or_insert_with(|| {
                let attempt = Arc::new(SessionAttempt {
                    endpoint_id: addr.id,
                    interest: AtomicUsize::new(0),
                    cancellation: self.inner.cancellation.child_token(),
                    outcome: watch::channel(None).0,
                });
                let manager = self.clone();
                let task_attempt = attempt.clone();
                self.inner.tasks.spawn(async move {
                    manager.run_attempt(peer, addr, task_attempt).await;
                });
                attempt
            })
            .clone();
        if attempt.interest.load(Ordering::SeqCst) >= 256 {
            return Err(DeviceSessionError::Closed(
                "Device Session peer admission capacity exhausted".into(),
            ));
        }
        attempt.interest.fetch_add(1, Ordering::SeqCst);
        Ok(DeviceSessionInterest {
            manager: self.clone(),
            peer,
            attempt,
        })
    }

    async fn get_or_connect(
        &self,
        peer_device_id: DeviceId,
        peer_addr: EndpointAddr,
        deadline: Instant,
    ) -> Result<Arc<DeviceSessionConnection>, DeviceSessionError> {
        // An expired intent must neither start a dial nor later open a stream.
        timeout_at(deadline, "Device Session admission", async {}).await?;
        self.validate_peer_addr(peer_device_id, &peer_addr)?;
        if self.inner.shutdown.load(Ordering::SeqCst) {
            return Err(DeviceSessionError::Closed(
                "Device Session Manager is shut down".into(),
            ));
        }
        if let Some(session) = self.active_session(peer_device_id) {
            return Ok(session);
        }
        let interest = self.acquire_interest(peer_device_id, peer_addr)?;
        let mut result = interest.attempt.outcome.subscribe();
        let mut connectivity = self.subscribe_connectivity_changes();
        timeout_at(deadline, "Device Session admission", async {
            loop {
                if let Some(session) = self.active_session(peer_device_id) { return Ok(session); }
                if let Some(outcome) = result.borrow().clone() {
                    match outcome {
                        Ok(session) if session.connection.close_reason().is_none() && !session.draining.load(Ordering::SeqCst) => return Ok(session),
                        Err(error) => return Err(error),
                        _ => {},
                    }
                }
                tokio::select! {
                    _ = interest.attempt.cancellation.cancelled() => return Err(DeviceSessionError::Closed("Device Session admission cancelled".into())),
                    _ = result.changed() => {},
                    _ = connectivity.changed() => {},
                }
            }
        }).await?
    }

    async fn run_attempt(&self, peer: DeviceId, addr: EndpointAddr, attempt: Arc<SessionAttempt>) {
        loop {
            let started = Instant::now();
            let work = async {
                if let Some(session) = self.active_session(peer) {
                    return Ok(session);
                }
                for retry in 0..=self.inner.config.reconnect_attempts {
                    self.set_peer_state(
                        peer,
                        addr.id,
                        DeviceSessionState::Connecting,
                        None,
                        retry > 0,
                    );
                    let result = self.establish(peer, addr.clone(), &attempt).await;
                    // A failed competing candidate must never downgrade a
                    // healthy incoming session, or trigger another dial.
                    if let Some(session) = self.active_session(peer) {
                        return Ok(session);
                    }
                    match result {
                        Ok(session) => return Ok(session),
                        Err(error) => {
                            tracing::info!(peer = %hex::encode(peer), retry, elapsed_ms = started.elapsed().as_millis(), error = %error, "Device Session attempt failed");
                            let retryable = matches!(
                                error,
                                DeviceSessionError::Connect(_)
                                    | DeviceSessionError::Io(_)
                                    | DeviceSessionError::Timeout { .. }
                                    | DeviceSessionError::Closed(_)
                            );
                            if !retryable || retry == self.inner.config.reconnect_attempts {
                                return Err(error);
                            }
                            tokio::time::sleep(Duration::from_millis(250 * (1u64 << retry.min(4))))
                                .await;
                        }
                    }
                }
                unreachable!()
            };
            let result = tokio::select! {
                biased;
                _ = attempt.cancellation.cancelled() => {
                    tracing::info!(peer = %hex::encode(peer), elapsed_ms = started.elapsed().as_millis(), "Device Session attempt cancelled");
                    return;
                },
                result = work => result,
            };
            {
                let mut attempts = lock(&self.inner.attempts);
                if !attempts
                    .get(&peer)
                    .is_some_and(|current| Arc::ptr_eq(current, &attempt))
                {
                    return;
                }
                attempt.outcome.send_replace(Some(result.clone()));
                if let Err(error) = &result {
                    if let DeviceSessionError::VersionIncompatible {
                        local_version,
                        remote_version,
                    } = error
                    {
                        self.set_version_incompatible(
                            peer,
                            addr.id,
                            *local_version,
                            *remote_version,
                        );
                    } else {
                        self.set_peer_state(
                            peer,
                            addr.id,
                            DeviceSessionState::NotConnected,
                            Some(error.to_string()),
                            false,
                        );
                    }
                }
                if attempt.interest.load(Ordering::SeqCst) == 0 {
                    attempts.remove(&peer);
                    return;
                }
            }
            tracing::info!(peer = %hex::encode(peer), elapsed_ms = started.elapsed().as_millis(), success = result.is_ok(), "Device Session admission completed");
            let Ok(session) = result else {
                return;
            };
            // A retained viewer supplies demand across individual RPC deadlines
            // and transport loss. Only this task schedules connection recovery.
            tokio::select! {
                _ = attempt.cancellation.cancelled() => return,
                _ = session.connection.closed() => {},
            }
            attempt.outcome.send_replace(None);
        }
    }

    async fn establish(
        &self,
        peer: DeviceId,
        addr: EndpointAddr,
        attempt: &Arc<SessionAttempt>,
    ) -> Result<Arc<DeviceSessionConnection>, DeviceSessionError> {
        self.validate_peer_addr(peer, &addr)?;
        self.inner
            .physical_dial_attempts
            .fetch_add(1, Ordering::SeqCst);
        let deadline = Instant::now()
            + self.inner.config.connect_timeout
            + self.inner.config.handshake_timeout;
        let started = Instant::now();
        tracing::info!(peer = %hex::encode(peer), "Device Session transport dial started");
        let connection = timeout_at(
            Instant::now() + self.inner.config.connect_timeout,
            "QUIC connection establishment",
            self.inner
                .endpoint
                .connect(addr.clone(), DEVICE_SESSION_ALPN),
        )
        .await?
        .map_err(|error| DeviceSessionError::Connect(error.to_string()))?;
        tracing::info!(peer = %hex::encode(peer), elapsed_ms = started.elapsed().as_millis(), "Device Session transport connected; authenticating");
        if let Err(error) = self.outgoing_handshake(&connection, peer, deadline).await {
            connection.close(SESSION_CLOSE_CODE, b"Device Session handshake failed");
            return Err(error);
        }
        let attempts = lock(&self.inner.attempts);
        if attempt.cancellation.is_cancelled()
            || self.inner.shutdown.load(Ordering::SeqCst)
            || !attempts
                .get(&peer)
                .is_some_and(|current| Arc::ptr_eq(current, attempt))
        {
            connection.close(SESSION_CLOSE_CODE, b"obsolete Device Session attempt");
            return Err(DeviceSessionError::Closed(
                "obsolete Device Session attempt".into(),
            ));
        }
        if let Err(error) = self.validate_peer_addr(peer, &addr) {
            connection.close(SESSION_CLOSE_CODE, b"Device no longer paired");
            return Err(error);
        }
        let installed = self.install_candidate(peer, connection, DeviceSessionOrigin::Outgoing);
        if installed.candidate_selected {
            let manager = self.clone();
            let session = installed.active.clone();
            self.inner.tasks.spawn(async move {
                tokio::select! {
                    _ = manager.inner.cancellation.cancelled() => {},
                    _ = manager.serve_connection(session) => {},
                }
            });
        }
        Ok(installed.active)
    }

    fn validate_peer_addr(
        &self,
        peer_device_id: DeviceId,
        peer_addr: &EndpointAddr,
    ) -> Result<(), DeviceSessionError> {
        let peer = self
            .inner
            .peer_directory
            .peer_by_device_id(&peer_device_id)?
            .ok_or_else(|| DeviceSessionError::PeerNotPaired(hex::encode(peer_device_id)))?;
        if peer.public_key != *peer_addr.id.as_bytes() {
            return Err(DeviceSessionError::PeerEndpointMismatch {
                device_id: hex::encode(peer_device_id),
            });
        }
        Ok(())
    }

    fn active_session(&self, peer_device_id: DeviceId) -> Option<Arc<DeviceSessionConnection>> {
        let mut sessions = lock(&self.inner.sessions);
        let session = sessions.get(&peer_device_id).cloned()?;
        if session.draining.load(Ordering::SeqCst) || session.connection.close_reason().is_some() {
            sessions.remove(&peer_device_id);
            return None;
        }
        Some(session)
    }

    fn install_candidate(
        &self,
        peer_device_id: DeviceId,
        connection: Connection,
        origin: DeviceSessionOrigin,
    ) -> InstallOutcome {
        let peer_endpoint_id = connection.remote_id();
        let preferred_origin = preferred_origin(self.inner.endpoint.id(), peer_endpoint_id);
        let generation = self.inner.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let now = now_ms();
        let candidate = Arc::new(DeviceSessionConnection {
            peer_device_id,
            peer_endpoint_id,
            generation,
            origin,
            connection,
            active_streams: AtomicUsize::new(0),
            draining: AtomicBool::new(false),
            stream_idle: Notify::new(),
            established_at_ms: now,
            last_used_at_ms: AtomicU64::new(now),
        });
        let mut sessions = lock(&self.inner.sessions);
        if let Some(existing) = sessions.get(&peer_device_id).cloned() {
            let existing_usable = !existing.draining.load(Ordering::SeqCst)
                && existing.connection.close_reason().is_none();
            let candidate_wins = !existing_usable
                || existing.origin == origin
                || (existing.origin != preferred_origin && origin == preferred_origin);
            if !candidate_wins {
                let manager = self.clone();
                let loser = candidate.clone();
                self.inner.tasks.spawn(async move {
                    manager.serve_connection(loser).await;
                });
                self.retire_candidate(candidate);
                return InstallOutcome {
                    active: existing,
                    candidate_selected: false,
                };
            }
            self.inner
                .session_replacements
                .fetch_add(1, Ordering::SeqCst);
            self.retire_candidate(existing);
        }
        sessions.insert(peer_device_id, Arc::clone(&candidate));
        drop(sessions);
        let state_changed = self.set_peer_state(
            peer_device_id,
            peer_endpoint_id,
            DeviceSessionState::Connected,
            None,
            false,
        );
        if !state_changed {
            self.notify_connectivity_changed();
        }
        InstallOutcome {
            active: candidate,
            candidate_selected: true,
        }
    }

    fn retire_candidate(&self, session: Arc<DeviceSessionConnection>) {
        // The peer can have authenticated this candidate before learning the
        // winning generation. Continue serving that bounded overlap; closing an
        // apparently idle loser immediately races its first command in flight.
        session.draining.store(true, Ordering::SeqCst);
        let cancellation = self.inner.cancellation.child_token();
        let grace = self.inner.config.replacement_drain_timeout;
        self.inner.tasks.spawn(async move {
            tokio::select! {
                _ = cancellation.cancelled() => {},
                _ = tokio::time::sleep(grace) => {},
            }
            session
                .connection
                .close(SESSION_CLOSE_CODE, b"duplicate Device Session drained");
        });
    }

    async fn execute_probe(
        &self,
        session: &Arc<DeviceSessionConnection>,
    ) -> Result<(), DeviceSessionError> {
        let _stream_guard = session.begin_stream()?;
        let (mut send, mut recv) = session
            .connection
            .open_bi()
            .await
            .map_err(|error| DeviceSessionError::Closed(error.to_string()))?;
        send.set_priority(PROBE_STREAM_PRIORITY)
            .map_err(|error| DeviceSessionError::Closed(error.to_string()))?;
        send.write_all(&[PROBE_STREAM_KIND])
            .await
            .map_err(|error| DeviceSessionError::Io(error.to_string()))?;
        send.finish()
            .map_err(|error| DeviceSessionError::Closed(error.to_string()))?;
        let mut response = [0u8; 1];
        recv.read_exact(&mut response)
            .await
            .map_err(|error| DeviceSessionError::Io(error.to_string()))?;
        if response[0] != PROBE_RESPONSE_OK {
            return Err(DeviceSessionError::Handshake(format!(
                "unknown probe response status {}",
                response[0]
            )));
        }
        Ok(())
    }

    async fn preserve_responsive_generation_after_timeout(
        &self,
        session: &Arc<DeviceSessionConnection>,
    ) -> bool {
        let deadline = Instant::now() + self.inner.config.network_change_probe_timeout;
        match timeout_at(
            deadline,
            "session health probe",
            self.execute_probe(session),
        )
        .await
        {
            Ok(Ok(())) => {
                self.set_peer_state(
                    session.peer_device_id,
                    session.peer_endpoint_id,
                    DeviceSessionState::Connected,
                    None,
                    false,
                );
                true
            }
            Ok(Err(error)) | Err(error) => {
                self.invalidate_if_generation(session.peer_device_id, session.generation, &error);
                false
            }
        }
    }

    async fn execute_command(
        &self,
        session: &Arc<DeviceSessionConnection>,
        payload: &[u8],
    ) -> Result<Vec<u8>, DeviceSessionError> {
        let _stream_guard = session.begin_stream()?;
        let (mut send, mut recv) = session
            .connection
            .open_bi()
            .await
            .map_err(|error| DeviceSessionError::Closed(error.to_string()))?;
        send.set_priority(COMMAND_STREAM_PRIORITY)
            .map_err(|error| DeviceSessionError::Closed(error.to_string()))?;
        send.write_all(&[COMMAND_STREAM_KIND])
            .await
            .map_err(|error| DeviceSessionError::Io(error.to_string()))?;
        write_frame(&mut send, payload, COMMAND_FRAME_LIMIT).await?;
        send.finish()
            .map_err(|error| DeviceSessionError::Closed(error.to_string()))?;
        let mut status = [0u8; 1];
        recv.read_exact(&mut status)
            .await
            .map_err(|error| DeviceSessionError::Io(error.to_string()))?;
        let response = read_frame(&mut recv, COMMAND_FRAME_LIMIT).await?;
        match status[0] {
            COMMAND_RESPONSE_OK => Ok(response),
            COMMAND_RESPONSE_ERROR => Err(DeviceSessionError::CommandRejected(
                String::from_utf8_lossy(&response).into_owned(),
            )),
            other => Err(DeviceSessionError::Handshake(format!(
                "unknown Command response status {other}"
            ))),
        }
    }

    async fn execute_live_activity(
        &self,
        session: &Arc<DeviceSessionConnection>,
        payload: &[u8],
    ) -> Result<DeviceSessionLiveStream, DeviceSessionError> {
        let stream_guard = session.begin_stream()?;
        let (mut send, mut recv) = session
            .connection
            .open_bi()
            .await
            .map_err(|error| DeviceSessionError::Closed(error.to_string()))?;
        send.set_priority(LIVE_ACTIVITY_STREAM_PRIORITY)
            .map_err(|error| DeviceSessionError::Closed(error.to_string()))?;
        send.write_all(&[LIVE_ACTIVITY_STREAM_KIND])
            .await
            .map_err(|error| DeviceSessionError::Io(error.to_string()))?;
        write_frame(&mut send, payload, LIVE_ACTIVITY_FRAME_LIMIT).await?;
        send.finish()
            .map_err(|error| DeviceSessionError::Closed(error.to_string()))?;
        let mut status = [0u8; 1];
        recv.read_exact(&mut status)
            .await
            .map_err(|error| DeviceSessionError::Io(error.to_string()))?;
        match status[0] {
            LIVE_ACTIVITY_RESPONSE_OK => Ok(DeviceSessionLiveStream {
                manager: self.clone(),
                session: Arc::clone(session),
                recv,
                _stream_guard: stream_guard,
            }),
            LIVE_ACTIVITY_RESPONSE_ERROR => {
                let response = read_frame(&mut recv, LIVE_ACTIVITY_FRAME_LIMIT).await?;
                Err(DeviceSessionError::LiveActivityRejected(
                    String::from_utf8_lossy(&response).into_owned(),
                ))
            }
            other => Err(DeviceSessionError::Handshake(format!(
                "unknown Live Activity response status {other}"
            ))),
        }
    }

    async fn execute_bulk_transfer(
        &self,
        session: &Arc<DeviceSessionConnection>,
    ) -> Result<DeviceSessionBulkStream, DeviceSessionError> {
        let stream_guard = session.begin_stream()?;
        let (mut send, mut recv) = session
            .connection
            .open_bi()
            .await
            .map_err(|error| DeviceSessionError::Closed(error.to_string()))?;
        send.set_priority(BULK_TRANSFER_STREAM_PRIORITY)
            .map_err(|error| DeviceSessionError::Closed(error.to_string()))?;
        send.write_all(&[BULK_TRANSFER_STREAM_KIND])
            .await
            .map_err(|error| DeviceSessionError::Io(error.to_string()))?;
        let mut status = [0u8; 1];
        recv.read_exact(&mut status)
            .await
            .map_err(|error| DeviceSessionError::Io(error.to_string()))?;
        match status[0] {
            BULK_TRANSFER_RESPONSE_OK => Ok(DeviceSessionBulkStream {
                session: Arc::clone(session),
                send,
                recv,
                _stream_guard: stream_guard,
            }),
            BULK_TRANSFER_RESPONSE_ERROR => {
                let response = read_frame(&mut recv, BULK_TRANSFER_FRAME_LIMIT).await?;
                Err(DeviceSessionError::BulkTransferRejected(
                    String::from_utf8_lossy(&response).into_owned(),
                ))
            }
            other => Err(DeviceSessionError::Handshake(format!(
                "unknown Bulk Transfer response status {other}"
            ))),
        }
    }

    async fn serve_connection(&self, session: Arc<DeviceSessionConnection>) {
        let mut idle_check = tokio::time::interval(Duration::from_secs(15));
        loop {
            let accepted = tokio::select! {
                _ = self.inner.cancellation.cancelled() => break,
                _ = session.connection.closed() => break,
                _ = idle_check.tick() => {
                    if session.active_streams.load(Ordering::SeqCst) == 0
                        && now_ms().saturating_sub(session.last_used_at_ms.load(Ordering::SeqCst)) >= 60_000
                        && !lock(&self.inner.attempts).contains_key(&session.peer_device_id) {
                        session.connection.close(SESSION_CLOSE_CODE, b"Device Session idle retention expired");
                        break;
                    }
                    continue;
                }
                accepted = session.connection.accept_bi() => accepted,
            };
            let Ok((send, recv)) = accepted else {
                break;
            };
            let manager = self.clone();
            let session = Arc::clone(&session);
            self.inner.tasks.spawn(async move {
                tokio::select! {
                    _ = manager.inner.cancellation.cancelled() => {},
                    _ = session.connection.closed() => {},
                    _ = manager.handle_logical_stream(session.clone(), send, recv) => {},
                }
            });
        }
        self.invalidate_if_generation(
            session.peer_device_id,
            session.generation,
            &DeviceSessionError::Closed(
                session
                    .connection
                    .close_reason()
                    .map(|reason| reason.to_string())
                    .unwrap_or_else(|| "connection ended".to_string()),
            ),
        );
    }

    async fn handle_logical_stream(
        &self,
        session: Arc<DeviceSessionConnection>,
        mut send: iroh::endpoint::SendStream,
        mut recv: iroh::endpoint::RecvStream,
    ) -> Result<(), DeviceSessionError> {
        let _stream_guard = session.begin_incoming_stream()?;
        let mut kind = [0u8; 1];
        recv.read_exact(&mut kind)
            .await
            .map_err(|error| DeviceSessionError::Io(error.to_string()))?;
        match kind[0] {
            PROBE_STREAM_KIND => {
                send.set_priority(PROBE_STREAM_PRIORITY)
                    .map_err(|error| DeviceSessionError::Closed(error.to_string()))?;
                send.write_all(&[PROBE_RESPONSE_OK])
                    .await
                    .map_err(|error| DeviceSessionError::Io(error.to_string()))?;
                send.finish()
                    .map_err(|error| DeviceSessionError::Closed(error.to_string()))
            }
            COMMAND_STREAM_KIND => {
                send.set_priority(COMMAND_STREAM_PRIORITY)
                    .map_err(|error| DeviceSessionError::Closed(error.to_string()))?;
                self.handle_command_stream(&mut send, &mut recv).await
            }
            LIVE_ACTIVITY_STREAM_KIND => {
                send.set_priority(LIVE_ACTIVITY_STREAM_PRIORITY)
                    .map_err(|error| DeviceSessionError::Closed(error.to_string()))?;
                self.handle_live_activity_stream(&mut send, &mut recv).await
            }
            BULK_TRANSFER_STREAM_KIND => {
                send.set_priority(BULK_TRANSFER_STREAM_PRIORITY)
                    .map_err(|error| DeviceSessionError::Closed(error.to_string()))?;
                self.handle_bulk_transfer_stream(session, send, recv, _stream_guard)
                    .await
            }
            _ => {
                write_command_error(&mut send, "unsupported logical stream").await?;
                Ok(())
            }
        }
    }

    async fn handle_command_stream(
        &self,
        send: &mut iroh::endpoint::SendStream,
        recv: &mut iroh::endpoint::RecvStream,
    ) -> Result<(), DeviceSessionError> {
        let payload = read_frame(recv, COMMAND_FRAME_LIMIT).await?;
        let Some(handler) = self.inner.command_handler.as_ref() else {
            write_command_error(send, "Command handler unavailable").await?;
            return Ok(());
        };
        match handler.handle_command(&payload).await {
            Ok(response) => {
                send.write_all(&[COMMAND_RESPONSE_OK])
                    .await
                    .map_err(|error| DeviceSessionError::Io(error.to_string()))?;
                write_frame(send, &response, COMMAND_FRAME_LIMIT).await?;
            }
            Err(error) => {
                write_command_error(send, &error).await?;
                return Ok(());
            }
        }
        send.finish()
            .map_err(|error| DeviceSessionError::Closed(error.to_string()))?;
        Ok(())
    }

    async fn handle_bulk_transfer_stream(
        &self,
        session: Arc<DeviceSessionConnection>,
        mut send: iroh::endpoint::SendStream,
        recv: iroh::endpoint::RecvStream,
        stream_guard: ActiveStreamGuard,
    ) -> Result<(), DeviceSessionError> {
        let handler = read_lock(&self.inner.bulk_transfer_handler).clone();
        let Some(handler) = handler else {
            write_bulk_transfer_error(&mut send, "Bulk Transfer handler unavailable").await?;
            return Ok(());
        };
        send.write_all(&[BULK_TRANSFER_RESPONSE_OK])
            .await
            .map_err(|error| DeviceSessionError::Io(error.to_string()))?;
        handler
            .handle_bulk_transfer(DeviceSessionBulkStream {
                session,
                send,
                recv,
                _stream_guard: stream_guard,
            })
            .await
            .map_err(DeviceSessionError::BulkTransferRejected)
    }

    async fn handle_live_activity_stream(
        &self,
        send: &mut iroh::endpoint::SendStream,
        recv: &mut iroh::endpoint::RecvStream,
    ) -> Result<(), DeviceSessionError> {
        let payload = read_frame(recv, LIVE_ACTIVITY_FRAME_LIMIT).await?;
        let handler = read_lock(&self.inner.live_activity_handler).clone();
        let Some(handler) = handler else {
            write_live_activity_error(send, "Live Activity handler unavailable").await?;
            return Ok(());
        };
        let mut stream = match handler.subscribe_live_activity(&payload).await {
            Ok(stream) => stream,
            Err(error) => {
                write_live_activity_error(send, &error).await?;
                return Ok(());
            }
        };
        send.write_all(&[LIVE_ACTIVITY_RESPONSE_OK])
            .await
            .map_err(|error| DeviceSessionError::Io(error.to_string()))?;
        loop {
            let frame = tokio::select! {
                _ = send.stopped() => return Ok(()),
                frame = stream.next() => frame,
            };
            let Some(frame) = frame else {
                break;
            };
            write_frame(send, &frame, LIVE_ACTIVITY_FRAME_LIMIT).await?;
        }
        send.finish()
            .map_err(|error| DeviceSessionError::Closed(error.to_string()))?;
        Ok(())
    }

    async fn outgoing_handshake(
        &self,
        connection: &Connection,
        expected_peer_device_id: DeviceId,
        deadline: Instant,
    ) -> Result<DeviceId, DeviceSessionError> {
        let (mut send, mut recv) = timeout_at(
            deadline.min(Instant::now() + self.inner.config.handshake_timeout),
            "opening handshake stream",
            connection.open_bi(),
        )
        .await?
        .map_err(|error| DeviceSessionError::Handshake(error.to_string()))?;
        let client_nonce = random_nonce();
        let mut hello = ClientHello {
            version: self.inner.config.protocol_version,
            device_id: self.inner.identity.local_device_id(),
            network_id: self.inner.membership.network_id(),
            capabilities: self.inner.config.capabilities.clone(),
            nonce: client_nonce,
            proof: [0; 32],
        };
        hello.proof = client_proof(
            self.inner.membership.proof_key(),
            self.inner.endpoint.id(),
            connection.remote_id(),
            &hello,
        );
        let bytes = serde_json::to_vec(&hello)
            .map_err(|error| DeviceSessionError::Handshake(error.to_string()))?;
        write_frame(&mut send, &bytes, HANDSHAKE_FRAME_LIMIT).await?;
        send.finish()
            .map_err(|error| DeviceSessionError::Handshake(error.to_string()))?;
        let response = timeout_at(
            deadline.min(Instant::now() + self.inner.config.handshake_timeout),
            "waiting for ServerHello",
            read_frame(&mut recv, HANDSHAKE_FRAME_LIMIT),
        )
        .await??;
        let server: ServerHello = serde_json::from_slice(&response)
            .map_err(|error| DeviceSessionError::Handshake(error.to_string()))?;
        if !server.accepted {
            return Err(rejected_server_hello(
                &server,
                self.inner.config.protocol_version,
            ));
        }
        if server.version != self.inner.config.protocol_version {
            return Err(DeviceSessionError::VersionIncompatible {
                local_version: self.inner.config.protocol_version,
                remote_version: server.version,
            });
        }
        if server.device_id != expected_peer_device_id {
            return Err(DeviceSessionError::PeerEndpointMismatch {
                device_id: hex::encode(expected_peer_device_id),
            });
        }
        if server.network_id != self.inner.membership.network_id() {
            return Err(DeviceSessionError::MembershipRejected);
        }
        if !server.capabilities.contains(COMMAND_CAPABILITY) {
            return Err(DeviceSessionError::CapabilityMissing(
                COMMAND_CAPABILITY.to_string(),
            ));
        }
        if !server.capabilities.contains(LIVE_ACTIVITY_CAPABILITY) {
            return Err(DeviceSessionError::CapabilityMissing(
                LIVE_ACTIVITY_CAPABILITY.to_string(),
            ));
        }
        if !server.capabilities.contains(BULK_TRANSFER_CAPABILITY) {
            return Err(DeviceSessionError::CapabilityMissing(
                BULK_TRANSFER_CAPABILITY.to_string(),
            ));
        }
        let proof = server.proof.ok_or(DeviceSessionError::MembershipRejected)?;
        let expected_proof = server_proof(
            self.inner.membership.proof_key(),
            connection.remote_id(),
            self.inner.endpoint.id(),
            &hello,
            &server,
        );
        verify_proof(&expected_proof, &proof)?;
        Ok(server.device_id)
    }

    async fn accept_handshake(
        &self,
        connection: &Connection,
    ) -> Result<Option<DeviceId>, DeviceSessionError> {
        let (mut send, mut recv) = timeout_at(
            Instant::now() + self.inner.config.handshake_timeout,
            "accepting handshake stream",
            connection.accept_bi(),
        )
        .await?
        .map_err(|error| DeviceSessionError::Handshake(error.to_string()))?;
        let bytes = read_frame(&mut recv, HANDSHAKE_FRAME_LIMIT).await?;
        let client: ClientHello = serde_json::from_slice(&bytes)
            .map_err(|error| DeviceSessionError::Handshake(error.to_string()))?;
        let remote_endpoint_id = connection.remote_id();
        let peer = self
            .inner
            .peer_directory
            .peer_by_public_key(remote_endpoint_id.as_bytes())?
            .ok_or_else(|| DeviceSessionError::PeerNotPaired(hex::encode(client.device_id)))?;
        let rejection = if peer.device_id != client.device_id {
            Some((
                "identity_mismatch",
                "Device id does not match the authenticated endpoint",
            ))
        } else if client.version != self.inner.config.protocol_version {
            Some((
                "version_incompatible",
                "Device Session protocol versions are incompatible",
            ))
        } else if client.network_id != self.inner.membership.network_id()
            || verify_client_proof(
                self.inner.membership.proof_key(),
                remote_endpoint_id,
                self.inner.endpoint.id(),
                &client,
            )
            .is_err()
        {
            Some((
                "membership_rejected",
                "Device did not prove Bound Device Network membership",
            ))
        } else if !client.capabilities.contains(COMMAND_CAPABILITY) {
            Some((
                "capability_missing",
                "Device does not support Command logical streams",
            ))
        } else if !client.capabilities.contains(LIVE_ACTIVITY_CAPABILITY) {
            Some((
                "capability_missing",
                "Device does not support Live Activity logical streams",
            ))
        } else if !client.capabilities.contains(BULK_TRANSFER_CAPABILITY) {
            Some((
                "capability_missing",
                "Device does not support Bulk Transfer logical streams",
            ))
        } else {
            None
        };
        if let Some((reason, detail)) = rejection {
            if reason == "version_incompatible" {
                self.set_version_incompatible(
                    client.device_id,
                    remote_endpoint_id,
                    self.inner.config.protocol_version,
                    client.version,
                );
            }
            let response = ServerHello {
                accepted: false,
                version: self.inner.config.protocol_version,
                device_id: self.inner.identity.local_device_id(),
                network_id: self.inner.membership.network_id(),
                capabilities: self.inner.config.capabilities.clone(),
                nonce: random_nonce(),
                proof: None,
                reason: Some(reason.to_string()),
                detail: Some(detail.to_string()),
            };
            write_server_hello(&mut send, &response).await?;
            let stopped = timeout_at(
                Instant::now() + self.inner.config.handshake_timeout,
                "confirming handshake rejection delivery",
                send.stopped(),
            )
            .await?
            .map_err(|error| DeviceSessionError::Closed(error.to_string()))?;
            if let Some(code) = stopped {
                return Err(DeviceSessionError::Closed(format!(
                    "peer stopped handshake response with code {code}"
                )));
            }
            return Ok(None);
        }
        let mut response = ServerHello {
            accepted: true,
            version: self.inner.config.protocol_version,
            device_id: self.inner.identity.local_device_id(),
            network_id: self.inner.membership.network_id(),
            capabilities: self.inner.config.capabilities.clone(),
            nonce: random_nonce(),
            proof: None,
            reason: None,
            detail: None,
        };
        response.proof = Some(server_proof(
            self.inner.membership.proof_key(),
            self.inner.endpoint.id(),
            remote_endpoint_id,
            &client,
            &response,
        ));
        write_server_hello(&mut send, &response).await?;
        Ok(Some(client.device_id))
    }

    fn invalidate_if_generation(
        &self,
        peer_device_id: DeviceId,
        generation: u64,
        error: &DeviceSessionError,
    ) {
        let removed = {
            let mut sessions = lock(&self.inner.sessions);
            if sessions
                .get(&peer_device_id)
                .is_some_and(|session| session.generation == generation)
            {
                sessions.remove(&peer_device_id)
            } else {
                None
            }
        };
        if let Some(session) = removed {
            session.draining.store(true, Ordering::SeqCst);
            session
                .connection
                .close(SESSION_CLOSE_CODE, b"Device Session transport failed");
            self.set_peer_state(
                peer_device_id,
                session.peer_endpoint_id,
                DeviceSessionState::NotConnected,
                Some(error.to_string()),
                false,
            );
        }
    }

    fn set_peer_state(
        &self,
        peer_device_id: DeviceId,
        endpoint_id: EndpointId,
        state: DeviceSessionState,
        last_error: Option<String>,
        increment_reconnects: bool,
    ) -> bool {
        let sessions = lock(&self.inner.sessions);
        let healthy = sessions.get(&peer_device_id).is_some_and(|session| {
            !session.draining.load(Ordering::SeqCst) && session.connection.close_reason().is_none()
        });
        let (state, last_error, increment_reconnects) = if healthy
            && matches!(
                state,
                DeviceSessionState::NotConnected | DeviceSessionState::Connecting
            ) {
            (DeviceSessionState::Connected, None, false)
        } else {
            (state, last_error, increment_reconnects)
        };
        let now = now_ms();
        let mut statuses = lock(&self.inner.peer_status);
        let previous_state = statuses
            .get(&peer_device_id)
            .map(|status| status.state)
            .unwrap_or(DeviceSessionState::NotConnected);
        let status = statuses.entry(peer_device_id).or_insert(PeerStatus {
            endpoint_id,
            state,
            reconnects: 0,
            state_changed_at_ms: now,
            last_reconnect_at_ms: None,
            version_incompatible: None,
            last_error: None,
        });
        status.endpoint_id = endpoint_id;
        if status.state != state {
            status.state_changed_at_ms = now;
        }
        status.state = state;
        status.last_error = last_error;
        if increment_reconnects {
            status.reconnects += 1;
            status.last_reconnect_at_ms = Some(now);
        }
        status.version_incompatible = None;
        drop(statuses);
        drop(sessions);
        let changed = previous_state != state;
        if changed {
            self.notify_connectivity_changed();
        }
        changed
    }

    fn set_version_incompatible(
        &self,
        peer_device_id: DeviceId,
        endpoint_id: EndpointId,
        local_version: u16,
        remote_version: u16,
    ) {
        let error = DeviceSessionError::VersionIncompatible {
            local_version,
            remote_version,
        };
        let now = now_ms();
        let mut statuses = lock(&self.inner.peer_status);
        let previous_state = statuses
            .get(&peer_device_id)
            .map(|status| status.state)
            .unwrap_or(DeviceSessionState::NotConnected);
        let status = statuses.entry(peer_device_id).or_insert(PeerStatus {
            endpoint_id,
            state: DeviceSessionState::NotConnected,
            reconnects: 0,
            state_changed_at_ms: now,
            last_reconnect_at_ms: None,
            version_incompatible: None,
            last_error: None,
        });
        status.endpoint_id = endpoint_id;
        if status.state != DeviceSessionState::NotConnected {
            status.state_changed_at_ms = now;
        }
        status.state = DeviceSessionState::NotConnected;
        status.version_incompatible = Some((local_version, remote_version));
        status.last_error = Some(error.to_string());
        drop(statuses);
        if previous_state != DeviceSessionState::NotConnected {
            self.notify_connectivity_changed();
        }
    }

    fn notify_connectivity_changed(&self) {
        let next = self.inner.connectivity_revision.borrow().wrapping_add(1);
        self.inner.connectivity_revision.send_replace(next);
    }
}

fn connectivity_state_for(
    session: Option<&Arc<DeviceSessionConnection>>,
    status: Option<&PeerStatus>,
) -> DeviceConnectivityState {
    if session.is_some_and(|session| {
        !session.draining.load(Ordering::SeqCst) && session.connection.close_reason().is_none()
    }) {
        return DeviceConnectivityState::Connected;
    }
    match status.map(|status| status.state) {
        Some(DeviceConnectivityState::Connecting) => DeviceConnectivityState::Connecting,
        Some(DeviceConnectivityState::Connected | DeviceConnectivityState::NotConnected) | None => {
            DeviceConnectivityState::NotConnected
        }
    }
}

struct InstallOutcome {
    active: Arc<DeviceSessionConnection>,
    candidate_selected: bool,
}

#[derive(Clone)]
pub(crate) struct DeviceSessionProtocolHandler {
    manager: DeviceSessionManager,
}

impl std::fmt::Debug for DeviceSessionProtocolHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceSessionProtocolHandler")
            .field("manager", &self.manager)
            .finish()
    }
}

impl DeviceSessionProtocolHandler {
    pub(crate) fn new(manager: DeviceSessionManager) -> Self {
        Self { manager }
    }
}

impl ProtocolHandler for DeviceSessionProtocolHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        self.manager
            .accept_incoming(connection)
            .await
            .map_err(AcceptError::from_err)
    }

    async fn shutdown(&self) {
        self.manager.shutdown().await;
    }
}

fn preferred_origin(local: EndpointId, remote: EndpointId) -> DeviceSessionOrigin {
    if local.as_bytes() < remote.as_bytes() {
        DeviceSessionOrigin::Outgoing
    } else {
        DeviceSessionOrigin::Incoming
    }
}

fn random_nonce() -> [u8; 32] {
    let mut nonce = [0; 32];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    nonce
}

fn client_proof(
    key: &[u8; 32],
    client_endpoint: EndpointId,
    server_endpoint: EndpointId,
    hello: &ClientHello,
) -> [u8; 32] {
    proof(
        key,
        b"rua-device-session-client",
        client_endpoint,
        server_endpoint,
        hello.version,
        hello.device_id,
        hello.network_id,
        &hello.capabilities,
        hello.nonce,
        None,
    )
}

fn verify_client_proof(
    key: &[u8; 32],
    client_endpoint: EndpointId,
    server_endpoint: EndpointId,
    hello: &ClientHello,
) -> Result<(), DeviceSessionError> {
    let expected = client_proof(key, client_endpoint, server_endpoint, hello);
    verify_proof(&expected, &hello.proof)
}

fn server_proof(
    key: &[u8; 32],
    server_endpoint: EndpointId,
    client_endpoint: EndpointId,
    client: &ClientHello,
    server: &ServerHello,
) -> [u8; 32] {
    proof(
        key,
        b"rua-device-session-server",
        server_endpoint,
        client_endpoint,
        server.version,
        server.device_id,
        server.network_id,
        &server.capabilities,
        server.nonce,
        Some(client.nonce),
    )
}

#[allow(clippy::too_many_arguments)]
fn proof(
    key: &[u8; 32],
    label: &[u8],
    local_endpoint: EndpointId,
    remote_endpoint: EndpointId,
    version: u16,
    device_id: DeviceId,
    network_id: [u8; 32],
    capabilities: &BTreeSet<String>,
    nonce: [u8; 32],
    peer_nonce: Option<[u8; 32]>,
) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts 32-byte keys");
    mac.update(label);
    mac.update(local_endpoint.as_bytes());
    mac.update(remote_endpoint.as_bytes());
    mac.update(&version.to_be_bytes());
    mac.update(&device_id);
    mac.update(&network_id);
    for capability in capabilities {
        mac.update(capability.as_bytes());
        mac.update(&[0]);
    }
    mac.update(&nonce);
    if let Some(peer_nonce) = peer_nonce {
        mac.update(&peer_nonce);
    }
    mac.finalize().into_bytes().into()
}

fn verify_proof(expected: &[u8; 32], actual: &[u8; 32]) -> Result<(), DeviceSessionError> {
    if bool::from(expected.ct_eq(actual)) {
        Ok(())
    } else {
        Err(DeviceSessionError::MembershipRejected)
    }
}

fn rejected_server_hello(server: &ServerHello, local_version: u16) -> DeviceSessionError {
    match server.reason.as_deref() {
        Some("version_incompatible") => DeviceSessionError::VersionIncompatible {
            local_version,
            remote_version: server.version,
        },
        Some("membership_rejected") => DeviceSessionError::MembershipRejected,
        Some("capability_missing") => DeviceSessionError::CapabilityMissing(
            server
                .detail
                .clone()
                .unwrap_or_else(|| COMMAND_CAPABILITY.to_string()),
        ),
        Some(reason) => DeviceSessionError::Handshake(format!(
            "{reason}: {}",
            server.detail.as_deref().unwrap_or("rejected")
        )),
        None => DeviceSessionError::Handshake(
            server
                .detail
                .clone()
                .unwrap_or_else(|| "peer rejected handshake".to_string()),
        ),
    }
}

async fn write_server_hello(
    writer: &mut iroh::endpoint::SendStream,
    response: &ServerHello,
) -> Result<(), DeviceSessionError> {
    let bytes = serde_json::to_vec(response)
        .map_err(|error| DeviceSessionError::Handshake(error.to_string()))?;
    write_frame(writer, &bytes, HANDSHAKE_FRAME_LIMIT).await?;
    writer
        .finish()
        .map_err(|error| DeviceSessionError::Closed(error.to_string()))
}

async fn write_command_error(
    writer: &mut iroh::endpoint::SendStream,
    error: &str,
) -> Result<(), DeviceSessionError> {
    writer
        .write_all(&[COMMAND_RESPONSE_ERROR])
        .await
        .map_err(|error| DeviceSessionError::Io(error.to_string()))?;
    write_frame(writer, error.as_bytes(), COMMAND_FRAME_LIMIT).await?;
    writer
        .finish()
        .map_err(|error| DeviceSessionError::Closed(error.to_string()))
}

async fn write_live_activity_error(
    writer: &mut iroh::endpoint::SendStream,
    error: &str,
) -> Result<(), DeviceSessionError> {
    writer
        .write_all(&[LIVE_ACTIVITY_RESPONSE_ERROR])
        .await
        .map_err(|error| DeviceSessionError::Io(error.to_string()))?;
    write_frame(writer, error.as_bytes(), LIVE_ACTIVITY_FRAME_LIMIT).await?;
    writer
        .finish()
        .map_err(|error| DeviceSessionError::Closed(error.to_string()))
}

async fn write_bulk_transfer_error(
    writer: &mut iroh::endpoint::SendStream,
    error: &str,
) -> Result<(), DeviceSessionError> {
    writer
        .write_all(&[BULK_TRANSFER_RESPONSE_ERROR])
        .await
        .map_err(|error| DeviceSessionError::Io(error.to_string()))?;
    write_frame(writer, error.as_bytes(), BULK_TRANSFER_FRAME_LIMIT).await?;
    writer
        .finish()
        .map_err(|error| DeviceSessionError::Closed(error.to_string()))
}

async fn write_frame<W>(
    writer: &mut W,
    payload: &[u8],
    limit: usize,
) -> Result<(), DeviceSessionError>
where
    W: AsyncWrite + Unpin,
{
    if payload.len() > limit {
        return Err(DeviceSessionError::FrameTooLarge { limit });
    }
    writer
        .write_u32(payload.len() as u32)
        .await
        .map_err(io_error)?;
    writer.write_all(payload).await.map_err(io_error)
}

async fn read_frame<R>(reader: &mut R, limit: usize) -> Result<Vec<u8>, DeviceSessionError>
where
    R: AsyncRead + Unpin,
{
    let len = reader.read_u32().await.map_err(io_error)? as usize;
    if len > limit {
        return Err(DeviceSessionError::FrameTooLarge { limit });
    }
    let mut payload = vec![0; len];
    reader.read_exact(&mut payload).await.map_err(io_error)?;
    Ok(payload)
}

async fn read_optional_frame<R>(
    reader: &mut R,
    limit: usize,
) -> Result<Option<Vec<u8>>, DeviceSessionError>
where
    R: AsyncRead + Unpin,
{
    let len = match reader.read_u32().await {
        Ok(len) => len as usize,
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(io_error(error)),
    };
    if len > limit {
        return Err(DeviceSessionError::FrameTooLarge { limit });
    }
    let mut payload = vec![0; len];
    reader.read_exact(&mut payload).await.map_err(io_error)?;
    Ok(Some(payload))
}

async fn timeout_at<F, T>(
    deadline: Instant,
    stage: &'static str,
    future: F,
) -> Result<T, DeviceSessionError>
where
    F: std::future::Future<Output = T>,
{
    let remaining = deadline.saturating_duration_since(Instant::now());
    let budget_ms = u64::try_from(remaining.as_millis()).unwrap_or(u64::MAX);
    if remaining.is_zero() {
        return Err(DeviceSessionError::Timeout { stage, budget_ms });
    }
    tokio::time::timeout(remaining, future)
        .await
        .map_err(|_| DeviceSessionError::Timeout { stage, budget_ms })
}

fn parse_device_id(value: &str) -> Result<DeviceId, DeviceSessionError> {
    hex::decode(value)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| DeviceSessionError::InvalidDeviceId(value.to_string()))
}

fn io_error(error: std::io::Error) -> DeviceSessionError {
    DeviceSessionError::Io(error.to_string())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn read_lock<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn write_lock<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
