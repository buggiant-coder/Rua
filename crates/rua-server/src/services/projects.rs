use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rua_core::ProjectId;
use rua_project::{
    ProjectCreateResourcesRequest, ProjectCreationOutcome, ProjectDeletePreparation,
    ProjectDescriptor, ProjectDirectoryListing, ProjectResourceDirectoryInput,
};
use thiserror::Error;

use crate::services::device_command_bus::{
    deliver_project_device_command_via_session, ProjectDeviceAction, ProjectDeviceCommand,
    ProjectDeviceResponse, ProjectResourceOperation,
};
use crate::services::device_context::current_sync_device_id;
use crate::services::project_lifecycle::{
    ProjectArchiveResult, ProjectLifecyclePreview, ProjectPermanentDeleteResult,
    ProjectReopenResult,
};
use crate::state::AppState;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BrowseDirectoryResponse {
    pub device_id: String,
    pub path: std::path::PathBuf,
    pub roots: Vec<rua_project::ProjectDirectoryEntry>,
    pub parent_path: Option<std::path::PathBuf>,
    pub entries: Vec<rua_project::ProjectDirectoryEntry>,
    #[serde(default)]
    pub omitted_entries: usize,
    pub proposed_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<usize>,
}

impl BrowseDirectoryResponse {
    pub(crate) fn from_listing(device_id: String, listing: ProjectDirectoryListing) -> Self {
        Self {
            device_id,
            next_offset: None,
            parent_path: listing.path.parent().map(Path::to_owned),
            proposed_name: rua_project::propose_project_display_name(&listing.path),
            path: listing.path,
            entries: listing.entries,
            omitted_entries: listing.omitted_entries,
            roots: listing.roots,
        }
    }
}

const PROJECT_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Error)]
pub enum ProjectPlacementError {
    #[error("Project not found: {0}")]
    NotFound(String),
    #[error("local Device identity is unavailable")]
    MissingLocalDevice,
    #[error("Project Device command is unavailable: {0}")]
    Unavailable(String),
    #[error("{0}")]
    OutcomeUnconfirmed(String),
    #[error("Project request was rejected: {0}")]
    Rejected(String),
    #[error("invalid Project request: {0}")]
    Invalid(String),
    #[error("Project conflict: {0}")]
    Conflict(String),
    #[error("Project storage failed: {0}")]
    Store(String),
    #[error("unexpected Project Device response")]
    UnexpectedResponse,
}

#[derive(Clone)]
pub struct ProjectPlacementService {
    local_device_id: String,
    catalog: Arc<rua_project::ProjectCatalog>,
    device_network: Arc<crate::services::device_session_access::DeviceNetworkAccess>,
}

impl ProjectPlacementService {
    pub fn from_state(state: &AppState) -> Result<Self, ProjectPlacementError> {
        let local_device_id = current_sync_device_id(&state.sync_device)
            .ok_or(ProjectPlacementError::MissingLocalDevice)?;
        Ok(Self {
            local_device_id,
            catalog: Arc::clone(&state.project_catalog),
            device_network: Arc::clone(&state.device_network),
        })
    }

    /// Only newly associated locations require a read on their owning Device.
    async fn validate_locations(
        &self,
        existing: &[rua_project::ProjectResourceDirectory],
        directories: &mut [ProjectResourceDirectoryInput],
    ) -> Result<(), ProjectPlacementError> {
        for directory in directories {
            if existing.iter().any(|old| {
                old.device_id == directory.device_id
                    && old.resource_root.as_os_str() == directory.resource_root.as_os_str()
                    && directory
                        .directory_id
                        .as_ref()
                        .is_none_or(|id| id == &old.directory_id)
            }) {
                continue;
            }
            if directory.device_id.trim().is_empty() {
                return Err(ProjectPlacementError::Invalid(
                    "A resource requires a Device".into(),
                ));
            }
            directory.resource_root = if directory.device_id == self.local_device_id {
                validate_resource_directory(directory.resource_root.clone())
                    .await
                    .map_err(ProjectPlacementError::Invalid)?
            } else {
                match self
                    .deliver(
                        &directory.device_id,
                        ProjectDeviceAction::ValidateResourceDirectory {
                            resource_root: directory.resource_root.clone(),
                        },
                    )
                    .await?
                {
                    ProjectDeviceResponse::ValidatedResourceDirectory {
                        device_id,
                        resource_root,
                    } if device_id == directory.device_id => resource_root,
                    _ => return Err(ProjectPlacementError::UnexpectedResponse),
                }
            };
        }
        Ok(())
    }

