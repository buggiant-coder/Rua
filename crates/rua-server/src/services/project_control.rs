//! Project control target preparation.
//!
//! A Project Chat is a fixed-resource conversation: its Conversation Authority is
//! always the Project's resource Device, its Agent Selection belongs to the Chat,
//! and its authority Epoch is only known from the resource Device. Callers express
//! a Project id plus a Chat id and receive the complete typed control target; they
//! never assemble discovery, materialization, or placement steps themselves.
//!
//! Preparation is strictly read-only. It never creates a Chat, never writes a Chat
//! header or configuration, and never installs an authority placement: an
//! unmaterialized Project Chat stays absent, and materialization belongs to the
//! first accepted input. The initial zero-Epoch discovery envelope is a read-only
//! protocol detail, not an authority grant.

use std::path::{Path, PathBuf};

use rua_core::{ChatAddress, ChatScopeStore, ProjectId};
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

/// Reason the resource Device reports for a Project Chat that has no Chat state.
const PROJECT_CHAT_NOT_FOUND_REASON: &str = "project_chat_not_found";

const PROJECT_AUTHORITY_FIXED_DETAIL: &str =
    "Project Chat authority is fixed to its resource Device";

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
    /// The resource Device rejected the control; the reason and detail are part of
    /// the device control contract.
    #[error("{detail}")]
    Rejected { reason: String, detail: String },
    /// The resource Device answered with a control view that contradicts the
    /// Project fixed-resource rule.
    #[error("{0}")]
    InvalidAuthorityState(String),
    /// The resource Device could not be reached or could not answer completely.
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
            // A Project Chat never follows a redirect, so a redirect here can only
            // mean the responder does not honour its fixed resource Device.
            DeviceCommandDeliveryError::AuthorityRedirect(_) => Self::Rejected {
                reason: "project_authority_fixed".to_string(),
                detail: PROJECT_AUTHORITY_FIXED_DETAIL.to_string(),
            },
            other => Self::Unavailable(other.to_string()),
        }
    }
}

/// Prepared control target for one existing Project Chat.
///
/// Holding this value means the Project is Active, the Chat exists on its resource
/// Device, and the resource Device confirmed the fixed authority placement for the
/// selected Agent.
#[derive(Debug, Clone)]
pub struct ProjectControlTarget {
    address: ChatAddress,
    scope_root: PathBuf,
    agent_id: String,
    source_device_id: String,
    resource_device_id: String,
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
            target_device_id: self.resource_device_id.clone(),
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
            target_device_id: self.resource_device_id.clone(),
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
/// `Ok(None)` means the Chat is not materialized on its resource Device. Read-only
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
    let address = ChatAddress::project(project_id, &chat_id);
    let scope_root = ChatScopeStore::resolve(&state.rua_root, address.clone())
        .root()
        .to_path_buf();
    let source_device_id = current_sync_device_id(&state.sync_device)
        .ok_or(ProjectControlError::MissingDeviceIdentity)?;
    let command = ConversationControlDeviceCommand {
        command_id: format!("project-control-discover-{}", uuid::Uuid::new_v4()),
        agent_id: String::new(),
        address: address.clone(),
        source_device_id: source_device_id.clone(),
        target_device_id: project.resource_device_id.clone(),
        authority_epoch: ConversationAuthorityEpoch::from_parts(0, 0, [0; 8]),
        action: ConversationControlDeviceAction::Discover,
    };
    let response =
        match dispatch_conversation_control(&state.conversation_input_intake, command).await {
            Ok(response) => response,
            Err(DeviceCommandDeliveryError::Rejected { reason, .. })
                if reason == PROJECT_CHAT_NOT_FOUND_REASON =>
            {
                return Ok(None)
            }
            Err(error) => return Err(ProjectControlError::from_delivery(error)),
        };
    let ConversationControlDeviceResponse::View {
        control,
        execution_agent_id,
        ..
    } = response
    else {
        return Err(ProjectControlError::UnexpectedResponse(
            "Project control discovery returned an unexpected response".to_string(),
        ));
    };
    let authority_device_id = control
        .placement
        .authority_device_id
        .as_deref()
        .or(control.authority_device_id.as_deref())
        .ok_or_else(|| {
            ProjectControlError::Unavailable(
                "Project control discovery did not identify an authority Device".to_string(),
            )
        })?;
    if authority_device_id != project.resource_device_id {
        return Err(ProjectControlError::InvalidAuthorityState(
            "Project Chat authority does not match its resource Device".to_string(),
        ));
    }
    if control.placement.switchable {
        return Err(ProjectControlError::InvalidAuthorityState(
            "Project Chat authority must not be switchable".to_string(),
        ));
    }
    let authority_epoch = control.placement.authority_epoch.ok_or_else(|| {
        ProjectControlError::Unavailable(
            "Project control discovery did not identify an authority Epoch".to_string(),
        )
    })?;
    let agent_id = execution_agent_id
        .filter(|agent_id| !agent_id.trim().is_empty())
        .ok_or_else(|| {
            ProjectControlError::Unavailable(
                "Project control discovery did not identify the selected Agent".to_string(),
            )
        })?;
    Ok(Some(ProjectControlTarget {
        address,
        scope_root,
        agent_id,
        source_device_id,
        resource_device_id: project.resource_device_id,
        authority_epoch,
    }))
}
