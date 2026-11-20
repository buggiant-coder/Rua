use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::Path;

use chrono::{DateTime, Utc};
use rua_core::ChatAddress;
use rua_session::{chat_index::ChatIndexRecord, WorkspaceSessionStore};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct StorageChatOwner {
    pub kind: String,
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatStorageEntry {
    pub address: ChatAddress,
    pub owner: StorageChatOwner,
    pub name: String,
    pub pinned: bool,
    pub updated_at: DateTime<Utc>,
    pub session_bytes: u64,
    pub task_bytes: u64,
    pub tool_log_bytes: u64,
    pub media_bytes: u64,
    pub attributed_bytes: u64,
    pub reclaimable_bytes: u64,
    pub total_bytes: u64,
    pub deletable: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ChatReferences {
    pub tool_use_ids: BTreeSet<String>,
    pub media_ids: BTreeSet<String>,
}

#[derive(Debug)]
pub struct ScopeStorageSnapshot {
    pub entries: Vec<ChatStorageEntry>,
    pub references: HashMap<String, ChatReferences>,
    pub media_references: HashMap<String, BTreeSet<String>>,
}

#[derive(Debug, Default)]
pub struct AttachmentCleanup {
    pub freed_bytes: u64,
    pub shared_media_preserved: BTreeMap<String, Vec<String>>,
}

#[derive(Debug)]
pub struct PreparedChatDeletion {
    pub snapshot: ScopeStorageSnapshot,
    pub cleanup: AttachmentCleanup,
}

#[derive(Debug)]
pub enum ChatDeletionPreparationError {
    NotFound(String),
    Pinned(String),
    Failed(String),
}

impl std::fmt::Display for ChatDeletionPreparationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(detail) | Self::Pinned(detail) | Self::Failed(detail) => {
                formatter.write_str(detail)
            }
        }
    }
}

pub async fn prepare_chat_deletion(
    state: &crate::state::AppState,
    scope_root: &Path,
    deleting_chat_ids: &BTreeSet<String>,
) -> Result<PreparedChatDeletion, ChatDeletionPreparationError> {
    let snapshot = scan_scope_for_deletion(scope_root, deleting_chat_ids)
        .await
        .map_err(ChatDeletionPreparationError::Failed)?;
    for chat_id in deleting_chat_ids {
        let entry = snapshot
            .entries
            .iter()
            .find(|entry| entry.address.chat_id().as_str() == chat_id)
            .ok_or_else(|| {
                ChatDeletionPreparationError::NotFound(format!("Chat not found: {chat_id}"))
            })?;
        if entry.pinned {
            return Err(ChatDeletionPreparationError::Pinned(format!(
                "Pinned Chat '{chat_id}' cannot be deleted; Unpin it first"
            )));
        }
    }

    let (removable_media, _) = removable_media(&snapshot, deleting_chat_ids);
    for media_id in &removable_media {
        ensure_delete_publication(
            crate::sync_publication::publish_sync_fact(
                state,
                crate::sync_publication::SyncPublicationFact::MediaDeleted {
                    media_id: media_id.clone(),
                },
            )
            .await,
            "Media",
        )?;
    }
    for chat_id in deleting_chat_ids {
        ensure_delete_publication(
            crate::sync_publication::publish_sync_fact(
                state,
                crate::sync_publication::SyncPublicationFact::ChatDeleted {
                    scope_root: scope_root.to_path_buf(),
                    address: ChatAddress::new(chat_id.clone()),
                },
            )
            .await,
            "Chat",
        )?;
    }
    let cleanup =
        remove_local_attachments(scope_root, &snapshot, deleting_chat_ids, &removable_media)
            .map_err(ChatDeletionPreparationError::Failed)?;
    Ok(PreparedChatDeletion { snapshot, cleanup })
}

pub async fn scan_scope(scope_root: &Path) -> Result<ScopeStorageSnapshot, String> {
    scan_scope_selected(scope_root, None).await
}

/// Measures only deleted Chats; other histories are read only to protect shared media.
pub async fn scan_scope_for_deletion(
    scope_root: &Path,
    deleting_chat_ids: &BTreeSet<String>,
) -> Result<ScopeStorageSnapshot, String> {
    scan_scope_selected(scope_root, Some(deleting_chat_ids)).await
}

