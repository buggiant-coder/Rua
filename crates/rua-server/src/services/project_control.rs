//! Resolve controls from the existing Chat's saved Project membership and authority placement.

use std::path::{Path, PathBuf};

use rua_core::{ChatAddress, ChatStore, ProjectId};
use rua_project::ProjectLifecycle;
use thiserror::Error;

use crate::services::conversation_authority_placement::ConversationAuthorityEpoch;
use crate::services::conversation_control_dispatcher::{
    dispatch_acp_control, dispatch_conversation_control,
};
use crate::services::device_command_bus::{
    AcpChatDeviceAction, AcpChatDeviceCommand, AcpChatDeviceResponse,
    ConversationControlDeviceAction, ConversationControlDeviceCommand,
    ConversationControlDeviceResponse, DeviceCommandDeliveryError,
};
use crate::services::device_context::current_sync_device_id;
use crate::state::AppState;

/// Failure to prepare or drive a Project control target.
///
/// The domain distinguishes what the caller can still act on; HTTP status mapping
/// stays with the endpoint.
#[derive(Debug, Error)]
pub enum ProjectControlError {
    #[error("Project not found: {project_id}")]
    ProjectNotFound { project_id: ProjectId },
    #[error("Archived Project cannot accept Chat controls")]
    ArchivedProject,
    #[error("{0}")]
    InvalidChatId(String),
    #[error("source Device identity is unavailable")]
    MissingDeviceIdentity,
    #[error("{0}")]
    Store(String),
    #[error("Project Chat not found: {chat_id}")]
    ProjectChatNotFound { chat_id: String },
    /// The authority Device rejected the control; the reason and detail are part of
    /// the device control contract.
    #[error("{detail}")]
    Rejected { reason: String, detail: String },
    /// The stored Chat authority placement is missing or invalid.
    #[error("{0}")]
    InvalidAuthorityState(String),
    /// The authority Device could not be reached or could not answer completely.
    #[error("{0}")]
    Unavailable(String),
    #[error("{0}")]
    UpgradeRequired(String),
    #[error("{0}")]
    UnexpectedResponse(String),
}

impl ProjectControlError {
    fn from_delivery(error: DeviceCommandDeliveryError) -> Self {
        if error.is_session_version_incompatible() {
            return Self::UpgradeRequired(error.to_string());
        }
        match error {
            DeviceCommandDeliveryError::Rejected { reason, detail } => {
                Self::Rejected { reason, detail }
            }
            other => Self::Unavailable(other.to_string()),
        }
    }
}

/// Prepared control target for one existing Project Chat.
///
/// The saved Chat belongs to the Active Project and identifies the Agent and authority Device.
#[derive(Debug, Clone)]
pub struct ProjectControlTarget {
    address: ChatAddress,
    scope_root: PathBuf,
    agent_id: String,
    source_device_id: String,
    authority_device_id: String,
    authority_epoch: ConversationAuthorityEpoch,
}

impl ProjectControlTarget {
    pub fn address(&self) -> &ChatAddress {
        &self.address
    }

    pub fn scope_root(&self) -> &Path {
        &self.scope_root
    }

    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    pub fn authority_epoch(&self) -> ConversationAuthorityEpoch {
        self.authority_epoch
    }

    pub async fn dispatch_control(
        &self,
        state: &AppState,
        action: ConversationControlDeviceAction,
    ) -> Result<ConversationControlDeviceResponse, ProjectControlError> {
        let command = ConversationControlDeviceCommand {
            command_id: format!("project-control-{}", uuid::Uuid::new_v4()),
            agent_id: self.agent_id.clone(),
            address: self.address.clone(),
            source_device_id: self.source_device_id.clone(),
            target_device_id: self.authority_device_id.clone(),
            authority_epoch: self.authority_epoch,
            action,
        };
        dispatch_conversation_control(&state.conversation_input_intake, command)
            .await
            .map_err(ProjectControlError::from_delivery)
    }

    pub async fn dispatch_acp(
        &self,
        state: &AppState,
        action: AcpChatDeviceAction,
    ) -> Result<AcpChatDeviceResponse, ProjectControlError> {
        let command = AcpChatDeviceCommand {
            command_id: format!("project-acp-{}", uuid::Uuid::new_v4()),
            agent_id: self.agent_id.clone(),
            address: self.address.clone(),
            source_device_id: self.source_device_id.clone(),
            target_device_id: self.authority_device_id.clone(),
            authority_epoch: self.authority_epoch,
            action,
        };
        dispatch_acp_control(&state.conversation_input_intake, command)
            .await
            .map_err(ProjectControlError::from_delivery)
    }
}

/// Resolve the control target of an existing Project Chat.
///
pub async fn resolve_project_control_target(
    state: &AppState,
    project_id: &str,
    chat_id: &str,
) -> Result<ProjectControlTarget, ProjectControlError> {
    match resolve_target(state, project_id, chat_id).await? {
        Some(target) => Ok(target),
        None => Err(ProjectControlError::ProjectChatNotFound {
            chat_id: chat_id.to_string(),
        }),
    }
}

/// Resolve the control target of a Project Chat that may not exist yet.
///
/// `Ok(None)` means the Chat is not materialized in the local replica. Read-only
/// operations that are meaningful without a Chat use this; control operations use
/// [`resolve_project_control_target`] instead.
pub async fn resolve_optional_project_control_target(
    state: &AppState,
    project_id: &str,
    chat_id: &str,
) -> Result<Option<ProjectControlTarget>, ProjectControlError> {
    resolve_target(state, project_id, chat_id).await
}

async fn resolve_target(
    state: &AppState,
    project_id: &str,
    chat_id: &str,
) -> Result<Option<ProjectControlTarget>, ProjectControlError> {
    let project_id = ProjectId::new(project_id);
    let project = state
        .project_catalog
        .get(&project_id)
        .map_err(|error| ProjectControlError::Store(error.to_string()))?
        .ok_or_else(|| ProjectControlError::ProjectNotFound {
            project_id: project_id.clone(),
        })?;
    if project.lifecycle != ProjectLifecycle::Active {
        return Err(ProjectControlError::ArchivedProject);
    }
    let chat_id = crate::services::console_chat::resolve_chat_id(chat_id)
        .map_err(ProjectControlError::InvalidChatId)?;
    let address = ChatAddress::new(&chat_id);
    let scope_root = ChatStore::resolve(&state.rua_root, address.clone())
        .root()
        .to_path_buf();
    let source_device_id = current_sync_device_id(&state.sync_device)
        .ok_or(ProjectControlError::MissingDeviceIdentity)?;
    let Some(header) =
        crate::services::chat_context::load_live_chat_header(&state.rua_root, &chat_id)
            .await
            .map_err(|e| ProjectControlError::Store(e.to_string()))?
    else {
        return Ok(None);
    };
    if header.project_id.as_deref() != Some(project_id.as_str()) {
        return Ok(None);
    }
    let placement =
        crate::services::conversation_authority_placement::parse_conversation_authority_placement(
            &serde_json::Value::Object(header.extra),
        )
        .map_err(|e| ProjectControlError::InvalidAuthorityState(e.to_string()))?;
    Ok(Some(ProjectControlTarget {
        address,
        scope_root,
        agent_id: header.agent_id,
        source_device_id,
        authority_device_id: placement.device_id,
        authority_epoch: placement.authority_epoch,
    }))
}