    pub async fn edit_resources(
        &self,
        project_id: &ProjectId,
        expected_revision: u64,
        mut directories: Vec<ProjectResourceDirectoryInput>,
        state: &AppState,
    ) -> Result<ProjectCreationOutcome, ProjectPlacementError> {
        let project = self
            .catalog
            .get(project_id)
            .map_err(map_project_error)?
            .ok_or_else(|| ProjectPlacementError::NotFound(project_id.to_string()))?;
        if project.resources.revision != expected_revision {
            return Err(map_project_error(
                rua_project::ProjectError::ResourceRevisionConflict,
            ));
        }
        self.validate_locations(&project.resources.directories, &mut directories)
            .await?;
        let outcome = self
            .catalog
            .replace_resources(project_id, expected_revision, directories)
            .map_err(map_project_error)?;
        publish_project_descriptor(state, &outcome.project).await;
        Ok(outcome)
    }

    pub async fn browse_directory(
        &self,
        resource_device_id: &str,
        path: &str,
    ) -> Result<ProjectDirectoryListing, ProjectPlacementError> {
        if resource_device_id == self.local_device_id {
            return rua_project::list_project_directory(path).map_err(map_project_error);
        }
        let response = self
            .deliver(
                resource_device_id,
                ProjectDeviceAction::BrowseDirectory {
                    path: path.to_string(),
                },
            )
            .await?;
        match response {
            ProjectDeviceResponse::Directory {
                path,
                entries,
                roots,
                omitted_entries,
                device_id,
                ..
            } if device_id == resource_device_id => Ok(ProjectDirectoryListing {
                path,
                entries,
                roots,
                omitted_entries,
            }),
            _ => Err(ProjectPlacementError::UnexpectedResponse),
        }
    }

    pub async fn create_directory(
        &self,
        resource_device_id: &str,
        parent: &str,
        name: &str,
    ) -> Result<ProjectDirectoryListing, ProjectPlacementError> {
        if resource_device_id == self.local_device_id {
            return rua_project::create_project_directory(parent, name).map_err(map_project_error);
        }
        match self
            .deliver(
                resource_device_id,
                ProjectDeviceAction::CreateDirectory {
                    parent: parent.to_string(),
                    name: name.to_string(),
                },
            )
            .await?
        {
            ProjectDeviceResponse::Directory {
                path,
                entries,
                roots,
                omitted_entries,
                device_id,
                ..
            } if device_id == resource_device_id => Ok(ProjectDirectoryListing {
                path,
                entries,
                roots,
                omitted_entries,
            }),
            _ => Err(ProjectPlacementError::UnexpectedResponse),
        }
    }

    pub async fn browse_execution_directory(
        &self,
        device: &str,
        path: &str,
    ) -> Result<BrowseDirectoryResponse, ProjectPlacementError> {
        if device == self.local_device_id {
            let listing = crate::services::execution_directory::browse(path)
                .await
                .map_err(ProjectPlacementError::Invalid)?;
            return Ok(BrowseDirectoryResponse::from_listing(
                device.into(),
                listing,
            ));
        }
        match self
            .deliver(
                device,
                ProjectDeviceAction::BrowseExecutionDirectory { path: path.into() },
            )
            .await?
        {
            ProjectDeviceResponse::ExecutionDirectoryListing { listing }
                if listing.device_id == device =>
            {
                Ok(listing)
            }
            _ => Err(ProjectPlacementError::UnexpectedResponse),
        }
    }

