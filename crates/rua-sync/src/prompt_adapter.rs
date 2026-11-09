use crate::prompt_file_apply::{apply_prompt_file, committed_hlc, live_payload};
use rua_prompts::file_store::{PromptFileChange, PromptFileStore};
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use async_trait::async_trait;
use rand::RngCore;

use crate::hlc::HlcTimestamp;
use crate::merge_store::MergeStore;
use crate::opaque_replica_log::SyncChange;
use crate::sync_registry::{ApplyOutcome, DomainSyncAdapter};
use crate::{SyncError, SyncResult};

pub const PROMPT_DOMAIN: &str = "prompts";
pub const PROMPT_KIND_UPSERT: &str = "upsert";
pub const PROMPT_KIND_DELETE: &str = "delete";
pub const PROMPT_KIND_SNAPSHOT: &str = "snapshot";

pub struct PromptLibrarySyncAdapter {
    data_dir: PathBuf,
    device_id: String,
}

impl PromptLibrarySyncAdapter {
    pub fn new(data_dir: PathBuf, device_id: impl Into<String>) -> Self {
        Self {
            data_dir,
            device_id: device_id.into(),
        }
    }

    pub fn prompt_dir(&self) -> PathBuf {
        self.data_dir.join("prompt_library")
    }

    pub fn upsert_change(
        op_id: impl Into<String>,
        record_id: impl Into<String>,
        content: impl Into<String>,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncChange {
        SyncChange::new(
            op_id,
            PROMPT_DOMAIN,
            record_id,
            PROMPT_KIND_UPSERT,
            device_id,
            hlc,
            serde_json::json!({ "content": content.into() }),
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
            PROMPT_DOMAIN,
            record_id,
            PROMPT_KIND_DELETE,
            device_id,
            hlc,
            serde_json::Value::Object(Default::default()),
        )
    }

    pub fn committed_change(change: PromptFileChange, device_id: &str) -> SyncChange {
        let kind = if change.content.is_some() {
            PROMPT_KIND_UPSERT
        } else {
            PROMPT_KIND_DELETE
        };
        let payload = change
            .content
            .map(|content| live_payload(content, change.deleted_before, device_id))
            .unwrap_or_else(|| serde_json::json!({}));
        SyncChange::new(
            Self::new_op_id(),
            PROMPT_DOMAIN,
            change.path,
            kind,
            device_id,
            committed_hlc(change.version, device_id),
            payload,
        )
    }

    pub fn new_op_id() -> String {
        let mut bytes = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        hex::encode(bytes)
    }

    fn record_path(&self, record_id: &str) -> SyncResult<PathBuf> {
        if record_id.is_empty() {
            return Err(SyncError::Store(
                "prompt record id must not be empty".to_string(),
            ));
        }

        let relative = Path::new(record_id);
        if relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return Err(SyncError::Store(format!(
                "invalid prompt record id: {record_id}"
            )));
        }
        Ok(self.prompt_dir().join(relative))
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

    fn collect_markdown_files(
        root: &Path,
        current: &Path,
        out: &mut Vec<PathBuf>,
    ) -> SyncResult<()> {
        if !current.is_dir() {
            return Ok(());
        }

        for entry in std::fs::read_dir(current)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                Self::collect_markdown_files(root, &path, out)?;
            } else if path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("md"))
                .unwrap_or(false)
            {
                let relative = path
                    .strip_prefix(root)
                    .map_err(|e| SyncError::Store(format!("prompt export strip_prefix: {e}")))?;
                out.push(relative.to_path_buf());
            }
        }
        Ok(())
    }
}

#[async_trait]
impl DomainSyncAdapter for PromptLibrarySyncAdapter {
    fn domain(&self) -> &str {
        PROMPT_DOMAIN
    }

    async fn export_records(&self) -> SyncResult<Vec<SyncChange>> {
        let guard = PromptFileStore::for_library(&self.data_dir).lock().await;
        let root = self.prompt_dir();
        let mut files = Vec::new();
        Self::collect_markdown_files(&root, &root, &mut files)?;
        files.sort();

        let mut changes = Vec::with_capacity(files.len());
        for relative in files {
            let path = root.join(&relative);
            let record_id = relative.to_string_lossy().replace('\\', "/");
            let seed_path = format!("prompt_library/{record_id}");
            if guard.local_revision(&record_id)?.is_none()
                && crate::seed_filter::is_untouched_seed_file(&self.data_dir, &seed_path)?
            {
                continue;
            }
            let content = std::fs::read_to_string(&path)?;
            let hlc = guard
                .local_revision(&record_id)?
                .map(|(v, _)| committed_hlc(v, &self.device_id))
                .unwrap_or_else(|| Self::export_hlc(&path, &self.device_id));
            let payload = live_payload(content, guard.deletion_fence(&record_id)?, &self.device_id);
            changes.push(SyncChange::new(
                Self::new_op_id(),
                PROMPT_DOMAIN,
                record_id,
                PROMPT_KIND_SNAPSHOT,
                self.device_id.clone(),
                hlc,
                payload,
            ));
        }
        changes.extend(
            guard
                .deletions()?
                .into_iter()
                .map(|c| Self::committed_change(c, &self.device_id)),
        );
        changes.sort_by(|a, b| a.record_id.cmp(&b.record_id));
        Ok(changes)
    }

    async fn apply_change(
        &self,
        change: &SyncChange,
        merge_store: &MergeStore,
    ) -> SyncResult<ApplyOutcome> {
        if change.domain != PROMPT_DOMAIN {
            return Err(SyncError::Store(format!(
                "prompt adapter received wrong domain: {}",
                change.domain
            )));
        }

        self.record_path(&change.record_id)?;
        let guard = PromptFileStore::for_library(&self.data_dir).lock().await;
        apply_prompt_file(
            change,
            merge_store,
            &guard,
            &change.record_id,
            &self.device_id,
        )
    }
}
