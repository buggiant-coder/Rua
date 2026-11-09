pub mod agent_adapter;
pub mod agent_prompt_file_adapter;
pub mod agent_security_adapter;
pub mod agent_template_adapter;
pub mod chat_adapter;
pub mod conflict;
pub mod control_store;
pub mod crypto;
pub mod device_list;
pub mod device_list_adapter;
pub mod encrypted_io;
pub mod endpoint;
pub mod event;
pub mod evergreen_chat_adapter;
pub mod export;
pub mod hlc;
pub mod identity;
pub mod inbound_inbox;
pub mod liveactor_apply;
pub mod media_adapter;
pub mod merge;
pub mod merge_store;
pub mod network_migration_adapter;
pub mod opaque_replica_log;
pub mod pairing;
pub mod pairing_orchestrator;
pub mod power_policy;
pub mod project_adapter;
pub mod prompt_adapter;
pub mod provider_adapter;
pub mod revocation;
pub mod root_config_adapter;
mod seed_filter;
pub mod shared_mcp_adapter;
pub mod sidecar;
pub mod skill_adapter;
pub mod stage_diagnostics;
pub mod structured_memory_adapter;
pub mod sync_blob_store;
pub mod sync_registry;
mod sync_state;
pub mod transport;
pub mod trigger;
pub mod wormhole;

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("keystore: {0}")]
    Keystore(String),

    #[error("crypto: {0}")]
    Crypto(String),

    #[error("store: {0}")]
    Store(String),

    #[error("invalid sync payload: {0}")]
    InvalidPayload(String),

    #[error("pairing: {0}")]
    Pairing(String),

    #[error("transport: {0}")]
    Transport(String),

    #[error("replica log content unavailable: {0}")]
    ReplicaLogContentUnavailable(String),

    #[error("missing sync adapter for domain: {domain}")]
    MissingSyncAdapter { domain: String },
}

impl SyncError {
    pub fn is_permanent(&self) -> bool {
        matches!(self, Self::InvalidPayload(_))
    }
}

pub type SyncResult<T> = Result<T, SyncError>;

pub mod project_sidebar_adapter;

mod prompt_file_apply;
