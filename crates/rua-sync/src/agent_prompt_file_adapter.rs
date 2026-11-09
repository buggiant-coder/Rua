use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use async_trait::async_trait;
use rand::RngCore;
use rua_prompts::agent_files::{AgentPromptFileChange, AgentPromptFileStore, PromptFileVersion};

use crate::hlc::HlcTimestamp;
use crate::merge_store::MergeStore;
use crate::opaque_replica_log::SyncChange;
use crate::prompt_file_apply::{committed_hlc, live_payload};
use crate::sync_registry::{ApplyOutcome, DomainSyncAdapter};
use crate::{SyncError, SyncResult};

pub const AGENT_PROMPT_FILE_DOMAIN: &str = "agent_prompt_file";
pub const AGENT_PROMPT_FILE_KIND_UPSERT: &str = "upsert";
pub const AGENT_PROMPT_FILE_KIND_DELETE: &str = "delete";
pub const AGENT_PROMPT_FILE_KIND_SNAPSHOT: &str = "snapshot";
const PROMPTS_DIR: &str = "prompts";

enum WorkspaceMarkdownLocation<'a> {
    Root { filename: &'a str },
    Subdir { subdir: &'a str, filename: &'a str },
}

pub struct AgentPromptFileSyncAdapter {
    data_dir: PathBuf,
    device_id: String,
}

impl AgentPromptFileSyncAdapter {
    pub fn new(data_dir: PathBuf, device_id: impl Into<String>) -> Self {
        Self {
            data_dir,
            device_id: device_id.into(),
        }
    }

    pub fn root_record_id(agent_id: impl AsRef<str>, filename: impl AsRef<str>) -> String {
        format!("{}/root/{}", agent_id.as_ref(), filename.as_ref())
    }

    fn subdir_record_id(
        agent_id: impl AsRef<str>,
        subdir: impl AsRef<str>,
        filename: impl AsRef<str>,
    ) -> String {
        format!(
            "{}/subdir/{}/{}",
            agent_id.as_ref(),
            subdir.as_ref(),
            filename.as_ref()
        )
    }

