use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rua_core::{ChatAddress, ChatScope, ChatScopeStore, Result};
use rua_session::{
    chat_index, ChatMetadata, ChatSessionHeader, MessageLogEntry, NewConversationMessage,
    NewConversationTurn, SavedConversationSearchRequest, SavedConversationSearchResult,
    TurnIndexRecord, WorkspaceSessionStore,
};
use serde::{Deserialize, Serialize};

pub use rua_session::chat_index::{ChatSortField, SortOrder};
pub use rua_session::ChatTag;

const CHAT_POLICY_KEY: &str = "chat_policy";
const SELECTED_AGENT_ID_KEY: &str = "selected_agent_id";
const CHAT_MODEL_KEY: &str = "model";
const CHAT_THINKING_KEY: &str = "thinking";
const AUTHORITY_PLACEMENT_KEY: &str = "authority_placement";
const HARNESS_CONFIGURATION_KEY: &str = "harness_configuration";
const ACP_BINDING_KEY: &str = "acp_binding";
pub const TURN_MODEL_KEY: &str = "turn_model";
pub const TURN_THINKING_KEY: &str = "turn_thinking";
pub const TURN_AGENT_ASSIGNMENT_META_KEY: &str = "turn_agent_assignment";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentAssignmentPolicy {
    Fixed,
    Selectable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourcePolicy {
    None,
    ProjectBinding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityPlacementPolicy {
    Selectable,
    ResourceBound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatPolicy {
    pub agent_assignment: AgentAssignmentPolicy,
    pub resource: ResourcePolicy,
    pub authority_placement: AuthorityPlacementPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnAgentAssignment {
    pub agent_id: String,
    pub agent_name: String,
    pub assigned_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub system_prompt_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled_tools: Vec<String>,
}

impl ChatPolicy {
    pub fn for_scope(scope: &ChatScope) -> Self {
        match scope {
            ChatScope::Agent { .. } => Self {
                agent_assignment: AgentAssignmentPolicy::Fixed,
                resource: ResourcePolicy::None,
                authority_placement: AuthorityPlacementPolicy::Selectable,
            },
            ChatScope::Project { .. } => Self {
                agent_assignment: AgentAssignmentPolicy::Selectable,
                resource: ResourcePolicy::ProjectBinding,
                authority_placement: AuthorityPlacementPolicy::ResourceBound,
            },
        }
    }

    pub fn from_metadata(scope: &ChatScope, metadata: &serde_json::Value) -> Self {
        metadata
            .get(CHAT_POLICY_KEY)
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_else(|| Self::for_scope(scope))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chat {
    address: ChatAddress,
    policy: ChatPolicy,
    title: String,
    pinned: bool,
    tag: ChatTag,
    selected_agent_id: Option<String>,
    model: Option<serde_json::Value>,
    thinking: Option<serde_json::Value>,
    authority_placement: Option<serde_json::Value>,
    harness_configuration: Option<serde_json::Value>,
}

impl Chat {
    pub fn address(&self) -> &ChatAddress {
        &self.address
    }

    pub fn policy(&self) -> &ChatPolicy {
        &self.policy
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn pinned(&self) -> bool {
        self.pinned
    }

    pub fn tag(&self) -> ChatTag {
        self.tag
    }

    pub fn model(&self) -> Option<&serde_json::Value> {
        self.model.as_ref()
    }

    pub fn selected_agent_id(&self) -> Option<&str> {
        self.selected_agent_id.as_deref()
    }

    pub fn thinking(&self) -> Option<&serde_json::Value> {
        self.thinking.as_ref()
    }

    pub fn authority_placement(&self) -> Option<&serde_json::Value> {
        self.authority_placement.as_ref()
    }

    pub fn harness_configuration(&self) -> Option<&serde_json::Value> {
        self.harness_configuration.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatSetting {
    Policy(ChatPolicy),
    SelectedAgent(String),
    Model(serde_json::Value),
    Thinking(serde_json::Value),
    AuthorityPlacement(serde_json::Value),
    HarnessConfiguration(serde_json::Value),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatListUpdate {
    pub title: Option<String>,
    pub pinned: Option<bool>,
    pub tag: Option<ChatTag>,
    pub archived: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatHistoryQuery {
    pub before_turn_ordinal: Option<u64>,
    pub turn_limit: Option<usize>,
    pub around_turn_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatSearchQuery {
    pub query: String,
    pub offset: usize,
    pub limit: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatListQuery {
    pub user_id: Option<String>,
    pub channel: Option<String>,
    pub search: Option<String>,
    pub tag: Option<ChatTag>,
    pub conversation_view: chat_index::ChatConversationView,
    pub source_chat_id: Option<String>,
    pub executor: Option<String>,
    pub status: Option<String>,
    pub sort_by: ChatSortField,
    pub sort_order: SortOrder,
    pub offset: u64,
    pub limit: u64,
    pub archived: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct ChatHistoryPage {
    pub chat: Chat,
    pub header: ChatSessionHeader,
    pub turns: Vec<TurnIndexRecord>,
    pub messages: Vec<MessageLogEntry>,
    pub oldest_turn_ordinal: Option<u64>,
    pub newest_turn_ordinal: Option<u64>,
    pub has_more_before: bool,
    pub includes_latest_turn: bool,
}

#[derive(Debug, Clone)]
pub struct ChatHistoryBoundary {
    pub before_turn_id: String,
    pub target_turn: Option<TurnIndexRecord>,
    pub selected_variant_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ChatService {
    storage: ChatStorage,
}

#[derive(Debug, Clone)]
enum ChatStorage {
    RuaRoot(PathBuf),
    ResolvedScope { address: ChatAddress, root: PathBuf },
}

impl ChatService {
    pub fn new(rua_root: impl Into<PathBuf>) -> Self {
        Self {
            storage: ChatStorage::RuaRoot(rua_root.into()),
        }
    }

    pub fn for_scope_store(scope_store: ChatScopeStore) -> Self {
        Self::for_resolved_scope(scope_store.root(), scope_store.address().clone())
    }

    pub fn for_resolved_scope(scope_root: impl Into<PathBuf>, address: ChatAddress) -> Self {
        Self {
            storage: ChatStorage::ResolvedScope {
                address,
                root: scope_root.into(),
            },
        }
    }

    pub async fn load(&self, address: &ChatAddress) -> Result<Option<Chat>> {
        let scope_root = self.scope_root(address)?;
        let session_store = WorkspaceSessionStore::new(&scope_root);
        let Some(header) = session_store
            .load_chat_header(address.chat_id().as_str())
            .await?
        else {
            return Ok(None);
        };

        let policy = header
            .extra
            .get(CHAT_POLICY_KEY)
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|error| rua_core::RuaError::Validation {
                message: format!("invalid Chat Policy for {address}: {error}"),
            })?
            .unwrap_or_else(|| ChatPolicy::for_scope(address.scope()));

        let model = model_slot_from_header(&header);
        let thinking = model_and_thinking_from_header(&header).1.cloned();
        Ok(Some(Chat {
            address: address.clone(),
            policy,
            title: header.metadata.title,
            pinned: header.metadata.pinned,
            tag: header.tag,
            selected_agent_id: header
                .extra
                .get(SELECTED_AGENT_ID_KEY)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            model,
            thinking,
            authority_placement: header.extra.get(AUTHORITY_PLACEMENT_KEY).cloned(),
            harness_configuration: header.extra.get(HARNESS_CONFIGURATION_KEY).cloned(),
        }))
    }

    pub async fn policy(&self, address: &ChatAddress) -> Result<ChatPolicy> {
        Ok(self
            .load(address)
            .await?
            .map(|chat| *chat.policy())
            .unwrap_or_else(|| ChatPolicy::for_scope(address.scope())))
    }

    pub async fn update_setting(
        &self,
        address: &ChatAddress,
        setting: ChatSetting,
    ) -> Result<Chat> {
        if let ChatSetting::SelectedAgent(agent_id) = &setting {
            if let Some(fixed_agent_id) = address.agent_id() {
                if agent_id != fixed_agent_id.as_str() {
                    return Err(rua_core::RuaError::Validation {
                        message: format!(
                            "Agent-Scoped Chat has fixed Agent '{}'; cannot select '{}'",
                            fixed_agent_id.as_str(),
                            agent_id
                        ),
                    });
                }
            }
        }
        let scope_root = self.scope_root(address)?;
        let session_store = WorkspaceSessionStore::new(&scope_root);
        let (key, value) = match setting {
            ChatSetting::Policy(policy) => (
                CHAT_POLICY_KEY,
                serde_json::to_value(policy).map_err(|error| rua_core::RuaError::Validation {
                    message: format!("failed to serialize Chat Policy: {error}"),
                })?,
            ),
            ChatSetting::SelectedAgent(agent_id) => {
                (SELECTED_AGENT_ID_KEY, serde_json::Value::String(agent_id))
            }
            ChatSetting::Model(model) => (CHAT_MODEL_KEY, model),
            ChatSetting::Thinking(thinking) => (CHAT_THINKING_KEY, thinking),
            ChatSetting::AuthorityPlacement(placement) => (AUTHORITY_PLACEMENT_KEY, placement),
            ChatSetting::HarnessConfiguration(configuration) => {
                (HARNESS_CONFIGURATION_KEY, configuration)
            }
        };
        let updated = session_store
            .update_chat_header(address.chat_id().as_str(), move |header| {
                if key == CHAT_MODEL_KEY {
                    let (_, thinking) = capture_turn_model_and_thinking(Some(&value), None);
                    let thinking = thinking.unwrap_or(serde_json::Value::Null);
                    header
                        .extra
                        .insert(CHAT_THINKING_KEY.into(), thinking.clone());
                    if let Some(configuration) = header
                        .extra
                        .get_mut("hook_configuration")
                        .and_then(serde_json::Value::as_object_mut)
                    {
                        configuration.insert(CHAT_THINKING_KEY.into(), thinking);
                    }
                }
                if matches!(
                    key,
                    CHAT_MODEL_KEY | CHAT_THINKING_KEY | HARNESS_CONFIGURATION_KEY
                ) {
                    if let Some(configuration) = header
                        .extra
                        .get_mut("hook_configuration")
                        .and_then(serde_json::Value::as_object_mut)
                    {
                        configuration.insert(key.to_string(), value.clone());
                        configuration.insert(
                            "epoch".to_string(),
                            serde_json::json!(format!("hook-config-{}", uuid::Uuid::new_v4())),
                        );
                    }
                }
                header.extra.insert(key.to_string(), value);
            })
            .await?;
        if !updated {
            return Err(rua_core::RuaError::Io {
                message: format!("Chat Session File not found: {address}"),
            });
        }
        self.load(address)
            .await?
            .ok_or_else(|| rua_core::RuaError::Io {
                message: format!("Chat Session File disappeared after update: {address}"),
            })
    }

    pub async fn update_listing(
        &self,
        address: &ChatAddress,
        update: ChatListUpdate,
    ) -> Result<Chat> {
        let title = update.title.map(|title| title.trim().to_string());
        if title.as_ref().is_some_and(String::is_empty) {
            return Err(rua_core::RuaError::Validation {
                message: "Chat title must not be empty".to_string(),
            });
        }

        let scope_root = self.scope_root(address)?;
        let session_store = WorkspaceSessionStore::new(&scope_root);
        let updated_at = Utc::now().to_rfc3339();
        let updated = session_store
            .update_chat_header(address.chat_id().as_str(), move |header| {
                if let Some(title) = title {
                    header.metadata.title = title;
                }
                if let Some(pinned) = update.pinned {
                    header.metadata.pinned = pinned;
                }
                if let Some(tag) = update.tag {
                    header.tag = tag;
                }
                if let Some(archived) = update.archived {
                    header.metadata.archived = archived;
                    if archived {
                        header.metadata.pinned = false;
                    }
                }
                header.metadata.last_activity_at = updated_at;
            })
            .await?;
        if !updated {
            return Err(rua_core::RuaError::NotFound {
                resource: "Chat".to_string(),
                id: address.to_string(),
            });
        }
        let header = session_store
            .load_chat_header(address.chat_id().as_str())
            .await?
            .ok_or_else(|| rua_core::RuaError::Io {
                message: format!("Chat Session File disappeared after update: {address}"),
            })?;
        project_header(&scope_root, &header).await?;
        self.load(address)
            .await?
            .ok_or_else(|| rua_core::RuaError::Io {
                message: format!("Chat Session File disappeared after projection: {address}"),
            })
    }

    pub async fn replace_placeholder_title(
        &self,
        address: &ChatAddress,
        title: impl Into<String>,
    ) -> Result<Chat> {
        let title = title.into().trim().to_string();
        if title.is_empty() {
            return Err(rua_core::RuaError::Validation {
                message: "Chat title must not be empty".to_string(),
            });
        }

        let scope_root = self.scope_root(address)?;
        let session_store = WorkspaceSessionStore::new(&scope_root);
        let chat_id = address.chat_id().to_string();
        let updated_at = Utc::now().to_rfc3339();
        let updated = session_store
            .update_chat_header(address.chat_id().as_str(), move |header| {
                let current = header.metadata.title.trim();
                if header.turn_index.latest_turn == 0
                    && (current.is_empty() || current == "New Chat" || current == chat_id)
                {
                    header.metadata.title = title;
                    header.metadata.last_activity_at = updated_at;
                }
            })
            .await?;
        if !updated {
            return Err(rua_core::RuaError::NotFound {
                resource: "Chat".to_string(),
                id: address.to_string(),
            });
        }
        let header = session_store
            .load_chat_header(address.chat_id().as_str())
            .await?
            .ok_or_else(|| rua_core::RuaError::Io {
                message: format!("Chat Session File disappeared after update: {address}"),
            })?;
        project_header(&scope_root, &header).await?;
        self.load(address)
            .await?
            .ok_or_else(|| rua_core::RuaError::Io {
                message: format!("Chat Session File disappeared after projection: {address}"),
            })
    }

    pub async fn repair_projection(&self, address: &ChatAddress) -> Result<Chat> {
        let scope_root = self.scope_root(address)?;
        let session_store = WorkspaceSessionStore::new(&scope_root);
        let header = session_store
            .load_chat_header(address.chat_id().as_str())
            .await?
            .ok_or_else(|| rua_core::RuaError::NotFound {
                resource: "Chat".to_string(),
                id: address.to_string(),
            })?;
        project_header(&scope_root, &header).await?;
        self.load(address)
            .await?
            .ok_or_else(|| rua_core::RuaError::Io {
                message: format!("Chat Session File disappeared after repair: {address}"),
            })
    }

    pub async fn complete_agent_scope_materialization(
        &self,
        address: &ChatAddress,
    ) -> Result<Chat> {
        let agent_id = address
            .agent_id()
            .ok_or_else(|| rua_core::RuaError::Validation {
                message: "Agent-Scoped Chat materialization requires Agent Scope".to_string(),
            })?
            .to_string();
        let scope_root = self.scope_root(address)?;
        let session_store = WorkspaceSessionStore::new(&scope_root);
        let updated = session_store
            .update_chat_header(address.chat_id().as_str(), move |header| {
                apply_agent_scope_defaults(header, &agent_id);
            })
            .await?;
        if !updated {
            return Err(rua_core::RuaError::NotFound {
                resource: "Chat".to_string(),
                id: address.to_string(),
            });
        }
        self.load(address)
            .await?
            .ok_or_else(|| rua_core::RuaError::Io {
                message: format!("Chat Session File disappeared after materialization: {address}"),
            })
    }

    pub async fn resolve_history_boundary(
        &self,
        address: &ChatAddress,
        before_turn_id: &str,
    ) -> Result<ChatHistoryBoundary> {
        let scope_root = self.scope_root(address)?;
        let store = WorkspaceSessionStore::new(&scope_root);
        let header = store
            .load_chat_header(address.chat_id().as_str())
            .await?
            .ok_or_else(|| rua_core::RuaError::NotFound {
                resource: "Chat".to_string(),
                id: address.to_string(),
            })?;
        if header
            .extra
            .get(ACP_BINDING_KEY)
            .and_then(serde_json::Value::as_object)
            .is_some()
        {
            return Err(rua_core::RuaError::Validation {
                message: "Current chat is in ACP mode. Exit ACP before rollback or branch."
                    .to_string(),
            });
        }
        let turns = store
            .read_turn_range(address.chat_id().as_str(), 1, usize::MAX)
            .await?;
        let before_position = turns
            .iter()
            .position(|turn| turn.turn_id == before_turn_id)
            .ok_or_else(|| rua_core::RuaError::Validation {
                message: format!("Chat Turn not found: {before_turn_id}"),
            })?;
        if before_position == 0 {
            return Ok(ChatHistoryBoundary {
                before_turn_id: before_turn_id.to_string(),
                target_turn: None,
                selected_variant_id: None,
            });
        }

        let target_turn = turns[before_position - 1].clone();
        if rua_session::turn_origin_is_acp(&target_turn)
            && turns
                .get(before_position)
                .is_some_and(rua_session::turn_origin_is_acp)
        {
            return Err(rua_core::RuaError::Validation {
                message: "Cannot rollback or branch from the middle of an ACP segment.".to_string(),
            });
        }
        let selected_variant_id = target_turn
            .selected_assistant_response_variant()
            .map(|variant| variant.variant_id)
            .ok_or_else(|| rua_core::RuaError::Validation {
                message: "The previous Agent response has no selectable output.".to_string(),
            })?;
        Ok(ChatHistoryBoundary {
            before_turn_id: before_turn_id.to_string(),
            target_turn: Some(target_turn),
            selected_variant_id: Some(selected_variant_id),
        })
    }

    pub async fn branch_before(&self, address: &ChatAddress, before_turn_id: &str) -> Result<Chat> {
        let boundary = self
            .resolve_history_boundary(address, before_turn_id)
            .await?;
        let scope_root = self.scope_root(address)?;
        let store = WorkspaceSessionStore::new(&scope_root);
        let source_header = store
            .load_chat_header(address.chat_id().as_str())
            .await?
            .ok_or_else(|| rua_core::RuaError::NotFound {
                resource: "Chat".to_string(),
                id: address.to_string(),
            })?;
        let source_projection = chat_index::get(&scope_root, address.chat_id().as_str())
            .await
            .map_err(chat_index_error)?;
        let source_turns = store
            .read_turn_range(address.chat_id().as_str(), 1, usize::MAX)
            .await?;
        let branch_point = boundary.target_turn.zip(boundary.selected_variant_id);

        let now = Utc::now().to_rfc3339();
        let branch_id = uuid::Uuid::new_v4().to_string();
        let branch_address = ChatAddress::new(address.scope().clone(), branch_id.clone());
        let mut branch_header = ChatSessionHeader::new(ChatMetadata {
            chat_id: branch_id.clone(),
            agent_id: source_header.agent_id.clone(),
            title: branch_chat_name(&source_header.metadata.title),
            created_at: now.clone(),
            last_activity_at: now,
            pinned: false,
            archived: false,
            preview: None,
        });
        branch_header.extra = source_header.extra;
        store.save_chat_header(&branch_header).await?;

        if let Some((target_turn, target_variant_id)) = branch_point {
            for turn in source_turns {
                let selected_variant_id = if turn.turn_id == target_turn.turn_id {
                    Some(target_variant_id.as_str())
                } else {
                    turn.selected_variant_id.as_deref()
                };
                let (messages, selected_variant_id) = read_selected_turn_messages(
                    &store,
                    address.chat_id().as_str(),
                    &turn,
                    selected_variant_id,
                    turn.turn_id == target_turn.turn_id,
                )
                .await?
                .ok_or_else(|| rua_core::RuaError::Validation {
                    message: "selected_variant_id is not valid for turn_id".to_string(),
                })?;
                store
                    .append_conversation_turn(
                        &branch_id,
                        NewConversationTurn {
                            turn_id: turn.turn_id.clone(),
                            lifecycle: turn.lifecycle.clone(),
                            selected_variant_id,
                            messages: messages
                                .into_iter()
                                .map(message_to_new_conversation_message)
                                .collect(),
                        },
                    )
                    .await?;
                if turn.turn_id == target_turn.turn_id {
                    break;
                }
            }
            store
                .inherit_compatible_summary_checkpoint(address.chat_id().as_str(), &branch_id)
                .await?;
            store
                .inherit_compatible_history_replacements(address, &branch_address)
                .await?;
        }

        let branch_header =
            store
                .load_chat_header(&branch_id)
                .await?
                .ok_or_else(|| rua_core::RuaError::Io {
                    message: format!("Branched Chat Session File disappeared: {branch_address}"),
                })?;
        if let Some(mut projection) = source_projection {
            projection.status = "idle".to_string();
            projection.pin_order = 0;
            let projection = project_chat_header(&branch_header, Some(projection))?;
            chat_index::upsert(&scope_root, projection)
                .await
                .map_err(chat_index_error)?;
        } else {
            project_header(&scope_root, &branch_header).await?;
        }
        Self::for_resolved_scope(&scope_root, branch_address.clone())
            .load(&branch_address)
            .await?
            .ok_or_else(|| rua_core::RuaError::Io {
                message: format!("Branched Chat disappeared after projection: {branch_address}"),
            })
    }

    pub async fn rollback_before(
        &self,
        address: &ChatAddress,
        before_turn_id: &str,
    ) -> Result<Chat> {
        let boundary = self
            .resolve_history_boundary(address, before_turn_id)
            .await?;
        let scope_root = self.scope_root(address)?;
        let store = WorkspaceSessionStore::new(&scope_root);
        let changed = match (boundary.target_turn, boundary.selected_variant_id) {
            (None, None) => {
                store
                    .rollback_active_conversation_to_empty(address.chat_id().as_str())
                    .await?
            }
            (Some(target), Some(selected_variant_id)) => store
                .rollback_active_conversation_to_turn(
                    address.chat_id().as_str(),
                    &target.turn_id,
                    selected_variant_id,
                )
                .await?
                .is_some(),
            _ => false,
        };
        if !changed {
            return Err(rua_core::RuaError::Validation {
                message: "Chat branch point is no longer valid".to_string(),
            });
        }

        let updated_at = Utc::now().to_rfc3339();
        let updated = store
            .update_chat_header(address.chat_id().as_str(), move |header| {
                header.metadata.last_activity_at = updated_at;
            })
            .await?;
        if !updated {
            return Err(rua_core::RuaError::NotFound {
                resource: "Chat".to_string(),
                id: address.to_string(),
            });
        }
        let header = store
            .load_chat_header(address.chat_id().as_str())
            .await?
            .ok_or_else(|| rua_core::RuaError::Io {
                message: format!("Rolled-back Chat Session File disappeared: {address}"),
            })?;
        project_header(&scope_root, &header).await?;
        self.load(address)
            .await?
            .ok_or_else(|| rua_core::RuaError::Io {
                message: format!("Rolled-back Chat disappeared after projection: {address}"),
            })
    }

    pub async fn select_variant(
        &self,
        address: &ChatAddress,
        turn_id: &str,
        selected_variant_id: &str,
    ) -> Result<Chat> {
        let scope_root = self.scope_root(address)?;
        let store = WorkspaceSessionStore::new(&scope_root);
        if !store
            .set_turn_selected_variant(
                address.chat_id().as_str(),
                turn_id,
                selected_variant_id.to_string(),
            )
            .await?
        {
            return Err(rua_core::RuaError::Validation {
                message: "selected_variant_id is not valid for turn_id".to_string(),
            });
        }
        let updated_at = Utc::now().to_rfc3339();
        let updated = store
            .update_chat_header(address.chat_id().as_str(), move |header| {
                header.metadata.last_activity_at = updated_at;
            })
            .await?;
        if !updated {
            return Err(rua_core::RuaError::NotFound {
                resource: "Chat".to_string(),
                id: address.to_string(),
            });
        }
        let header = store
            .load_chat_header(address.chat_id().as_str())
            .await?
            .ok_or_else(|| rua_core::RuaError::Io {
                message: format!("Variant-selected Chat Session File disappeared: {address}"),
            })?;
        project_header(&scope_root, &header).await?;
        self.load(address)
            .await?
            .ok_or_else(|| rua_core::RuaError::Io {
                message: format!("Variant-selected Chat disappeared after projection: {address}"),
            })
    }

    pub async fn read_history(
        &self,
        address: &ChatAddress,
        query: ChatHistoryQuery,
    ) -> Result<Option<ChatHistoryPage>> {
        let scope_root = self.scope_root(address)?;
        let store = WorkspaceSessionStore::new(&scope_root);
        let Some(header) = store.load_chat_header(address.chat_id().as_str()).await? else {
            return Ok(None);
        };
        let chat = self
            .load(address)
            .await?
            .ok_or_else(|| rua_core::RuaError::Io {
                message: format!("Chat Session File disappeared during History read: {address}"),
            })?;
        let all_turns = store
            .read_turn_range(address.chat_id().as_str(), 1, usize::MAX)
            .await?;
        let latest_turn_ordinal = all_turns.last().map(|turn| turn.ordinal).unwrap_or(0);
        let turns = if let Some(turn_id) = query.around_turn_id.as_deref() {
            if query.before_turn_ordinal.is_some() {
                return Err(rua_core::RuaError::Validation {
                    message: "around_turn_id and before_turn_ordinal are mutually exclusive"
                        .to_string(),
                });
            }
            let position = all_turns
                .iter()
                .position(|turn| turn.turn_id == turn_id)
                .ok_or_else(|| rua_core::RuaError::NotFound {
                    resource: "Chat Turn".to_string(),
                    id: turn_id.to_string(),
                })?;
            let limit = query.turn_limit.unwrap_or(20).clamp(1, 100);
            let start = position.saturating_sub(limit / 2);
            let end = (start + limit).min(all_turns.len());
            all_turns[start..end].to_vec()
        } else if let Some(limit) = query.turn_limit {
            if limit == 0 {
                Vec::new()
            } else {
                let before = query
                    .before_turn_ordinal
                    .unwrap_or(latest_turn_ordinal.saturating_add(1));
                let mut selected = all_turns
                    .iter()
                    .filter(|turn| turn.ordinal < before)
                    .rev()
                    .take(limit)
                    .cloned()
                    .collect::<Vec<_>>();
                selected.reverse();
                selected
            }
        } else {
            all_turns.clone()
        };
        let first_sequence = turns.first().map(|turn| turn.first_sequence).unwrap_or(1);
        let last_sequence = turns.last().map(|turn| turn.last_sequence).unwrap_or(0);
        let oldest_turn_ordinal = turns.first().map(|turn| turn.ordinal);
        let newest_turn_ordinal = turns.last().map(|turn| turn.ordinal);
        let first_turn_ordinal = all_turns.first().map(|turn| turn.ordinal).unwrap_or(1);
        let has_more_before = oldest_turn_ordinal
            .map(|ordinal| ordinal > first_turn_ordinal)
            .unwrap_or(false);
        let includes_latest_turn = if all_turns.is_empty() {
            true
        } else {
            newest_turn_ordinal
                .map(|ordinal| ordinal >= latest_turn_ordinal)
                .unwrap_or(false)
        };
        let messages = if last_sequence > 0 {
            let mut message_log = header.message_log.clone();
            message_log.latest_sequence = last_sequence;
            store
                .read_message_sequence_range_from_header(
                    address.chat_id().as_str(),
                    first_sequence,
                    last_sequence,
                    &message_log,
                )
                .await?
        } else {
            Vec::new()
        };

        Ok(Some(ChatHistoryPage {
            chat,
            header,
            turns,
            messages,
            oldest_turn_ordinal,
            newest_turn_ordinal,
            has_more_before,
            includes_latest_turn,
        }))
    }

    pub async fn search(
        &self,
        address: &ChatAddress,
        query: ChatSearchQuery,
    ) -> Result<SavedConversationSearchResult> {
        let scope_root = self.scope_root(address)?;
        let store = WorkspaceSessionStore::new(scope_root);
        let request = SavedConversationSearchRequest {
            agent_id: address
                .agent_id()
                .map(ToString::to_string)
                .unwrap_or_default(),
            query: query.query,
            offset: query.offset,
            limit: query.limit,
        };
        match address.scope() {
            ChatScope::Agent { .. } => store.search_saved_conversations(request).await,
            ChatScope::Project { .. } => {
                store
                    .search_saved_conversations_in_current_scope(request)
                    .await
            }
        }
    }

    pub async fn list(
        &self,
        address: &ChatAddress,
        query: ChatListQuery,
    ) -> Result<chat_index::ChatIndexPage> {
        let scope_root = self.scope_root(address)?;
        chat_index::list(
            &scope_root,
            chat_index::ChatIndexFilter {
                user_id: query.user_id,
                channel: query.channel,
                search: query.search,
                tag: query.tag,
                conversation_view: query.conversation_view,
                source_chat_id: query.source_chat_id,
                executor: query.executor,
                status: query.status,
                sort_by: query.sort_by,
                sort_order: query.sort_order,
                archived: query.archived,
            },
            query.offset,
            query.limit,
        )
        .await
        .map_err(chat_index_error)
    }

    pub async fn load_turn_agent_assignment(
        &self,
        address: &ChatAddress,
        turn_id: &str,
    ) -> Result<TurnAgentAssignment> {
        let scope_root = self.scope_root(address)?;
        let messages = WorkspaceSessionStore::new(&scope_root)
            .read_turn_messages(address.chat_id().as_str(), turn_id)
            .await?;
        let value = messages
            .iter()
            .find(|message| message.role == "user")
            .and_then(|message| message.metadata.get(TURN_AGENT_ASSIGNMENT_META_KEY))
            .cloned()
            .ok_or_else(|| rua_core::RuaError::Validation {
                message: format!("Turn Agent Assignment is missing for Turn {turn_id}"),
            })?;
        serde_json::from_value(value).map_err(|error| rua_core::RuaError::Validation {
            message: format!("invalid Turn Agent Assignment for Turn {turn_id}: {error}"),
        })
    }

    pub fn storage_root(&self, address: &ChatAddress) -> Result<PathBuf> {
        self.scope_root(address)
    }

    pub fn media_root(&self, address: &ChatAddress) -> Result<PathBuf> {
        Ok(self.storage_root(address)?.join("media"))
    }

    fn scope_root(&self, address: &ChatAddress) -> Result<PathBuf> {
        match &self.storage {
            ChatStorage::RuaRoot(rua_root) => {
                Ok(ChatScopeStore::resolve(rua_root, address.clone())
                    .root()
                    .to_path_buf())
            }
            ChatStorage::ResolvedScope {
                address: bound_address,
                root,
            } if bound_address == address => Ok(root.clone()),
            ChatStorage::ResolvedScope {
                address: bound_address,
                ..
            } => Err(rua_core::RuaError::Validation {
                message: format!("Chat Service is bound to {}, not {address}", bound_address),
            }),
        }
    }
}

fn branch_chat_name(source: &str) -> String {
    if source.trim().is_empty() {
        "Branched Chat".to_string()
    } else {
        format!("{source} (branch)")
    }
}

async fn read_selected_turn_messages(
    store: &WorkspaceSessionStore,
    chat_id: &str,
    turn: &TurnIndexRecord,
    selected_variant_id: Option<&str>,
    limit_runtime_notes_to_selected_variant: bool,
) -> Result<Option<(Vec<MessageLogEntry>, Option<String>)>> {
    let user_message = store
        .read_message_range(chat_id, turn.user_message_sequence, 1)
        .await?
        .into_iter()
        .next()
        .filter(|message| message.sequence == turn.user_message_sequence && message.role == "user");
    let variants = turn.effective_assistant_response_variants();
    let selected_variant = if let Some(selected_variant_id) = selected_variant_id {
        let Some(variant) = variants
            .iter()
            .find(|variant| variant.variant_id == selected_variant_id)
            .cloned()
        else {
            return Ok(None);
        };
        Some(variant)
    } else {
        variants.last().cloned()
    };

    let mut messages = Vec::new();
    if let Some(user_message) = user_message {
        messages.push(user_message);
    }
    let selected_variant_id = selected_variant
        .as_ref()
        .map(|variant| variant.variant_id.clone());
    if let Some(selected_variant) = selected_variant {
        messages.extend(
            store
                .read_message_range(
                    chat_id,
                    selected_variant.first_sequence,
                    (selected_variant.last_sequence - selected_variant.first_sequence + 1) as usize,
                )
                .await?
                .into_iter()
                .filter(|message| message.role != "user"),
        );
        let runtime_note_end_sequence = if limit_runtime_notes_to_selected_variant {
            selected_variant.last_sequence
        } else {
            turn.last_sequence
        };
        messages.extend(
            store
                .read_message_range(
                    chat_id,
                    turn.first_sequence,
                    (turn.last_sequence - turn.first_sequence + 1) as usize,
                )
                .await?
                .into_iter()
                .filter(|message| {
                    message.sequence <= runtime_note_end_sequence
                        && rua_session::RuntimeNote::from_message_log_entry(message).is_some()
                }),
        );
    } else if !limit_runtime_notes_to_selected_variant {
        messages.extend(
            store
                .read_message_range(
                    chat_id,
                    turn.first_sequence,
                    (turn.last_sequence - turn.first_sequence + 1) as usize,
                )
                .await?
                .into_iter()
                .filter(|message| {
                    rua_session::RuntimeNote::from_message_log_entry(message).is_some()
                }),
        );
    }
    messages.sort_by_key(|message| message.sequence);
    messages.dedup_by_key(|message| message.sequence);
    Ok(Some((messages, selected_variant_id)))
}

fn message_to_new_conversation_message(message: MessageLogEntry) -> NewConversationMessage {
    NewConversationMessage {
        message_id: message.message_id,
        role: message.role,
        created_at: message.created_at,
        status: message.status,
        content: message.content,
        provider: message.provider,
        tool: message.tool,
        metadata: message.metadata,
    }
}

pub fn apply_agent_scope_defaults(header: &mut ChatSessionHeader, agent_id: &str) {
    header.extra.insert(
        SELECTED_AGENT_ID_KEY.to_string(),
        serde_json::Value::String(agent_id.to_string()),
    );
    header.extra.insert(
        CHAT_POLICY_KEY.to_string(),
        serde_json::to_value(ChatPolicy {
            agent_assignment: AgentAssignmentPolicy::Fixed,
            resource: ResourcePolicy::None,
            authority_placement: AuthorityPlacementPolicy::Selectable,
        })
        .expect("Chat Policy is serializable"),
    );
}

pub fn apply_project_scope_defaults(header: &mut ChatSessionHeader, selected_agent_id: &str) {
    header.extra.insert(
        SELECTED_AGENT_ID_KEY.to_string(),
        serde_json::Value::String(selected_agent_id.to_string()),
    );
    header.extra.insert(
        CHAT_POLICY_KEY.to_string(),
        serde_json::to_value(ChatPolicy {
            agent_assignment: AgentAssignmentPolicy::Selectable,
            resource: ResourcePolicy::ProjectBinding,
            authority_placement: AuthorityPlacementPolicy::ResourceBound,
        })
        .expect("Chat Policy is serializable"),
    );
}

/// The model selection used by both Chat views and execution. Once a complete
/// configuration exists, legacy header fields cannot override it, including an
/// absent selection. Explicit thinking settings have the same precedence as at
/// Turn claim.
pub fn model_slot_from_header(header: &ChatSessionHeader) -> Option<serde_json::Value> {
    let (model, thinking) = model_and_thinking_from_header(header);
    let mut slot = model?.as_object()?.clone();
    if let Some(thinking) = thinking.and_then(serde_json::Value::as_object) {
        slot.extend(thinking.clone());
    }
    Some(serde_json::Value::Object(slot))
}

fn model_and_thinking_from_header(
    header: &ChatSessionHeader,
) -> (Option<&serde_json::Value>, Option<&serde_json::Value>) {
    let configuration = header.extra.get("hook_configuration");
    let complete = configuration.is_some_and(|value| value.get("running_config").is_some());
    let field = |key: &str| {
        let value = configuration.and_then(|configuration| configuration.get(key));
        if complete {
            value
        } else {
            value.or_else(|| header.extra.get(key))
        }
        .filter(|value| !value.is_null())
    };
    (field(CHAT_MODEL_KEY), field(CHAT_THINKING_KEY))
}

pub fn capture_turn_model_and_thinking(
    model: Option<&serde_json::Value>,
    thinking: Option<&serde_json::Value>,
) -> (Option<serde_json::Value>, Option<serde_json::Value>) {
    let mut turn_model = serde_json::Map::new();
    let mut turn_thinking = serde_json::Map::new();
    if let Some(model) = model.and_then(serde_json::Value::as_object) {
        for key in ["provider_id", "model"] {
            if let Some(value) = model.get(key) {
                turn_model.insert(key.to_string(), value.clone());
            }
        }
        for (key, value) in model {
            if !matches!(key.as_str(), "provider_id" | "model") {
                turn_thinking.insert(key.clone(), value.clone());
            }
        }
    }
    let turn_thinking = match thinking {
        Some(serde_json::Value::Object(explicit)) => {
            turn_thinking.extend(explicit.clone());
            (!turn_thinking.is_empty()).then_some(serde_json::Value::Object(turn_thinking))
        }
        Some(explicit) => Some(explicit.clone()),
        None => (!turn_thinking.is_empty()).then_some(serde_json::Value::Object(turn_thinking)),
    };
    (
        (!turn_model.is_empty()).then_some(serde_json::Value::Object(turn_model)),
        turn_thinking,
    )
}

async fn project_header(scope_root: &Path, header: &ChatSessionHeader) -> Result<()> {
    let existing = chat_index::get(scope_root, &header.chat_id)
        .await
        .map_err(chat_index_error)?;
    let projection = project_chat_header(header, existing)?;
    chat_index::upsert(scope_root, projection)
        .await
        .map_err(chat_index_error)
}

/// Projects Session-owned fields while preserving index-only listing state.
pub fn project_chat_header(
    header: &ChatSessionHeader,
    existing: Option<chat_index::ChatIndexRecord>,
) -> Result<chat_index::ChatIndexRecord> {
    let created_at = parse_session_time(&header.metadata.created_at)?;
    let updated_at = parse_session_time(&header.metadata.last_activity_at)?;
    let mut projection = existing.unwrap_or_else(|| chat_index::ChatIndexRecord {
        id: header.chat_id.clone(),
        name: header.metadata.title.clone(),
        user_id: "default".to_string(),
        channel: "console".to_string(),
        created_at,
        updated_at,
        meta: serde_json::Value::Object(Default::default()),
        status: "idle".to_string(),
        pinned: header.metadata.pinned,
        pin_order: 0,
        tag: header.tag,
        archived: header.metadata.archived,
    });
    projection.id = header.chat_id.clone();
    projection.name = header.metadata.title.clone();
    projection.created_at = created_at;
    projection.updated_at = updated_at;
    let mut meta =
        projection
            .meta
            .as_object()
            .cloned()
            .ok_or_else(|| rua_core::RuaError::Validation {
                message: "Chat index metadata must be an object".into(),
            })?;
    meta.remove(rua_session::collaboration::COLLABORATION_META_KEY);
    meta.extend(header.extra.clone());
    projection.meta = serde_json::Value::Object(meta);
    projection.pinned = header.metadata.pinned;
    projection.tag = header.tag;
    projection.archived = header.metadata.archived;
    Ok(projection)
}

fn parse_session_time(value: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| rua_core::RuaError::Validation {
            message: format!("invalid Chat timestamp '{value}': {error}"),
        })
}

fn chat_index_error(message: String) -> rua_core::RuaError {
    rua_core::RuaError::Io { message }
}
