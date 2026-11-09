use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use async_trait::async_trait;
use rand::RngCore;

use crate::hlc::HlcTimestamp;
use crate::merge_store::{live_cursor, tombstone_cursor, MergeStore};
use crate::opaque_replica_log::SyncChange;
use crate::sidecar::LwwDecision;
use crate::sync_registry::{sync_cursor_record_id, ApplyOutcome, DomainSyncAdapter};
use crate::{SyncError, SyncResult};

pub const PROVIDER_SECRET_DOMAIN: &str = "provider_secret";
pub const PROVIDER_SECRET_KIND_UPSERT: &str = "upsert";
pub const PROVIDER_SECRET_KIND_DELETE: &str = "delete";
pub const PROVIDER_SECRET_KIND_SNAPSHOT: &str = "snapshot";

const DEVICE_LOCAL_FIELDS: &[&str] = &["workspace_dir"];

pub struct ProviderSecretSyncAdapter {
    secret_dir: PathBuf,
    device_id: String,
    provider_manager: Option<Arc<rua_provider::manager::ProviderManager>>,
    local_secret_key: Option<[u8; 32]>,
}

impl ProviderSecretSyncAdapter {
    pub fn new(secret_dir: PathBuf, device_id: impl Into<String>) -> Self {
        Self {
            secret_dir,
            device_id: device_id.into(),
            provider_manager: None,
            local_secret_key: None,
        }
    }

    pub fn with_provider_manager(
        mut self,
        provider_manager: Arc<rua_provider::manager::ProviderManager>,
    ) -> Self {
        self.provider_manager = Some(provider_manager);
        self
    }

    pub fn with_local_secret_key(mut self, key: [u8; 32]) -> Self {
        self.local_secret_key = Some(key);
        self
    }

    pub fn upsert_change(
        op_id: impl Into<String>,
        provider_id: impl Into<String>,
        config: serde_json::Value,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncChange {
        SyncChange::new(
            op_id,
            PROVIDER_SECRET_DOMAIN,
            provider_id,
            PROVIDER_SECRET_KIND_UPSERT,
            device_id,
            hlc,
            config,
        )
    }

    pub fn delete_change(
        op_id: impl Into<String>,
        provider_id: impl Into<String>,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncChange {
        SyncChange::new(
            op_id,
            PROVIDER_SECRET_DOMAIN,
            provider_id,
            PROVIDER_SECRET_KIND_DELETE,
            device_id,
            hlc,
            serde_json::Value::Object(Default::default()),
        )
    }

    pub fn new_op_id() -> String {
        let mut bytes = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        hex::encode(bytes)
    }

    fn providers_dir(&self) -> PathBuf {
        self.secret_dir.join("providers").join("providers")
    }

    fn provider_path(&self, provider_id: &str) -> SyncResult<PathBuf> {
        validate_provider_id(provider_id)?;
        Ok(self.providers_dir().join(format!("{provider_id}.json")))
    }

    fn provider_loader(&self) -> rua_provider::ProviderConfigLoader {
        match self.local_secret_key {
            Some(key) => rua_provider::ProviderConfigLoader::with_key(self.secret_dir.clone(), key),
            None => rua_provider::ProviderConfigLoader::new(self.secret_dir.clone()),
        }
    }

    async fn read_local_provider_value(&self, provider_id: &str) -> SyncResult<serde_json::Value> {
        let config = self
            .provider_loader()
            .load_provider(provider_id)
            .await
            .map_err(|e| SyncError::Store(format!("read provider {provider_id}: {e}")))?;
        serde_json::to_value(config)
            .map_err(|e| SyncError::Store(format!("serialize provider: {e}")))
    }

    async fn write_local_provider_value(
        &self,
        provider_id: &str,
        value: &serde_json::Value,
    ) -> SyncResult<()> {
        let config = serde_json::from_value::<rua_provider::ProviderConfig>(value.clone())
            .map_err(|e| SyncError::Store(format!("deserialize provider {provider_id}: {e}")))?;
        self.provider_loader()
            .save_provider(&config)
            .await
            .map_err(|e| SyncError::Store(format!("write provider {provider_id}: {e}")))
    }

    fn export_hlc(path: &Path, device_id: &str) -> HlcTimestamp {
        let wall_ms = path
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0);
        let mut device = [0u8; 8];
        device.copy_from_slice(&blake3::hash(device_id.as_bytes()).as_bytes()[..8]);
        HlcTimestamp {
            wall_ms,
            counter: 0,
            device_id: device,
        }
    }

    fn localize_provider_config(
        provider_id: &str,
        mut config: serde_json::Value,
    ) -> SyncResult<serde_json::Value> {
        let obj = config.as_object_mut().ok_or_else(|| {
            SyncError::Store("provider config payload must be an object".to_string())
        })?;
        obj.insert(
            "id".to_string(),
            serde_json::Value::String(provider_id.to_string()),
        );
        Ok(config)
    }

    fn reload_provider(&self, provider_id: &str, config: &serde_json::Value) {
        let Some(manager) = &self.provider_manager else {
            return;
        };
        let parsed = match serde_json::from_value::<rua_provider::ProviderConfig>(config.clone()) {
            Ok(config) => config,
            Err(e) => {
                tracing::warn!(
                    provider_id,
                    error = %e,
                    "[sync-v4] provider reload skipped: invalid provider config"
                );
                return;
            }
        };
        match rua_provider::manager::create_provider_from_config(&parsed) {
            Ok(provider) => manager.register(provider),
            Err(e) => {
                tracing::warn!(
                    provider_id,
                    error = %e,
                    "[sync-v4] provider reload skipped: create provider failed"
                );
            }
        }
    }

    fn remove_provider(&self, provider_id: &str) {
        if let Some(manager) = &self.provider_manager {
            manager.remove(provider_id);
        }
    }
}

#[async_trait]
impl DomainSyncAdapter for ProviderSecretSyncAdapter {
    fn domain(&self) -> &str {
        PROVIDER_SECRET_DOMAIN
    }