async fn scan_scope_selected(
    scope_root: &Path,
    deleting_chat_ids: Option<&BTreeSet<String>>,
) -> Result<ScopeStorageSnapshot, String> {
    let (records, other_records): (Vec<_>, Vec<_>) =
        crate::services::chat_index::list_all(scope_root)
            .await?
            .into_iter()
            .partition(|record| deleting_chat_ids.is_none_or(|ids| ids.contains(&record.id)));
    let store = WorkspaceSessionStore::new(scope_root);
    let mut entries = Vec::with_capacity(records.len());
    let mut references = HashMap::new();
    let mut media_references: HashMap<String, BTreeSet<String>> = HashMap::new();

    let mut measured = Vec::with_capacity(records.len());
    for record in records {
        let address = ChatAddress::new(record.id.clone());
        let refs = collect_chat_references(&store, &record).await?;
        for media_id in &refs.media_ids {
            media_references
                .entry(media_id.clone())
                .or_default()
                .insert(record.id.clone());
        }
        let sizes = measure_chat(scope_root, &store, &record, &refs)?;
        let header = store
            .load_chat_header(&record.id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("Chat header missing: {}", record.id))?;
        let (kind, id) = match header.project_id {
            Some(project_id) => ("project", project_id),
            None => ("agent", header.agent_id),
        };
        let owner = StorageChatOwner {
            kind: kind.into(),
            name: id.clone(),
            id,
        };
        measured.push((record, address, refs, sizes, owner));
    }

    if !media_references.is_empty() {
        for record in other_records {
            let refs = collect_chat_references(&store, &record).await?;
            for media_id in refs.media_ids {
                if let Some(owners) = media_references.get_mut(&media_id) {
                    owners.insert(record.id.clone());
                }
            }
        }
    }

    if let Some(rua_root) = scope_root.parent() {
        let deleted = rua_session::chat_deletion::ChatDeleteIntentStore::from_rua_root(rua_root);
        for (raw, owners) in &mut media_references {
            let Ok(id) = uuid::Uuid::parse_str(raw) else {
                continue;
            };
            if let Some(manifest) =
                rua_session::chat_media::ChatMediaManifest::load(&scope_root.join("media"), id)
                    .map_err(|e| e.to_string())?
            {
                for owner in manifest.chat_ids {
                    if deleted.load(&ChatAddress::new(owner.clone()))?.is_none() {
                        owners.insert(owner);
                    }
                }
            }
        }
    }

    for (record, address, refs, sizes, owner) in measured {
        let attributed_bytes = sizes.total_bytes();
        let shared_media_bytes = refs
            .media_ids
            .iter()
            .filter(|media_id| {
                media_references
                    .get(*media_id)
                    .is_some_and(|chat_ids| chat_ids.len() > 1)
            })
            .try_fold(0_u64, |total, media_id| {
                Ok::<_, String>(total.saturating_add(media_size(scope_root, media_id)?))
            })?;
        let reclaimable_bytes = attributed_bytes.saturating_sub(shared_media_bytes);
        entries.push(ChatStorageEntry {
            address,
            owner,
            name: record.name.clone(),
            pinned: record.pinned,
            updated_at: record.updated_at,
            session_bytes: sizes.session_bytes,
            task_bytes: sizes.task_bytes,
            tool_log_bytes: sizes.tool_log_bytes,
            media_bytes: sizes.media_bytes,
            attributed_bytes,
            reclaimable_bytes,
            total_bytes: attributed_bytes,
            deletable: !record.pinned,
        });
        references.insert(record.id, refs);
    }

    Ok(ScopeStorageSnapshot {
        entries,
        references,
        media_references,
    })
}

pub fn removable_media(
    snapshot: &ScopeStorageSnapshot,
    deleting_chat_ids: &BTreeSet<String>,
) -> (BTreeSet<String>, BTreeMap<String, Vec<String>>) {
    let mut removable = BTreeSet::new();
    let mut preserved = BTreeMap::new();
    for chat_id in deleting_chat_ids {
        let Some(references) = snapshot.references.get(chat_id) else {
            continue;
        };
        for media_id in &references.media_ids {
            let shared = snapshot
                .media_references
                .get(media_id)
                .is_some_and(|chat_ids| chat_ids.iter().any(|id| !deleting_chat_ids.contains(id)));
            if shared {
                preserved
                    .entry(chat_id.clone())
                    .or_insert_with(Vec::new)
                    .push(media_id.clone());
            } else {
                removable.insert(media_id.clone());
            }
        }
    }
    (removable, preserved)
}