    async fn validate_execution_default(
        &self,
        selection: Option<&rua_core::ExecutionDirectorySelection>,
        state: &AppState,
    ) -> Result<(), ProjectPlacementError> {
        let Some(selection) = selection else {
            return Ok(());
        };
        selection
            .validate()
            .map_err(|error| ProjectPlacementError::Invalid(error.to_string()))?;
        let Some(path) = selection.path.as_ref() else {
            if selection.device_id != self.local_device_id
                && !crate::services::device_context::known_devices_for_runtime(
                    &state.rua_root,
                    Some(&self.local_device_id),
                )
                .iter()
                .any(|d| d["device_id"] == selection.device_id)
            {
                return Err(ProjectPlacementError::Invalid(
                    "Execution Device is not paired".into(),
                ));
            }
            return Ok(());
        };
        if selection.device_id == self.local_device_id {
            crate::services::execution_directory::validate(path, &state.rua_root)
                .await
                .map_err(ProjectPlacementError::Invalid)?;
            return Ok(());
        }
        match self
            .deliver(
                &selection.device_id,
                ProjectDeviceAction::ValidateExecutionDirectory { path: path.clone() },
            )
            .await?
        {
            ProjectDeviceResponse::ValidatedExecutionDirectory {
                device_id,
                path: checked,
            } if device_id == selection.device_id && checked.as_os_str() == path.as_os_str() => {
                Ok(())
            }
            _ => Err(ProjectPlacementError::UnexpectedResponse),
        }
    }

    pub async fn set_execution_default(
        &self,
        id: &ProjectId,
        selection: Option<rua_core::ExecutionDirectorySelection>,
        state: &AppState,
    ) -> Result<ProjectDescriptor, ProjectPlacementError> {
        self.validate_execution_default(selection.as_ref(), state)
            .await?;
        let project = self
            .catalog
            .set_execution_default(id, selection)
            .map_err(map_project_error)?;
        publish_project_descriptor(state, &project).await;
        Ok(project)
    }

    pub async fn create(
        &self,
        mut directories: Vec<ProjectResourceDirectoryInput>,
        display_name: Option<String>,
        execution_default: Option<rua_core::ExecutionDirectorySelection>,
        state: &AppState,
    ) -> Result<ProjectCreationOutcome, ProjectPlacementError> {
        self.validate_locations(&[], &mut directories).await?;
        self.validate_execution_default(execution_default.as_ref(), state)
            .await?;
        let outcome = self
            .catalog
            .create_resources(ProjectCreateResourcesRequest {
                display_name,
                directories,
                execution_default,
            })
            .map_err(map_project_error)?;
        publish_project_descriptor(state, &outcome.project).await;
        Ok(outcome)
    }

    pub async fn archive_idle_chats(
        &self,
        project: &ProjectDescriptor,
        chat_ids: Option<Vec<String>>,
        state: &AppState,
    ) -> Result<crate::services::project_chat::ProjectBulkArchiveResult, ProjectPlacementError>
    {
        state
            .conversation_input_intake
            .archive_idle_project_chats(&project.project_id, chat_ids, &self.local_device_id)
            .await
            .map_err(ProjectPlacementError::Invalid)
    }

    pub async fn inspect_resource(
        &self,
        project: &ProjectDescriptor,
        device_id: &str,
        resource_root: &Path,
        operation: ProjectResourceOperation,
    ) -> Result<ProjectDeviceResponse, ProjectPlacementError> {
        if !project
            .resources
            .contains_location(device_id, resource_root)
        {
            return Err(ProjectPlacementError::Invalid(format!(
                "Project does not contain resource {device_id} {}",
                resource_root.display()
            )));
        }
        if device_id == self.local_device_id {
            let device_id = device_id.to_owned();
            let resource_root = resource_root.to_owned();
            return tokio::task::spawn_blocking(move || {
                inspect_project_resource(&device_id, &resource_root, operation)
            })
            .await
            .map_err(|e| ProjectPlacementError::Unavailable(e.to_string()))?
            .map_err(ProjectPlacementError::Invalid);
        }
        self.deliver(
            device_id,
            ProjectDeviceAction::InspectResource {
                project_id: project.project_id.clone(),
                device_id: device_id.into(),
                resource_root: resource_root.into(),
                operation,
            },
        )
        .await
        .map_err(|error| match error {
            ProjectPlacementError::Unavailable(detail) => {
                ProjectPlacementError::Unavailable(format!(
                    "Device {device_id}, resource {}: {detail}",
                    resource_root.display()
                ))
            }
            other => other,
        })
    }

    pub async fn lifecycle_preview(
        &self,
        project: &ProjectDescriptor,
        state: &AppState,
    ) -> Result<ProjectLifecyclePreview, ProjectPlacementError> {
        state
            .conversation_input_intake
            .inspect_project_lifecycle(&project.project_id)
            .await
            .map_err(ProjectPlacementError::Rejected)
    }

