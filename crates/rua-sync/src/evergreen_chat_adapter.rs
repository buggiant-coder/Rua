use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use async_trait::async_trait;
use rand::RngCore;
use rua_core::{ChatAddress, ChatScope, ChatScopeStore};

use crate::hlc::HlcTimestamp;
use crate::merge_store::{live_cursor, tombstone_cursor, MergeStore};
use crate::opaque_replica_log::SyncChange;
use crate::sidecar::{LwwDecision, RecordState};
use crate::sync_registry::{sync_cursor_record_id, ApplyOutcome, DomainSyncAdapter};
use crate::{SyncError, SyncResult};

pub const EVERGREEN_CHAT_DOMAIN: &str = "evergreen_chat";
pub const EVERGREEN_CHAT_KIND_FILE_UPSERT: &str = "file_upsert";
pub const EVERGREEN_CHAT_KIND_FILE_DELETE: &str = "file_delete";
pub const EVERGREEN_CHAT_KIND_FILE_SNAPSHOT: &str = "file_snapshot";

pub struct EvergreenChatSyncAdapter {
    data_dir: PathBuf,
    device_id: String,
}

#[derive(Debug, Clone)]
pub struct EvergreenChatFileSnapshot {
    pub relative: String,
    pub payload: serde_json::Value,
    pub hlc: HlcTimestamp,
    pub is_delete: bool,
}

impl EvergreenChatSyncAdapter {
    pub fn new(data_dir: PathBuf, device_id: impl Into<String>) -> Self {
        Self {
            data_dir,
            device_id: device_id.into(),
        }
    }

