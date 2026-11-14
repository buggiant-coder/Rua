use std::sync::Arc;
use std::time::Duration;

use iroh::address_lookup::memory::MemoryLookup;
use iroh::endpoint::presets;
use iroh::protocol::{DynProtocolHandler, Router};
use iroh::{Endpoint, SecretKey, Watcher};
use iroh_gossip::net::Gossip;
use iroh_mdns_address_lookup::MdnsAddressLookup;
use serde::Serialize;

use crate::gossip::DeviceNetworkGossipBus;
use crate::peer_directory::{
    DevicePeerAddressResolver, DevicePeerDirectory, DevicePeerDirectoryError,
};
use crate::session::{
    DeviceConnectivityProjection, DeviceConnectivityState, DeviceSessionCommandHandler,
    DeviceSessionConfig, DeviceSessionError, DeviceSessionManager, DeviceSessionManagerDiagnostics,
    DeviceSessionProtocolHandler, DEVICE_SESSION_ALPN,
};

pub type ProtocolHandlerRegistration = (Vec<u8>, Box<dyn DynProtocolHandler>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceNetworkPreset {
    Production,
    Local,
}

#[derive(Clone)]
pub struct DeviceNetworkIdentity {
    endpoint_secret: SecretKey,
    local_device_id: [u8; 8],
}

impl std::fmt::Debug for DeviceNetworkIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceNetworkIdentity")
            .field("endpoint_id", &self.endpoint_secret.public())
            .field("local_device_id", &hex::encode(self.local_device_id))
            .finish()
    }
}

impl DeviceNetworkIdentity {
    pub fn new(endpoint_secret: SecretKey) -> Self {
        let mut local_device_id = [0; 8];
        local_device_id.copy_from_slice(&endpoint_secret.public().as_bytes()[..8]);
        Self {
            endpoint_secret,
            local_device_id,
        }
    }

    pub fn endpoint_secret(&self) -> &SecretKey {
        &self.endpoint_secret
    }

    pub fn local_device_id(&self) -> [u8; 8] {
        self.local_device_id
    }
}

pub struct BoundDeviceNetworkMembership {
    network_id: [u8; 32],
    network_secret: [u8; 32],
}

impl Clone for BoundDeviceNetworkMembership {
    fn clone(&self) -> Self {
        Self {
            network_id: self.network_id,
            network_secret: self.network_secret,
        }
    }
}

impl std::fmt::Debug for BoundDeviceNetworkMembership {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoundDeviceNetworkMembership")
            .field("network_id", &hex::encode(self.network_id))
            .finish_non_exhaustive()
    }
}

impl BoundDeviceNetworkMembership {
    pub fn new(network_secret: [u8; 32]) -> Self {
        let namespace_secret = iroh_docs::NamespaceSecret::from_bytes(&network_secret);
        Self {
            network_id: *namespace_secret.id().as_bytes(),
            network_secret,
        }
    }

    pub fn network_id(&self) -> [u8; 32] {
        self.network_id
    }

    pub(crate) fn proof_key(&self) -> &[u8; 32] {
        &self.network_secret
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DeviceNetworkError {
    #[error("endpoint bind failed: {0}")]
    Bind(String),
    #[error("router shutdown failed: {0}")]
    Shutdown(String),
}

pub struct DeviceNetworkBootstrap {
    endpoint: Endpoint,
    gossip: Gossip,
    identity: DeviceNetworkIdentity,
    membership: BoundDeviceNetworkMembership,
    peer_directory: DevicePeerDirectory,
    peer_lookup: MemoryLookup,
    peer_resolver: DevicePeerAddressResolver,
    mdns_enabled: bool,
}

impl std::fmt::Debug for DeviceNetworkBootstrap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceNetworkBootstrap")
            .field("endpoint_id", &self.endpoint.id())
            .field("identity", &self.identity)
            .field("membership", &self.membership)
            .field("peer_directory", &self.peer_directory)
            .field("mdns_enabled", &self.mdns_enabled)
            .finish()
    }
}

