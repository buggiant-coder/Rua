use std::path::Path;

use chrono::{DateTime, Utc};
use rua_core::ChatAddress;
use serde::{Deserialize, Serialize};

use crate::errors::ApiError;
use crate::services::chat_index;
use crate::services::conversation_authority_placement::{
    chat_acp_binding_active, parse_conversation_authority_placement,
    ConversationAuthorityPlacement, ConversationAuthorityPlacementError, CHAT_ACP_BINDING_META_KEY,
    CHAT_AUTHORITY_PLACEMENT_META_KEY, CHAT_MODEL_META_KEY,
};
use crate::state::AppState;
use crate::sync_publication::{publish_sync_fact, SyncPublicationFact};

#[derive(Debug)]
pub enum ChatMetadataStoreError {
    InvalidAuthorityPlacement(ConversationAuthorityPlacementError),
    Conflict(String),
    Internal(String),
    Io(std::io::Error),
    Json(serde_json::Error),
    Session(rua_core::RuaError),
}

impl std::fmt::Display for ChatMetadataStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidAuthorityPlacement(error) => write!(f, "{error}"),
            Self::Conflict(message) | Self::Internal(message) => write!(f, "{message}"),
            Self::Io(error) => write!(f, "io: {error}"),
            Self::Json(error) => write!(f, "json: {error}"),
            Self::Session(error) => write!(f, "session: {error}"),
        }
    }
}

impl std::error::Error for ChatMetadataStoreError {}

impl From<ConversationAuthorityPlacementError> for ChatMetadataStoreError {
    fn from(error: ConversationAuthorityPlacementError) -> Self {
        Self::InvalidAuthorityPlacement(error)
    }
}

impl From<std::io::Error> for ChatMetadataStoreError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for ChatMetadataStoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<rua_core::RuaError> for ChatMetadataStoreError {
    fn from(error: rua_core::RuaError) -> Self {
        Self::Session(error)
    }
}

