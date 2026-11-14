use std::future::Future;
use std::path::Path;
use std::sync::{Arc, RwLock, Weak};
use std::time::Duration;

use iroh::{Endpoint, PublicKey, SecretKey};
pub use iroh_blobs::api::Store as BlobsApiStore;
use iroh_blobs::store::fs::options::Options as BlobsFsOptions;
use iroh_blobs::store::{GcConfig, ProtectCb, ProtectOutcome};
use iroh_blobs::BlobsProtocol;
pub use iroh_blobs::Hash as BlobHash;
use iroh_docs::actor::SyncHandle;
use iroh_docs::api::Doc;
use iroh_docs::engine::{DefaultAuthorStorage, Engine, ProtectCallbackHandler};
use iroh_docs::protocol::Docs;
use iroh_docs::{Capability, NamespaceId};
use iroh_gossip::net::Gossip;
use rua_device_network::{
    BoundDeviceNetworkMembership, DeviceNetworkBootstrap, DeviceNetworkIdentity,
    DeviceNetworkPreset, DeviceNetworkRuntime, DeviceSessionCommandHandler,
    ProtocolHandlerRegistration,
};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::identity::DeviceIdentity;
use crate::opaque_replica_log::OpaqueReplicaLog;
use crate::sync_blob_store::StatefulBlobDownloader;
use crate::SyncError;

pub use rua_device_network::n0_endpoint_builder;

#[derive(Default)]
struct PayloadGcContext {
    logs: RwLock<std::collections::BTreeMap<NamespaceId, Arc<OpaqueReplicaLog>>>,
}

impl PayloadGcContext {
    fn register(&self, namespace: NamespaceId, log: OpaqueReplicaLog) {
        self.logs
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(namespace, Arc::new(log));
    }
}

fn payload_gc_callback(docs_callback: ProtectCb, context: Weak<PayloadGcContext>) -> ProtectCb {
    Arc::new(move |live| {
        let docs_callback = Arc::clone(&docs_callback);
        let context = context.clone();
        // The GC callback requires a Sync future; local RPC/blob readers only
        // promise Send. Serialize polling without spawning an unowned task.
        let future = std::sync::Mutex::new(Box::pin(async move {
            if matches!((docs_callback)(live).await, ProtectOutcome::Abort) {
                return ProtectOutcome::Abort;
            }
            let Some(context) = context.upgrade() else {
                return ProtectOutcome::Abort;
            };
            let logs = context
                .logs
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .values()
                .cloned()
                .collect::<Vec<_>>();
            if logs.is_empty() {
                return ProtectOutcome::Abort;
            }
            for log in logs {
                if let Err(error) = log.protect_referenced_payloads(live).await {
                    warn!(%error, "Sync Blob GC aborted: encrypted payload reachability is unknown");
                    return ProtectOutcome::Abort;
                }
            }
            ProtectOutcome::Continue
        }));
        Box::pin(std::future::poll_fn(move |context| {
            future
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_mut()
                .poll(context)
        }))
    })
}