    pub async fn archive(
        &self,
        project: &ProjectDescriptor,
        state: &AppState,
    ) -> Result<ProjectArchiveResult, ProjectPlacementError> {
        let result = state
            .conversation_input_intake
            .archive_project(&project.project_id)
            .await
            .map_err(ProjectPlacementError::Conflict)?;
        if result.changed {
            publish_project_descriptor(state, &result.project).await;
        }
        Ok(result)
    }

    pub async fn reopen(
        &self,
        project: &ProjectDescriptor,
        state: &AppState,
    ) -> Result<ProjectReopenResult, ProjectPlacementError> {
        let result = state
            .conversation_input_intake
            .reopen_project(&project.project_id)
            .await
            .map_err(ProjectPlacementError::Conflict)?;
        if result.changed {
            publish_project_descriptor(state, &result.project).await;
        }
        Ok(result)
    }

    pub async fn permanent_delete(
        &self,
        project_id: &ProjectId,
        state: &AppState,
    ) -> Result<ProjectPermanentDeleteResult, ProjectPlacementError> {
        self.catalog
            .begin_lifecycle_transition(project_id)
            .map_err(map_project_error)?;
        let result = async {
            let preparation = self
                .catalog
                .begin_permanent_delete(project_id)
                .map_err(map_project_error)?;
            let (newly_tombstoned, already_complete) = match preparation {
                ProjectDeletePreparation::Pending {
                    newly_tombstoned, ..
                } => (newly_tombstoned, false),
                ProjectDeletePreparation::Complete { .. } => (false, true),
            };
            let (scope_root, addresses) = state
                .conversation_input_intake
                .project_delete_scope(project_id)
                .await
                .map_err(ProjectPlacementError::Conflict)?;
            let publication = crate::sync_publication::publish_project_permanently_deleted(
                state,
                project_id.clone(),
                scope_root.clone(),
                addresses.clone(),
            )
            .await;
            if publication.status == crate::sync_publication::PublicationStatus::AppendFailed {
                if publication.appended == 0 && newly_tombstoned {
                    self.catalog
                        .rollback_permanent_delete(project_id)
                        .map_err(map_project_error)?;
                }
                return Err(ProjectPlacementError::Conflict(format!(
                    "Project delete tombstone publication failed: {}",
                    publication.errors.join("; ")
                )));
            }

            let mut cleanup = state
                .conversation_input_intake
                .cleanup_permanently_deleted_project(project_id, &scope_root, &addresses)
                .await
                .map_err(ProjectPlacementError::Conflict)?;
            let finalized = self
                .catalog
                .finalize_permanent_delete(project_id)
                .map_err(map_project_error)?;
            cleanup.changed = finalized && !already_complete;
            Ok(cleanup)
        }
        .await;
        self.catalog.end_lifecycle_transition(project_id);
        result
    }

    async fn deliver(
        &self,
        target_device_id: &str,
        action: ProjectDeviceAction,
    ) -> Result<ProjectDeviceResponse, ProjectPlacementError> {
        let session_manager =
            crate::services::device_session_access::current_session_manager(&self.device_network)
                .await
                .map_err(ProjectPlacementError::Unavailable)?;
        deliver_project_device_command_via_session(
            &session_manager,
            &ProjectDeviceCommand {
                command_id: format!("project-command-{}", uuid::Uuid::new_v4()),
                source_device_id: self.local_device_id.clone(),
                target_device_id: target_device_id.to_string(),
                action,
            },
            PROJECT_COMMAND_TIMEOUT,
        )
        .await
        .map_err(|error| match error {
            crate::services::device_command_bus::DeviceCommandDeliveryError::OutcomeUnconfirmed(
                detail,
            ) => ProjectPlacementError::OutcomeUnconfirmed(detail),
            crate::services::device_command_bus::DeviceCommandDeliveryError::Rejected {
                detail,
                ..
            } => ProjectPlacementError::Rejected(detail),
            other => ProjectPlacementError::Unavailable(other.to_string()),
        })
    }
}