impl DeviceNetworkBootstrap {
    pub async fn bind(
        preset: DeviceNetworkPreset,
        identity: DeviceNetworkIdentity,
        membership: BoundDeviceNetworkMembership,
        peer_directory: DevicePeerDirectory,
    ) -> Result<Self, DeviceNetworkError> {
        let (builder, mdns_enabled) = match preset {
            DeviceNetworkPreset::Production => (n0_endpoint_builder(), true),
            DeviceNetworkPreset::Local => (Endpoint::builder(presets::Minimal), false),
        };
        Self::bind_with_builder(builder, mdns_enabled, identity, membership, peer_directory).await
    }

    /// Injects an isolated transport while retaining normal identity, membership,
    /// address resolution, authentication, and router setup in contract tests.
    #[cfg(feature = "test-util")]
    #[doc(hidden)]
    pub async fn bind_with_endpoint_builder(
        builder: iroh::endpoint::Builder,
        identity: DeviceNetworkIdentity,
        membership: BoundDeviceNetworkMembership,
        peer_directory: DevicePeerDirectory,
    ) -> Result<Self, DeviceNetworkError> {
        Self::bind_with_builder(builder, false, identity, membership, peer_directory).await
    }

    async fn bind_with_builder(
        builder: iroh::endpoint::Builder,
        mdns_enabled: bool,
        identity: DeviceNetworkIdentity,
        membership: BoundDeviceNetworkMembership,
        peer_directory: DevicePeerDirectory,
    ) -> Result<Self, DeviceNetworkError> {
        let peer_lookup = MemoryLookup::with_provenance("rua_device_directory");
        let endpoint = builder
            .address_lookup(peer_lookup.clone())
            .secret_key(identity.endpoint_secret().clone())
            .bind()
            .await
            .map_err(|error| DeviceNetworkError::Bind(error.to_string()))?;
        let peer_resolver = DevicePeerAddressResolver::new(
            endpoint.clone(),
            peer_directory.clone(),
            peer_lookup.clone(),
        );
        let gossip = Gossip::builder().spawn(endpoint.clone());
        Ok(Self {
            endpoint,
            gossip,
            identity,
            membership,
            peer_directory,
            peer_lookup,
            peer_resolver,
            mdns_enabled,
        })
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    pub fn gossip(&self) -> &Gossip {
        &self.gossip
    }

    pub fn identity(&self) -> &DeviceNetworkIdentity {
        &self.identity
    }

    pub fn membership(&self) -> &BoundDeviceNetworkMembership {
        &self.membership
    }

    pub fn spawn(self, protocols: Vec<ProtocolHandlerRegistration>) -> DeviceNetworkRuntime {
        self.spawn_with_session_config(protocols, None, DeviceSessionConfig::default())
    }

    pub fn spawn_with_command_handler(
        self,
        protocols: Vec<ProtocolHandlerRegistration>,
        command_handler: Arc<dyn DeviceSessionCommandHandler>,
    ) -> DeviceNetworkRuntime {
        self.spawn_with_session_config(
            protocols,
            Some(command_handler),
            DeviceSessionConfig::default(),
        )
    }

    #[doc(hidden)]
    pub fn spawn_with_session_config(
        self,
        protocols: Vec<ProtocolHandlerRegistration>,
        command_handler: Option<Arc<dyn DeviceSessionCommandHandler>>,
        session_config: DeviceSessionConfig,
    ) -> DeviceNetworkRuntime {
        let session_manager = DeviceSessionManager::new(
            self.endpoint.clone(),
            self.identity.clone(),
            self.membership.clone(),
            self.peer_directory.clone(),
            self.peer_resolver.clone(),
            command_handler,
            session_config,
        );
        let mut router = Router::builder(self.endpoint.clone())
            .accept(iroh_gossip::ALPN, self.gossip.clone())
            .accept(
                DEVICE_SESSION_ALPN,
                DeviceSessionProtocolHandler::new(session_manager.clone()),
            );
        for (alpn, handler) in protocols {
            router = router.accept(alpn, handler);
        }
        let router = router.spawn();
        let gossip_bus = DeviceNetworkGossipBus::spawn(
            self.gossip.clone(),
            self.membership.network_id(),
            self.endpoint.id(),
            self.peer_directory.clone(),
            self.peer_lookup.clone(),
        );
        DeviceNetworkRuntime {
            inner: Arc::new(DeviceNetworkRuntimeInner {
                endpoint: self.endpoint,
                router,
                gossip: self.gossip,
                gossip_bus,
                identity: self.identity,
                membership: self.membership,
                peer_directory: self.peer_directory,
                peer_resolver: self.peer_resolver,
                session_manager,
                mdns_enabled: self.mdns_enabled,
            }),
        }
    }
}

struct DeviceNetworkRuntimeInner {
    endpoint: Endpoint,
    router: Router,
    gossip: Gossip,
    gossip_bus: DeviceNetworkGossipBus,
    identity: DeviceNetworkIdentity,
    membership: BoundDeviceNetworkMembership,
    peer_directory: DevicePeerDirectory,
    peer_resolver: DevicePeerAddressResolver,
    session_manager: DeviceSessionManager,
    mdns_enabled: bool,
}

#[derive(Clone)]
pub struct DeviceNetworkRuntime {
    inner: Arc<DeviceNetworkRuntimeInner>,
}

impl std::fmt::Debug for DeviceNetworkRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceNetworkRuntime")
            .field("diagnostics", &self.diagnostics())
            .finish()
    }
}