pub fn remove_local_attachments(
    scope_root: &Path,
    snapshot: &ScopeStorageSnapshot,
    deleting_chat_ids: &BTreeSet<String>,
    removable_media_ids: &BTreeSet<String>,
) -> Result<AttachmentCleanup, String> {
    let mut cleanup = AttachmentCleanup {
        shared_media_preserved: removable_media(snapshot, deleting_chat_ids).1,
        ..AttachmentCleanup::default()
    };
    let tool_log_dir = scope_root.join("tool_call_logs");
    for chat_id in deleting_chat_ids {
        let Some(references) = snapshot.references.get(chat_id) else {
            continue;
        };
        for tool_use_id in &references.tool_use_ids {
            let key = rua_tool::result_store::recall_key(tool_use_id);
            for extension in ["json", "output"] {
                cleanup.freed_bytes +=
                    remove_regular_file(&tool_log_dir.join(format!("{key}.{extension}")))?;
            }
        }
    }

    let media_dir = scope_root.join("media");
    for media_id in removable_media_ids {
        for path in [
            media_dir.join(media_id),
            media_dir.join(format!(".{media_id}.media.json")),
        ] {
            cleanup.freed_bytes += remove_regular_file(&path)?;
        }
    }
    Ok(cleanup)
}

pub fn planned_freed_bytes(
    scope_root: &Path,
    snapshot: &ScopeStorageSnapshot,
    deleting_chat_ids: &BTreeSet<String>,
    removable_media_ids: &BTreeSet<String>,
) -> Result<u64, String> {
    let mut total = snapshot
        .entries
        .iter()
        .filter(|entry| deleting_chat_ids.contains(entry.address.chat_id().as_str()))
        .map(|entry| entry.session_bytes.saturating_add(entry.task_bytes))
        .fold(0_u64, u64::saturating_add);
    let tool_ids = deleting_chat_ids
        .iter()
        .filter_map(|chat_id| snapshot.references.get(chat_id))
        .flat_map(|references| references.tool_use_ids.iter())
        .collect::<BTreeSet<_>>();
    for tool_use_id in tool_ids {
        let key = rua_tool::result_store::recall_key(tool_use_id);
        total = total
            .saturating_add(file_size(
                &scope_root
                    .join("tool_call_logs")
                    .join(format!("{key}.json")),
            )?)
            .saturating_add(file_size(
                &scope_root
                    .join("tool_call_logs")
                    .join(format!("{key}.output")),
            )?);
    }
    for media_id in removable_media_ids {
        total = total.saturating_add(media_size(scope_root, media_id)?);
    }
    Ok(total)
}

#[derive(Debug, Default)]
struct ChatSizes {
    session_bytes: u64,
    task_bytes: u64,
    tool_log_bytes: u64,
    media_bytes: u64,
}

impl ChatSizes {
    fn total_bytes(&self) -> u64 {
        self.session_bytes
            .saturating_add(self.task_bytes)
            .saturating_add(self.tool_log_bytes)
            .saturating_add(self.media_bytes)
    }
}

fn measure_chat(
    scope_root: &Path,
    store: &WorkspaceSessionStore,
    record: &ChatIndexRecord,
    references: &ChatReferences,
) -> Result<ChatSizes, String> {
    let chat_dir = store.chat_dir(&record.id);
    let task_dir = chat_dir.join("tasks");
    let task_bytes = regular_files_size(&task_dir)?;
    let session_bytes = regular_files_size_excluding(&chat_dir, Some(&task_dir))?;

    let tool_log_dir = scope_root.join("tool_call_logs");
    let mut tool_log_bytes = 0_u64;
    for tool_use_id in &references.tool_use_ids {
        let key = rua_tool::result_store::recall_key(tool_use_id);
        tool_log_bytes = tool_log_bytes
            .saturating_add(file_size(&tool_log_dir.join(format!("{key}.json")))?)
            .saturating_add(file_size(&tool_log_dir.join(format!("{key}.output")))?);
    }

    let mut media_bytes = 0_u64;
    for media_id in &references.media_ids {
        media_bytes = media_bytes.saturating_add(media_size(scope_root, media_id)?);
    }

    Ok(ChatSizes {
        session_bytes,
        task_bytes,
        tool_log_bytes,
        media_bytes,
    })
}