impl From<ChatMetadataStoreError> for ApiError {
    fn from(error: ChatMetadataStoreError) -> Self {
        match error {
            ChatMetadataStoreError::InvalidAuthorityPlacement(error) => {
                ApiError::BadRequest(error.to_string())
            }
            ChatMetadataStoreError::Conflict(message) => ApiError::Conflict(message),
            other => ApiError::Internal(other.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMetadataRecord {
    pub id: String,
    pub name: String,
    pub user_id: String,
    pub channel: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub meta: serde_json::Value,
    #[serde(default = "default_chat_status")]
    pub status: String,
    #[serde(default)]
    pub model: Option<serde_json::Value>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub pin_order: i64,
    #[serde(default)]
    pub tag: rua_chat::ChatTag,
    #[serde(default)]
    pub archived: bool,
}

impl ChatMetadataRecord {
    pub fn to_json_value(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_else(|_| serde_json::json!({}))
    }
}

#[derive(Debug, Clone)]
pub struct AcpBindingMetadata {
    pub target_device_id: Option<String>,
    pub runner: String,
    pub cwd: Option<String>,
    pub runner_session_id: Option<String>,
}

fn default_chat_status() -> String {
    "idle".to_string()
}

fn placeholder_chat(chat_id: &str, now: DateTime<Utc>) -> ChatMetadataRecord {
    ChatMetadataRecord {
        id: chat_id.to_string(),
        name: "New Chat".to_string(),
        user_id: "default".to_string(),
        channel: "console".to_string(),
        created_at: now,
        updated_at: now,
        meta: serde_json::json!({
            rua_core::CHAT_COLLABORATION_MODE_META_KEY:
                rua_core::AgentCollaborationMode::default(),
        }),
        status: "idle".to_string(),
        model: None,
        pinned: false,
        pin_order: 0,
        tag: rua_chat::ChatTag::None,
        archived: false,
    }
}

pub(crate) fn new_targeted_chat_metadata(
    address: &ChatAddress,
    chat_name: &str,
    authority_placement: &ConversationAuthorityPlacement,
    initial_model: Option<serde_json::Value>,
    now: DateTime<Utc>,
) -> std::result::Result<ChatMetadataRecord, ChatMetadataStoreError> {
    authority_placement.validate()?;
    let mut chat = placeholder_chat(address.chat_id().as_str(), now);
    let name = truncate_chat_name(chat_name);
    if !name.is_empty() {
        chat.name = name;
    }
    chat.model = initial_model;
    meta_object(&mut chat.meta).insert(
        CHAT_AUTHORITY_PLACEMENT_META_KEY.to_string(),
        serde_json::to_value(authority_placement)?,
    );
    Ok(chat)
}

fn meta_object(meta: &mut serde_json::Value) -> &mut serde_json::Map<String, serde_json::Value> {
    if !meta.is_object() {
        *meta = serde_json::json!({});
    }
    meta.as_object_mut().expect("meta normalized to object")
}

fn is_placeholder_chat_title(title: &str) -> bool {
    let title = title.trim();
    title.is_empty() || title == "New Chat"
}

fn truncate_chat_name(msg: &str) -> String {
    let trimmed = msg.trim();
    if trimmed.chars().count() <= 80 {
        trimmed.to_string()
    } else {
        let head: String = trimmed.chars().take(77).collect();
        format!("{head}...")
    }
}

pub(crate) fn chat_index_entry(chat: &ChatMetadataRecord) -> chat_index::ChatIndexEntry {
    chat_index::ChatIndexEntry {
        id: chat.id.clone(),
        name: chat.name.clone(),
        user_id: chat.user_id.clone(),
        channel: chat.channel.clone(),
        created_at: chat.created_at,
        updated_at: chat.updated_at,
        meta: chat.meta.clone(),
        status: chat.status.clone(),
        pinned: chat.pinned,
        pin_order: chat.pin_order,
        tag: chat.tag,
        archived: chat.archived,
    }
}

pub(crate) async fn load_chat_record(
    workspace_dir: &Path,
    chat_id: &str,
) -> std::result::Result<Option<ChatMetadataRecord>, ChatMetadataStoreError> {
    let header = rua_session::WorkspaceSessionStore::new(workspace_dir)
        .load_chat_header(chat_id)
        .await?;
    let projection = chat_index::get(workspace_dir, chat_id)
        .await
        .map_err(|error| {
            ChatMetadataStoreError::Internal(format!("failed to read chat index: {error}"))
        })?;
    let Some(header) = header else {
        if projection.is_some() {
            return Err(ChatMetadataStoreError::Conflict(format!(
                "Chat Session File is missing for indexed Chat '{chat_id}'"
            )));
        }
        return Ok(None);
    };

    let projection = rua_chat::project_chat_header(&header, projection)?;
    let chat = ChatMetadataRecord {
        id: projection.id,
        name: projection.name,
        user_id: projection.user_id,
        channel: projection.channel,
        created_at: projection.created_at,
        updated_at: projection.updated_at,
        meta: projection.meta,
        status: projection.status,
        model: rua_chat::model_slot_from_header(&header),
        pinned: projection.pinned,
        pin_order: projection.pin_order,
        tag: projection.tag,
        archived: projection.archived,
    };
    Ok(Some(chat))
}

pub(crate) async fn stage_synced_chat_metadata(
    workspace_dir: &Path,
    chat: &ChatMetadataRecord,
) -> std::result::Result<(), ChatMetadataStoreError> {
    chat_index::upsert(workspace_dir, chat_index_entry(chat))
        .await
        .map_err(|error| {
            ChatMetadataStoreError::Internal(format!(
                "failed to stage synced chat metadata: {error}"
            ))
        })
}

fn sync_header_metadata(header: &mut rua_session::ChatSessionHeader, chat: &ChatMetadataRecord) {
    header.metadata.archived = chat.archived;
    header.metadata.title = chat.name.clone();
    header.metadata.pinned = chat.pinned;
    header.metadata.last_activity_at = chat.updated_at.to_rfc3339();
    sync_header_extra_key(header, chat, CHAT_AUTHORITY_PLACEMENT_META_KEY);
    sync_header_extra_key(header, chat, CHAT_ACP_BINDING_META_KEY);
    sync_header_extra_key(header, chat, rua_core::CHAT_COLLABORATION_MODE_META_KEY);
    sync_header_model(header, chat);
}

fn sync_header_extra_key(
    header: &mut rua_session::ChatSessionHeader,
    chat: &ChatMetadataRecord,
    key: &str,
) {
    if let Some(value) = chat.meta.get(key) {
        header.extra.insert(key.to_string(), value.clone());
    } else {
        header.extra.remove(key);
    }
}

fn sync_header_model(header: &mut rua_session::ChatSessionHeader, chat: &ChatMetadataRecord) {
    if let Some(value) = chat.model.as_ref() {
        header
            .extra
            .insert(CHAT_MODEL_META_KEY.to_string(), value.clone());
    }
}

async fn save_sidecars(
    state: Option<&AppState>,
    scope_root: &Path,
    address: &ChatAddress,
    execution_agent_id: &str,
    chat: &ChatMetadataRecord,
) -> std::result::Result<(), ChatMetadataStoreError> {
    let store = rua_session::WorkspaceSessionStore::new(scope_root);
    let mut header = match store.load_chat_header(&chat.id).await? {
        Some(header) => header,
        None => chat_header_from_metadata_record(chat, execution_agent_id),
    };
    if !execution_agent_id.is_empty() {
        header.agent_id = execution_agent_id.to_string();
    }
    sync_header_metadata(&mut header, chat);
    store.save_chat_header(&header).await?;
    save_index_and_publish(state, scope_root, address, chat).await
}

pub(crate) fn chat_header_from_metadata_record(
    chat: &ChatMetadataRecord,
    execution_agent_id: &str,
) -> rua_session::ChatSessionHeader {
    let mut header = rua_session::ChatSessionHeader::new(rua_session::ChatMetadata {
        chat_id: chat.id.clone(),
        agent_id: execution_agent_id.to_string(),
        title: chat.name.clone(),
        created_at: chat.created_at.to_rfc3339(),
        last_activity_at: chat.updated_at.to_rfc3339(),
        pinned: chat.pinned,
        archived: chat.archived,
        preview: None,
    });
    sync_header_metadata(&mut header, chat);
    header
}

async fn save_authority_placement_sidecars(
    state: Option<&AppState>,
    scope_root: &Path,
    address: &ChatAddress,
    chat: &ChatMetadataRecord,
) -> std::result::Result<(), ChatMetadataStoreError> {
    let store = rua_session::WorkspaceSessionStore::new(scope_root);
    if let Some(mut header) = store.load_chat_header(&chat.id).await? {
        if let Some(placement) = chat.meta.get(CHAT_AUTHORITY_PLACEMENT_META_KEY) {
            header.extra.insert(
                CHAT_AUTHORITY_PLACEMENT_META_KEY.to_string(),
                placement.clone(),
            );
            store.save_chat_header(&header).await?;
        }
    }
    save_index_and_publish(state, scope_root, address, chat).await
}

async fn save_index_and_publish(
    state: Option<&AppState>,
    scope_root: &Path,
    address: &ChatAddress,
    chat: &ChatMetadataRecord,
) -> std::result::Result<(), ChatMetadataStoreError> {
    chat_index::upsert(scope_root, chat_index_entry(chat))
        .await
        .map_err(|error| {
            ChatMetadataStoreError::Internal(format!("failed to update chat index: {error}"))
        })?;

    if let Some(state) = state {
        let chat_json = chat.to_json_value();
        let _ = publish_sync_fact(
            state,
            SyncPublicationFact::ChatIndexUpserted {
                address: address.clone(),
                chat: chat_json,
            },
        )
        .await;
        let _ = publish_sync_fact(
            state,
            SyncPublicationFact::StableConversationState {
                scope_root: scope_root.to_path_buf(),
                address: address.clone(),
            },
        )
        .await;
    }

    Ok(())
}

pub async fn materialize_targeted_chat_metadata(
    state: Option<&AppState>,
    scope_root: &Path,
    address: &ChatAddress,
    execution_agent_id: &str,
    chat_name: &str,
    authority_placement: &ConversationAuthorityPlacement,
    initial_model: Option<serde_json::Value>,
) -> std::result::Result<ChatMetadataRecord, ChatMetadataStoreError> {
    let chat_id = address.chat_id().as_str();
    authority_placement.validate()?;
    let name = truncate_chat_name(chat_name);
    let now = Utc::now();

    let existing_chat = load_chat_record(scope_root, chat_id).await?;
    let preserve_existing_session = existing_chat.is_some();
    let mut chat = existing_chat.map(Ok).unwrap_or_else(|| {
        new_targeted_chat_metadata(
            address,
            chat_name,
            authority_placement,
            initial_model.clone(),
            now,
        )
    })?;
    match parse_conversation_authority_placement(&chat.meta) {
        Ok(existing_target) if existing_target == *authority_placement => {}
        Ok(existing_target) => {
            return Err(ChatMetadataStoreError::Conflict(format!(
                "chat authority is Device '{}' at Epoch {}, not Device '{}' at Epoch {}",
                existing_target.device_id,
                existing_target.authority_epoch,
                authority_placement.device_id,
                authority_placement.authority_epoch,
            )));
        }
        Err(_) if preserve_existing_session => {
            return Err(ChatMetadataStoreError::Conflict(
                "existing Chat Session File is missing Conversation Authority Placement"
                    .to_string(),
            ));
        }
        Err(_) => {
            meta_object(&mut chat.meta).insert(
                CHAT_AUTHORITY_PLACEMENT_META_KEY.to_string(),
                serde_json::to_value(authority_placement)?,
            );
        }
    }
    if !preserve_existing_session && !name.is_empty() && is_placeholder_chat_title(&chat.name) {
        chat.name = name;
    }
    if !preserve_existing_session && chat.model.is_none() {
        chat.model = initial_model;
    }

    if preserve_existing_session {
        save_authority_placement_sidecars(state, scope_root, address, &chat).await?;
    } else {
        save_sidecars(state, scope_root, address, execution_agent_id, &chat).await?;
    }
    Ok(chat)
}

pub async fn update_authority_placement(
    state: Option<&AppState>,
    scope_root: &Path,
    address: &ChatAddress,
    next_target: ConversationAuthorityPlacement,
) -> std::result::Result<ChatMetadataRecord, ChatMetadataStoreError> {
    write_authority_placement(state, scope_root, address, next_target, true).await
}

pub async fn apply_confirmed_authority_placement(
    scope_root: &Path,
    address: &ChatAddress,
    next_target: ConversationAuthorityPlacement,
) -> std::result::Result<ChatMetadataRecord, ChatMetadataStoreError> {
    write_authority_placement(None, scope_root, address, next_target, false).await
}

async fn write_authority_placement(
    state: Option<&AppState>,
    scope_root: &Path,
    address: &ChatAddress,
    next_target: ConversationAuthorityPlacement,
    enforce_acp_lock: bool,
) -> std::result::Result<ChatMetadataRecord, ChatMetadataStoreError> {
    let chat_id = address.chat_id().as_str();
    next_target.validate()?;
    let mut chat = load_chat_record(scope_root, chat_id)
        .await?
        .unwrap_or_else(|| placeholder_chat(chat_id, Utc::now()));

    let store = rua_session::WorkspaceSessionStore::new(scope_root);
    let existing_header = store.load_chat_header(chat_id).await?;
    let header_has_acp_binding = existing_header.as_ref().is_some_and(|header| {
        chat_acp_binding_active(&serde_json::Value::Object(header.extra.clone()))
    });
    if enforce_acp_lock && (chat_acp_binding_active(&chat.meta) || header_has_acp_binding) {
        return Err(ChatMetadataStoreError::Conflict(
            "Current chat is in ACP mode. Exit ACP before changing the authority device."
                .to_string(),
        ));
    }

    let meta = meta_object(&mut chat.meta);
    let placement = serde_json::to_value(next_target)?;
    meta.insert(
        CHAT_AUTHORITY_PLACEMENT_META_KEY.to_string(),
        placement.clone(),
    );
    if existing_header.is_none() {
        let execution_agent_id = address.agent_id().map(|id| id.as_str()).unwrap_or_default();
        let mut header = chat_header_from_metadata_record(&chat, execution_agent_id);
        if let Some(agent_id) = address.agent_id() {
            rua_chat::apply_agent_scope_defaults(&mut header, agent_id.as_str());
        }
        store.save_chat_header(&header).await?;
    }
    rua_chat::ChatService::for_resolved_scope(scope_root, address.clone())
        .update_setting(
            address,
            rua_chat::ChatSetting::AuthorityPlacement(placement),
        )
        .await?;
    if chat
        .meta
        .get(rua_session::collaboration::COLLABORATION_META_KEY)
        .is_some_and(serde_json::Value::is_object)
    {
        rua_chat::ChatService::for_resolved_scope(scope_root, address.clone())
            .repair_projection(address)
            .await?;
    }
    if let Some(state) = state {
        let _ = publish_sync_fact(
            state,
            SyncPublicationFact::StableConversationState {
                scope_root: scope_root.to_path_buf(),
                address: address.clone(),
            },
        )
        .await;
    }
    Ok(chat)
}

pub async fn persist_acp_binding(
    state: Option<&AppState>,
    scope_root: &Path,
    address: &ChatAddress,
    execution_agent_id: &str,
    binding: AcpBindingMetadata,
) -> std::result::Result<ChatMetadataRecord, ChatMetadataStoreError> {
    let chat_id = address.chat_id().as_str();
    let now = Utc::now();
    let mut chat = load_chat_record(scope_root, chat_id)
        .await?
        .unwrap_or_else(|| placeholder_chat(chat_id, now));
    let meta = meta_object(&mut chat.meta);
    let meta_value = serde_json::Value::Object(meta.clone());
    if chat_acp_binding_active(&meta_value) {
        return Err(ChatMetadataStoreError::Conflict(
            "chat is already in ACP passthrough mode".to_string(),
        ));
    }
    let existing_target =
        crate::services::conversation_authority_target::load_optional_chat_authority_placement(
            scope_root, chat_id,
        )
        .await
        .map_err(ChatMetadataStoreError::Internal)?
        .ok_or_else(|| {
            ChatMetadataStoreError::Conflict(
                "Chat is missing Conversation Authority Placement".to_string(),
            )
        })?;
    match binding.target_device_id.as_deref() {
        Some(target) if existing_target.device_id == target => {}
        Some(target) => {
            return Err(ChatMetadataStoreError::Conflict(format!(
                "chat authority is device '{}', not ACP device '{target}'",
                existing_target.device_id
            )));
        }
        None => {
            return Err(ChatMetadataStoreError::Conflict(
                "ACP target device is required".to_string(),
            ));
        }
    }
    meta.insert(
        CHAT_AUTHORITY_PLACEMENT_META_KEY.to_string(),
        serde_json::to_value(&existing_target)?,
    );
    meta.insert(
        CHAT_ACP_BINDING_META_KEY.to_string(),
        serde_json::json!({
            "kind": "acp_passthrough",
            "device_id": binding.target_device_id,
            "runner": binding.runner,
            "cwd": binding.cwd,
            "runner_session_id": binding.runner_session_id,
            "lease_id": format!("acp-{}", uuid::Uuid::new_v4()),
            "started_at": now.to_rfc3339(),
        }),
    );
    chat.updated_at = now;
    save_sidecars(state, scope_root, address, execution_agent_id, &chat).await?;
    Ok(chat)
}

pub async fn clear_acp_binding(
    state: Option<&AppState>,
    scope_root: &Path,
    address: &ChatAddress,
    execution_agent_id: &str,
) -> std::result::Result<Option<ChatMetadataRecord>, ChatMetadataStoreError> {
    let chat_id = address.chat_id().as_str();
    let Some(mut chat) = load_chat_record(scope_root, chat_id).await? else {
        return Ok(None);
    };
    if !chat_acp_binding_active(&chat.meta) {
        return Ok(None);
    }
    let meta = meta_object(&mut chat.meta);
    meta.remove(CHAT_ACP_BINDING_META_KEY);
    chat.updated_at = Utc::now();
    save_sidecars(state, scope_root, address, execution_agent_id, &chat).await?;
    Ok(Some(chat))
}
