use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use async_trait::async_trait;
use rand::RngCore;
use rua_core::{ChatAddress, ChatScope, ChatScopeStore};
use rua_session::chat_index::{self, ChatIndexRecord};

use crate::hlc::HlcTimestamp;
use crate::merge_store::{live_cursor, tombstone_cursor, MergeStore};
use crate::opaque_replica_log::SyncChange;
use crate::sidecar::LwwDecision;
use crate::sync_registry::{sync_cursor_record_id, ApplyOutcome, DomainSyncAdapter};
use crate::{SyncError, SyncResult};

pub const CHAT_DOMAIN: &str = "chat";
pub const CHAT_KIND_INDEX_UPSERT: &str = "index_upsert";
pub const CHAT_KIND_INDEX_DELETE: &str = "index_delete";

pub struct ChatIndexSyncAdapter {
    data_dir: PathBuf,
    device_id: String,
}

impl ChatIndexSyncAdapter {
    pub fn new(data_dir: PathBuf, device_id: impl Into<String>) -> Self {
        Self {
            data_dir,
            device_id: device_id.into(),
        }
    }

    pub fn index_change(
        op_id: impl Into<String>,
        address: ChatAddress,
        chat: serde_json::Value,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncChange {
        SyncChange::new(
            op_id,
            CHAT_DOMAIN,
            index_record_id(&address),
            CHAT_KIND_INDEX_UPSERT,
            device_id,
            hlc,
            chat,
        )
    }

    pub fn delete_change(
        op_id: impl Into<String>,
        address: &ChatAddress,
        device_id: impl Into<String>,
        hlc: HlcTimestamp,
    ) -> SyncChange {
        SyncChange::new(
            op_id,
            CHAT_DOMAIN,
            index_record_id(address),
            CHAT_KIND_INDEX_DELETE,
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

    fn scope_root(&self, address: &ChatAddress) -> PathBuf {
        ChatScopeStore::resolve(&self.data_dir, address.clone())
            .root()
            .to_path_buf()
    }

    fn chat_index_path(&self, address: &ChatAddress) -> PathBuf {
        self.scope_root(address).join("chat-index.sqlite")
    }

    fn project_is_deleted(&self, address: &ChatAddress) -> bool {
        address.project_id().is_some_and(|project_id| {
            rua_project::project_tombstone_exists(&self.data_dir.join("projects"), project_id)
        })
    }

    fn export_hlc(path: &std::path::Path, device_id: &str) -> HlcTimestamp {
        let wall_ms = path
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0);
        let mut device = [0_u8; 8];
        device.copy_from_slice(&blake3::hash(device_id.as_bytes()).as_bytes()[..8]);
        HlcTimestamp {
            wall_ms,
            counter: 0,
            device_id: device,
        }
    }

    fn apply_index_upsert(
        &self,
        address: &ChatAddress,
        change: &SyncChange,
        merge_store: &MergeStore,
    ) -> SyncResult<ApplyOutcome> {
        let cursor_id = sync_cursor_record_id(CHAT_DOMAIN, &change.record_id);
        if merge_store
            .get_record_cursor(&cursor_id)?
            .is_some_and(|cursor| cursor.state == crate::sidecar::RecordState::Tombstoned)
        {
            return Ok(ApplyOutcome::Skipped);
        }
        if merge_store.resolve_lww(&cursor_id, change.hlc, false)? == LwwDecision::Skip {
            return Ok(ApplyOutcome::Skipped);
        }
        if self.project_is_deleted(address) {
            chat_index::delete_sync(&self.scope_root(address), address.chat_id().as_str())
                .map_err(SyncError::Store)?;
            return Ok(ApplyOutcome::Applied {
                cursor: tombstone_cursor(change.hlc, None),
            });
        }

        let scope_root = self.scope_root(address);
        let mut incoming = change.payload.clone();
        if let Some(local) = chat_index::get_sync(&scope_root, address.chat_id().as_str())
            .map_err(SyncError::Store)?
            .map(chat_index_record_to_json)
        {
            preserve_newer_authority_placement(&local, &mut incoming);
        }
        let record = chat_index_record_from_json(address.chat_id().as_str(), &incoming)?;
        chat_index::upsert_sync(&scope_root, &record).map_err(SyncError::Store)?;
        Ok(ApplyOutcome::Applied {
            cursor: live_cursor(change.hlc, None),
        })
    }

    fn apply_index_delete(
        &self,
        address: &ChatAddress,
        change: &SyncChange,
        merge_store: &MergeStore,
    ) -> SyncResult<ApplyOutcome> {
        let cursor_id = sync_cursor_record_id(CHAT_DOMAIN, &change.record_id);
        if merge_store.resolve_lww(&cursor_id, change.hlc, true)? == LwwDecision::Skip {
            return Ok(ApplyOutcome::Skipped);
        }
        chat_index::delete_sync(&self.scope_root(address), address.chat_id().as_str())
            .map_err(SyncError::Store)?;
        Ok(ApplyOutcome::Applied {
            cursor: tombstone_cursor(change.hlc, None),
        })
    }
}

#[async_trait]
impl DomainSyncAdapter for ChatIndexSyncAdapter {
    fn domain(&self) -> &str {
        CHAT_DOMAIN
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
                if !entry.path().is_dir() {
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
                let scope_address = ChatAddress::new(scope.clone(), "__scope_export__");
                let chat_index_path = self.chat_index_path(&scope_address);
                for chat in chat_index::list_all_sync(&entry.path()).map_err(SyncError::Store)? {
                    changes.push(Self::index_change(
                        Self::new_op_id(),
                        ChatAddress::new(scope.clone(), chat.id.clone()),
                        chat_index_record_to_json(chat),
                        self.device_id.clone(),
                        Self::export_hlc(&chat_index_path, &self.device_id),
                    ));
                }
            }
        }
        changes.sort_by(|left, right| left.record_id.cmp(&right.record_id));
        Ok(changes)
    }

    async fn apply_change(
        &self,
        change: &SyncChange,
        merge_store: &MergeStore,
    ) -> SyncResult<ApplyOutcome> {
        if change.domain != CHAT_DOMAIN {
            return Err(SyncError::Store(format!(
                "Chat Index adapter received wrong domain: {}",
                change.domain
            )));
        }
        let address = parse_chat_record_id(&change.record_id)?;
        match change.kind.as_str() {
            CHAT_KIND_INDEX_UPSERT => self.apply_index_upsert(&address, change, merge_store),
            CHAT_KIND_INDEX_DELETE => self.apply_index_delete(&address, change, merge_store),
            _ => Err(SyncError::Store(format!(
                "unsupported Chat Index change kind: {}",
                change.kind
            ))),
        }
    }
}

pub fn index_record_id(address: &ChatAddress) -> String {
    format!(
        "{}/{}/chats/{}",
        address.scope().kind(),
        address.scope().owner_id(),
        address.chat_id()
    )
}

pub fn parse_chat_record_id(record_id: &str) -> SyncResult<ChatAddress> {
    let mut parts = record_id.split('/');
    let scope_kind = parts
        .next()
        .filter(|value| *value == "agent" || *value == "project")
        .ok_or_else(|| SyncError::Store(format!("invalid Chat Index record id: {record_id}")))?;
    let owner_id = parts
        .next()
        .filter(|value| valid_component(value))
        .ok_or_else(|| SyncError::Store(format!("invalid Chat Index record id: {record_id}")))?;
    if parts.next() != Some("chats") {
        return Err(SyncError::Store(format!(
            "invalid Chat Index record id: {record_id}"
        )));
    }
    let chat_id = parts
        .next()
        .filter(|value| valid_component(value))
        .ok_or_else(|| SyncError::Store(format!("invalid Chat Index record id: {record_id}")))?;
    if parts.next().is_some() {
        return Err(SyncError::Store(format!(
            "invalid Chat Index record id: {record_id}"
        )));
    }
    let scope = ChatScope::from_kind_and_owner(scope_kind, owner_id)
        .ok_or_else(|| SyncError::Store(format!("invalid Chat Index record id: {record_id}")))?;
    Ok(ChatAddress::new(scope, chat_id))
}

fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && !value.contains("..")
        && !value.contains('/')
        && !value.contains('\\')
}

