use std::path::Path;
use std::sync::Arc;

use iroh_docs::api::Doc;
use iroh_docs::NamespaceId;
use rua_sync::crypto::SyncKey;
use rua_sync::endpoint::{BlobsApiStore, SyncNode};
use rua_sync::identity::{DeviceIdentity, NetworkSecret};
use rua_sync::opaque_replica_log::{AuthorId, OpaqueReplicaLog};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoundSyncNetworkError {
    NotPaired,
    MissingDiskSecret,
    DiskSecretMismatch {
        disk_namespace_id: String,
        memory_namespace_id: String,
    },
    IdentityMismatch {
        runtime_namespace_id: String,
        secret_namespace_id: String,
    },
    DeviceIdentityMismatch {
        runtime_device_id: String,
        state_device_id: String,
    },
    PersistedNamespaceMismatch {
        bound_namespace_id: String,
        secret_namespace_id: String,
    },
    CorruptDiskSecret(String),
}

impl std::fmt::Display for BoundSyncNetworkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotPaired => write!(f, "sync network is not paired"),
            Self::MissingDiskSecret => write!(f, "sync/network_secret.key is missing"),
            Self::DiskSecretMismatch {
                disk_namespace_id,
                memory_namespace_id,
            } => write!(
                f,
                "disk network secret namespace {disk_namespace_id} does not match in-memory namespace {memory_namespace_id}"
            ),
            Self::IdentityMismatch {
                runtime_namespace_id,
                secret_namespace_id,
            } => write!(
                f,
                "runtime namespace {runtime_namespace_id} does not match network secret namespace {secret_namespace_id}"
            ),
            Self::DeviceIdentityMismatch {
                runtime_device_id,
                state_device_id,
            } => write!(
                f,
                "runtime Device {runtime_device_id} does not match in-memory Device {state_device_id}"
            ),
            Self::PersistedNamespaceMismatch {
                bound_namespace_id,
                secret_namespace_id,
            } => write!(
                f,
                "persisted namespace {bound_namespace_id} does not match network secret namespace {secret_namespace_id}"
            ),
            Self::CorruptDiskSecret(error) => write!(f, "network secret load failed: {error}"),
        }
    }
}

impl std::error::Error for BoundSyncNetworkError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncNetworkMembershipState {
    Disabled,
    EnabledNoNetwork,
    NetworkCreated,
    Bound,
    IdentityMismatch,
    CorruptIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
/// Display projection of durable network membership, not live-node readiness.
pub struct SyncNetworkMembership {
    pub state: SyncNetworkMembershipState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl SyncNetworkMembership {
    pub fn disabled() -> Self {
        Self {
            state: SyncNetworkMembershipState::Disabled,
            namespace_id: None,
            detail: None,
        }
    }

    pub fn enabled_no_network() -> Self {
        Self {
            state: SyncNetworkMembershipState::EnabledNoNetwork,
            namespace_id: None,
            detail: None,
        }
    }

    pub fn network_created(namespace_id: NamespaceId) -> Self {
        Self {
            state: SyncNetworkMembershipState::NetworkCreated,
            namespace_id: Some(namespace_hex(namespace_id)),
            detail: None,
        }
    }

    pub fn bound(namespace_id: NamespaceId) -> Self {
        Self {
            state: SyncNetworkMembershipState::Bound,
            namespace_id: Some(namespace_hex(namespace_id)),
            detail: None,
        }
    }

    pub fn identity_mismatch(error: BoundSyncNetworkError) -> Self {
        Self {
            state: SyncNetworkMembershipState::IdentityMismatch,
            namespace_id: None,
            detail: Some(error.to_string()),
        }
    }

    pub fn corrupt(error: impl Into<String>) -> Self {
        Self {
            state: SyncNetworkMembershipState::CorruptIdentity,
            namespace_id: None,
            detail: Some(error.into()),
        }
    }
}

pub struct BoundSyncNetwork {
    namespace_id: NamespaceId,
    network_secret: Arc<NetworkSecret>,
    device: Arc<DeviceIdentity>,
    sync_key: SyncKey,
    doc: Doc,
    blobs_store: BlobsApiStore,
}

impl std::fmt::Debug for BoundSyncNetwork {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoundSyncNetwork")
            .field("namespace_id", &namespace_hex(self.namespace_id))
            .field("local_device_id", &self.local_device_id_hex())
            .finish_non_exhaustive()
    }
}

impl BoundSyncNetwork {
    pub fn from_optional_parts(
        sync_dir: &Path,
        node: Option<&SyncNode>,
        network_secret: Option<Arc<NetworkSecret>>,
        device: Option<Arc<DeviceIdentity>>,
    ) -> Result<Self, BoundSyncNetworkError> {
        let node = node.ok_or(BoundSyncNetworkError::NotPaired)?;
        let network_secret = network_secret.ok_or(BoundSyncNetworkError::NotPaired)?;
        let device = device.ok_or(BoundSyncNetworkError::NotPaired)?;
        Self::from_parts(sync_dir, node, network_secret, device)
    }

