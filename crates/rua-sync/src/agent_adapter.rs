use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use async_trait::async_trait;
use rand::RngCore;

use crate::hlc::HlcTimestamp;
use crate::merge_store::{live_cursor, tombstone_cursor, MergeStore};
use crate::opaque_replica_log::SyncChange;
use crate::sidecar::LwwDecision;
use crate::sync_registry::{sync_cursor_record_id, ApplyOutcome, DomainSyncAdapter};
use crate::{SyncError, SyncResult};
use tracing::warn;

pub const AGENT_CONFIG_DOMAIN: &str = "agent_config";
pub const AGENT_CONFIG_KIND_UPSERT: &str = "upsert";
pub const AGENT_CONFIG_KIND_DELETE: &str = "delete";
pub const AGENT_CONFIG_KIND_SNAPSHOT: &str = "snapshot";
const DEVICE_LOCAL_AGENT_FIELDS: &[&str] = &["channels", "security", "running", "default_model"];
const DEVICE_LOCAL_ACP_RUNNER_FIELDS: &[&str] = &["command", "args", "env"];

pub struct AgentConfigSyncAdapter {
    data_dir: PathBuf,
    device_id: String,
    local_mcp_key: Option<[u8; 32]>,
}

impl AgentConfigSyncAdapter {
    pub fn new(data_dir: PathBuf, device_id: impl Into<String>) -> Self {
        Self {
            data_dir,
            device_id: device_id.into(),
            local_mcp_key: None,
        }
    }

    pub fn with_local_mcp_key(mut self, key: [u8; 32]) -> Self {
        self.local_mcp_key = Some(key);
        self
    }

