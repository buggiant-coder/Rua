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

pub const ROOT_CONFIG_DOMAIN: &str = "root_config";
pub const ROOT_CONFIG_KIND_UPSERT: &str = "upsert";
pub const ROOT_CONFIG_KIND_DELETE: &str = "delete";
pub const ROOT_CONFIG_KIND_SNAPSHOT: &str = "snapshot";

pub const ROOT_CONFIG_RECORD_SETTINGS: &str = "settings";
pub const ROOT_CONFIG_RECORD_SUBAGENT_TEMPLATES: &str = "subagent_templates";
// Retained only to consume older replica entries without changing local state.
pub const ROOT_CONFIG_RECORD_DEFAULT_MODEL: &str = "default_model";
pub const ROOT_CONFIG_RECORD_AGENTS: &str = "agents";
pub const ROOT_CONFIG_RECORD_SECURITY: &str = "security";
pub const ROOT_CONFIG_RECORD_USER_TIMEZONE: &str = "user_timezone";

pub struct RootConfigSyncAdapter {
    data_dir: PathBuf,
    secret_dir: PathBuf,
    device_id: String,
}

impl RootConfigSyncAdapter {
    pub fn new(data_dir: PathBuf, secret_dir: PathBuf, device_id: impl Into<String>) -> Self {
        Self {
            data_dir,
            secret_dir,
            device_id: device_id.into(),
        }
    }

