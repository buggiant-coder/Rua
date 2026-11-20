use std::path::{Path, PathBuf};

use chrono::Utc;
use rua_chat::TurnAgentAssignment;
use rua_core::ChatAddress;
use rua_session::{ChatSessionHeader, WorkspaceSessionStore};
use serde::{Deserialize, Serialize};

use crate::services::chat_index;
use crate::services::conversation_authority::{
    ConversationInputPayload, FirstConversationSubmissionPersistenceContext,
};
use crate::services::conversation_authority_placement::ConversationAuthorityPlacement;

pub const PROJECT_WORKING_DIRECTORY_META_KEY: &str = "project_working_directory_id";

/// Existing Chats retain their original directory; only drafts inherit the primary.
pub fn working_directory(
    binding: &rua_project::ProjectResourceBinding,
    header: Option<&ChatSessionHeader>,
    draft_directory_id: Option<&str>,
) -> Result<PathBuf, String> {
    let id = match header {
        Some(header) => header
            .extra
            .get(PROJECT_WORKING_DIRECTORY_META_KEY)
            .and_then(serde_json::Value::as_str)
            .unwrap_or(rua_project::INITIAL_PROJECT_DIRECTORY_ID),
        None => draft_directory_id.unwrap_or(&binding.resources.primary_directory_id),
    };
    let directory = binding
        .resources
        .directory(id)
        .map_err(|error| error.to_string())?;
    rua_project::validate_project_directory(&directory.resource_root)
        .map_err(|error| error.to_string())?;
    Ok(directory.resource_root.clone())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatAgentSelection {
    pub agent_id: String,
    pub selected_at: String,
    pub revision: u64,
}

#[derive(Debug, Clone)]
pub struct ProjectChatMaterializationSnapshot {
    pub project_id: String,
    pub initial_model: Option<serde_json::Value>,
    pub configuration: crate::services::product_hooks::ChatHookConfiguration,
}

pub fn selection_from_assignment(assignment: &TurnAgentAssignment) -> ChatAgentSelection {
    ChatAgentSelection {
        agent_id: assignment.agent_id.clone(),
        selected_at: assignment.assigned_at.clone(),
        revision: 1,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatAgentSwitchEvent {
    pub event_id: String,
    pub previous_agent_id: String,
    pub selected_agent_id: String,
    pub source_device_id: String,
    pub committed_at: String,
    pub revision: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChatAgentSwitchBlocker {
    ActiveTurn,
    QueuedInput,
    UnresolvedApproval,
    UnansweredPrompt,
    ActiveAcpBinding,
    FixedCollaborationExecutor,
    ArchivedChat,
    ArchivedProject,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatAgentState {
    pub selection: ChatAgentSelection,
    pub selected_agent_available: bool,
    pub switchable: bool,
    #[serde(default)]
    pub blockers: Vec<ChatAgentSwitchBlocker>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectChatUpdate {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub pinned: Option<bool>,
    #[serde(default)]
    pub tag: Option<rua_chat::ChatTag>,
    #[serde(default)]
    pub pin_order: Option<i64>,
    #[serde(default)]
    pub model: Option<serde_json::Value>,
    #[serde(default)]
    pub collaboration_mode: Option<rua_core::AgentCollaborationMode>,
    #[serde(default)]
    pub archived: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectChatDeleteResult {
    pub chat_id: String,
    pub was_busy: bool,
    #[serde(default)]
    pub freed_bytes: u64,
    #[serde(default)]
    pub shared_media_preserved: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProjectChatDeleteError {
    NotFound(String),
    Busy(String),
    Pinned(String),
    Failed(String),
    Unavailable(String),
}

impl std::fmt::Display for ProjectChatDeleteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(detail)
            | Self::Busy(detail)
            | Self::Pinned(detail)
            | Self::Failed(detail)
            | Self::Unavailable(detail) => formatter.write_str(detail),
        }
    }
}

impl std::error::Error for ProjectChatDeleteError {}

pub async fn materialize_first_project_turn(
    scope_root: &Path,
    address: &ChatAddress,
    assignment: &TurnAgentAssignment,
    input: &ConversationInputPayload,
    persistence_context: &FirstConversationSubmissionPersistenceContext,
    placement: &ConversationAuthorityPlacement,
    snapshot: ProjectChatMaterializationSnapshot,
) -> Result<(), String> {
    let media_ids = crate::services::first_chat_submission::validate_submission_media(
        scope_root,
        address,
        Some(&snapshot.project_id),
        input,
    )
    .await?;
    materialize_staged_project_chat(scope_root, address, |staging_root| async move {
        stage_first_turn(
            &staging_root,
            address,
            assignment,
            input,
            persistence_context,
            placement,
            snapshot,
        )
        .await
    })
    .await?;
    if let Err(error) =
        crate::services::chat_media::ChatMediaService::confirm_staged_media(scope_root, &media_ids)
            .await
    {
        tracing::warn!(%error, "accepted Project Chat media staging marker retained");
    }
    Ok(())
}

#[doc(hidden)]
pub async fn persist_first_project_submission(
    scope_root: &Path,
    address: &ChatAddress,
    assignment: &TurnAgentAssignment,
    input: &ConversationInputPayload,
    persistence_context: &FirstConversationSubmissionPersistenceContext,
    placement: &ConversationAuthorityPlacement,
    snapshot: ProjectChatMaterializationSnapshot,
) -> Result<(), String> {
    let store = WorkspaceSessionStore::new(scope_root);
    let indexed = chat_index::get(scope_root, address.chat_id().as_str()).await?;
    let header = store
        .load_chat_header(address.chat_id().as_str())
        .await
        .map_err(|error| error.to_string())?;
    match (indexed, header) {
        (None, None) => {
            materialize_first_project_turn(
                scope_root,
                address,
                assignment,
                input,
                persistence_context,
                placement,
                snapshot,
            )
            .await
        }
        (Some(_), Some(header)) => {
            if header.project_id.as_deref() != Some(snapshot.project_id.as_str()) {
                return Err("Chat already belongs to a different Project".into());
            }
            let winner_placement =
                crate::services::conversation_authority_placement::parse_conversation_authority_placement(
                    &serde_json::Value::Object(header.extra.clone()),
                )
                .map_err(|error| error.to_string())?;
            if winner_placement.device_id != placement.device_id
                || winner_placement.base_manifest_hash != placement.base_manifest_hash
            {
                return Err("Project Chat was materialized by a different target".to_string());
            }
            let winner = header
                .extra
                .get(crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY)
                .cloned()
                .ok_or_else(|| "Chat configuration snapshot is unavailable".to_string())
                .and_then(crate::services::product_hooks::ChatHookConfiguration::from_persisted)?;
            let winner_assignment = winner
                .agent_assignment
                .clone()
                .ok_or_else(|| "Project Turn Agent Assignment is unavailable".to_string())?;
            let effective_input =
                crate::services::first_chat_submission::input_with_configuration(input, &winner)?;
            crate::services::first_chat_submission::append_accepted_submission(
                scope_root,
                address,
                &winner_assignment,
                &effective_input,
                persistence_context,
                &winner_placement,
            )
            .await
        }
        _ => Err("Project Chat storage and index are inconsistent".to_string()),
    }
}

pub async fn materialize_project_chat_draft(
    scope_root: &Path,
    address: &ChatAddress,
    selection: &ChatAgentSelection,
    placement: &ConversationAuthorityPlacement,
    snapshot: ProjectChatMaterializationSnapshot,
) -> Result<(), String> {
    materialize_staged_project_chat(scope_root, address, |staging_root| async move {
        stage_project_chat_draft(&staging_root, address, selection, placement, snapshot).await
    })
    .await
}

async fn materialize_staged_project_chat<F, Fut>(
    scope_root: &Path,
    address: &ChatAddress,
    stage: F,
) -> Result<(), String>
where
    F: FnOnce(PathBuf) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    crate::services::first_chat_submission::materialize_staged_chat(
        scope_root,
        address,
        |staging_root| async move {
            stage(staging_root.clone()).await?;
            let header = WorkspaceSessionStore::new(&staging_root)
                .load_chat_header(address.chat_id().as_str())
                .await
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "staged Project Chat header is missing".to_string())?;
            rua_chat::project_chat_header(&header, None).map_err(|error| error.to_string())
        },
    )
    .await
}

pub async fn update_project_chat(
    scope_root: &Path,
    address: &ChatAddress,
    update: ProjectChatUpdate,
) -> Result<crate::endpoints::chats::ChatSpecDto, String> {
    if update.name.is_none()
        && update.pinned.is_none()
        && update.tag.is_none()
        && update.pin_order.is_none()
        && update.model.is_none()
        && update.collaboration_mode.is_none()
        && update.archived.is_none()
    {
        return Err("Project Chat update is empty".to_string());
    }
    if update.model.is_some() {
        return Err("Project Chat model updates must use Conversation Control".to_string());
    }
    let chat_service =
        rua_chat::ChatService::from_workspace(rua_core::WorkspaceRoot::new(scope_root));
    let common_chat = chat_service
        .update_listing(
            address,
            rua_chat::ChatListUpdate {
                title: update.name.clone(),
                pinned: update.pinned,
                pin_order: update.pin_order,
                tag: update.tag,
                archived: update.archived,
            },
        )
        .await
        .map_err(|error| error.to_string())?;
    let mut chat = crate::endpoints::chats::chat_spec_from_common_chat(scope_root, &common_chat)
        .await
        .map_err(|error| error.to_string())?;
    let Some(collaboration_mode) = update.collaboration_mode else {
        return Ok(chat);
    };
    chat.collaboration_mode = collaboration_mode;
    let store = WorkspaceSessionStore::new(scope_root);
    let mut header = store
        .load_chat_header(address.chat_id().as_str())
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("Project Chat header not found: {}", address.chat_id()))?;
    let previous_epoch = header
        .extra
        .get(crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY)
        .and_then(|value| value.get("epoch"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    header.extra.insert(
        rua_core::CHAT_COLLABORATION_MODE_META_KEY.to_string(),
        serde_json::json!(chat.collaboration_mode),
    );
    if let Some(configuration) = header
        .extra
        .get_mut(crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY)
        .and_then(serde_json::Value::as_object_mut)
    {
        configuration.insert(
            "collaboration_mode".to_string(),
            serde_json::json!(chat.collaboration_mode),
        );
        configuration.insert(
            "epoch".to_string(),
            serde_json::json!(format!("hook-config-{}", uuid::Uuid::new_v4())),
        );
    }
    header.metadata.last_activity_at = chat.updated_at.to_rfc3339();
    if let (Some(epoch), Some(configuration)) = (
        previous_epoch.as_deref(),
        header
            .extra
            .get(crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY),
    ) {
        store
            .replace_chat_configuration(address.chat_id().as_str(), epoch, configuration.clone())
            .await
            .map_err(|error| error.to_string())?;
    }
    store
        .save_chat_header(&header)
        .await
        .map_err(|error| error.to_string())?;
    chat_index::upsert(
        scope_root,
        crate::endpoints::chats::chat_to_index_entry(&chat),
    )
    .await?;
    Ok(chat)
}

pub async fn remove_project_chat_storage(
    scope_root: &Path,
    address: &ChatAddress,
) -> Result<crate::endpoints::chats::ChatSpecDto, ProjectChatDeleteError> {
    let chat = crate::endpoints::chats::chat_from_index_entry(
        chat_index::get(scope_root, address.chat_id().as_str())
            .await
            .map_err(ProjectChatDeleteError::Failed)?
            .ok_or_else(|| {
                ProjectChatDeleteError::NotFound(format!(
                    "Project Chat not found: {}",
                    address.chat_id()
                ))
            })?,
    );
    WorkspaceSessionStore::new(scope_root)
        .delete_chat_storage_by_chat_id(&chat.id)
        .await
        .map_err(|error| ProjectChatDeleteError::Failed(error.to_string()))?;
    chat_index::delete(scope_root, address.chat_id().as_str())
        .await
        .map_err(ProjectChatDeleteError::Failed)?;
    Ok(chat)
}

async fn stage_first_turn(
    staging_root: &Path,
    address: &ChatAddress,
    assignment: &TurnAgentAssignment,
    input: &ConversationInputPayload,
    persistence_context: &FirstConversationSubmissionPersistenceContext,
    placement: &ConversationAuthorityPlacement,
    snapshot: ProjectChatMaterializationSnapshot,
) -> Result<(), String> {
    stage_project_chat_header(
        &WorkspaceSessionStore::new(staging_root),
        address,
        &assignment.agent_id,
        &input.message,
        placement,
        snapshot,
    )
    .await?;
    crate::services::first_chat_submission::append_accepted_submission(
        staging_root,
        address,
        assignment,
        input,
        persistence_context,
        placement,
    )
    .await?;

    let store = WorkspaceSessionStore::new(staging_root);
    if let Some(directory_id) = input.extra.get(PROJECT_WORKING_DIRECTORY_META_KEY) {
        store
            .update_chat_header(address.chat_id().as_str(), |header| {
                header.extra.insert(
                    PROJECT_WORKING_DIRECTORY_META_KEY.into(),
                    directory_id.clone(),
                );
            })
            .await
            .map_err(|error| error.to_string())?;
    }

    if let Some(selection) = input
        .extra
        .get(crate::services::conversation_input_intake::INITIAL_ACP_KEY)
    {
        store
            .update_chat_header(address.chat_id().as_str(), |header| {
                header.extra.insert(
                    crate::services::conversation_input_intake::INITIAL_ACP_KEY.into(),
                    selection.clone(),
                );
            })
            .await
            .map_err(|error| error.to_string())?;
    }
    let persisted_header = store
        .load_chat_header(address.chat_id().as_str())
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "staged Project Chat header is missing".to_string())?;
    let turns = store
        .read_turn_range(address.chat_id().as_str(), 1, 2)
        .await
        .map_err(|error| error.to_string())?;
    if persisted_header.turn_index.latest_turn != 1
        || persisted_header.message_log.latest_sequence != 1
        || turns.len() != 1
        || turns[0].turn_id != persistence_context.turn_id
    {
        return Err("staged Project Chat first Turn failed validation".to_string());
    }
    Ok(())
}

async fn stage_project_chat_draft(
    staging_root: &Path,
    address: &ChatAddress,
    selection: &ChatAgentSelection,
    placement: &ConversationAuthorityPlacement,
    snapshot: ProjectChatMaterializationSnapshot,
) -> Result<(), String> {
    let store = WorkspaceSessionStore::new(staging_root);
    stage_project_chat_header(
        &store,
        address,
        &selection.agent_id,
        "",
        placement,
        snapshot,
    )
    .await?;
    let persisted_header = store
        .load_chat_header(address.chat_id().as_str())
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "staged Project Chat header is missing".to_string())?;
    if persisted_header.turn_index.latest_turn != 0
        || persisted_header.message_log.latest_sequence != 0
    {
        return Err("staged Project Chat draft unexpectedly contains a Turn".to_string());
    }
    Ok(())
}

async fn stage_project_chat_header(
    store: &WorkspaceSessionStore,
    address: &ChatAddress,
    agent_id: &str,
    title: &str,
    placement: &ConversationAuthorityPlacement,
    snapshot: ProjectChatMaterializationSnapshot,
) -> Result<(), String> {
    let chat = crate::services::chat_metadata_store::new_targeted_chat_metadata(
        address,
        title,
        placement,
        snapshot.initial_model,
        Utc::now(),
    )
    .map_err(|error| error.to_string())?;
    let mut header =
        crate::services::chat_metadata_store::chat_header_from_metadata_record(&chat, agent_id);
    header.project_id = Some(snapshot.project_id);
    rua_chat::apply_chat_defaults(&mut header);
    header.extra.insert(
        "hook_configuration".into(),
        serde_json::to_value(snapshot.configuration).map_err(|error| error.to_string())?,
    );
    store
        .save_chat_header(&header)
        .await
        .map_err(|error| error.to_string())
}

pub fn selection_from_header(header: &ChatSessionHeader) -> Result<ChatAgentSelection, String> {
    let selected_agent_id = header.agent_id.trim();
    if selected_agent_id.is_empty() {
        return Err("Chat selected Agent is missing".into());
    }
    let events: Vec<ChatAgentSwitchEvent> = header
        .extra
        .get(rua_session::AGENT_SWITCH_EVENTS_KEY)
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| format!("invalid Chat Agent switch audit log: {error}"))?
        .unwrap_or_default();
    let (selected_at, revision) = match events.last() {
        Some(event) => {
            if event.selected_agent_id != selected_agent_id {
                return Err("Chat selected Agent does not match its switch audit log".into());
            }
            (event.committed_at.clone(), event.revision)
        }
        None => (header.metadata.created_at.clone(), 1),
    };
    Ok(ChatAgentSelection {
        agent_id: selected_agent_id.to_string(),
        selected_at,
        revision,
    })
}

pub async fn load_chat_agent_selection(
    scope_root: &Path,
    chat_id: &str,
) -> Result<ChatAgentSelection, String> {
    let header = WorkspaceSessionStore::new(scope_root)
        .load_chat_header(chat_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("Project Chat not found: {chat_id}"))?;
    selection_from_header(&header)
}

impl ChatAgentState {
    pub(crate) fn check_switch(
        &self,
        expected_agent_id: &str,
        next_agent_id: &str,
    ) -> Result<(), String> {
        if self.selection.agent_id != expected_agent_id {
            return Err(format!(
                "Chat Agent selection changed from '{expected_agent_id}' to '{}'",
                self.selection.agent_id
            ));
        }
        if self.selection.agent_id != next_agent_id && !self.switchable {
            return Err(format!(
                "Chat Agent switch blocked by {}",
                serde_json::json!(self.blockers)
            ));
        }
        Ok(())
    }
}

/// Agent replacement shares admission with foreground input, configuration updates and approvals.
pub struct ChatAgentSwitch<'a> {
    pub(crate) header: ChatSessionHeader,
    pub(crate) state: ChatAgentState,
    next_agent_id: &'a str,
    scope_root: &'a Path,
    address: &'a ChatAddress,
    _transition: tokio::sync::OwnedMutexGuard<()>,
    _approvals: tokio::sync::OwnedMutexGuard<()>,
}