fn chat_index_record_to_json(record: ChatIndexRecord) -> serde_json::Value {
    serde_json::json!({
        "id": record.id,
        "name": record.name,
        "user_id": record.user_id,
        "channel": record.channel,
        "created_at": record.created_at,
        "updated_at": record.updated_at,
        "meta": record.meta,
        "status": record.status,
        "pinned": record.pinned,
        "pin_order": record.pin_order,
        "tag": record.tag,
    })
}

fn chat_index_record_from_json(
    chat_id: &str,
    value: &serde_json::Value,
) -> SyncResult<ChatIndexRecord> {
    let now = chrono::Utc::now();
    let parse_time = |field: &str| {
        value
            .get(field)
            .and_then(serde_json::Value::as_str)
            .and_then(|raw| chrono::DateTime::parse_from_rfc3339(raw).ok())
            .map(|time| time.with_timezone(&chrono::Utc))
    };
    Ok(ChatIndexRecord {
        id: chat_id.to_string(),
        name: value
            .get("name")
            .or_else(|| value.pointer("/metadata/title"))
            .and_then(serde_json::Value::as_str)
            .filter(|name| !name.trim().is_empty())
            .unwrap_or("New Chat")
            .to_string(),
        user_id: value
            .get("user_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("default")
            .to_string(),
        channel: value
            .get("channel")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("console")
            .to_string(),
        created_at: parse_time("created_at").unwrap_or(now),
        updated_at: parse_time("updated_at")
            .or_else(|| {
                value
                    .pointer("/metadata/last_activity_at")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|raw| chrono::DateTime::parse_from_rfc3339(raw).ok())
                    .map(|time| time.with_timezone(&chrono::Utc))
            })
            .unwrap_or(now),
        meta: value
            .get("meta")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({})),
        status: value
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("idle")
            .to_string(),
        pinned: value
            .get("pinned")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        pin_order: value
            .get("pin_order")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0),
        tag: value
            .get("tag")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|error| SyncError::Store(format!("invalid Chat tag: {error}")))?
            .unwrap_or_default(),
        archived: value
            .get("archived")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    })
}

fn preserve_newer_authority_placement(local: &serde_json::Value, incoming: &mut serde_json::Value) {
    let Some(local_placement) = local.pointer("/meta/authority_placement") else {
        return;
    };
    let Some(local_epoch) = local_placement
        .get("authority_epoch")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| HlcTimestamp::from_sortable_string(value).ok())
    else {
        return;
    };
    let incoming_epoch = incoming
        .pointer("/meta/authority_placement/authority_epoch")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| HlcTimestamp::from_sortable_string(value).ok());
    if incoming_epoch.is_some_and(|epoch| epoch > local_epoch) {
        return;
    }
    let Some(incoming_object) = incoming.as_object_mut() else {
        return;
    };
    let meta = incoming_object
        .entry("meta")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    let Some(meta) = meta.as_object_mut() else {
        return;
    };
    meta.insert("authority_placement".to_string(), local_placement.clone());
}