impl DeviceNetworkRuntime {
    pub fn endpoint(&self) -> &Endpoint {
        &self.inner.endpoint
    }

    pub fn gossip(&self) -> &Gossip {
        &self.inner.gossip
    }

    pub fn gossip_bus(&self) -> &DeviceNetworkGossipBus {
        &self.inner.gossip_bus
    }

    pub fn identity(&self) -> &DeviceNetworkIdentity {
        &self.inner.identity
    }

    pub fn membership(&self) -> &BoundDeviceNetworkMembership {
        &self.inner.membership
    }

    pub fn peer_directory(&self) -> &DevicePeerDirectory {
        &self.inner.peer_directory
    }

    pub fn session_manager(&self) -> &DeviceSessionManager {
        &self.inner.session_manager
    }

    pub async fn resolve_peer_addr(
        &self,
        endpoint_id: iroh::EndpointId,
    ) -> Result<Option<iroh::EndpointAddr>, DevicePeerDirectoryError> {
        let proven_addr = self
            .inner
            .session_manager
            .proven_transport_addr(endpoint_id);
        self.inner
            .peer_resolver
            .resolve_by_endpoint_id(endpoint_id, proven_addr)
            .await
    }

    pub fn connectivity(&self) -> Vec<DeviceConnectivityProjection> {
        self.inner.session_manager.connectivity()
    }

    pub fn connectivity_state(
        &self,
        peer_device_id: &str,
    ) -> Result<DeviceConnectivityState, DeviceSessionError> {
        self.inner
            .session_manager
            .connectivity_state(peer_device_id)
    }

    pub fn diagnostics(&self) -> DeviceNetworkDiagnostics {
        let home_relays = self
            .inner
            .endpoint
            .home_relay_status()
            .get()
            .iter()
            .map(|status| status.url().to_string())
            .collect();
        let device_sessions = self.inner.session_manager.diagnostics();
        let connectivity = device_sessions
            .sessions
            .iter()
            .map(|session| DeviceConnectivityProjection {
                peer_device_id: session.peer_device_id.clone(),
                peer_endpoint_id: session.peer_endpoint_id.clone(),
                state: session.state,
            })
            .collect();
        DeviceNetworkDiagnostics {
            state: if self.inner.router.is_shutdown() {
                "stopped"
            } else {
                "active"
            },
            local_device_id: hex::encode(self.inner.identity.local_device_id()),
            endpoint_id: self.inner.endpoint.id().to_string(),
            network_id: hex::encode(self.inner.membership.network_id()),
            mdns_enabled: self.inner.mdns_enabled,
            bound_sockets: self
                .inner
                .endpoint
                .bound_sockets()
                .into_iter()
                .map(|address| address.to_string())
                .collect(),
            home_relays,
            gossip_running: !self.inner.router.is_shutdown(),
            gossip_neighbors: self.inner.gossip_bus.neighbor_count(),
            connectivity,
            device_sessions,
        }
    }

