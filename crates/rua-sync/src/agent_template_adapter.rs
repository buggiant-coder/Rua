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

pub const AGENT_TEMPLATE_DOMAIN: &str = "agent_template";
pub const AGENT_TEMPLATE_KIND_UPSERT: &str = "upsert";
pub const AGENT_TEMPLATE_KIND_DELETE: &str = "delete";
pub const AGENT_TEMPLATE_KIND_SNAPSHOT: &str = "snapshot";

pub struct AgentTemplateSyncAdapter {
    data_dir: PathBuf,
    device_id: String,
}

impl AgentTemplateSyncAdapter {
    pub fn new(data_dir: PathBuf, device_id: impl Into<String>) -> Self {
        Self {
            data_dir,
            device_id: device_id.into(),
        }
    }

    pub fn upsert_change(
        op_id: impl Into<String>,
        template_id: impl Into<String>,
        template: serde_json::Value,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncChange {
        SyncChange::new(
            op_id,
            AGENT_TEMPLATE_DOMAIN,
            template_id,
            AGENT_TEMPLATE_KIND_UPSERT,
            device_id,
            hlc,
            template,
        )
    }

    pub fn delete_change(
        op_id: impl Into<String>,
        template_id: impl Into<String>,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncChange {
        SyncChange::new(
            op_id,
            AGENT_TEMPLATE_DOMAIN,
            template_id,
            AGENT_TEMPLATE_KIND_DELETE,
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

    fn file_path(&self) -> PathBuf {
        self.data_dir.join("agent_templates.json")
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
impl DomainSyncAdapter for AgentTemplateSyncAdapter {
    fn domain(&self) -> &str {
        AGENT_TEMPLATE_DOMAIN
    }

    async fn export_records(&self) -> SyncResult<Vec<SyncChange>> {
        let path = self.file_path();
        if crate::seed_filter::is_untouched_seed_file(&self.data_dir, "agent_templates.json")? {
            return Ok(Vec::new());
        }
        let file = read_templates_file(&path)?;
        let Some(templates) = file.get("templates").and_then(|value| value.as_object()) else {
            return Ok(Vec::new());
        };
        let hlc = Self::export_hlc(&path, &self.device_id);
        let mut changes = Vec::with_capacity(templates.len());
        for (template_id, template) in templates {
            validate_template_id(template_id)?;
            changes.push(SyncChange::new(
                Self::new_op_id(),
                AGENT_TEMPLATE_DOMAIN,
                template_id,
                AGENT_TEMPLATE_KIND_SNAPSHOT,
                self.device_id.clone(),
                hlc,
                template.clone(),
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
        if change.domain != AGENT_TEMPLATE_DOMAIN {
            return Err(SyncError::Store(format!(
                "agent-template adapter received wrong domain: {}",
                change.domain
            )));
        }
        validate_template_id(&change.record_id)?;
        let cursor_id = sync_cursor_record_id(AGENT_TEMPLATE_DOMAIN, &change.record_id);
        let is_delete = change.kind == AGENT_TEMPLATE_KIND_DELETE;
        let decision = merge_store.resolve_lww(&cursor_id, change.hlc, is_delete)?;
        if decision == LwwDecision::Skip {
            return Ok(ApplyOutcome::Skipped);
        }

        let path = self.file_path();
        let mut file = read_templates_file(&path)?;
        let root = file
            .as_object_mut()
            .ok_or_else(|| SyncError::Store("agent_templates.json root must be object".into()))?;
        root.entry("version")
            .or_insert_with(|| serde_json::Value::Number(1.into()));
        let templates = root
            .entry("templates")
            .or_insert_with(|| serde_json::Value::Object(Default::default()));
        let templates = templates.as_object_mut().ok_or_else(|| {
            SyncError::Store("agent_templates.json templates must be object".into())
        })?;

        if is_delete {
            templates.remove(&change.record_id);
            rua_config::atomic_write_json(&path, &file)?;
            return Ok(ApplyOutcome::Applied {
                cursor: tombstone_cursor(change.hlc, None),
            });
        }
        if change.kind != AGENT_TEMPLATE_KIND_UPSERT && change.kind != AGENT_TEMPLATE_KIND_SNAPSHOT
        {
            return Err(SyncError::Store(format!(
                "unsupported agent-template change kind: {}",
                change.kind
            )));
        }

        let merged = change.payload.clone();
        templates.insert(change.record_id.clone(), merged.clone());
        rua_config::atomic_write_json(&path, &file)?;

        Ok(ApplyOutcome::Applied {
            cursor: live_cursor(change.hlc, None),
        })
    }
}

fn read_templates_file(path: &Path) -> SyncResult<serde_json::Value> {
    if !path.is_file() {
        return Ok(serde_json::json!({ "version": 1, "templates": {} }));
    }
    let value = std::fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_else(|| serde_json::json!({ "version": 1, "templates": {} }));
    Ok(value)
}

fn validate_template_id(value: &str) -> SyncResult<()> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\\')
    {
        return Err(SyncError::Store(format!(
            "invalid agent-template id: {value}"
        )));
    }
    Ok(())
}
