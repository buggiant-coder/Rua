use std::path::{Path, PathBuf};

use crate::device_list::{DeviceEntry, DeviceList};
use iroh::address_lookup::memory::MemoryLookup;
use iroh::{Endpoint, EndpointAddr, EndpointId, TransportAddr};

#[derive(Debug, Clone, thiserror::Error)]
pub enum DevicePeerDirectoryError {
    #[error("read paired Device directory: {0}")]
    Read(std::sync::Arc<std::io::Error>),
    #[error("parse paired Device directory: {0}")]
    Parse(std::sync::Arc<serde_json::Error>),
    #[error("paired Device {device_id} has an invalid endpoint identity")]
    InvalidEndpointIdentity { device_id: String },
}

impl From<std::io::Error> for DevicePeerDirectoryError {
    fn from(error: std::io::Error) -> Self {
        Self::Read(std::sync::Arc::new(error))
    }
}
impl From<serde_json::Error> for DevicePeerDirectoryError {
    fn from(error: serde_json::Error) -> Self {
        Self::Parse(std::sync::Arc::new(error))
    }
}

#[derive(Debug, Clone)]
pub struct DevicePeerDirectory {
    path: PathBuf,
}

#[derive(Clone)]
pub struct DevicePeerAddressResolver {
    endpoint: Endpoint,
    peer_directory: DevicePeerDirectory,
    peer_lookup: MemoryLookup,
}

impl std::fmt::Debug for DevicePeerAddressResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DevicePeerAddressResolver")
            .field("endpoint_id", &self.endpoint.id())
            .field("peer_directory", &self.peer_directory)
            .finish()
    }
}

impl DevicePeerAddressResolver {
    pub(crate) fn new(
        endpoint: Endpoint,
        peer_directory: DevicePeerDirectory,
        peer_lookup: MemoryLookup,
    ) -> Self {
        Self {
            endpoint,
            peer_directory,
            peer_lookup,
        }
    }

    pub async fn resolve_by_device_id(
        &self,
        device_id: &[u8; 8],
        proven_addr: Option<TransportAddr>,
    ) -> Result<Option<EndpointAddr>, DevicePeerDirectoryError> {
        let Some(peer) = self.peer_directory.peer_by_device_id(device_id)? else {
            return Ok(None);
        };
        self.resolve_entry(&peer, proven_addr).await.map(Some)
    }

    pub async fn resolve_by_endpoint_id(
        &self,
        endpoint_id: EndpointId,
        proven_addr: Option<TransportAddr>,
    ) -> Result<Option<EndpointAddr>, DevicePeerDirectoryError> {
        let Some(peer) = self
            .peer_directory
            .peer_by_public_key(endpoint_id.as_bytes())?
        else {
            return Ok(None);
        };
        self.resolve_entry(&peer, proven_addr).await.map(Some)
    }

    async fn resolve_entry(
        &self,
        peer: &DeviceEntry,
        proven_addr: Option<TransportAddr>,
    ) -> Result<EndpointAddr, DevicePeerDirectoryError> {
        let mut resolved = endpoint_addr_from_entry(peer)?;
        if let Some(info) = self.peer_lookup.get_endpoint_info(resolved.id) {
            resolved.addrs.extend(info.addrs().cloned());
        }
        if let Some(info) = self.endpoint.remote_info(resolved.id).await {
            resolved
                .addrs
                .extend(info.into_addrs().map(|info| info.into_addr()));
        }
        if let Some(proven_addr) = proven_addr {
            resolved.addrs.insert(proven_addr);
        }
        if !resolved.addrs.is_empty() {
            self.peer_lookup.add_endpoint_info(resolved.clone());
        }
        Ok(resolved)
    }
}

impl DevicePeerDirectory {
    pub fn from_sync_dir(sync_dir: impl AsRef<Path>) -> Self {
        Self {
            path: sync_dir.as_ref().join("device_list.json"),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn read(&self) -> Result<DeviceList, DevicePeerDirectoryError> {
        match std::fs::read(&self.path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(DeviceList::default()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn peer_by_device_id(
        &self,
        device_id: &[u8; 8],
    ) -> Result<Option<DeviceEntry>, DevicePeerDirectoryError> {
        Ok(self
            .read()?
            .devices
            .into_iter()
            .find(|entry| &entry.device_id == device_id))
    }

    pub fn peer_by_public_key(
        &self,
        public_key: &[u8; 32],
    ) -> Result<Option<DeviceEntry>, DevicePeerDirectoryError> {
        Ok(self
            .read()?
            .devices
            .into_iter()
            .find(|entry| &entry.public_key == public_key))
    }

    pub fn endpoint_addr_by_device_id(
        &self,
        device_id: &[u8; 8],
    ) -> Result<Option<EndpointAddr>, DevicePeerDirectoryError> {
        let Some(peer) = self.peer_by_device_id(device_id)? else {
            return Ok(None);
        };
        endpoint_addr_from_entry(&peer).map(Some)
    }
}

pub(crate) fn endpoint_addr_from_entry(
    peer: &DeviceEntry,
) -> Result<EndpointAddr, DevicePeerDirectoryError> {
    let endpoint_id = EndpointId::from_bytes(&peer.public_key).map_err(|_| {
        DevicePeerDirectoryError::InvalidEndpointIdentity {
            device_id: hex::encode(peer.device_id),
        }
    })?;
    let mut addr = EndpointAddr::new(endpoint_id);
    for direct_addr in &peer.direct_addrs {
        if let Ok(socket_addr) = direct_addr.parse() {
            addr.addrs.insert(TransportAddr::Ip(socket_addr));
        }
    }
    Ok(addr)
}