    pub async fn shutdown(&self) -> Result<(), DeviceNetworkError> {
        self.inner.gossip_bus.shutdown();
        self.inner.session_manager.shutdown().await;
        self.inner
            .router
            .shutdown()
            .await
            .map_err(|error| DeviceNetworkError::Shutdown(error.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceNetworkDiagnostics {
    pub state: &'static str,
    pub local_device_id: String,
    pub endpoint_id: String,
    pub network_id: String,
    pub mdns_enabled: bool,
    pub bound_sockets: Vec<String>,
    pub home_relays: Vec<String>,
    pub gossip_running: bool,
    pub gossip_neighbors: usize,
    pub connectivity: Vec<DeviceConnectivityProjection>,
    pub device_sessions: DeviceSessionManagerDiagnostics,
}

pub fn published_device_address_filter() -> iroh::address_lookup::AddrFilter {
    iroh::address_lookup::AddrFilter::new(|addresses| {
        std::borrow::Cow::Owned(
            addresses
                .iter()
                .filter(|address| match address {
                    iroh::TransportAddr::Relay(_) => true,
                    iroh::TransportAddr::Ip(std::net::SocketAddr::V6(address)) => {
                        let segments = address.ip().segments();
                        // Advertise global-unicast candidates (2000::/3), not
                        // private/link-local interfaces or documentation ranges.
                        // Iroh still validates reachability and retains the relay.
                        segments[0] & 0xe000 == 0x2000
                            && !(segments[0] == 0x2001 && segments[1] == 0x0db8)
                            && !(segments[0] == 0x3fff && segments[1] & 0xf000 == 0)
                    }
                    _ => false,
                })
                .cloned()
                .collect(),
        )
    })
}

pub fn n0_endpoint_builder() -> iroh::endpoint::Builder {
    let transport_config = iroh::endpoint::QuicTransportConfig::builder()
        // Android/Mac batch sends can stall after the handshake (Android reports
        // EIO while iroh retains the original GSO limit). Use individual datagrams.
        .enable_segmentation_offload(false)
        .keep_alive_interval(Duration::from_secs(10))
        .build();
    let builder = Endpoint::builder(presets::N0)
        .clear_address_lookup()
        .address_lookup(
            iroh::address_lookup::PkarrPublisher::n0_dns()
                .addr_filter(published_device_address_filter()),
        )
        .address_lookup(iroh::address_lookup::DnsAddressLookup::n0_dns())
        .ca_roots_config(iroh::tls::CaRootsConfig::embedded())
        .transport_config(transport_config)
        // Match the newer N0 preset: DNS caches can retain a peer's previous
        // relay after a network change. Resolve its signed current record over
        // HTTPS concurrently so a stale DNS answer cannot be the only path.
        .address_lookup(iroh::address_lookup::PkarrResolver::n0_dns())
        .address_lookup(MdnsAddressLookup::builder());
    android_safe_dns(builder)
}

#[cfg(target_os = "android")]
fn android_safe_dns(builder: iroh::endpoint::Builder) -> iroh::endpoint::Builder {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    use iroh::dns::{DnsProtocol, DnsResolver};

    let resolver = DnsResolver::builder()
        .with_nameservers([
            (
                SocketAddr::new(IpAddr::V4(Ipv4Addr::new(223, 5, 5, 5)), 53),
                DnsProtocol::Udp,
            ),
            (
                SocketAddr::new(IpAddr::V4(Ipv4Addr::new(119, 29, 29, 29)), 53),
                DnsProtocol::Udp,
            ),
            (
                SocketAddr::new(IpAddr::V4(Ipv4Addr::new(114, 114, 114, 114)), 53),
                DnsProtocol::Udp,
            ),
        ])
        .build();

    builder.dns_resolver(resolver)
}

#[cfg(not(target_os = "android"))]
fn android_safe_dns(builder: iroh::endpoint::Builder) -> iroh::endpoint::Builder {
    builder
}