    pub fn from_parts(
        sync_dir: &Path,
        node: &SyncNode,
        network_secret: Arc<NetworkSecret>,
        device: Arc<DeviceIdentity>,
    ) -> Result<Self, BoundSyncNetworkError> {
        let disk_secret = NetworkSecret::load(sync_dir)
            .map_err(|error| BoundSyncNetworkError::CorruptDiskSecret(error.to_string()))?
            .ok_or(BoundSyncNetworkError::MissingDiskSecret)?;

        if disk_secret.to_bytes() != network_secret.to_bytes() {
            return Err(BoundSyncNetworkError::DiskSecretMismatch {
                disk_namespace_id: namespace_hex(disk_secret.namespace_id()),
                memory_namespace_id: namespace_hex(network_secret.namespace_id()),
            });
        }

        let runtime_namespace_id = node.namespace_id();
        let secret_namespace_id = network_secret.namespace_id();
        if runtime_namespace_id != secret_namespace_id {
            return Err(BoundSyncNetworkError::IdentityMismatch {
                runtime_namespace_id: namespace_hex(runtime_namespace_id),
                secret_namespace_id: namespace_hex(secret_namespace_id),
            });
        }

        let runtime_device_id = node.network_runtime().identity().local_device_id();
        let state_device_id = device.device_id_short();
        if runtime_device_id != state_device_id {
            return Err(BoundSyncNetworkError::DeviceIdentityMismatch {
                runtime_device_id: hex::encode(runtime_device_id),
                state_device_id: hex::encode(state_device_id),
            });
        }

        let sync_key = network_secret.derive_sync_key();
        Ok(Self {
            namespace_id: secret_namespace_id,
            network_secret,
            device,
            sync_key,
            doc: node.doc().clone(),
            blobs_store: node.blobs_store().clone(),
        })
    }

    pub fn namespace_id(&self) -> NamespaceId {
        self.namespace_id
    }

    pub fn namespace_id_hex(&self) -> String {
        namespace_hex(self.namespace_id)
    }

    pub fn network_secret(&self) -> &Arc<NetworkSecret> {
        &self.network_secret
    }

    pub fn sync_key(&self) -> &SyncKey {
        &self.sync_key
    }

    pub fn local_device_id_hex(&self) -> String {
        hex::encode(self.device.device_id_short())
    }

    pub fn local_author(&self) -> AuthorId {
        self.device.to_iroh_author().id()
    }

    pub fn blob_context(&self) -> (BlobsApiStore, SyncKey) {
        (self.blobs_store.clone(), self.sync_key.clone())
    }

    pub fn doc(&self) -> &Doc {
        &self.doc
    }

    pub fn blobs_store(&self) -> &BlobsApiStore {
        &self.blobs_store
    }

    pub fn opaque_replica_log(&self) -> OpaqueReplicaLog {
        OpaqueReplicaLog::new(
            self.doc.clone(),
            self.device.to_iroh_author(),
            self.sync_key.clone(),
            self.blobs_store.clone(),
        )
    }
}

fn namespace_hex(namespace_id: NamespaceId) -> String {
    hex::encode(namespace_id.as_bytes())
}