    pub fn root_upsert_change(
        op_id: impl Into<String>,
        agent_id: impl AsRef<str>,
        filename: impl AsRef<str>,
        content: impl Into<String>,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncChange {
        SyncChange::new(
            op_id,
            AGENT_PROMPT_FILE_DOMAIN,
            Self::root_record_id(agent_id, filename),
            AGENT_PROMPT_FILE_KIND_UPSERT,
            device_id,
            hlc,
            serde_json::json!({ "content": content.into() }),
        )
    }

    pub fn root_delete_change(
        op_id: impl Into<String>,
        agent_id: impl AsRef<str>,
        filename: impl AsRef<str>,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncChange {
        SyncChange::new(
            op_id,
            AGENT_PROMPT_FILE_DOMAIN,
            Self::root_record_id(agent_id, filename),
            AGENT_PROMPT_FILE_KIND_DELETE,
            device_id,
            hlc,
            serde_json::Value::Object(Default::default()),
        )
    }

    pub fn subdir_upsert_change(
        op_id: impl Into<String>,
        agent_id: impl AsRef<str>,
        subdir: impl AsRef<str>,
        filename: impl AsRef<str>,
        content: impl Into<String>,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncChange {
        SyncChange::new(
            op_id,
            AGENT_PROMPT_FILE_DOMAIN,
            Self::subdir_record_id(agent_id, subdir, filename),
            AGENT_PROMPT_FILE_KIND_UPSERT,
            device_id,
            hlc,
            serde_json::json!({ "content": content.into() }),
        )
    }

    pub fn subdir_delete_change(
        op_id: impl Into<String>,
        agent_id: impl AsRef<str>,
        subdir: impl AsRef<str>,
        filename: impl AsRef<str>,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncChange {
        SyncChange::new(
            op_id,
            AGENT_PROMPT_FILE_DOMAIN,
            Self::subdir_record_id(agent_id, subdir, filename),
            AGENT_PROMPT_FILE_KIND_DELETE,
            device_id,
            hlc,
            serde_json::Value::Object(Default::default()),
        )
    }

    pub fn committed_change(
        agent_id: &str,
        change: AgentPromptFileChange,
        device_id: &str,
    ) -> SyncChange {
        let record_id = match change.path.split_once('/') {
            Some((subdir, filename)) => Self::subdir_record_id(agent_id, subdir, filename),
            None => Self::root_record_id(agent_id, &change.path),
        };
        let kind = if change.content.is_some() {
            AGENT_PROMPT_FILE_KIND_UPSERT
        } else {
            AGENT_PROMPT_FILE_KIND_DELETE
        };
        let payload = change
            .content
            .map(|content| live_payload(content, change.deleted_before, device_id))
            .unwrap_or_else(|| serde_json::json!({}));
        SyncChange::new(
            Self::new_op_id(),
            AGENT_PROMPT_FILE_DOMAIN,
            record_id,
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

    fn workspaces_dir(&self) -> PathBuf {
        self.data_dir.join("workspaces")
    }

    fn record_path(&self, record_id: &str) -> SyncResult<(String, PathBuf)> {
        let (agent_id, location) = parse_workspace_record_id(record_id)?;
        let workspace = self.workspaces_dir().join(agent_id);
        let path = match location {
            WorkspaceMarkdownLocation::Root { filename } => {
                workspace.join(PROMPTS_DIR).join(filename)
            }
            WorkspaceMarkdownLocation::Subdir { subdir, filename } => {
                workspace.join(PROMPTS_DIR).join(subdir).join(filename)
            }
        };
        Ok((agent_id.to_string(), path))
    }

    fn export_hlc(path: &Path, device_id: &str) -> HlcTimestamp {
        let wall_ms = path
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0);
        committed_hlc(PromptFileVersion::local(wall_ms), device_id)
    }
}

#[async_trait]
impl DomainSyncAdapter for AgentPromptFileSyncAdapter {
    fn domain(&self) -> &str {
        AGENT_PROMPT_FILE_DOMAIN
    }

    async fn export_records(&self) -> SyncResult<Vec<SyncChange>> {
        let workspaces = self.workspaces_dir();
        if !workspaces.is_dir() {
            return Ok(Vec::new());
        }

        let mut changes = Vec::new();
        for workspace in std::fs::read_dir(&workspaces)? {
            let workspace = workspace?;
            if !workspace.path().is_dir() {
                continue;
            }
            let Some(agent_id) = workspace.file_name().to_str().map(ToOwned::to_owned) else {
                continue;
            };
            if parse_path_part(&agent_id).is_err() {
                continue;
            }
            let guard = AgentPromptFileStore::new(workspace.path()).lock().await;
            let prompt_dir = workspace.path().join(PROMPTS_DIR);
            let mut subdirs = Vec::new();
            if prompt_dir.is_dir() {
                for entry in std::fs::read_dir(&prompt_dir)? {
                    let entry = entry?;
                    let path = entry.path();
                    let Some(name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
                        continue;
                    };
                    if path.is_file() && name.ends_with(".md") {
                        if parse_path_part(&name).is_err() {
                            continue;
                        }
                        let seed_path = format!("workspaces/{agent_id}/prompts/{name}");
                        if guard.deletion_fence(&name)?.is_none()
                            && crate::seed_filter::is_untouched_seed_file(
                                &self.data_dir,
                                &seed_path,
                            )?
                        {
                            continue;
                        }
                        let content = std::fs::read_to_string(&path)?;
                        changes.push(SyncChange::new(
                            Self::new_op_id(),
                            AGENT_PROMPT_FILE_DOMAIN,
                            Self::root_record_id(&agent_id, &name),
                            AGENT_PROMPT_FILE_KIND_SNAPSHOT,
                            self.device_id.clone(),
                            guard
                                .local_revision(&name)?
                                .map(|(version, _)| committed_hlc(version, &self.device_id))
                                .unwrap_or_else(|| Self::export_hlc(&path, &self.device_id)),
                            live_payload(content, guard.deletion_fence(&name)?, &self.device_id),
                        ));
                    } else if path.is_dir() {
                        subdirs.push((name, path));
                    }
                }
            }

            let mut seen_subdirs = HashSet::new();
            for (subdir, subdir_path) in subdirs {
                if subdir.eq_ignore_ascii_case("MEMORY") || parse_path_part(&subdir).is_err() {
                    continue;
                }
                if !seen_subdirs.insert(subdir.to_ascii_uppercase()) {
                    continue;
                }
                for file in std::fs::read_dir(&subdir_path)? {
                    let file = file?;
                    let path = file.path();
                    if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
                        continue;
                    }
                    let Some(filename) = file.file_name().to_str().map(ToOwned::to_owned) else {
                        continue;
                    };
                    if parse_path_part(&filename).is_err() {
                        continue;
                    }
                    let seed_path = format!("workspaces/{agent_id}/prompts/{subdir}/{filename}");
                    if guard
                        .deletion_fence(&format!("{subdir}/{filename}"))?
                        .is_none()
                        && crate::seed_filter::is_untouched_seed_file(&self.data_dir, &seed_path)?
                    {
                        continue;
                    }
                    let content = std::fs::read_to_string(&path)?;
                    changes.push(SyncChange::new(
                        Self::new_op_id(),
                        AGENT_PROMPT_FILE_DOMAIN,
                        Self::subdir_record_id(&agent_id, &subdir, &filename),
                        AGENT_PROMPT_FILE_KIND_SNAPSHOT,
                        self.device_id.clone(),
                        guard
                            .local_revision(&format!("{subdir}/{filename}"))?
                            .map(|(version, _)| committed_hlc(version, &self.device_id))
                            .unwrap_or_else(|| Self::export_hlc(&path, &self.device_id)),
                        live_payload(
                            content,
                            guard.deletion_fence(&format!("{subdir}/{filename}"))?,
                            &self.device_id,
                        ),
                    ));
                }
            }
            for deletion in guard.deletions()? {
                if deletion
                    .path
                    .split('/')
                    .next()
                    .is_some_and(|dir| dir.eq_ignore_ascii_case("MEMORY"))
                {
                    continue;
                }
                changes.push(Self::committed_change(&agent_id, deletion, &self.device_id));
            }
        }
        changes.sort_by(|a, b| a.record_id.cmp(&b.record_id));
        Ok(changes)
    }

    async fn apply_change(
        &self,
        change: &SyncChange,
        merge_store: &MergeStore,
    ) -> SyncResult<ApplyOutcome> {
        if change.domain != AGENT_PROMPT_FILE_DOMAIN {
            return Err(SyncError::Store(format!(
                "agent prompt file adapter received wrong domain: {}",
                change.domain
            )));
        }

        let (agent_id, path) = self.record_path(&change.record_id)?;
        let workspace = self.workspaces_dir().join(&agent_id);
        let relative = path
            .strip_prefix(workspace.join(PROMPTS_DIR))
            .map_err(|e| SyncError::Store(e.to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        let guard = AgentPromptFileStore::new(workspace).lock().await;
        crate::prompt_file_apply::apply_prompt_file(
            change,
            merge_store,
            &guard,
            &relative,
            &self.device_id,
        )
    }
}

fn parse_workspace_record_id(record_id: &str) -> SyncResult<(&str, WorkspaceMarkdownLocation<'_>)> {
    let parts = record_id.split('/').collect::<Vec<_>>();
    match parts.as_slice() {
        [agent_id, "root", filename] => {
            parse_path_part(agent_id)?;
            parse_path_part(filename)?;
            validate_md_filename(filename)?;
            Ok((agent_id, WorkspaceMarkdownLocation::Root { filename }))
        }
        [agent_id, "subdir", subdir, filename] => {
            parse_path_part(agent_id)?;
            parse_path_part(subdir)?;
            parse_path_part(filename)?;
            validate_md_filename(filename)?;
            Ok((
                agent_id,
                WorkspaceMarkdownLocation::Subdir { subdir, filename },
            ))
        }
        _ => Err(SyncError::Store(format!(
            "invalid workspace markdown record id: {record_id}"
        ))),
    }
}

fn validate_md_filename(filename: &str) -> SyncResult<()> {
    if !filename.ends_with(".md") {
        return Err(SyncError::Store(format!(
            "memory filename must end with .md: {filename}"
        )));
    }
    Ok(())
}

fn parse_path_part(part: &str) -> SyncResult<()> {
    if part.is_empty() {
        return Err(SyncError::Store(
            "path segment must not be empty".to_string(),
        ));
    }
    let path = Path::new(part);
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) || part.contains('/')
        || part.contains('\\')
    {
        return Err(SyncError::Store(format!("invalid path segment: {part}")));
    }
    Ok(())
}