struct ExternalBlobGcTask {
    cancel: CancellationToken,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl ExternalBlobGcTask {
    fn spawn(store: BlobsApiStore, config: GcConfig) -> Arc<Self> {
        let cancel = CancellationToken::new();
        let cancelled = cancel.clone();
        let task = tokio::spawn(async move {
            tokio::select! {
                () = cancelled.cancelled() => {}
                () = iroh_blobs::store::run_gc(store, config) => {}
            }
        });
        Arc::new(Self {
            cancel,
            task: Mutex::new(Some(task)),
        })
    }

    async fn shutdown(&self) {
        self.cancel.cancel();
        if let Some(task) = self.task.lock().await.take() {
            if let Err(error) = task.await {
                warn!(error = %error, "Sync Blob GC task failed during shutdown");
            }
        }
    }
}

impl Drop for ExternalBlobGcTask {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

const SYNC_BLOB_GC_INTERVAL_DEFAULT_SECS: u64 = 600;

fn sync_blob_gc_interval() -> Duration {
    let secs = std::env::var("RUA_SYNC_BLOB_GC_INTERVAL_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(SYNC_BLOB_GC_INTERVAL_DEFAULT_SECS);
    Duration::from_secs(secs)
}

#[derive(Clone)]
pub struct SyncNode {
    network_runtime: DeviceNetworkRuntime,
    docs: Docs,
    sync_handle: SyncHandle,
    doc: Doc,
    namespace_id: NamespaceId,
    blobs_store: BlobsApiStore,
    blob_downloader: StatefulBlobDownloader,
    external_blob_gc_task: Option<Arc<ExternalBlobGcTask>>,
    payload_gc_context: Arc<PayloadGcContext>,
}

impl SyncNode {
    pub async fn bind(
        sync_dir: &Path,
        namespace_secret: &iroh_docs::NamespaceSecret,
        device: &DeviceIdentity,
        blobs_store: Option<iroh_blobs::api::Store>,
    ) -> Result<Self, SyncError> {
        Self::bind_with_handlers(
            sync_dir,
            namespace_secret,
            device,
            blobs_store,
            None,
            sync_blob_gc_interval(),
        )
        .await
    }

    pub async fn bind_with_command_handler(
        sync_dir: &Path,
        namespace_secret: &iroh_docs::NamespaceSecret,
        device: &DeviceIdentity,
        blobs_store: Option<iroh_blobs::api::Store>,
        command_handler: Arc<dyn DeviceSessionCommandHandler>,
    ) -> Result<Self, SyncError> {
        Self::bind_with_handlers(
            sync_dir,
            namespace_secret,
            device,
            blobs_store,
            Some(command_handler),
            sync_blob_gc_interval(),
        )
        .await
    }

    pub async fn bind_with_gc_interval(
        sync_dir: &Path,
        namespace_secret: &iroh_docs::NamespaceSecret,
        device: &DeviceIdentity,
        blobs_store: Option<iroh_blobs::api::Store>,
        blob_gc_interval: Duration,
    ) -> Result<Self, SyncError> {
        Self::bind_with_handlers(
            sync_dir,
            namespace_secret,
            device,
            blobs_store,
            None,
            blob_gc_interval,
        )
        .await
    }

    async fn bind_with_handlers(
        sync_dir: &Path,
        namespace_secret: &iroh_docs::NamespaceSecret,
        device: &DeviceIdentity,
        blobs_store: Option<iroh_blobs::api::Store>,
        command_handler: Option<Arc<dyn DeviceSessionCommandHandler>>,
        blob_gc_interval: Duration,
    ) -> Result<Self, SyncError> {
        crate::sync_state::bind_network(sync_dir, namespace_secret.id().as_bytes())?;
        let bootstrap = bind_device_network(
            DeviceNetworkPreset::Production,
            sync_dir,
            namespace_secret,
            device,
        )
        .await?;

        let payload_gc_context = Arc::new(PayloadGcContext::default());
        let (blobs_store, protect_handler, external_blob_gc_task) = match blobs_store {
            Some(blobs_store) => {
                let (protect_handler, protect_cb) = ProtectCallbackHandler::new();
                let task = ExternalBlobGcTask::spawn(
                    blobs_store.clone(),
                    GcConfig {
                        interval: blob_gc_interval,
                        add_protected: Some(payload_gc_callback(
                            protect_cb,
                            Arc::downgrade(&payload_gc_context),
                        )),
                    },
                );
                (blobs_store, Some(protect_handler), Some(task))
            }
            None => {
                let blobs_dir = sync_dir.join("iroh-blobs");
                std::fs::create_dir_all(&blobs_dir)
                    .map_err(|e| SyncError::Store(format!("create blobs dir: {e}")))?;
                let (protect_handler, protect_cb) = ProtectCallbackHandler::new();
                let mut opts = BlobsFsOptions::new(&blobs_dir);
                opts.gc = Some(GcConfig {
                    interval: blob_gc_interval,
                    add_protected: Some(payload_gc_callback(
                        protect_cb,
                        Arc::downgrade(&payload_gc_context),
                    )),
                });

                let db_path = blobs_dir.join("blobs.db");
                let store: BlobsApiStore =
                    iroh_blobs::store::fs::FsStore::load_with_opts(db_path, opts)
                        .await
                        .map_err(|e| SyncError::Store(format!("open blobs store: {e}")))?
                        .into();
                (store, Some(protect_handler), None)
            }
        };
        let blobs_store_saved = blobs_store.clone();
        let blob_downloader = StatefulBlobDownloader::new(
            iroh_blobs::api::downloader::Downloader::new_with_opts(
                &blobs_store,
                bootstrap.endpoint(),
                iroh_blobs::util::connection_pool::Options {
                    // Match the endpoint handshake budget. The pool's 1 s default
                    // aborts viable relay/recovery handshakes before QUIC can finish.
                    connect_timeout: Duration::from_secs(10),
                    // Share connections during transfers, but do not reuse an idle
                    // session after a peer may have silently exited or restarted.
                    idle_timeout: Duration::ZERO,
                    ..Default::default()
                },
            ),
            bootstrap.endpoint().clone(),
        );
        let blobs_protocol = BlobsProtocol::new(&blobs_store, None);

        let docs_dir = sync_dir.join("iroh-docs");
        std::fs::create_dir_all(&docs_dir)
            .map_err(|e| SyncError::Store(format!("create docs dir: {e}")))?;

        let (docs, sync_handle) = spawn_docs_persistent(
            docs_dir.clone(),
            bootstrap.endpoint().clone(),
            blobs_store,
            blob_downloader.raw_downloader(),
            bootstrap.gossip().clone(),
            protect_handler,
        )
        .await?;
        let protocols: Vec<ProtocolHandlerRegistration> = vec![
            (iroh_blobs::ALPN.to_vec(), Box::new(blobs_protocol)),
            (iroh_docs::ALPN.to_vec(), Box::new(docs.clone())),
        ];
        let network_runtime = match command_handler {
            Some(command_handler) => {
                bootstrap.spawn_with_command_handler(protocols, command_handler)
            }
            None => bootstrap.spawn(protocols),
        };

        let namespace_id = namespace_secret.id();

        let author = device.to_iroh_author();
        docs.author_import(author)
            .await
            .map_err(|e| SyncError::Store(format!("import author: {e}")))?;

        let ns_clone = iroh_docs::NamespaceSecret::from_bytes(&namespace_secret.to_bytes());
        let doc = docs
            .import_namespace(Capability::Write(ns_clone))
            .await
            .map_err(|e| SyncError::Store(format!("import namespace: {e}")))?;

        let network = crate::identity::NetworkSecret::from_namespace_secret(
            iroh_docs::NamespaceSecret::from_bytes(&namespace_secret.to_bytes()),
        );
        payload_gc_context.register(
            namespace_id,
            OpaqueReplicaLog::new(
                doc.clone(),
                device.to_iroh_author(),
                network.derive_sync_key(),
                blobs_store_saved.clone(),
            ),
        );

        recover_proven_own_removals(sync_dir, namespace_secret, device, &doc, &blobs_store_saved)
            .await?;

        start_paired_document_sync(&doc, &network_runtime).await?;

        let removed_auto_tags = blobs_store_saved
            .tags()
            .delete_prefix(b"auto-")
            .await
            .map_err(|e| SyncError::Store(format!("remove legacy Sync Blob auto tags: {e}")))?;
        if removed_auto_tags > 0 {
            info!(
                removed_auto_tags,
                "removed legacy Sync Blob auto tags; unreferenced blobs will be reclaimed by GC"
            );
        }

        let node_id = network_runtime.endpoint().id();
        info!(node_id = %node_id.fmt_short(), "Device Network Runtime bound with Sync consumer");

        Ok(Self {
            network_runtime,
            docs,
            sync_handle,
            doc,
            namespace_id,
            blobs_store: blobs_store_saved,
            blob_downloader,
            external_blob_gc_task,
            payload_gc_context,
        })
    }

    pub fn endpoint(&self) -> &Endpoint {
        self.network_runtime.endpoint()
    }

    pub fn network_runtime(&self) -> &DeviceNetworkRuntime {
        &self.network_runtime
    }

    pub fn sync_handle(&self) -> &SyncHandle {
        &self.sync_handle
    }

    pub fn doc(&self) -> &Doc {
        &self.doc
    }

    pub fn docs(&self) -> &Docs {
        &self.docs
    }

    pub fn namespace_id(&self) -> NamespaceId {
        self.namespace_id
    }

    pub fn blobs_store(&self) -> &BlobsApiStore {
        &self.blobs_store
    }

    pub fn blob_downloader(&self) -> &StatefulBlobDownloader {
        &self.blob_downloader
    }

    pub fn node_id(&self) -> PublicKey {
        self.network_runtime.endpoint().id()
    }

    pub async fn register_peer(&self, peer: iroh::EndpointAddr) -> Result<(), SyncError> {
        self.doc
            .start_sync(vec![peer])
            .await
            .map_err(|e| SyncError::Store(format!("register peer: {e}")))
    }

    pub async fn open_opaque_log_for_namespace(
        &self,
        ns: &crate::identity::NetworkSecret,
        device: &crate::identity::DeviceIdentity,
    ) -> Result<crate::opaque_replica_log::OpaqueReplicaLog, SyncError> {
        let ns_clone = iroh_docs::NamespaceSecret::from_bytes(&ns.namespace_secret().to_bytes());
        let doc = self
            .docs
            .import_namespace(Capability::Write(ns_clone))
            .await
            .map_err(|e| SyncError::Store(format!("import additional namespace: {e}")))?;
        doc.start_sync(vec![])
            .await
            .map_err(|e| SyncError::Store(format!("start sync additional: {e}")))?;

        let author = device.to_iroh_author();
        self.docs
            .author_import(author.clone())
            .await
            .map_err(|e| SyncError::Store(format!("import author for additional ns: {e}")))?;

        let sync_key = ns.derive_sync_key();
        self.payload_gc_context.register(
            ns.namespace_id(),
            OpaqueReplicaLog::new(
                doc.clone(),
                author.clone(),
                sync_key.clone(),
                self.blobs_store.clone(),
            ),
        );
        Ok(crate::opaque_replica_log::OpaqueReplicaLog::new(
            doc,
            author,
            sync_key,
            self.blobs_store.clone(),
        ))
    }

    pub async fn shutdown(&self) {
        if let Some(task) = &self.external_blob_gc_task {
            task.shutdown().await;
        }
        if let Err(error) = self.network_runtime.shutdown().await {
            tracing::warn!(error = %error, "failed to shut down Device Network Runtime");
        }
        if let Err(error) = self.blobs_store.shutdown().await {
            tracing::warn!(error = %error, "failed to shut down Sync Blob Store");
        }
    }
}

#[cfg(feature = "test-util")]
fn dialable_test_socket_addr(sock: std::net::SocketAddr) -> std::net::SocketAddr {
    if !sock.ip().is_unspecified() {
        return sock;
    }

    match sock {
        std::net::SocketAddr::V4(sock) => (std::net::Ipv4Addr::LOCALHOST, sock.port()).into(),
        std::net::SocketAddr::V6(sock) => (std::net::Ipv6Addr::LOCALHOST, sock.port()).into(),
    }
}

#[cfg(feature = "test-util")]
impl SyncNode {
    pub fn local_peer_addr(&self) -> iroh::EndpointAddr {
        let mut addr = iroh::EndpointAddr::new(self.endpoint().id());
        for sock in self.endpoint().bound_sockets() {
            let sock = dialable_test_socket_addr(sock);
            addr.addrs.insert(iroh::TransportAddr::Ip(sock));
        }
        addr
    }

    pub async fn bind_local(
        sync_dir: &Path,
        namespace_secret: &iroh_docs::NamespaceSecret,
        device: &DeviceIdentity,
    ) -> Result<SyncNode, SyncError> {
        crate::sync_state::bind_network(sync_dir, namespace_secret.id().as_bytes())?;
        let bootstrap = bind_device_network(
            DeviceNetworkPreset::Local,
            sync_dir,
            namespace_secret,
            device,
        )
        .await?;

        let blobs_store: iroh_blobs::api::Store =
            iroh_blobs::store::mem::MemStore::default().into();
        let blobs_store_saved = blobs_store.clone();
        let blob_downloader = StatefulBlobDownloader::new(
            iroh_blobs::api::downloader::Downloader::new_with_opts(
                &blobs_store,
                bootstrap.endpoint(),
                iroh_blobs::util::connection_pool::Options {
                    // Match the endpoint handshake budget. The pool's 1 s default
                    // aborts viable relay/recovery handshakes before QUIC can finish.
                    connect_timeout: Duration::from_secs(10),
                    // Share connections during transfers, but do not reuse an idle
                    // session after a peer may have silently exited or restarted.
                    idle_timeout: Duration::ZERO,
                    ..Default::default()
                },
            ),
            bootstrap.endpoint().clone(),
        );
        let blobs_protocol = BlobsProtocol::new(&blobs_store, None);

        let docs_dir = sync_dir.join("iroh-docs");
        std::fs::create_dir_all(&docs_dir)
            .map_err(|e| SyncError::Store(format!("create docs dir: {e}")))?;

        let (docs, sync_handle) = spawn_docs_mem(
            bootstrap.endpoint().clone(),
            blobs_store,
            blob_downloader.raw_downloader(),
            bootstrap.gossip().clone(),
        )
        .await?;
        let network_runtime = bootstrap.spawn(vec![
            (iroh_blobs::ALPN.to_vec(), Box::new(blobs_protocol)),
            (iroh_docs::ALPN.to_vec(), Box::new(docs.clone())),
        ]);

        let namespace_id = namespace_secret.id();

        let author = device.to_iroh_author();
        docs.author_import(author)
            .await
            .map_err(|e| SyncError::Store(format!("import author: {e}")))?;

        let ns_clone = iroh_docs::NamespaceSecret::from_bytes(&namespace_secret.to_bytes());
        let doc = docs
            .import_namespace(Capability::Write(ns_clone))
            .await
            .map_err(|e| SyncError::Store(format!("import namespace: {e}")))?;

        recover_proven_own_removals(sync_dir, namespace_secret, device, &doc, &blobs_store_saved)
            .await?;

        start_paired_document_sync(&doc, &network_runtime).await?;

        let node_id = network_runtime.endpoint().id();
        info!(node_id = %node_id.fmt_short(), "local Device Network Runtime bound with Sync consumer");

        Ok(SyncNode {
            network_runtime,
            docs,
            sync_handle,
            doc,
            namespace_id,
            blobs_store: blobs_store_saved,
            blob_downloader,
            external_blob_gc_task: None,
            payload_gc_context: Arc::new(PayloadGcContext::default()),
        })
    }
}

async fn recover_proven_own_removals(
    sync_dir: &Path,
    namespace_secret: &iroh_docs::NamespaceSecret,
    device: &DeviceIdentity,
    doc: &Doc,
    blobs_store: &BlobsApiStore,
) -> Result<(), SyncError> {
    let candidates = crate::sync_state::quarantined_tombstone_records(sync_dir)?;
    if candidates.is_empty() {
        return Ok(());
    }
    let network = crate::identity::NetworkSecret::from_namespace_secret(
        iroh_docs::NamespaceSecret::from_bytes(&namespace_secret.to_bytes()),
    );
    let log = crate::opaque_replica_log::OpaqueReplicaLog::new(
        doc.clone(),
        device.to_iroh_author(),
        network.derive_sync_key(),
        blobs_store.clone(),
    );
    let local_device_id = hex::encode(device.device_id_short());
    let author = device.to_iroh_author().id();
    let namespace_id = namespace_secret.id();
    let mut recovered = 0usize;
    let mut unavailable = 0usize;
    for (domain, record_id) in candidates {
        let key = log.record_entry_key(&domain, &record_id);
        // This reads local content only. Startup does not wait for a provider;
        // missing or unproven history stays quarantined for later recovery.
        match log.read_change(author, &key).await {
            Ok(Some(change)) if change.device_id == local_device_id => {
                if crate::sync_state::restore_verified_removal(
                    sync_dir,
                    namespace_id.as_bytes(),
                    &change,
                )? {
                    recovered += 1;
                }
            }
            Ok(_) => {}
            Err(error) => {
                unavailable += 1;
                tracing::debug!(%error, %domain, %record_id, "own removal proof unavailable");
            }
        }
    }
    if recovered > 0 {
        info!(recovered, "restored current-network own removal ordering");
    }
    if unavailable > 0 {
        warn!(
            unavailable,
            "legacy tombstones remain quarantined without local proof"
        );
    }
    Ok(())
}

async fn spawn_docs_persistent(
    docs_dir: std::path::PathBuf,
    endpoint: Endpoint,
    blobs_store: BlobsApiStore,
    downloader: iroh_blobs::api::downloader::Downloader,
    gossip: Gossip,
    protect_handler: Option<ProtectCallbackHandler>,
) -> Result<(Docs, SyncHandle), SyncError> {
    let replica_store = iroh_docs::store::Store::persistent(docs_dir.join("docs.redb"))
        .map_err(|e| SyncError::Store(format!("open docs store: {e}")))?;
    let engine_result = Engine::spawn(
        endpoint.clone(),
        gossip.clone(),
        replica_store,
        blobs_store.clone(),
        downloader.clone(),
        DefaultAuthorStorage::Persistent(docs_dir.join("default-author")),
        protect_handler,
    )
    .await;

    let engine = match engine_result {
        Ok(engine) => engine,
        Err(e) if e.to_string().contains("default author is missing") => {
            warn!("[sync] stale default-author detected, deleting and retrying");
            let da_path = docs_dir.join("default-author");
            if let Err(rm_err) = std::fs::remove_file(&da_path) {
                warn!(error = %rm_err, "[sync] failed to remove stale default-author file");
            }
            let replica_store = iroh_docs::store::Store::persistent(docs_dir.join("docs.redb"))
                .map_err(|e| SyncError::Store(format!("open docs store (retry): {e}")))?;
            Engine::spawn(
                endpoint,
                gossip,
                replica_store,
                blobs_store,
                downloader.clone(),
                DefaultAuthorStorage::Persistent(docs_dir.join("default-author")),
                None,
            )
            .await
            .map_err(|e| {
                SyncError::Store(format!(
                    "engine spawn (retry after stale default-author): {e}"
                ))
            })?
        }
        Err(e) => return Err(SyncError::Store(format!("engine spawn: {e}"))),
    };

    let sync_handle = engine.sync.clone();
    let docs = Docs::new(engine);
    Ok((docs, sync_handle))
}

#[cfg(feature = "test-util")]
async fn spawn_docs_mem(
    endpoint: Endpoint,
    blobs_store: BlobsApiStore,
    downloader: iroh_blobs::api::downloader::Downloader,
    gossip: Gossip,
) -> Result<(Docs, SyncHandle), SyncError> {
    let replica_store = iroh_docs::store::Store::memory();
    let engine = Engine::spawn(
        endpoint,
        gossip,
        replica_store,
        blobs_store,
        downloader,
        DefaultAuthorStorage::Mem,
        None,
    )
    .await
    .map_err(|e| SyncError::Store(format!("engine spawn: {e}")))?;
    let sync_handle = engine.sync.clone();
    let docs = Docs::new(engine);
    Ok((docs, sync_handle))
}

async fn start_paired_document_sync(
    doc: &Doc,
    network_runtime: &DeviceNetworkRuntime,
) -> Result<(), SyncError> {
    // Restore continuous replication for every paired device on each start.
    // A manual single-peer cycle does not register the caller with Docs' live actor.
    let peers = network_runtime
        .peer_directory()
        .endpoint_addrs()
        .map_err(|e| SyncError::Store(format!("load paired sync peers: {e}")))?
        .into_iter()
        .filter(|peer| peer.id != network_runtime.endpoint().id())
        .collect();
    doc.start_sync(peers)
        .await
        .map_err(|e| SyncError::Store(format!("start sync: {e}")))
}

async fn bind_device_network(
    preset: DeviceNetworkPreset,
    sync_dir: &Path,
    namespace_secret: &iroh_docs::NamespaceSecret,
    device: &DeviceIdentity,
) -> Result<DeviceNetworkBootstrap, SyncError> {
    let sk = device.signing_key();
    let identity = DeviceNetworkIdentity::new(SecretKey::from_bytes(&sk.to_bytes()));
    let membership = BoundDeviceNetworkMembership::new(namespace_secret.to_bytes());
    DeviceNetworkBootstrap::bind(
        preset,
        identity,
        membership,
        rua_device_network::DevicePeerDirectory::from_sync_dir(sync_dir),
    )
    .await
    .map_err(|error| SyncError::Store(error.to_string()))
}