    async fn export_records(&self) -> SyncResult<Vec<SyncChange>> {
        let providers_dir = self.providers_dir();
        if !providers_dir.is_dir() {
            return Ok(Vec::new());
        }

        let mut changes = Vec::new();
        for entry in std::fs::read_dir(&providers_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Some(provider_id) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            validate_provider_id(provider_id)?;
            let config = self.read_local_provider_value(provider_id).await?;
            changes.push(SyncChange::new(
                Self::new_op_id(),
                PROVIDER_SECRET_DOMAIN,
                provider_id,
                PROVIDER_SECRET_KIND_SNAPSHOT,
                self.device_id.clone(),
                Self::export_hlc(&path, &self.device_id),
                config,
            ));
        }
        changes.sort_by(|a, b| a.record_id.cmp(&b.record_id));
        Ok(changes)
    }

    async fn apply_change(
        &self,
        change: &SyncChange,
        merge_store: &MergeStore,
    ) -> SyncResult<ApplyOutcome> {
        if change.domain != PROVIDER_SECRET_DOMAIN {
            return Err(SyncError::Store(format!(
                "provider adapter received wrong domain: {}",
                change.domain
            )));
        }
        validate_provider_id(&change.record_id)?;

        let cursor_id = sync_cursor_record_id(PROVIDER_SECRET_DOMAIN, &change.record_id);
        let is_delete = change.kind == PROVIDER_SECRET_KIND_DELETE;
        let decision = merge_store.resolve_lww(&cursor_id, change.hlc, is_delete)?;
        if decision == LwwDecision::Skip {
            return Ok(ApplyOutcome::Skipped);
        }

        let path = self.provider_path(&change.record_id)?;
        if is_delete {
            if path.exists() {
                std::fs::remove_file(&path)?;
            }
            self.remove_provider(&change.record_id);
            return Ok(ApplyOutcome::Applied {
                cursor: tombstone_cursor(change.hlc, None),
            });
        }
        if change.kind != PROVIDER_SECRET_KIND_UPSERT
            && change.kind != PROVIDER_SECRET_KIND_SNAPSHOT
        {
            return Err(SyncError::Store(format!(
                "unsupported provider change kind: {}",
                change.kind
            )));
        }

        let local_value = if path.exists() {
            self.read_local_provider_value(&change.record_id)
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!(
                        provider_id = %change.record_id,
                        error = %e,
                        "[sync-v4] failed to read local provider for merge; using empty base"
                    );
                    serde_json::json!({})
                })
        } else {
            serde_json::json!({})
        };
        let mut merged = change.payload.clone();
        preserve_device_local_fields(&mut merged, &local_value);
        let localized = Self::localize_provider_config(&change.record_id, merged)?;

        self.write_local_provider_value(&change.record_id, &localized)
            .await?;
        self.reload_provider(&change.record_id, &localized);

        Ok(ApplyOutcome::Applied {
            cursor: live_cursor(change.hlc, None),
        })
    }
}

fn validate_provider_id(provider_id: &str) -> SyncResult<()> {
    if provider_id.is_empty()
        || provider_id.contains("..")
        || provider_id.contains('/')
        || provider_id.contains('\\')
    {
        return Err(SyncError::Store(format!(
            "invalid provider id: {provider_id}"
        )));
    }
    Ok(())
}

fn preserve_device_local_fields(merged: &mut serde_json::Value, local: &serde_json::Value) {
    let Some(merged_obj) = merged.as_object_mut() else {
        return;
    };
    let Some(local_obj) = local.as_object() else {
        return;
    };
    for field in DEVICE_LOCAL_FIELDS {
        if let Some(value) = local_obj.get(*field) {
            merged_obj.insert((*field).to_string(), value.clone());
        }
    }
}