    pub fn upsert_change(
        op_id: impl Into<String>,
        agent_id: impl Into<String>,
        config: serde_json::Value,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncChange {
        SyncChange::new(
            op_id,
            AGENT_CONFIG_DOMAIN,
            agent_id,
            AGENT_CONFIG_KIND_UPSERT,
            device_id,
            hlc,
            config,
        )
    }

    pub fn delete_change(
        op_id: impl Into<String>,
        agent_id: impl Into<String>,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncChange {
        SyncChange::new(
            op_id,
            AGENT_CONFIG_DOMAIN,
            agent_id,
            AGENT_CONFIG_KIND_DELETE,
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

    fn workspaces_dir(&self) -> PathBuf {
        self.data_dir.join("workspaces")
    }

    fn agent_path(&self, agent_id: &str) -> SyncResult<PathBuf> {
        validate_agent_id(agent_id)?;
        Ok(self.workspaces_dir().join(agent_id).join("agent.json"))
    }

    fn config_path(&self) -> PathBuf {
        self.data_dir.join("config.json")
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

    fn localize_agent_config(
        &self,
        agent_id: &str,
        mut config: serde_json::Value,
    ) -> SyncResult<serde_json::Value> {
        let workspace_dir = self.workspaces_dir().join(agent_id);
        let obj = config.as_object_mut().ok_or_else(|| {
            SyncError::Store("agent config payload must be an object".to_string())
        })?;
        obj.insert(
            "id".to_string(),
            serde_json::Value::String(agent_id.to_string()),
        );
        obj.insert(
            "workspace_dir".to_string(),
            serde_json::Value::String(workspace_dir.to_string_lossy().into_owned()),
        );
        Ok(config)
    }

    fn normalize_agent_config_for_sync(
        &self,
        mut config: serde_json::Value,
    ) -> SyncResult<serde_json::Value> {
        let Some(key) = self.local_mcp_key.as_ref() else {
            strip_device_local_agent_fields(&mut config);
            return Ok(config);
        };
        if let Some(mcp_value) = config.get("mcp").cloned() {
            let logical = rua_mcp::secret_field_io::decrypt_client_value(&mcp_value, Some(key))
                .map_err(|e| SyncError::Store(format!("decrypt agent MCP for sync: {e}")))?;
            if let Some(obj) = config.as_object_mut() {
                obj.insert("mcp".to_string(), logical);
            }
        }
        strip_device_local_agent_fields(&mut config);
        Ok(config)
    }

    fn encrypt_agent_config_for_local(
        &self,
        agent_id: &str,
        mut config: serde_json::Value,
    ) -> SyncResult<serde_json::Value> {
        let Some(key) = self.local_mcp_key.as_ref() else {
            return Ok(config);
        };
        if let Some(mcp_value) = config.get("mcp").cloned() {
            let encrypted = rua_mcp::secret_field_io::encrypt_client_value(
                &mcp_value,
                Some(key),
                format!("agent:{agent_id}:mcp"),
            )
            .map_err(|e| SyncError::Store(format!("encrypt agent MCP for local disk: {e}")))?;
            if let Some(obj) = config.as_object_mut() {
                obj.insert("mcp".to_string(), encrypted);
            }
        }
        Ok(config)
    }

    fn register_agent(&self, agent_id: &str) -> SyncResult<()> {
        let workspace_dir = self.workspaces_dir().join(agent_id);
        let config_path = self.config_path();
        let mut config: serde_json::Value = std::fs::read_to_string(&config_path)
            .ok()
            .and_then(|content| serde_json::from_str(&content).ok())
            .unwrap_or_else(|| serde_json::json!({}));

        let root = config
            .as_object_mut()
            .ok_or_else(|| SyncError::Store("config.json root must be an object".to_string()))?;
        let agents = root
            .entry("agents")
            .or_insert_with(|| serde_json::json!({}));
        let agents_obj = agents
            .as_object_mut()
            .ok_or_else(|| SyncError::Store("config.agents must be an object".to_string()))?;
        let profiles = agents_obj
            .entry("profiles")
            .or_insert_with(|| serde_json::json!({}));
        let profiles_obj = profiles.as_object_mut().ok_or_else(|| {
            SyncError::Store("config.agents.profiles must be an object".to_string())
        })?;
        profiles_obj.insert(
            agent_id.to_string(),
            serde_json::json!({
                "id": agent_id,
                "workspace_dir": workspace_dir.to_string_lossy(),
                "enabled": true,
            }),
        );

        let order = agents_obj
            .entry("agent_order")
            .or_insert_with(|| serde_json::json!([]));
        let order = order.as_array_mut().ok_or_else(|| {
            SyncError::Store("config.agents.agent_order must be an array".to_string())
        })?;
        let agent_value = serde_json::Value::String(agent_id.to_string());
        if !order.contains(&agent_value) {
            order.push(agent_value);
        }

        rua_config::atomic_write_json(&config_path, &config)?;
        Ok(())
    }

    fn unregister_agent(&self, agent_id: &str) -> SyncResult<()> {
        let config_path = self.config_path();
        let Some(mut config) = std::fs::read_to_string(&config_path)
            .ok()
            .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
        else {
            return Ok(());
        };

        let Some(agents_obj) = config
            .get_mut("agents")
            .and_then(|value| value.as_object_mut())
        else {
            return Ok(());
        };
        if let Some(profiles_obj) = agents_obj
            .get_mut("profiles")
            .and_then(|value| value.as_object_mut())
        {
            profiles_obj.remove(agent_id);
        }
        if let Some(order) = agents_obj
            .get_mut("agent_order")
            .and_then(|value| value.as_array_mut())
        {
            order.retain(|value| value.as_str() != Some(agent_id));
        }
        rua_config::atomic_write_json(&config_path, &config)?;
        Ok(())
    }
}

#[async_trait]
impl DomainSyncAdapter for AgentConfigSyncAdapter {
    fn domain(&self) -> &str {
        AGENT_CONFIG_DOMAIN
    }

    async fn export_records(&self) -> SyncResult<Vec<SyncChange>> {
        let workspaces = self.workspaces_dir();
        if !workspaces.is_dir() {
            return Ok(Vec::new());
        }

        let mut changes = Vec::new();
        for entry in std::fs::read_dir(&workspaces)? {
            let entry = entry?;
            let path = entry.path().join("agent.json");
            if !path.is_file() {
                continue;
            }
            let Some(agent_id) = entry.file_name().to_str().map(ToOwned::to_owned) else {
                continue;
            };
            let seed_path = format!("workspaces/{agent_id}/agent.json");
            if crate::seed_filter::is_untouched_seed_file(&self.data_dir, &seed_path)? {
                continue;
            }
            let config: serde_json::Value = match std::fs::read_to_string(&path)
                .map_err(SyncError::from)
                .and_then(|content| serde_json::from_str(&content).map_err(SyncError::from))
                .and_then(|config| self.normalize_agent_config_for_sync(config))
            {
                Ok(config) => config,
                Err(e) => {
                    warn!(
                        agent_id = %agent_id,
                        path = %path.display(),
                        error = %e,
                        "[sync-v4] skipping agent config during full export"
                    );
                    continue;
                }
            };
            changes.push(SyncChange::new(
                Self::new_op_id(),
                AGENT_CONFIG_DOMAIN,
                agent_id,
                AGENT_CONFIG_KIND_SNAPSHOT,
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
        if change.domain != AGENT_CONFIG_DOMAIN {
            return Err(SyncError::Store(format!(
                "agent config adapter received wrong domain: {}",
                change.domain
            )));
        }
        validate_agent_id(&change.record_id)?;

        let cursor_id = sync_cursor_record_id(AGENT_CONFIG_DOMAIN, &change.record_id);
        let is_delete = change.kind == AGENT_CONFIG_KIND_DELETE;
        let decision = merge_store.resolve_lww(&cursor_id, change.hlc, is_delete)?;
        if decision == LwwDecision::Skip {
            return Ok(ApplyOutcome::Skipped);
        }

        let path = self.agent_path(&change.record_id)?;
        if is_delete {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            self.unregister_agent(&change.record_id)?;
            return Ok(ApplyOutcome::Applied {
                cursor: tombstone_cursor(change.hlc, None),
            });
        }
        if change.kind != AGENT_CONFIG_KIND_UPSERT && change.kind != AGENT_CONFIG_KIND_SNAPSHOT {
            return Err(SyncError::Store(format!(
                "unsupported agent config change kind: {}",
                change.kind
            )));
        }

        let existing_disk_value: serde_json::Value = std::fs::read_to_string(&path)
            .ok()
            .and_then(|content| serde_json::from_str(&content).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        let remote_value = self.normalize_agent_config_for_sync(change.payload.clone())?;
        let localized = self.localize_agent_config(&change.record_id, remote_value)?;
        let mut disk_value =
            self.encrypt_agent_config_for_local(&change.record_id, localized.clone())?;
        restore_device_local_agent_fields(&mut disk_value, &existing_disk_value);

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        rua_config::atomic_write_json(&path, &disk_value)?;
        self.register_agent(&change.record_id)?;

        Ok(ApplyOutcome::Applied {
            cursor: live_cursor(change.hlc, None),
        })
    }
}

fn strip_device_local_agent_fields(config: &mut serde_json::Value) {
    let Some(obj) = config.as_object_mut() else {
        return;
    };
    for field in DEVICE_LOCAL_AGENT_FIELDS {
        obj.remove(*field);
    }
    strip_device_local_acp_runner_fields(config);
}

fn restore_device_local_agent_fields(target: &mut serde_json::Value, existing: &serde_json::Value) {
    let (Some(target_obj), Some(existing_obj)) = (target.as_object_mut(), existing.as_object())
    else {
        return;
    };
    for field in DEVICE_LOCAL_AGENT_FIELDS {
        if let Some(value) = existing_obj.get(*field) {
            target_obj.insert((*field).to_string(), value.clone());
        }
    }
    restore_device_local_acp_runner_fields(target, existing);
}

fn strip_device_local_acp_runner_fields(config: &mut serde_json::Value) {
    let Some(runners) = config
        .get_mut("acp")
        .and_then(|value| value.get_mut("agents"))
        .and_then(|value| value.as_object_mut())
    else {
        return;
    };
    for runner in runners.values_mut() {
        let Some(runner_obj) = runner.as_object_mut() else {
            continue;
        };
        for field in DEVICE_LOCAL_ACP_RUNNER_FIELDS {
            runner_obj.remove(*field);
        }
    }
}

fn restore_device_local_acp_runner_fields(
    target: &mut serde_json::Value,
    existing: &serde_json::Value,
) {
    let (Some(target_runners), Some(existing_runners)) = (
        target
            .get_mut("acp")
            .and_then(|value| value.get_mut("agents"))
            .and_then(|value| value.as_object_mut()),
        existing
            .get("acp")
            .and_then(|value| value.get("agents"))
            .and_then(|value| value.as_object()),
    ) else {
        return;
    };

    for (runner_key, existing_runner) in existing_runners {
        let Some(target_runner) = target_runners
            .get_mut(runner_key)
            .and_then(|value| value.as_object_mut())
        else {
            continue;
        };
        let Some(existing_runner_obj) = existing_runner.as_object() else {
            continue;
        };
        for field in DEVICE_LOCAL_ACP_RUNNER_FIELDS {
            if let Some(value) = existing_runner_obj.get(*field) {
                target_runner.insert((*field).to_string(), value.clone());
            }
        }
    }
}

fn validate_agent_id(agent_id: &str) -> SyncResult<()> {
    if agent_id.is_empty()
        || agent_id.contains("..")
        || agent_id.contains('/')
        || agent_id.contains('\\')
    {
        return Err(SyncError::Store(format!("invalid agent id: {agent_id}")));
    }
    Ok(())
}
