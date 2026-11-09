use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use async_trait::async_trait;
use base64::Engine;
use rand::RngCore;
use serde::{Deserialize, Serialize};

use crate::crypto::{self, SyncKey};
use crate::endpoint::BlobsApiStore;
use crate::hlc::HlcTimestamp;
use crate::merge::{is_text, three_way_merge};
use crate::merge_store::{live_cursor, tombstone_cursor, MergeStore};
use crate::opaque_replica_log::{
    put_encrypted_blob, read_encrypted_blob, read_encrypted_blob_with_peer_download, SyncChange,
};
use crate::sidecar::LwwDecision;
use crate::sync_blob_store::StatefulBlobDownloader;
use crate::sync_registry::{sync_cursor_record_id, ApplyOutcome, DomainSyncAdapter};
use crate::{SyncError, SyncResult};

pub const SKILL_BLOB_DOMAIN: &str = "skill_blob_manifest";
pub const SKILL_BLOB_KIND_UPSERT: &str = "upsert";
pub const SKILL_BLOB_KIND_DELETE: &str = "delete";
pub const SKILL_BLOB_KIND_SNAPSHOT: &str = "snapshot";
const INLINE_ENCRYPTED_FILE_LIMIT: usize = 256 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillBlobFileRef {
    pub rel_path: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hash_ref: String,
    pub size: u64,
    pub is_executable: bool,
    pub is_text: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline_ciphertext_b64: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillBlobManifest {
    #[serde(default = "default_skill_scope")]
    pub scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    pub skill_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_entry: Option<serde_json::Value>,
    #[serde(default)]
    pub files: Vec<SkillBlobFileRef>,
}

pub struct SkillBlobManifestSyncAdapter {
    data_dir: PathBuf,
    device_id: String,
    blob_context: Option<(BlobsApiStore, SyncKey)>,
    blob_download: Option<(StatefulBlobDownloader, Vec<iroh::EndpointId>)>,
}

impl SkillBlobManifestSyncAdapter {
    pub fn new(data_dir: PathBuf, device_id: impl Into<String>) -> Self {
        Self {
            data_dir,
            device_id: device_id.into(),
            blob_context: None,
            blob_download: None,
        }
    }

    pub fn with_blob_store(mut self, blobs_store: BlobsApiStore, sync_key: SyncKey) -> Self {
        self.blob_context = Some((blobs_store, sync_key));
        self
    }

    pub fn with_blob_download(
        mut self,
        downloader: StatefulBlobDownloader,
        providers: Vec<iroh::EndpointId>,
    ) -> Self {
        self.blob_download = Some((downloader, providers));
        self
    }

    pub fn delete_change(
        op_id: impl Into<String>,
        skill_name: impl Into<String>,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncChange {
        SyncChange::new(
            op_id,
            SKILL_BLOB_DOMAIN,
            skill_name,
            SKILL_BLOB_KIND_DELETE,
            device_id,
            hlc,
            serde_json::Value::Object(Default::default()),
        )
    }

    pub fn workspace_delete_change(
        op_id: impl Into<String>,
        agent_id: impl AsRef<str>,
        skill_name: impl AsRef<str>,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncChange {
        SyncChange::new(
            op_id,
            SKILL_BLOB_DOMAIN,
            workspace_record_id(agent_id.as_ref(), skill_name.as_ref()),
            SKILL_BLOB_KIND_DELETE,
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

    pub async fn snapshot_change_for_skill(
        &self,
        skill_name: &str,
        op_id: impl Into<String>,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncResult<SyncChange> {
        let manifest = self.build_pool_manifest(skill_name).await?;
        Ok(SyncChange::new(
            op_id,
            SKILL_BLOB_DOMAIN,
            skill_name,
            SKILL_BLOB_KIND_UPSERT,
            device_id,
            hlc,
            serde_json::to_value(manifest)?,
        ))
    }

    pub async fn snapshot_change_for_workspace_skill(
        &self,
        agent_id: &str,
        skill_name: &str,
        op_id: impl Into<String>,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncResult<SyncChange> {
        let manifest = self.build_workspace_manifest(agent_id, skill_name).await?;
        Ok(SyncChange::new(
            op_id,
            SKILL_BLOB_DOMAIN,
            workspace_record_id(agent_id, skill_name),
            SKILL_BLOB_KIND_UPSERT,
            device_id,
            hlc,
            serde_json::to_value(manifest)?,
        ))
    }

    fn skill_pool_dir(&self) -> PathBuf {
        self.data_dir.join("skill_pool")
    }

    fn workspaces_dir(&self) -> PathBuf {
        self.data_dir.join("workspaces")
    }

    fn pool_skill_dir(&self, skill_name: &str) -> SyncResult<PathBuf> {
        validate_skill_name(skill_name)?;
        Ok(self.skill_pool_dir().join(skill_name))
    }

    fn workspace_skill_dir(&self, agent_id: &str, skill_name: &str) -> SyncResult<PathBuf> {
        validate_agent_id(agent_id)?;
        validate_skill_name(skill_name)?;
        Ok(self
            .workspaces_dir()
            .join(agent_id)
            .join("skills")
            .join(skill_name))
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

    async fn build_pool_manifest(&self, skill_name: &str) -> SyncResult<SkillBlobManifest> {
        let skill_dir = self.pool_skill_dir(skill_name)?;
        self.build_manifest_from_dir(
            "pool",
            None,
            skill_name,
            &skill_dir,
            read_manifest_entry(&self.skill_pool_dir().join("skill.json"), skill_name),
        )
        .await
    }

    async fn build_workspace_manifest(
        &self,
        agent_id: &str,
        skill_name: &str,
    ) -> SyncResult<SkillBlobManifest> {
        let skill_dir = self.workspace_skill_dir(agent_id, skill_name)?;
        let workspace_dir = self.workspaces_dir().join(agent_id);
        self.build_manifest_from_dir(
            "workspace",
            Some(agent_id.to_string()),
            skill_name,
            &skill_dir,
            read_manifest_entry(&workspace_dir.join("skill.json"), skill_name),
        )
        .await
    }

    async fn build_manifest_from_dir(
        &self,
        scope: &str,
        agent_id: Option<String>,
        skill_name: &str,
        skill_dir: &Path,
        manifest_entry: Option<serde_json::Value>,
    ) -> SyncResult<SkillBlobManifest> {
        let (blobs_store, sync_key) = self.blob_context.as_ref().ok_or_else(|| {
            SyncError::Store("skill adapter missing encrypted blob context".to_string())
        })?;
        let mut paths = Vec::new();
        collect_files(skill_dir, &mut paths)?;
        paths.sort();

        let mut files = Vec::with_capacity(paths.len());
        for path in paths {
            let rel_path = normalize_relative_path(
                path.strip_prefix(skill_dir)
                    .map_err(|e| SyncError::Store(format!("skill strip_prefix: {e}")))?,
            )?;
            let content = std::fs::read(&path)?;
            let (hash_ref, inline_ciphertext_b64) = if content.len() <= INLINE_ENCRYPTED_FILE_LIMIT
            {
                (
                    String::new(),
                    Some(
                        base64::engine::general_purpose::STANDARD
                            .encode(crypto::symmetric_encrypt(&content, sync_key)?),
                    ),
                )
            } else {
                (
                    put_encrypted_blob(blobs_store, sync_key, &content).await?,
                    None,
                )
            };
            files.push(SkillBlobFileRef {
                rel_path,
                hash_ref,
                size: content.len() as u64,
                is_executable: is_executable(&path),
                is_text: is_text(&content),
                inline_ciphertext_b64,
            });
        }

        Ok(SkillBlobManifest {
            scope: scope.to_string(),
            agent_id,
            skill_name: skill_name.to_string(),
            manifest_entry,
            files,
        })
    }
}

#[async_trait]
impl DomainSyncAdapter for SkillBlobManifestSyncAdapter {
    fn domain(&self) -> &str {
        SKILL_BLOB_DOMAIN
    }

    async fn export_records(&self) -> SyncResult<Vec<SyncChange>> {
        let pool_dir = self.skill_pool_dir();
        let mut changes = Vec::new();

        if pool_dir.is_dir() {
            for entry in std::fs::read_dir(&pool_dir)? {
                let entry = entry?;
                if !entry.path().is_dir() {
                    continue;
                }
                let Some(skill_name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
                    continue;
                };
                validate_skill_name(&skill_name)?;
                let seed_dir = format!("skill_pool/{skill_name}");
                if crate::seed_filter::is_untouched_seed_entity(&self.data_dir, &seed_dir)? {
                    continue;
                }
                changes.push(
                    self.snapshot_change_for_skill(
                        &skill_name,
                        Self::new_op_id(),
                        self.device_id.clone(),
                        Self::export_hlc(&entry.path(), &self.device_id),
                    )
                    .await?,
                );
            }
        }

        let workspaces_dir = self.workspaces_dir();
        if workspaces_dir.is_dir() {
            for workspace in std::fs::read_dir(&workspaces_dir)? {
                let workspace = workspace?;
                if !workspace.path().is_dir() {
                    continue;
                }
                let Some(agent_id) = workspace.file_name().to_str().map(ToOwned::to_owned) else {
                    continue;
                };
                validate_agent_id(&agent_id)?;
                let skills_dir = workspace.path().join("skills");
                if !skills_dir.is_dir() {
                    continue;
                }
                for skill in std::fs::read_dir(&skills_dir)? {
                    let skill = skill?;
                    if !skill.path().is_dir() {
                        continue;
                    }
                    let Some(skill_name) = skill.file_name().to_str().map(ToOwned::to_owned) else {
                        continue;
                    };
                    validate_skill_name(&skill_name)?;
                    let seed_dir = format!("workspaces/{agent_id}/skills/{skill_name}");
                    if crate::seed_filter::is_untouched_seed_entity(&self.data_dir, &seed_dir)? {
                        continue;
                    }
                    changes.push(
                        self.snapshot_change_for_workspace_skill(
                            &agent_id,
                            &skill_name,
                            Self::new_op_id(),
                            self.device_id.clone(),
                            Self::export_hlc(&skill.path(), &self.device_id),
                        )
                        .await?,
                    );
                }
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
        if change.domain != SKILL_BLOB_DOMAIN {
            return Err(SyncError::Store(format!(
                "skill adapter received wrong domain: {}",
                change.domain
            )));
        }
        let target = SkillRecordTarget::parse(&change.record_id)?;

        let cursor_id = sync_cursor_record_id(SKILL_BLOB_DOMAIN, &change.record_id);
        let is_delete = change.kind == SKILL_BLOB_KIND_DELETE;
        let decision = merge_store.resolve_lww(&cursor_id, change.hlc, is_delete)?;
        if decision == LwwDecision::Skip {
            return Ok(ApplyOutcome::Skipped);
        }

        let skill_dir = match &target {
            SkillRecordTarget::Pool { skill_name } => self.pool_skill_dir(skill_name)?,
            SkillRecordTarget::Workspace {
                agent_id,
                skill_name,
            } => self.workspace_skill_dir(agent_id, skill_name)?,
        };
        if is_delete {
            if skill_dir.exists() {
                std::fs::remove_dir_all(&skill_dir)?;
            }
            self.remove_manifest_entry(&target)?;
            return Ok(ApplyOutcome::Applied {
                cursor: tombstone_cursor(change.hlc, None),
            });
        }
        if change.kind != SKILL_BLOB_KIND_UPSERT && change.kind != SKILL_BLOB_KIND_SNAPSHOT {
            return Err(SyncError::Store(format!(
                "unsupported skill change kind: {}",
                change.kind
            )));
        }

        let manifest: SkillBlobManifest = serde_json::from_value(change.payload.clone())?;
        if manifest.skill_name != target.skill_name() {
            return Err(SyncError::Store(format!(
                "skill manifest name mismatch: {} != {}",
                manifest.skill_name,
                target.skill_name()
            )));
        }
        if manifest.scope != target.scope() {
            return Err(SyncError::Store(format!(
                "skill manifest scope mismatch: {} != {}",
                manifest.scope,
                target.scope()
            )));
        }
        if target.agent_id() != manifest.agent_id.as_deref() {
            return Err(SyncError::Store(
                "skill manifest agent id mismatch".to_string(),
            ));
        }
        let (blobs_store, sync_key) = self.blob_context.as_ref().ok_or_else(|| {
            SyncError::Store("skill adapter missing encrypted blob context".to_string())
        })?;
        let mut resolved_files = Vec::with_capacity(manifest.files.len());

        for file in &manifest.files {
            validate_relative_path(&file.rel_path)?;
            let content = if let Some(content) = read_inline_file(file, sync_key)? {
                content
            } else {
                if file.hash_ref.is_empty() {
                    return Err(SyncError::Store(format!(
                        "skill file {} has neither inline content nor blob hash",
                        file.rel_path
                    )));
                }
                let content = if let Some((downloader, providers)) = &self.blob_download {
                    read_encrypted_blob_with_peer_download(
                        blobs_store,
                        sync_key,
                        &file.hash_ref,
                        downloader,
                        providers.clone(),
                    )
                    .await?
                } else {
                    read_encrypted_blob(blobs_store, sync_key, &file.hash_ref).await?
                };
                content.ok_or_else(|| {
                    SyncError::Store(format!("skill blob missing: {}", file.hash_ref))
                })?
            };
            resolved_files.push((
                file.rel_path.clone(),
                file.is_text,
                file.is_executable,
                content,
            ));
        }

        std::fs::create_dir_all(&skill_dir)?;
        let mut additional_cursors = Vec::new();
        for (rel_path, is_text_file, is_executable_file, content) in resolved_files {
            let file_path = skill_dir.join(&rel_path);
            if let Some(parent) = file_path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            if is_text_file {
                additional_cursors.push(apply_text_file(
                    &file_path,
                    &content,
                    change,
                    &rel_path,
                    merge_store,
                )?);
            } else {
                std::fs::write(&file_path, &content)?;
                let file_cursor_id = skill_file_cursor_id(&change.record_id, &rel_path);
                additional_cursors.push((file_cursor_id, live_cursor(change.hlc, None)));
            }
            set_executable(&file_path, is_executable_file)?;
        }
        self.write_manifest_entry(&target, manifest.manifest_entry.clone())?;

        Ok(ApplyOutcome::AppliedWithCursors {
            cursor: live_cursor(change.hlc, None),
            additional_cursors,
        })
    }
}

impl SkillBlobManifestSyncAdapter {
    fn manifest_path_for_target(&self, target: &SkillRecordTarget) -> PathBuf {
        match target {
            SkillRecordTarget::Pool { .. } => self.skill_pool_dir().join("skill.json"),
            SkillRecordTarget::Workspace { agent_id, .. } => {
                self.workspaces_dir().join(agent_id).join("skill.json")
            }
        }
    }

    fn write_manifest_entry(
        &self,
        target: &SkillRecordTarget,
        entry: Option<serde_json::Value>,
    ) -> SyncResult<()> {
        let Some(entry) = entry else {
            return Ok(());
        };
        let path = self.manifest_path_for_target(target);
        let mut payload = read_json_object(&path);
        let skills = payload
            .entry("skills".to_string())
            .or_insert_with(|| serde_json::Value::Object(Default::default()));
        let skills = skills.as_object_mut().ok_or_else(|| {
            SyncError::Store("skill manifest skills must be an object".to_string())
        })?;
        skills.insert(target.skill_name().to_string(), entry);
        rua_config::atomic_write_json(&path, &serde_json::Value::Object(payload))?;
        Ok(())
    }

    fn remove_manifest_entry(&self, target: &SkillRecordTarget) -> SyncResult<()> {
        let path = self.manifest_path_for_target(target);
        let mut payload = read_json_object(&path);
        if let Some(skills) = payload
            .get_mut("skills")
            .and_then(|value| value.as_object_mut())
        {
            skills.remove(target.skill_name());
        }
        rua_config::atomic_write_json(&path, &serde_json::Value::Object(payload))?;
        Ok(())
    }
}

enum SkillRecordTarget {
    Pool {
        skill_name: String,
    },
    Workspace {
        agent_id: String,
        skill_name: String,
    },
}

impl SkillRecordTarget {
    fn parse(record_id: &str) -> SyncResult<Self> {
        if let Some(rest) = record_id.strip_prefix("workspace/") {
            let mut parts = rest.split('/');
            let agent_id = parts
                .next()
                .ok_or_else(|| SyncError::Store("missing workspace skill agent id".to_string()))?;
            let skill_name = parts
                .next()
                .ok_or_else(|| SyncError::Store("missing workspace skill name".to_string()))?;
            if parts.next().is_some() {
                return Err(SyncError::Store(format!(
                    "invalid workspace skill record id: {record_id}"
                )));
            }
            validate_agent_id(agent_id)?;
            validate_skill_name(skill_name)?;
            Ok(Self::Workspace {
                agent_id: agent_id.to_string(),
                skill_name: skill_name.to_string(),
            })
        } else {
            validate_skill_name(record_id)?;
            Ok(Self::Pool {
                skill_name: record_id.to_string(),
            })
        }
    }

    fn scope(&self) -> &str {
        match self {
            Self::Pool { .. } => "pool",
            Self::Workspace { .. } => "workspace",
        }
    }

    fn agent_id(&self) -> Option<&str> {
        match self {
            Self::Pool { .. } => None,
            Self::Workspace { agent_id, .. } => Some(agent_id),
        }
    }

    fn skill_name(&self) -> &str {
        match self {
            Self::Pool { skill_name } | Self::Workspace { skill_name, .. } => skill_name,
        }
    }
}

fn apply_text_file(
    file_path: &Path,
    remote_bytes: &[u8],
    change: &SyncChange,
    rel_path: &str,
    merge_store: &MergeStore,
) -> SyncResult<(String, crate::sidecar::SidecarEntry)> {
    let remote = String::from_utf8_lossy(remote_bytes);
    let local = std::fs::read_to_string(file_path).unwrap_or_default();
    let file_cursor_id = skill_file_cursor_id(&change.record_id, rel_path);
    let current_cursor = merge_store.get_record_cursor(&file_cursor_id)?;
    let base = current_cursor
        .as_ref()
        .and_then(|entry| entry.base_blob_hash.as_ref())
        .and_then(|hash| merge_store.get_base_snapshot(hash).ok().flatten())
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_default();
    let local_hlc = current_cursor
        .map(|entry| entry.hlc)
        .unwrap_or(HlcTimestamp {
            wall_ms: 0,
            counter: 0,
            device_id: [0; 8],
        });

    let merged = three_way_merge(&base, &local, &remote, local_hlc, change.hlc).merged;
    std::fs::write(file_path, &merged)?;
    let base_hash = merge_store.put_base_snapshot(merged.as_bytes())?;
    Ok((file_cursor_id, live_cursor(change.hlc, Some(base_hash))))
}

fn read_inline_file(file: &SkillBlobFileRef, sync_key: &SyncKey) -> SyncResult<Option<Vec<u8>>> {
    let Some(ciphertext_b64) = file.inline_ciphertext_b64.as_deref() else {
        return Ok(None);
    };
    let ciphertext = base64::engine::general_purpose::STANDARD
        .decode(ciphertext_b64)
        .map_err(|e| SyncError::Store(format!("invalid inline skill ciphertext: {e}")))?;
    let plaintext = crypto::symmetric_decrypt(&ciphertext, sync_key)?;
    if plaintext.len() as u64 != file.size {
        return Err(SyncError::Store(format!(
            "inline skill file size mismatch for {}",
            file.rel_path
        )));
    }
    Ok(Some(plaintext))
}

fn skill_file_cursor_id(skill_name: &str, rel_path: &str) -> String {
    sync_cursor_record_id(SKILL_BLOB_DOMAIN, &format!("{skill_name}:{rel_path}"))
}

pub fn workspace_record_id(agent_id: &str, skill_name: &str) -> String {
    format!("workspace/{agent_id}/{skill_name}")
}

fn default_skill_scope() -> String {
    "pool".to_string()
}

fn validate_skill_name(skill_name: &str) -> SyncResult<()> {
    if skill_name.is_empty()
        || skill_name.contains("..")
        || skill_name.contains('/')
        || skill_name.contains('\\')
    {
        return Err(SyncError::Store(format!(
            "invalid skill name: {skill_name}"
        )));
    }
    Ok(())
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

fn read_manifest_entry(path: &Path, skill_name: &str) -> Option<serde_json::Value> {
    read_json_object(path)
        .get("skills")
        .and_then(|value| value.as_object())
        .and_then(|skills| skills.get(skill_name))
        .cloned()
}

fn read_json_object(path: &Path) -> serde_json::Map<String, serde_json::Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_else(|| {
            let mut root = serde_json::Map::new();
            root.insert(
                "skills".to_string(),
                serde_json::Value::Object(Default::default()),
            );
            root
        })
}

fn validate_relative_path(rel_path: &str) -> SyncResult<()> {
    if rel_path.is_empty() {
        return Err(SyncError::Store(
            "skill file path must not be empty".to_string(),
        ));
    }
    let path = Path::new(rel_path);
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(SyncError::Store(format!(
            "invalid skill file path: {rel_path}"
        )));
    }
    Ok(())
}

fn normalize_relative_path(path: &Path) -> SyncResult<String> {
    let rel_path = path.to_string_lossy().replace('\\', "/");
    validate_relative_path(&rel_path)?;
    Ok(rel_path)
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> SyncResult<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, out)?;
        } else if path.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|meta| meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}

fn set_executable(path: &Path, executable: bool) -> SyncResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if executable {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (path, executable);
    }
    Ok(())
}