fn media_size(scope_root: &Path, media_id: &str) -> Result<u64, String> {
    let media_dir = scope_root.join("media");
    Ok(
        file_size(&media_dir.join(media_id))?.saturating_add(file_size(
            &media_dir.join(format!(".{media_id}.media.json")),
        )?),
    )
}

pub(crate) async fn collect_chat_references(
    store: &WorkspaceSessionStore,
    record: &ChatIndexRecord,
) -> Result<ChatReferences, String> {
    let Some(header) = store
        .load_chat_header(&record.id)
        .await
        .map_err(|error| error.to_string())?
    else {
        return Ok(ChatReferences::default());
    };
    let limit = usize::try_from(header.message_log.latest_sequence)
        .map_err(|_| format!("Chat '{}' has too many messages to inspect", record.id))?;
    let messages = store
        .read_message_range_from_header(&record.id, 1, limit, &header.message_log)
        .await
        .map_err(|error| error.to_string())?;
    let mut references = ChatReferences::default();
    let mut media_ids = BTreeSet::new();
    for message in messages {
        let value = serde_json::to_value(message).map_err(|error| error.to_string())?;
        rua_session::chat_media::referenced_ids(&value, &mut media_ids);
        collect_tool_ids(&value, &mut references.tool_use_ids);
    }
    references
        .media_ids
        .extend(media_ids.into_iter().map(|id| id.to_string()));
    Ok(references)
}

pub(crate) fn remove_tool_archives(
    scope_root: &Path,
    references: &ChatReferences,
) -> Result<u64, String> {
    let mut freed = 0_u64;
    let tool_log_dir = scope_root.join("tool_call_logs");
    for tool_use_id in &references.tool_use_ids {
        let key = rua_tool::result_store::recall_key(tool_use_id);
        for extension in ["json", "output"] {
            freed = freed.saturating_add(remove_regular_file(
                &tool_log_dir.join(format!("{key}.{extension}")),
            )?);
        }
    }
    Ok(freed)
}

fn collect_tool_ids(value: &serde_json::Value, tool_ids: &mut BTreeSet<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                if matches!(key.as_str(), "tool_use_id" | "tool_call_id") {
                    if let Some(tool_use_id) = value.as_str().filter(|value| !value.is_empty()) {
                        tool_ids.insert(tool_use_id.to_string());
                    }
                }
                collect_tool_ids(value, tool_ids);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                collect_tool_ids(value, tool_ids);
            }
        }
        _ => {}
    }
}

fn regular_files_size(path: &Path) -> Result<u64, String> {
    regular_files_size_excluding(path, None)
}

fn regular_files_size_excluding(path: &Path, excluded: Option<&Path>) -> Result<u64, String> {
    if !path.exists() {
        return Ok(0);
    }
    let mut total = 0_u64;
    for entry in walkdir::WalkDir::new(path).follow_links(false) {
        let entry = entry.map_err(|error| format!("scan {}: {error}", path.display()))?;
        if excluded.is_some_and(|excluded| entry.path().starts_with(excluded)) {
            continue;
        }
        if entry.file_type().is_file() {
            total =
                total.saturating_add(entry.metadata().map_err(|error| error.to_string())?.len());
        }
    }
    Ok(total)
}

fn file_size(path: &Path) -> Result<u64, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(metadata.len()),
        Ok(_) => Ok(0),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(format!("inspect {}: {error}", path.display())),
    }
}

fn remove_regular_file(path: &Path) -> Result<u64, String> {
    let bytes = file_size(path)?;
    if bytes == 0 && !path.exists() {
        return Ok(0);
    }
    fs::remove_file(path).map_err(|error| format!("delete {}: {error}", path.display()))?;
    Ok(bytes)
}

fn ensure_delete_publication(
    result: crate::sync_publication::PublicationResult,
    label: &str,
) -> Result<(), ChatDeletionPreparationError> {
    use crate::sync_publication::PublicationStatus;

    match result.status {
        PublicationStatus::Published | PublicationStatus::SkippedDisabled => Ok(()),
        PublicationStatus::SkippedUnbound => Err(ChatDeletionPreparationError::Failed(format!(
            "{label} delete Sync publication is unavailable"
        ))),
        PublicationStatus::AppendFailed => Err(ChatDeletionPreparationError::Failed(format!(
            "{label} delete Sync publication failed: {}",
            result.errors.join("; ")
        ))),
    }
}