impl<'a> ChatAgentSwitch<'a> {
    pub async fn begin(
        scope_root: &'a Path,
        address: &'a ChatAddress,
        expected_agent_id: &str,
        next_agent_id: &'a str,
        authority: &crate::services::conversation_authority::ConversationAuthority,
        approvals: &rua_security::ApprovalAuthority,
    ) -> Result<Self, String> {
        let transition = authority.transition_gate(address).lock_owned().await;
        let registrations = approvals.hold_chat_registrations(address).await;
        let header = WorkspaceSessionStore::new(scope_root)
            .load_chat_header(address.chat_id().as_str())
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Chat not found: {}", address.chat_id()))?;
        let state =
            chat_agent_state_from_header(address, &header, true, authority, approvals).await?;
        state.check_switch(expected_agent_id, next_agent_id)?;
        Ok(Self {
            header,
            state,
            scope_root,
            address,
            next_agent_id,
            _transition: transition,
            _approvals: registrations,
        })
    }

    pub async fn commit(
        &self,
        configuration: &crate::services::product_hooks::ChatHookConfiguration,
        source_device_id: &str,
    ) -> Result<ChatAgentState, String> {
        configuration.validate()?;
        let header = &self.header;
        let current = &self.state.selection;
        if configuration.selected_agent_id != self.next_agent_id {
            return Err("Chat configuration does not match the requested Agent".into());
        }
        if current.agent_id == configuration.selected_agent_id {
            return Ok(self.state.clone());
        }
        let store = WorkspaceSessionStore::new(self.scope_root);
        let committed_at = Utc::now().to_rfc3339();
        let next = ChatAgentSelection {
            agent_id: configuration.selected_agent_id.clone(),
            selected_at: committed_at.clone(),
            revision: current.revision.saturating_add(1),
        };
        let event = ChatAgentSwitchEvent {
            event_id: format!("chat-agent-switch-{}", uuid::Uuid::new_v4()),
            previous_agent_id: current.agent_id.clone(),
            selected_agent_id: next.agent_id.clone(),
            source_device_id: source_device_id.to_string(),
            committed_at,
            revision: next.revision,
        };
        let configuration_key = crate::services::product_hooks::CHAT_HOOK_CONFIGURATION_META_KEY;
        let expected_epoch = header
            .extra
            .get(configuration_key)
            .and_then(|value| value.get("epoch"))
            .and_then(serde_json::Value::as_str);
        store
            .commit_chat_agent_switch(
                self.address.chat_id().as_str(),
                &current.agent_id,
                expected_epoch,
                &next.agent_id,
                serde_json::to_value(event).map_err(|error| error.to_string())?,
                &serde_json::to_value(configuration).map_err(|error| error.to_string())?,
            )
            .await
            .map_err(|error| error.to_string())?;
        Ok(ChatAgentState {
            selection: next,
            selected_agent_available: true,
            switchable: true,
            blockers: vec![],
        })
    }
}