    pub fn file_change(
        op_id: impl Into<String>,
        address: &ChatAddress,
        relative: impl AsRef<str>,
        payload: serde_json::Value,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncChange {
        SyncChange::new(
            op_id,
            EVERGREEN_CHAT_DOMAIN,
            file_record_id(address, relative.as_ref()),
            EVERGREEN_CHAT_KIND_FILE_UPSERT,
            device_id,
            hlc,
            payload,
        )
    }

    pub fn delete_change(
        op_id: impl Into<String>,
        address: &ChatAddress,
        relative: impl AsRef<str>,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncChange {
        SyncChange::new(
            op_id,
            EVERGREEN_CHAT_DOMAIN,
            file_record_id(address, relative.as_ref()),
            EVERGREEN_CHAT_KIND_FILE_DELETE,
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

    pub fn snapshot_changes_for_chat(
        scope_root: &Path,
        address: &ChatAddress,
        device_id: impl Into<String>,
        fallback_hlc: HlcTimestamp,
    ) -> SyncResult<Vec<SyncChange>> {
        let device_id = device_id.into();
        collect_evergreen_chat_file_snapshots(scope_root, address.chat_id().as_str(), &device_id)
            .into_iter()
            .filter_map(|item| {
                if !item.is_delete && item.relative.starts_with("history_replacements/") {
                    if rua_session::history_replacement_version_is_superseded(&item.payload) {
                        tracing::debug!(
                            relative = %item.relative,
                            "[sync-v4] skip superseded history replacement"
                        );
                        return None;
                    }
                    let record: rua_session::HistoryReplacementRecord =
                        match serde_json::from_value(item.payload.clone()).map_err(|error| {
                            SyncError::InvalidPayload(format!(
                                "invalid history replacement payload for {}: {error}",
                                item.relative
                            ))
                        }) {
                            Ok(record) => record,
                            Err(error) => return Some(Err(error)),
                        };
                    let record_id = item
                        .relative
                        .strip_prefix("history_replacements/")
                        .and_then(|name| name.strip_suffix(".json"))
                        .ok_or_else(|| {
                            SyncError::InvalidPayload(format!(
                                "invalid history replacement path: {}",
                                item.relative
                            ))
                        });
                    let record_id = match record_id {
                        Ok(record_id) => record_id,
                        Err(error) => return Some(Err(error)),
                    };
                    if let Err(error) = rua_session::validate_history_replacement_record(
                        address, record_id, &record,
                    ) {
                        return Some(Err(SyncError::InvalidPayload(format!(
                            "history replacement publication target mismatch for {}: {error}",
                            item.relative
                        ))));
                    }
                }
                Some(Ok(SyncChange::new(
                    Self::new_op_id(),
                    EVERGREEN_CHAT_DOMAIN,
                    file_record_id(address, &item.relative),
                    if item.is_delete {
                        EVERGREEN_CHAT_KIND_FILE_DELETE
                    } else {
                        EVERGREEN_CHAT_KIND_FILE_SNAPSHOT
                    },
                    device_id.clone(),
                    if item.hlc.wall_ms == 0 {
                        fallback_hlc
                    } else {
                        item.hlc
                    },
                    if item.is_delete {
                        serde_json::Value::Object(Default::default())
                    } else {
                        item.payload
                    },
                )))
            })
            .collect()
    }

    fn scope_root(&self, address: &ChatAddress) -> PathBuf {
        ChatScopeStore::resolve(&self.data_dir, address.clone())
            .root()
            .to_path_buf()
    }

    fn path_for_record(&self, address: &ChatAddress, relative: &str) -> SyncResult<PathBuf> {
        evergreen_chat_file_path(
            &self.scope_root(address),
            address.chat_id().as_str(),
            relative,
        )
        .ok_or_else(|| SyncError::Store(format!("invalid evergreen chat record: {relative}")))
    }

    fn project_is_deleted(&self, address: &ChatAddress) -> bool {
        address.project_id().is_some_and(|project_id| {
            rua_project::project_tombstone_exists(&self.data_dir.join("projects"), project_id)
        })
    }
}

#[async_trait]
impl DomainSyncAdapter for EvergreenChatSyncAdapter {
    fn domain(&self) -> &str {
        EVERGREEN_CHAT_DOMAIN
    }

    async fn export_records(&self) -> SyncResult<Vec<SyncChange>> {
        let mut changes = Vec::new();
        for (scope_kind, roots) in [
            ("agent", self.data_dir.join("workspaces")),
            ("project", self.data_dir.join("projects")),
        ] {
            if !roots.is_dir() {
                continue;
            }
            for entry in std::fs::read_dir(&roots)? {
                let entry = entry?;
                let scope_root = entry.path();
                if !scope_root.is_dir() {
                    continue;
                }
                let Some(owner_id) = entry.file_name().to_str().map(ToOwned::to_owned) else {
                    continue;
                };
                if scope_kind == "project"
                    && rua_project::project_tombstone_exists(
                        &self.data_dir.join("projects"),
                        &rua_core::ProjectId::new(owner_id.clone()),
                    )
                {
                    continue;
                }
                let scope = ChatScope::from_kind_and_owner(scope_kind, owner_id)
                    .expect("known Chat Scope kind");
                let mut chat_ids = BTreeSet::new();
                let sessions_dir = scope_root.join("sessions");
                if sessions_dir.is_dir() {
                    for session in std::fs::read_dir(&sessions_dir)? {
                        let session = session?;
                        if !session.path().is_dir() {
                            continue;
                        }
                        if let Some(chat_id) = session.file_name().to_str().map(ToOwned::to_owned) {
                            chat_ids.insert(chat_id);
                        }
                    }
                }
                let prompt_state_dir = scope_root.join("prompt-state");
                if prompt_state_dir.is_dir() {
                    for prompt_state in std::fs::read_dir(&prompt_state_dir)? {
                        let prompt_state = prompt_state?;
                        let path = prompt_state.path();
                        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                            continue;
                        }
                        if let Some(chat_id) = path.file_stem().and_then(|stem| stem.to_str()) {
                            chat_ids.insert(chat_id.to_string());
                        }
                    }
                }
                for chat_id in chat_ids {
                    changes.extend(Self::snapshot_changes_for_chat(
                        &scope_root,
                        &ChatAddress::new(scope.clone(), chat_id),
                        self.device_id.clone(),
                        zero_hlc(),
                    )?);
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
        if change.domain != EVERGREEN_CHAT_DOMAIN {
            return Err(SyncError::Store(format!(
                "evergreen chat adapter received wrong domain: {}",
                change.domain
            )));
        }
        let (address, relative) = parse_file_record_id(&change.record_id)?;
        let cursor_id = sync_cursor_record_id(EVERGREEN_CHAT_DOMAIN, &change.record_id);
        let is_delete = change.kind == EVERGREEN_CHAT_KIND_FILE_DELETE;
        if !is_delete
            && change.kind != EVERGREEN_CHAT_KIND_FILE_UPSERT
            && change.kind != EVERGREEN_CHAT_KIND_FILE_SNAPSHOT
        {
            return Err(SyncError::Store(format!(
                "unsupported evergreen chat change kind: {}",
                change.kind
            )));
        }
        let decision = merge_store.resolve_lww(&cursor_id, change.hlc, is_delete)?;
        if decision == LwwDecision::Skip {
            return Ok(ApplyOutcome::Skipped);
        }
        if !is_delete {
            if let Some(record_id) = history_replacement_record_id(relative) {
                if rua_session::history_replacement_version_is_superseded(&change.payload) {
                    tracing::debug!(
                        record_id = %change.record_id,
                        "[sync-v4] ignore superseded history replacement version"
                    );
                    return Ok(ApplyOutcome::Applied {
                        cursor: live_cursor(change.hlc, None),
                    });
                }
                let record: rua_session::HistoryReplacementRecord =
                    serde_json::from_value(change.payload.clone()).map_err(|error| {
                        SyncError::InvalidPayload(format!(
                            "invalid history replacement payload for {}: {error}",
                            change.record_id
                        ))
                    })?;
                rua_session::validate_history_replacement_record(&address, record_id, &record)
                    .map_err(|error| {
                        SyncError::InvalidPayload(format!(
                            "invalid history replacement record {}: {error}",
                            change.record_id
                        ))
                    })?;
            }
        }
        if relative.starts_with("projection_cache/") {
            tracing::debug!(
                record_id = %change.record_id,
                "[sync-v4] ignore derived evergreen projection cache record"
            );
            let cursor = if is_delete {
                tombstone_cursor(change.hlc, None)
            } else {
                live_cursor(change.hlc, None)
            };
            return Ok(ApplyOutcome::Applied { cursor });
        }
        if !is_delete && !relative.starts_with("messages/") {
            if let Some(cursor) = merge_store.get_record_cursor(&cursor_id)? {
                if cursor.state == RecordState::Live && change.hlc <= cursor.hlc {
                    return Ok(ApplyOutcome::Skipped);
                }
            }
        }

        let path = self.path_for_record(&address, relative)?;
        if !is_delete && self.project_is_deleted(&address) {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            return Ok(ApplyOutcome::Applied {
                cursor: tombstone_cursor(change.hlc, None),
            });
        }
        if is_delete {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            return Ok(ApplyOutcome::Applied {
                cursor: tombstone_cursor(change.hlc, None),
            });
        }

        if let Some(name) = relative.strip_prefix("configurations/") {
            let epoch = name.strip_suffix(".json").ok_or_else(|| {
                SyncError::InvalidPayload("invalid configuration snapshot filename".to_string())
            })?;
            if change
                .payload
                .get("epoch")
                .and_then(serde_json::Value::as_str)
                != Some(epoch)
            {
                return Err(SyncError::InvalidPayload(
                    "configuration epoch does not match record identity".to_string(),
                ));
            }
            if path.exists() {
                let existing = read_json_file(&path).ok_or_else(|| {
                    SyncError::Store("invalid existing Chat configuration snapshot".to_string())
                })?;
                if existing != change.payload {
                    return Err(SyncError::Store(
                        "Chat configuration epoch is immutable".to_string(),
                    ));
                }
            }
        }
        if relative == "session" || relative == "session.json" {
            let scope = ChatScopeStore::resolve(&self.data_dir, address.clone());
            rua_session::WorkspaceSessionStore::new(scope.root())
                .merge_replicated_chat_header(
                    address.chat_id().as_str(),
                    change.payload.clone(),
                    preserve_confirmed_authority,
                )
                .await
                .map_err(|error| SyncError::Store(error.to_string()))?;
        } else if relative.starts_with("messages/") {
            write_message_log_payload(&path, &change.payload)?;
        } else {
            write_json_payload(&path, &change.payload)?;
        }
        Ok(ApplyOutcome::Applied {
            cursor: live_cursor(change.hlc, None),
        })
    }
}

fn preserve_confirmed_authority(
    local: Option<&serde_json::Value>,
    incoming: &mut serde_json::Value,
) -> rua_core::Result<()> {
    let epoch = |placement: &serde_json::Value| {
        placement
            .get("authority_epoch")
            .and_then(serde_json::Value::as_str)
            .and_then(|value| HlcTimestamp::from_sortable_string(value).ok())
            .ok_or_else(|| rua_core::RuaError::Validation {
                message: "invalid Chat Authority epoch in Session snapshot".into(),
            })
    };
    let incoming_epoch = incoming
        .get("authority_placement")
        .map(&epoch)
        .transpose()?;
    let Some(local_placement) = local.and_then(|value| value.get("authority_placement")) else {
        return Ok(());
    };
    let local_epoch = epoch(local_placement)?;
    if incoming_epoch.is_some_and(|incoming_epoch| incoming_epoch > local_epoch) {
        return Ok(());
    }
    let object = incoming
        .as_object_mut()
        .ok_or_else(|| rua_core::RuaError::Validation {
            message: "Session snapshot must be an object".into(),
        })?;
    object.insert("authority_placement".into(), local_placement.clone());
    Ok(())
}

pub fn collect_evergreen_chat_file_snapshots(
    workspace_dir: &Path,
    chat_id: &str,
    device_id: &str,
) -> Vec<EvergreenChatFileSnapshot> {
    let chat_dir = workspace_dir.join("sessions").join(chat_id);
    let prompt_state_path = workspace_dir
        .join("prompt-state")
        .join(format!("{chat_id}.json"));
    if !chat_dir.is_dir() && !prompt_state_path.is_file() {
        return Vec::new();
    }

    let mut items = Vec::new();
    let header_path = chat_dir.join("session.json");
    if let Some(payload) = read_json_file(&header_path) {
        items.push(EvergreenChatFileSnapshot {
            relative: "session".to_string(),
            payload,
            hlc: export_hlc(&header_path, device_id),
            is_delete: false,
        });
    }

    let configuration_head_path = chat_dir.join("configuration_head.json");
    if let Some(payload) = read_json_file(&configuration_head_path) {
        items.push(EvergreenChatFileSnapshot {
            relative: "configuration_head".to_string(),
            payload,
            hlc: export_hlc(&configuration_head_path, device_id),
            is_delete: false,
        });
    }
    let projection_exclusions_path = chat_dir.join("projection_exclusions.json");
    if let Some(payload) = read_json_file(&projection_exclusions_path) {
        items.push(EvergreenChatFileSnapshot {
            relative: "projection_exclusions".to_string(),
            payload,
            hlc: export_hlc(&projection_exclusions_path, device_id),
            is_delete: false,
        });
    }

    let turn_index_path = chat_dir.join("turn_index.json");
    if let Some(payload) = read_json_file(&turn_index_path) {
        items.push(EvergreenChatFileSnapshot {
            relative: "turn_index".to_string(),
            payload,
            hlc: export_hlc(&turn_index_path, device_id),
            is_delete: false,
        });
    }

    collect_jsonl_dir(
        &mut items,
        &chat_dir.join("messages"),
        "messages",
        device_id,
    );
    collect_json_dir(
        &mut items,
        &chat_dir.join("checkpoints"),
        "checkpoints",
        device_id,
    );
    collect_json_dir(
        &mut items,
        &chat_dir.join("configurations"),
        "configurations",
        device_id,
    );
    collect_history_replacement_dir(
        &mut items,
        &chat_dir.join("history_replacements"),
        device_id,
    );
    collect_history_replacement_tombstones(
        &mut items,
        &chat_dir.join(".history_replacement_tombstones"),
        &chat_dir.join("history_replacements"),
        device_id,
    );
    if let Some(payload) = read_json_file(&prompt_state_path) {
        items.push(EvergreenChatFileSnapshot {
            relative: "prompt_state".to_string(),
            payload,
            hlc: export_hlc(&prompt_state_path, device_id),
            is_delete: false,
        });
    }
    items
}

pub fn file_record_id(address: &ChatAddress, relative: &str) -> String {
    format!(
        "{}/{}/{}/{relative}",
        address.scope().kind(),
        address.scope().owner_id(),
        address.chat_id()
    )
}

pub fn parse_file_record_id(record_id: &str) -> SyncResult<(ChatAddress, &str)> {
    let mut parts = record_id.splitn(4, '/');
    let scope_kind = parts
        .next()
        .filter(|value| *value == "agent" || *value == "project")
        .ok_or_else(|| {
            SyncError::Store(format!("invalid evergreen chat record id: {record_id}"))
        })?;
    let owner_id = parts
        .next()
        .filter(|value| valid_component(value))
        .ok_or_else(|| {
            SyncError::Store(format!("invalid evergreen chat record id: {record_id}"))
        })?;
    let chat_id = parts
        .next()
        .filter(|value| valid_component(value))
        .ok_or_else(|| {
            SyncError::Store(format!("invalid evergreen chat record id: {record_id}"))
        })?;
    let relative = parts
        .next()
        .filter(|value| !value.is_empty() && !value.contains("..") && !value.contains('\\'))
        .ok_or_else(|| {
            SyncError::Store(format!("invalid evergreen chat record id: {record_id}"))
        })?;
    let scope = ChatScope::from_kind_and_owner(scope_kind, owner_id).ok_or_else(|| {
        SyncError::Store(format!("invalid evergreen chat record id: {record_id}"))
    })?;
    Ok((ChatAddress::new(scope, chat_id), relative))
}

fn evergreen_chat_file_path(
    workspace_dir: &Path,
    chat_id: &str,
    relative: &str,
) -> Option<PathBuf> {
    if !valid_component(chat_id) {
        return None;
    }
    let chat_dir = workspace_dir.join("sessions").join(chat_id);
    if relative == "session" || relative == "session.json" {
        return Some(chat_dir.join("session.json"));
    }

    if relative == "configuration_head" {
        return Some(chat_dir.join("configuration_head.json"));
    }
    if relative == "projection_exclusions" || relative == "projection_exclusions.json" {
        return Some(chat_dir.join("projection_exclusions.json"));
    }
    if relative == "turn_index" || relative == "turn_index.json" {
        return Some(chat_dir.join("turn_index.json"));
    }
    if relative == "prompt_state" || relative == "prompt_state.json" {
        return Some(
            workspace_dir
                .join("prompt-state")
                .join(format!("{chat_id}.json")),
        );
    }
    if let Some(name) = relative.strip_prefix("messages/") {
        return safe_leaf_file(name).map(|name| chat_dir.join("messages").join(name));
    }
    if let Some(name) = relative.strip_prefix("checkpoints/") {
        return safe_leaf_file(name).map(|name| chat_dir.join("checkpoints").join(name));
    }
    if let Some(name) = relative.strip_prefix("configurations/") {
        return safe_leaf_file(name).map(|name| chat_dir.join("configurations").join(name));
    }
    if let Some(name) = relative.strip_prefix("history_replacements/") {
        return safe_history_replacement_file(name)
            .map(|name| chat_dir.join("history_replacements").join(name));
    }
    None
}

fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains('/')
        && !value.contains('\\')
}

fn safe_leaf_file(name: &str) -> Option<&str> {
    (valid_component(name) && (name.ends_with(".json") || name.ends_with(".jsonl"))).then_some(name)
}

fn safe_history_replacement_file(name: &str) -> Option<&str> {
    let record_id = name.strip_suffix(".json")?;
    history_replacement_record_id(&format!("history_replacements/{name}"))?;
    (record_id.len() == 67).then_some(name)
}

fn history_replacement_record_id(relative: &str) -> Option<&str> {
    let name = relative.strip_prefix("history_replacements/")?;
    let record_id = name.strip_suffix(".json")?;
    if record_id.len() != 67
        || !record_id.starts_with("hr-")
        || !record_id[3..].bytes().all(|byte| byte.is_ascii_hexdigit())
        || record_id[3..].bytes().any(|byte| byte.is_ascii_uppercase())
    {
        return None;
    }
    Some(record_id)
}

fn collect_jsonl_dir(
    items: &mut Vec<EvergreenChatFileSnapshot>,
    dir: &Path,
    relative_prefix: &str,
    device_id: &str,
) {
    let entries = match sorted_dir_entries(dir) {
        Some(entries) => entries,
        None => return,
    };
    for path in entries {
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        items.push(EvergreenChatFileSnapshot {
            relative: format!("{relative_prefix}/{name}"),
            payload: serde_json::Value::String(content),
            hlc: export_hlc(&path, device_id),
            is_delete: false,
        });
    }
}

fn collect_json_dir(
    items: &mut Vec<EvergreenChatFileSnapshot>,
    dir: &Path,
    relative_prefix: &str,
    device_id: &str,
) {
    let entries = match sorted_dir_entries(dir) {
        Some(entries) => entries,
        None => return,
    };
    for path in entries {
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(payload) = read_json_file(&path) else {
            continue;
        };
        items.push(EvergreenChatFileSnapshot {
            relative: format!("{relative_prefix}/{name}"),
            payload,
            hlc: export_hlc(&path, device_id),
            is_delete: false,
        });
    }
}

fn collect_history_replacement_dir(
    items: &mut Vec<EvergreenChatFileSnapshot>,
    dir: &Path,
    device_id: &str,
) {
    let entries = match sorted_dir_entries(dir) {
        Some(entries) => entries,
        None => return,
    };
    for path in entries {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if safe_history_replacement_file(name).is_none() {
            continue;
        }
        let Some(payload) = read_json_file(&path) else {
            continue;
        };
        items.push(EvergreenChatFileSnapshot {
            relative: format!("history_replacements/{name}"),
            payload,
            hlc: export_hlc(&path, device_id),
            is_delete: false,
        });
    }
}

fn collect_history_replacement_tombstones(
    items: &mut Vec<EvergreenChatFileSnapshot>,
    tombstone_dir: &Path,
    live_dir: &Path,
    device_id: &str,
) {
    let entries = match sorted_dir_entries(tombstone_dir) {
        Some(entries) => entries,
        None => return,
    };
    for path in entries {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if safe_history_replacement_file(name).is_none() || live_dir.join(name).is_file() {
            continue;
        }
        items.push(EvergreenChatFileSnapshot {
            relative: format!("history_replacements/{name}"),
            payload: serde_json::Value::Object(Default::default()),
            hlc: export_hlc(&path, device_id),
            is_delete: true,
        });
    }
}

fn sorted_dir_entries(dir: &Path) -> Option<Vec<PathBuf>> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect();
    entries.sort();
    Some(entries)
}

fn read_json_file(path: &Path) -> Option<serde_json::Value> {
    if !path.is_file() {
        return None;
    }
    std::fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
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

fn zero_hlc() -> HlcTimestamp {
    HlcTimestamp {
        wall_ms: 0,
        counter: 0,
        device_id: [0; 8],
    }
}

fn write_json_payload(path: &Path, payload: &serde_json::Value) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    rua_config::atomic_write_json(path, payload)
}

fn write_message_log_payload(path: &Path, payload: &serde_json::Value) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let remote_raw = message_payload_to_jsonl(payload)?;
    let merged = if path.is_file() {
        let local_raw = std::fs::read_to_string(path).unwrap_or_default();
        merge_jsonl_entries(&local_raw, &remote_raw)
    } else {
        remote_raw
    };
    rua_config::atomic_write(path, merged.as_bytes())
}

fn message_payload_to_jsonl(payload: &serde_json::Value) -> std::io::Result<String> {
    if let Some(raw) = payload.as_str() {
        return Ok(raw.to_string());
    }
    let entries = payload
        .get("entries")
        .and_then(|value| value.as_array())
        .or_else(|| payload.as_array());
    let mut out = String::new();
    if let Some(entries) = entries {
        for entry in entries {
            out.push_str(&serde_json::to_string(entry)?);
            out.push('\n');
        }
    } else {
        out.push_str(&serde_json::to_string(payload)?);
        out.push('\n');
    }
    Ok(out)
}

fn merge_jsonl_entries(local: &str, remote: &str) -> String {
    let mut by_id = BTreeMap::<String, serde_json::Value>::new();
    let mut order = Vec::<String>::new();
    let mut seen = HashSet::<String>::new();
    for entry in local.lines().chain(remote.lines()) {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        let id = message_identity(&value).unwrap_or_else(|| format!("line:{trimmed}"));
        if seen.insert(id.clone()) {
            order.push(id.clone());
        }
        by_id.insert(id, value);
    }
    let mut out = String::new();
    for id in order {
        if let Some(value) = by_id.get(&id) {
            if let Ok(line) = serde_json::to_string(value) {
                out.push_str(&line);
                out.push('\n');
            }
        }
    }
    out
}

fn message_identity(value: &serde_json::Value) -> Option<String> {
    value
        .get("message_id")
        .or_else(|| value.get("id"))
        .and_then(|value| value.as_str())
        .map(ToOwned::to_owned)
        .or_else(|| {
            value
                .get("sequence")
                .and_then(|value| value.as_u64())
                .map(|seq| format!("sequence:{seq}"))
        })
}
