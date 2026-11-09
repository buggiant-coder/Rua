use std::sync::Arc;

use serde::Deserialize;

use crate::state::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkMigrationApplyOutcome {
    NoPendingMigration,
    AlreadyCurrent,
    Migrated,
}

#[derive(Debug)]
pub enum NetworkMigrationError {
    Read(String),
    InvalidSecretLength(usize),
    NamespaceMismatch { expected: String, actual: String },
    SaveSecret(String),
    CaptureExport(String),
    DeviceIdentity(String),
    BindRuntime(String),
    RestartRuntime(String),
    PublishExport(String),
}

impl std::fmt::Display for NetworkMigrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(error) => write!(f, "read pending migration: {error}"),
            Self::InvalidSecretLength(len) => {
                write!(f, "migration namespace secret must be 32 bytes, got {len}")
            }
            Self::NamespaceMismatch { expected, actual } => write!(
                f,
                "migration target namespace {expected} does not match secret namespace {actual}"
            ),
            Self::SaveSecret(error) => write!(f, "save migration network secret: {error}"),
            Self::CaptureExport(error) => write!(f, "capture pre-migration Full Export: {error}"),
            Self::DeviceIdentity(error) => write!(f, "load device identity: {error}"),
            Self::BindRuntime(error) => write!(f, "bind migrated sync runtime: {error}"),
            Self::RestartRuntime(error) => write!(f, "restart sync runtime: {error}"),
            Self::PublishExport(error) => write!(f, "publish frozen Full Export: {error}"),
        }
    }
}

impl std::error::Error for NetworkMigrationError {}

#[derive(Deserialize)]
struct PendingMigrationFile {
    target_namespace_id: String,
    target_namespace_secret_bytes: Vec<u8>,
}

pub async fn apply_pending_network_migration(
    state: &AppState,
) -> Result<NetworkMigrationApplyOutcome, NetworkMigrationError> {
    if !state.sync_enabled.load(std::sync::atomic::Ordering::SeqCst) {
        return Ok(NetworkMigrationApplyOutcome::NoPendingMigration);
    }

    let sync_dir = state.rua_root.join("sync");
    let path = rua_sync::network_migration_adapter::pending_migration_path(&state.rua_root);
    if !path.is_file() {
        return Ok(NetworkMigrationApplyOutcome::NoPendingMigration);
    }

    let pending: PendingMigrationFile = std::fs::read_to_string(&path)
        .map_err(|error| NetworkMigrationError::Read(error.to_string()))
        .and_then(|content| {
            serde_json::from_str(&content)
                .map_err(|error| NetworkMigrationError::Read(error.to_string()))
        })?;

    let secret_bytes: [u8; 32] = pending
        .target_namespace_secret_bytes
        .as_slice()
        .try_into()
        .map_err(|_| {
            NetworkMigrationError::InvalidSecretLength(pending.target_namespace_secret_bytes.len())
        })?;
    let target_secret = rua_sync::identity::NetworkSecret::from_bytes(&secret_bytes);
    let actual_namespace_id = hex::encode(target_secret.namespace_id().as_bytes());
    if actual_namespace_id != pending.target_namespace_id {
        return Err(NetworkMigrationError::NamespaceMismatch {
            expected: pending.target_namespace_id,
            actual: actual_namespace_id,
        });
    }

    if let Ok(Some(current_secret)) = rua_sync::identity::NetworkSecret::load(&sync_dir) {
        if current_secret.to_bytes() == target_secret.to_bytes() {
            remove_pending_migration_file(&state.rua_root);
            return Ok(NetworkMigrationApplyOutcome::AlreadyCurrent);
        }
    }

    state.sync_runtime.shutdown_unavailable().await;
    let freeze_guard = state.inbound_apply_lock.lock().await;
    let current_node = state
        .sync_node
        .lock()
        .await
        .as_ref()
        .cloned()
        .ok_or_else(|| {
            NetworkMigrationError::CaptureExport("current SyncNode is unavailable".to_string())
        })?;
    let frozen_export = crate::services::sync_ops::capture_full_export_v4_bootstrap_for_network(
        state,
        current_node.blobs_store().clone(),
        &target_secret,
    )
    .await
    .ok_or_else(|| NetworkMigrationError::CaptureExport("domain export failed".to_string()))?;
    drop(freeze_guard);

    target_secret
        .save(&sync_dir)
        .map_err(|error| NetworkMigrationError::SaveSecret(error.to_string()))?;
    let target_secret = Arc::new(target_secret);
    let device = Arc::new(
        rua_sync::identity::DeviceIdentity::generate_or_load(&state.rua_root)
            .map_err(|error| NetworkMigrationError::DeviceIdentity(error.to_string()))?,
    );

    state.clear_sync_node().await;

    *state.sync_network_secret.write().unwrap() = Some(Arc::clone(&target_secret));
    *state.sync_device.write().unwrap() = Some(Arc::clone(&device));

    let command_handler = state.device_command_handler().ok_or_else(|| {
        NetworkMigrationError::BindRuntime("local Device identity is unavailable".to_string())
    })?;
    let node = rua_sync::endpoint::SyncNode::bind_with_command_handler(
        &sync_dir,
        target_secret.namespace_secret(),
        &device,
        None,
        command_handler,
    )
    .await
    .map_err(|error| NetworkMigrationError::BindRuntime(error.to_string()))?;
    state.install_sync_node(node).await;

    state.sync_runtime.shutdown_unavailable().await;
    state
        .ensure_sync_runtime_active()
        .await
        .map_err(|error| NetworkMigrationError::RestartRuntime(error.detail().to_string()))?;

    if !crate::services::sync_ops::publish_full_export_snapshot(state, frozen_export).await {
        return Err(NetworkMigrationError::PublishExport(
            "replica publication failed".to_string(),
        ));
    }
    remove_pending_migration_file(&state.rua_root);
    Ok(NetworkMigrationApplyOutcome::Migrated)
}

fn remove_pending_migration_file(data_dir: &std::path::Path) {
    if let Err(error) = rua_sync::control_store::SyncControlStore::from_data_dir(data_dir)
        .remove_pending_migration()
    {
        tracing::warn!(error = %error, "[sync-migration] remove pending marker");
    }
}