pub async fn chat_agent_state(
    scope_root: &Path,
    address: &ChatAddress,
    selected_agent_available: bool,
    authority: &crate::services::conversation_authority::ConversationAuthority,
    approval_authority: &rua_security::ApprovalAuthority,
) -> Result<ChatAgentState, String> {
    let header = WorkspaceSessionStore::new(scope_root)
        .load_chat_header(address.chat_id().as_str())
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("Project Chat not found: {}", address.chat_id()))?;
    chat_agent_state_from_header(
        address,
        &header,
        selected_agent_available,
        authority,
        approval_authority,
    )
    .await
}

pub(crate) async fn chat_agent_state_from_header(
    address: &ChatAddress,
    header: &ChatSessionHeader,
    selected_agent_available: bool,
    authority: &crate::services::conversation_authority::ConversationAuthority,
    approval_authority: &rua_security::ApprovalAuthority,
) -> Result<ChatAgentState, String> {
    let selection = selection_from_header(header)?;
    let blockers = chat_agent_switch_blockers(address, header, authority, approval_authority).await;
    Ok(ChatAgentState {
        selection,
        selected_agent_available,
        switchable: blockers.is_empty(),
        blockers,
    })
}

async fn chat_agent_switch_blockers(
    address: &ChatAddress,
    header: &ChatSessionHeader,
    authority: &crate::services::conversation_authority::ConversationAuthority,
    approval_authority: &rua_security::ApprovalAuthority,
) -> Vec<ChatAgentSwitchBlocker> {
    let mut blockers = Vec::new();
    if authority.active_turn(address).await.is_some() {
        blockers.push(ChatAgentSwitchBlocker::ActiveTurn);
    }
    if !authority.queue_snapshot(address).await.is_empty() {
        blockers.push(ChatAgentSwitchBlocker::QueuedInput);
    }
    if !approval_authority
        .list_pending_for_address(address)
        .await
        .is_empty()
    {
        blockers.push(ChatAgentSwitchBlocker::UnresolvedApproval);
    }
    if rua_tool::extended::ask_user::AskUserService::global()
        .get_pending_by_session(address.chat_id().as_str())
        .await
        .is_some()
    {
        blockers.push(ChatAgentSwitchBlocker::UnansweredPrompt);
    }
    if crate::services::conversation_authority_placement::chat_acp_binding_active(
        &serde_json::Value::Object(header.extra.clone()),
    ) {
        blockers.push(ChatAgentSwitchBlocker::ActiveAcpBinding);
    }
    if header
        .extra
        .contains_key(rua_session::collaboration::COLLABORATION_META_KEY)
    {
        blockers.push(ChatAgentSwitchBlocker::FixedCollaborationExecutor);
    }
    if header.metadata.archived {
        blockers.push(ChatAgentSwitchBlocker::ArchivedChat);
    }
    blockers
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectBulkArchiveResult {
    pub items: Vec<ProjectChatArchiveItem>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectChatArchiveItem {
    pub chat_id: String,
    pub status: ProjectChatArchiveStatus,
    pub reason: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectChatArchiveStatus {
    Archived,
    Skipped,
    Failed,
}