    pub fn upsert_change(
        op_id: impl Into<String>,
        record_id: impl Into<String>,
        value: serde_json::Value,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncChange {
        SyncChange::new(
            op_id,
            ROOT_CONFIG_DOMAIN,
            record_id,
            ROOT_CONFIG_KIND_UPSERT,
            device_id,
            hlc,
            value,
        )
    }

    pub fn delete_change(
        op_id: impl Into<String>,
        record_id: impl Into<String>,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncChange {
        SyncChange::new(
            op_id,
            ROOT_CONFIG_DOMAIN,
            record_id,
            ROOT_CONFIG_KIND_DELETE,
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

    fn record_path(&self, record_id: &str) -> SyncResult<PathBuf> {
        match record_id {
            ROOT_CONFIG_RECORD_SETTINGS => Ok(self.data_dir.join("settings.json")),
            ROOT_CONFIG_RECORD_SUBAGENT_TEMPLATES => {
                Ok(self.data_dir.join("subagent_templates.json"))
            }
            ROOT_CONFIG_RECORD_DEFAULT_MODEL => {
                Ok(self.secret_dir.join("providers").join("default_model.json"))
            }
            ROOT_CONFIG_RECORD_AGENTS
            | ROOT_CONFIG_RECORD_SECURITY
            | ROOT_CONFIG_RECORD_USER_TIMEZONE => Ok(self.data_dir.join("config.json")),
            other => Err(SyncError::Store(format!(
                "unsupported root-config record: {other}"
            ))),
        }
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
}

#[async_trait]
impl DomainSyncAdapter for RootConfigSyncAdapter {
    fn domain(&self) -> &str {
        ROOT_CONFIG_DOMAIN
    }

    async fn export_records(&self) -> SyncResult<Vec<SyncChange>> {
        let mut changes = Vec::new();
        for record_id in [
            ROOT_CONFIG_RECORD_SETTINGS,
            ROOT_CONFIG_RECORD_SUBAGENT_TEMPLATES,
            ROOT_CONFIG_RECORD_AGENTS,
            ROOT_CONFIG_RECORD_SECURITY,
            ROOT_CONFIG_RECORD_USER_TIMEZONE,
        ] {
            let path = self.record_path(record_id)?;
            let Some(value) = read_record_value(record_id, &path, &self.data_dir)? else {
                continue;
            };
            changes.push(SyncChange::new(
                Self::new_op_id(),
                ROOT_CONFIG_DOMAIN,
                record_id,
                ROOT_CONFIG_KIND_SNAPSHOT,
                self.device_id.clone(),
                Self::export_hlc(&path, &self.device_id),
                value,
            ));
        }
        Ok(changes)
    }

    async fn apply_change(
        &self,
        change: &SyncChange,
        merge_store: &MergeStore,
    ) -> SyncResult<ApplyOutcome> {
        if change.domain != ROOT_CONFIG_DOMAIN {
            return Err(SyncError::Store(format!(
                "root-config adapter received wrong domain: {}",
                change.domain
            )));
        }
        let path = self.record_path(&change.record_id)?;
        let cursor_id = sync_cursor_record_id(ROOT_CONFIG_DOMAIN, &change.record_id);
        let is_delete = change.kind == ROOT_CONFIG_KIND_DELETE;
        let decision = merge_store.resolve_lww(&cursor_id, change.hlc, is_delete)?;
        if decision == LwwDecision::Skip {
            return Ok(ApplyOutcome::Skipped);
        }
        if change.record_id == ROOT_CONFIG_RECORD_DEFAULT_MODEL {
            let cursor = if is_delete {
                tombstone_cursor(change.hlc, None)
            } else {
                live_cursor(change.hlc, None)
            };
            return Ok(ApplyOutcome::Applied { cursor });
        }

        if is_delete {
            delete_record_value(&change.record_id, &path)?;
            return Ok(ApplyOutcome::Applied {
                cursor: tombstone_cursor(change.hlc, None),
            });
        }
        if change.kind != ROOT_CONFIG_KIND_UPSERT && change.kind != ROOT_CONFIG_KIND_SNAPSHOT {
            return Err(SyncError::Store(format!(
                "unsupported root-config change kind: {}",
                change.kind
            )));
        }

        let merged = if change.record_id == ROOT_CONFIG_RECORD_AGENTS {
            let local = read_raw_agents_registry(&path)?
                .unwrap_or_else(|| serde_json::Value::Object(Default::default()));
            merge_agents_registry(&local, &change.payload, &self.data_dir)
        } else {
            change.payload.clone()
        };
        write_record_value(&change.record_id, &path, &merged)?;

        Ok(ApplyOutcome::Applied {
            cursor: live_cursor(change.hlc, None),
        })
    }
}

fn read_json_file(path: &Path) -> Option<serde_json::Value> {
    if !path.is_file() {
        return None;
    }
    std::fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
}

fn read_record_value(
    record_id: &str,
    path: &Path,
    data_dir: &Path,
) -> SyncResult<Option<serde_json::Value>> {
    let Some(value) = read_json_file(path) else {
        return Ok(None);
    };
    if record_id == ROOT_CONFIG_RECORD_AGENTS {
        return value
            .get("agents")
            .cloned()
            .map(|agents| {
                filter_untouched_seeded_agents(sanitize_agents_registry(agents), data_dir)
            })
            .transpose();
    }
    if record_id == ROOT_CONFIG_RECORD_SECURITY {
        return Ok(value
            .get("security")
            .cloned()
            .map(strip_device_local_security));
    }
    if record_id == ROOT_CONFIG_RECORD_USER_TIMEZONE {
        return Ok(value.get("user_timezone").cloned());
    }
    Ok(Some(value))
}

fn read_raw_agents_registry(path: &Path) -> SyncResult<Option<serde_json::Value>> {
    let Some(value) = read_json_file(path) else {
        return Ok(None);
    };
    Ok(value.get("agents").cloned())
}

fn write_record_value(record_id: &str, path: &Path, value: &serde_json::Value) -> SyncResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if record_id == ROOT_CONFIG_RECORD_AGENTS {
        let mut root = read_json_file(path).unwrap_or_else(|| serde_json::json!({}));
        let root_obj = root
            .as_object_mut()
            .ok_or_else(|| SyncError::Store("config.json root must be object".to_string()))?;
        root_obj.insert("agents".to_string(), value.clone());
        rua_config::atomic_write_json(path, &root)?;
        return Ok(());
    }
    if record_id == ROOT_CONFIG_RECORD_USER_TIMEZONE {
        let mut root = read_json_file(path).unwrap_or_else(|| serde_json::json!({}));
        let root_obj = root
            .as_object_mut()
            .ok_or_else(|| SyncError::Store("config.json root must be object".to_string()))?;
        root_obj.insert("user_timezone".to_string(), value.clone());
        rua_config::atomic_write_json(path, &root)?;
        return Ok(());
    }
    if record_id == ROOT_CONFIG_RECORD_SECURITY {
        let mut root = read_json_file(path).unwrap_or_else(|| serde_json::json!({}));
        let root_obj = root
            .as_object_mut()
            .ok_or_else(|| SyncError::Store("config.json root must be object".to_string()))?;
        let mut next_security = root_obj
            .get("security")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        let next_obj = next_security
            .as_object_mut()
            .ok_or_else(|| SyncError::Store("config.json security must be object".to_string()))?;
        let incoming = value.as_object().ok_or_else(|| {
            SyncError::Store("root security config payload must be object".to_string())
        })?;
        for (key, value) in incoming {
            if key != "allow_no_auth_hosts" {
                next_obj.insert(key.clone(), value.clone());
            }
        }
        root_obj.insert("security".to_string(), next_security);
        rua_config::atomic_write_json(path, &root)?;
        return Ok(());
    }
    rua_config::atomic_write_json(path, value)?;
    Ok(())
}

fn delete_record_value(record_id: &str, path: &Path) -> SyncResult<()> {
    if record_id == ROOT_CONFIG_RECORD_AGENTS {
        let Some(mut root) = read_json_file(path) else {
            return Ok(());
        };
        let root_obj = root
            .as_object_mut()
            .ok_or_else(|| SyncError::Store("config.json root must be object".to_string()))?;
        root_obj.remove("agents");
        rua_config::atomic_write_json(path, &root)?;
        return Ok(());
    }
    if record_id == ROOT_CONFIG_RECORD_USER_TIMEZONE {
        let Some(mut root) = read_json_file(path) else {
            return Ok(());
        };
        let root_obj = root
            .as_object_mut()
            .ok_or_else(|| SyncError::Store("config.json root must be object".to_string()))?;
        root_obj.remove("user_timezone");
        rua_config::atomic_write_json(path, &root)?;
        return Ok(());
    }
    if record_id == ROOT_CONFIG_RECORD_SECURITY {
        let Some(mut root) = read_json_file(path) else {
            return Ok(());
        };
        let root_obj = root
            .as_object_mut()
            .ok_or_else(|| SyncError::Store("config.json root must be object".to_string()))?;
        root_obj.remove("security");
        rua_config::atomic_write_json(path, &root)?;
        return Ok(());
    }

    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

fn merge_agents_registry(
    local: &serde_json::Value,
    remote: &serde_json::Value,
    data_dir: &Path,
) -> serde_json::Value {
    let mut merged = local.clone();
    if !merged.is_object() {
        merged = serde_json::json!({});
    }

    let Some(merged_obj) = merged.as_object_mut() else {
        return remote.clone();
    };
    let Some(remote_obj) = remote.as_object() else {
        return merged;
    };

    if let Some(remote_profiles) = remote_obj
        .get("profiles")
        .and_then(|value| value.as_object())
    {
        let profiles = merged_obj
            .entry("profiles")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(local_profiles) = profiles.as_object_mut() {
            for (agent_id, profile) in remote_profiles {
                local_profiles.insert(agent_id.clone(), profile.clone());
            }
        } else {
            merged_obj.insert(
                "profiles".to_string(),
                serde_json::Value::Object(remote_profiles.clone()),
            );
        }
    }

    if let Some(remote_order) = remote_obj
        .get("agent_order")
        .and_then(|value| value.as_array())
    {
        let mut order = remote_order.clone();
        if let Some(local_order) = local.get("agent_order").and_then(|value| value.as_array()) {
            for agent_id in local_order {
                if !order.contains(agent_id) {
                    order.push(agent_id.clone());
                }
            }
        }
        merged_obj.insert("agent_order".to_string(), serde_json::Value::Array(order));
    }

    for (key, value) in remote_obj {
        if key != "profiles" && key != "agent_order" {
            merged_obj.insert(key.clone(), value.clone());
        }
    }

    localize_agent_profile_workspace_dirs(&mut merged, data_dir);
    merged
}

fn sanitize_agents_registry(mut value: serde_json::Value) -> serde_json::Value {
    if let Some(profiles) = value
        .get_mut("profiles")
        .and_then(|profiles| profiles.as_object_mut())
    {
        for profile in profiles.values_mut() {
            if let Some(profile_obj) = profile.as_object_mut() {
                profile_obj.remove("workspace_dir");
            }
        }
    }
    value
}

fn filter_untouched_seeded_agents(
    mut value: serde_json::Value,
    data_dir: &Path,
) -> SyncResult<serde_json::Value> {
    let Some(profiles) = value
        .get_mut("profiles")
        .and_then(|profiles| profiles.as_object_mut())
    else {
        return Ok(value);
    };

    let mut filtered_agent_ids = Vec::new();
    for agent_id in profiles.keys() {
        let agent_json = format!("workspaces/{agent_id}/agent.json");
        if crate::seed_filter::is_untouched_seed_file(data_dir, &agent_json)? {
            filtered_agent_ids.push(agent_id.clone());
        }
    }

    if filtered_agent_ids.is_empty() {
        return Ok(value);
    }

    for agent_id in &filtered_agent_ids {
        profiles.remove(agent_id);
    }

    if let Some(order) = value
        .get_mut("agent_order")
        .and_then(|order| order.as_array_mut())
    {
        order.retain(|agent_id| {
            agent_id
                .as_str()
                .map(|agent_id| {
                    !filtered_agent_ids
                        .iter()
                        .any(|filtered| filtered == agent_id)
                })
                .unwrap_or(true)
        });
    }

    let active_agent_is_filtered = value
        .get("active_agent")
        .and_then(|active_agent| active_agent.as_str())
        .map(|active_agent| {
            filtered_agent_ids
                .iter()
                .any(|filtered| filtered == active_agent)
        })
        .unwrap_or(false);
    if active_agent_is_filtered {
        if let Some(obj) = value.as_object_mut() {
            obj.remove("active_agent");
        }
    }

    Ok(value)
}

fn localize_agent_profile_workspace_dirs(value: &mut serde_json::Value, data_dir: &Path) {
    let Some(profiles) = value
        .get_mut("profiles")
        .and_then(|profiles| profiles.as_object_mut())
    else {
        return;
    };
    for (agent_id, profile) in profiles {
        let Some(profile_obj) = profile.as_object_mut() else {
            continue;
        };
        let workspace_dir = data_dir.join("workspaces").join(agent_id);
        profile_obj.insert(
            "workspace_dir".to_string(),
            serde_json::Value::String(workspace_dir.to_string_lossy().into_owned()),
        );
    }
}

fn strip_device_local_security(mut value: serde_json::Value) -> serde_json::Value {
    if let Some(obj) = value.as_object_mut() {
        obj.remove("allow_no_auth_hosts");
    }
    value
}