async fn publish_project_descriptor(state: &AppState, project: &ProjectDescriptor) {
    let publication =
        crate::sync_publication::publish_project_descriptor_committed(state, project.clone()).await;
    if publication.status == crate::sync_publication::PublicationStatus::AppendFailed {
        tracing::warn!(
            project_id = %project.project_id,
            errors = ?publication.errors,
            "Project descriptor committed but Sync publication failed"
        );
    }
}

pub(crate) async fn validate_resource_directory(root: PathBuf) -> Result<PathBuf, String> {
    if !root.is_absolute() {
        return Err("Resource root must be absolute".into());
    }
    tokio::task::spawn_blocking(move || {
        let path = rua_project::normalize_project_root(&root.to_string_lossy())
            .map_err(|e| e.to_string())?;
        rua_project::validate_project_directory(&path).map_err(|e| e.to_string())?;
        Ok(path)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn map_project_error(error: rua_project::ProjectError) -> ProjectPlacementError {
    match error {
        rua_project::ProjectError::NotFound(id) => ProjectPlacementError::NotFound(id.to_string()),
        rua_project::ProjectError::InvalidResources(_)
        | rua_project::ProjectError::InvalidExecutionDefault(_)
        | rua_project::ProjectError::InvalidPath(_)
        | rua_project::ProjectError::DirectoryUnavailable { .. }
        | rua_project::ProjectError::InvalidResourceDevice
        | rua_project::ProjectError::InvalidDisplayName
        | rua_project::ProjectError::InvalidProjectId => {
            ProjectPlacementError::Invalid(error.to_string())
        }
        rua_project::ProjectError::ResourceRevisionConflict
        | rua_project::ProjectError::PermanentDeleteRequiresArchive(_)
        | rua_project::ProjectError::PermanentlyDeleted(_)
        | rua_project::ProjectError::LifecycleTransitionInProgress(_) => {
            ProjectPlacementError::Conflict(error.to_string())
        }
        other => ProjectPlacementError::Store(other.to_string()),
    }
}

/// Inspect a directory after verifying that its saved owner is this Device.
pub(crate) fn inspect_project_resource(
    device_id: &str,
    resource_root: &Path,
    operation: ProjectResourceOperation,
) -> Result<ProjectDeviceResponse, String> {
    if !resource_root.is_absolute() {
        return Err("The selected resource root must be an absolute path on its Device".into());
    }
    let root = resource_root.canonicalize().map_err(|error| {
        format!(
            "Resource directory on {device_id} at {} is unavailable: {error}",
            resource_root.display()
        )
    })?;
    match operation {
        ProjectResourceOperation::Files {
            path,
            offset,
            limit,
        } => {
            if !path.is_absolute() {
                return Err("Resource path must be absolute on its owning Device".into());
            }
            let path = path.canonicalize().map_err(|error| {
                format!(
                    "Resource path on {device_id} at {} is unavailable: {error}",
                    path.display()
                )
            })?;
            if !path.starts_with(&root) {
                return Err("Path must stay inside the selected resource directory".into());
            }
            let limit = limit.clamp(1, 200);
            let listing = rua_project::list_project_directory_contents(&path)
                .map_err(|error| error.to_string())?;
            // Leave room for the response envelope even when path names need JSON escaping.
            let total = listing.entries.len();
            let mut entries = Vec::new();
            let mut bytes = 0;
            for entry in listing.entries.into_iter().skip(offset).take(limit) {
                let size = serde_json::to_vec(&entry)
                    .map_err(|error| error.to_string())?
                    .len()
                    + 1;
                if bytes + size > 256 * 1024 {
                    if entries.is_empty() {
                        return Err("Directory entry exceeds the response size limit".into());
                    }
                    break;
                }
                bytes += size;
                entries.push(entry);
            }
            let end = offset.saturating_add(entries.len());
            Ok(ProjectDeviceResponse::Directory {
                device_id: device_id.into(),
                next_offset: (end < total).then_some(end),
                path: listing.path,
                entries,
                omitted_entries: listing.omitted_entries,
                roots: Vec::new(),
            })
        }
        ProjectResourceOperation::GitStatus => rua_project::inspect_project_git(&root)
            .map(|status| ProjectDeviceResponse::GitStatus { status }),
        ProjectResourceOperation::GitHistory { offset, limit } => {
            rua_project::inspect_project_git_history(&root, offset, limit.clamp(1, 100))
                .map(|history| ProjectDeviceResponse::GitHistory { history })
        }
    }
}
