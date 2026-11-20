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

const PROJECT_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Error)]
pub enum ProjectPlacementError {
    #[error("Project not found: {0}")]
    NotFound(String),
    #[error("local Device identity is unavailable")]
    MissingLocalDevice,
    #[error("Project Device command is unavailable: {0}")]
    Unavailable(String),
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
                ..
            } => Ok(ProjectDirectoryListing {
                path,
                entries,
                roots,
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
                ..
            } => Ok(ProjectDirectoryListing {
                path,
                entries,
                roots,
            }),
            _ => Err(ProjectPlacementError::UnexpectedResponse),
        }
    }

    pub async fn create(
        &self,
        mut directories: Vec<ProjectResourceDirectoryInput>,
        display_name: Option<String>,
        state: &AppState,
    ) -> Result<ProjectCreationOutcome, ProjectPlacementError> {
        self.validate_locations(&[], &mut directories).await?;
        let outcome = self
            .catalog
            .create_resources(ProjectCreateResourcesRequest {
                display_name,
                directories,
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
        directory_id: &str,
        operation: ProjectResourceOperation,
    ) -> Result<ProjectDeviceResponse, ProjectPlacementError> {
        let directory = project
            .resources
            .directory(directory_id)
            .map_err(map_project_error)?;
        if directory.device_id == self.local_device_id {
            let directory = directory.clone();
            return tokio::task::spawn_blocking(move || {
                inspect_project_resource(&directory, operation)
            })
            .await
            .map_err(|e| ProjectPlacementError::Unavailable(e.to_string()))?
            .map_err(ProjectPlacementError::Invalid);
        }
        self.deliver(
            &directory.device_id,
            ProjectDeviceAction::InspectResource {
                project_id: project.project_id.clone(),
                directory_id: directory_id.into(),
                operation,
            },
        )
        .await
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
    directory: &rua_project::ProjectResourceDirectory,
    operation: ProjectResourceOperation,
) -> Result<ProjectDeviceResponse, String> {
    let root = directory.resource_root.canonicalize().map_err(|error| {
        format!(
            "Resource directory {} is unavailable: {error}",
            directory.resource_root.display()
        )
    })?;
    match operation {
        ProjectResourceOperation::Files { path } => {
            let requested = Path::new(&path);
            let candidate = if requested.is_absolute() {
                requested.to_path_buf()
            } else {
                root.join(requested)
            };
            let path = candidate
                .canonicalize()
                .map_err(|error| format!("Resource path unavailable: {error}"))?;
            if !path.starts_with(&root) {
                return Err("Path must stay inside the selected resource directory".into());
            }
            let listing = rua_project::list_project_directory(&path.to_string_lossy())
                .map_err(|error| error.to_string())?;
            Ok(ProjectDeviceResponse::Directory {
                device_id: directory.device_id.clone(),
                path: listing.path,
                entries: listing.entries,
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
