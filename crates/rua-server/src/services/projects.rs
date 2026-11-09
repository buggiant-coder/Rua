use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rua_core::ProjectId;
use rua_project::{
    ProjectCreateResourcesRequest, ProjectCreationOutcome, ProjectDeletePreparation,
    ProjectDescriptor, ProjectDirectoryListing, ProjectResourceBindingStore,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectCreateResolution {
    Created(ProjectCreationOutcome),
    Duplicate(ProjectDescriptor),
}

#[derive(Debug, Error)]
pub enum ProjectPlacementError {
    #[error("local Device identity is unavailable")]
    MissingLocalDevice,
    #[error("Project Device command is unavailable: {0}")]
    Unavailable(String),
    #[error("Project request was rejected: {0}")]
    Rejected(String),
    #[error("invalid Project request: {0}")]
    Invalid(String),
    #[error("Project lifecycle conflict: {0}")]
    Lifecycle(String),
    #[error("Project storage failed: {0}")]
    Store(String),
    #[error("unexpected Project Device response")]
    UnexpectedResponse,
}

#[derive(Clone)]
pub struct ProjectPlacementService {
    local_device_id: String,
    catalog: Arc<rua_project::ProjectCatalog>,
    bindings: Arc<ProjectResourceBindingStore>,
    device_network: Arc<crate::services::device_session_access::DeviceNetworkAccess>,
    data_dir: PathBuf,
}

impl ProjectPlacementService {
    pub fn from_state(state: &AppState) -> Result<Self, ProjectPlacementError> {
        let local_device_id = current_sync_device_id(&state.sync_device)
            .ok_or(ProjectPlacementError::MissingLocalDevice)?;
        Ok(Self {
            local_device_id,
            catalog: Arc::clone(&state.project_catalog),
            bindings: Arc::clone(&state.project_bindings),
            device_network: Arc::clone(&state.device_network),
            data_dir: state.rua_root.clone(),
        })
    }

    pub fn new(
        local_device_id: String,
        catalog: Arc<rua_project::ProjectCatalog>,
        bindings: Arc<ProjectResourceBindingStore>,
        device_network: Arc<crate::services::device_session_access::DeviceNetworkAccess>,
        data_dir: PathBuf,
    ) -> Self {
        Self {
            local_device_id,
            catalog,
            bindings,
            device_network,
            data_dir,
        }
    }

    pub async fn edit_resources(
        &self,
        project_id: &ProjectId,
        expected_revision: u64,
        resource_roots: &[PathBuf],
        primary_index: usize,
        directory_ids: &[Option<String>],
        state: &AppState,
    ) -> Result<ProjectCreationOutcome, ProjectPlacementError> {
        let project = self
            .catalog
            .get(project_id)
            .map_err(map_project_error)?
            .ok_or_else(|| ProjectPlacementError::Invalid("Project not found".into()))?;
        if project.resource_device_id == self.local_device_id {
            let outcome =
                crate::services::project_lifecycle::ProjectResourceEditor::from_app_state(state)
                    .edit_resources(
                        project_id,
                        expected_revision,
                        resource_roots,
                        primary_index,
                        directory_ids,
                    )
                    .await
                    .map_err(ProjectPlacementError::Lifecycle)?;
            return Ok(outcome);
        }
        match self
            .deliver(
                &project.resource_device_id,
                ProjectDeviceAction::EditResources {
                    project_id: project_id.clone(),
                    expected_revision,
                    resource_roots: resource_roots.to_vec(),
                    primary_index,
                    directory_ids: directory_ids.to_vec(),
                },
            )
            .await?
        {
            ProjectDeviceResponse::ResourcesEdited { outcome } => {
                self.catalog
                    .apply_descriptor(outcome.project.clone())
                    .map_err(map_project_error)?;
                Ok(outcome)
            }
            _ => Err(ProjectPlacementError::UnexpectedResponse),
        }
    }

    pub async fn set_primary_directory(
        &self,
        project_id: &ProjectId,
        expected_revision: u64,
        directory_id: &str,
        state: &AppState,
    ) -> Result<ProjectDescriptor, ProjectPlacementError> {
        let project = self
            .catalog
            .get(project_id)
            .map_err(map_project_error)?
            .ok_or_else(|| ProjectPlacementError::Invalid("Project not found".into()))?;
        if project.resource_device_id == self.local_device_id {
            let project =
                crate::services::project_lifecycle::ProjectResourceEditor::from_app_state(state)
                    .set_primary_directory(project_id, expected_revision, directory_id)
                    .await
                    .map_err(ProjectPlacementError::Lifecycle)?;
            return Ok(project);
        }
        match self
            .deliver(
                &project.resource_device_id,
                ProjectDeviceAction::SetPrimaryDirectory {
                    project_id: project_id.clone(),
                    expected_revision,
                    directory_id: directory_id.into(),
                },
            )
            .await?
        {
            ProjectDeviceResponse::ResourcesUpdated { project } => {
                self.catalog
                    .apply_descriptor(project.clone())
                    .map_err(map_project_error)?;
                Ok(project)
            }
            _ => Err(ProjectPlacementError::UnexpectedResponse),
        }
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
        resource_device_id: &str,
        resource_roots: &[String],
        primary_index: usize,
        display_name: Option<String>,
        state: &AppState,
    ) -> Result<ProjectCreateResolution, ProjectPlacementError> {
        if resource_device_id == self.local_device_id {
            let outcome = match self
                .catalog
                .create_resources(ProjectCreateResourcesRequest {
                    resource_device_id: resource_device_id.to_string(),
                    resource_roots: resource_roots.iter().map(PathBuf::from).collect(),
                    primary_index,
                    display_name,
                }) {
                Ok(outcome) => outcome,
                Err(rua_project::ProjectError::DuplicateBinding { owner }) => {
                    return Ok(ProjectCreateResolution::Duplicate(*owner));
                }
                Err(error) => return Err(map_project_error(error)),
            };
            let publication = crate::sync_publication::publish_project_descriptor_committed(
                state,
                outcome.project.clone(),
            )
            .await;
            if publication.status == crate::sync_publication::PublicationStatus::AppendFailed {
                tracing::warn!(
                    project_id = %outcome.project.project_id,
                    errors = ?publication.errors,
                    "Project created locally but descriptor publication failed"
                );
            }
            return Ok(ProjectCreateResolution::Created(outcome));
        }

        let response = self
            .deliver(
                resource_device_id,
                ProjectDeviceAction::CreateResources {
                    resource_roots: resource_roots.to_vec(),
                    primary_index,
                    display_name,
                },
            )
            .await?;
        match response {
            ProjectDeviceResponse::Created { outcome } => {
                if let Err(error) = self.catalog.apply_descriptor(outcome.project.clone()) {
                    tracing::warn!(
                        project_id = %outcome.project.project_id,
                        error = %error,
                        "remote Project committed but controller catalog hydration failed"
                    );
                }
                Ok(ProjectCreateResolution::Created(outcome))
            }
            ProjectDeviceResponse::Duplicate { owner } => {
                if let Err(error) = self.catalog.apply_descriptor(owner.clone()) {
                    tracing::warn!(
                        project_id = %owner.project_id,
                        error = %error,
                        "duplicate Project owner returned but controller catalog hydration failed"
                    );
                }
                Ok(ProjectCreateResolution::Duplicate(owner))
            }
            _ => Err(ProjectPlacementError::UnexpectedResponse),
        }
    }

    pub async fn archive_idle_chats(
        &self,
        project: &ProjectDescriptor,
        chat_ids: Option<Vec<String>>,
        state: &AppState,
    ) -> Result<crate::services::project_chat::ProjectBulkArchiveResult, ProjectPlacementError>
    {
        if project.resource_device_id == self.local_device_id {
            return state
                .conversation_input_intake
                .archive_idle_project_chats(&project.project_id, chat_ids, &self.local_device_id)
                .await
                .map_err(ProjectPlacementError::Invalid);
        }
        match self
            .deliver(
                &project.resource_device_id,
                ProjectDeviceAction::ArchiveIdleChats {
                    project_id: project.project_id.clone(),
                    chat_ids,
                },
            )
            .await?
        {
            ProjectDeviceResponse::ChatsArchived { result } => Ok(result),
            _ => Err(ProjectPlacementError::UnexpectedResponse),
        }
    }

    pub async fn inspect_resource(
        &self,
        project: &ProjectDescriptor,
        directory_id: &str,
        operation: ProjectResourceOperation,
    ) -> Result<ProjectDeviceResponse, ProjectPlacementError> {
        if project.resource_device_id == self.local_device_id {
            let binding = self
                .bindings
                .get(&project.project_id)
                .map_err(map_project_error)?
                .ok_or_else(|| {
                    ProjectPlacementError::Invalid("Project binding is unavailable".into())
                })?;
            let directory_id = directory_id.to_string();
            return tokio::task::spawn_blocking(move || {
                inspect_project_resource(&binding, &directory_id, operation)
            })
            .await
            .map_err(|error| ProjectPlacementError::Unavailable(error.to_string()))?
            .map_err(ProjectPlacementError::Invalid);
        }
        self.deliver(
            &project.resource_device_id,
            ProjectDeviceAction::InspectResource {
                project_id: project.project_id.clone(),
                directory_id: directory_id.into(),
                operation,
            },
        )
        .await
    }

    pub async fn git_status(
        &self,
        project: &ProjectDescriptor,
    ) -> Result<rua_project::ProjectGitStatus, ProjectPlacementError> {
        if project.resource_device_id == self.local_device_id {
            let binding = self
                .bindings
                .get(&project.project_id)
                .map_err(map_project_error)?
                .ok_or_else(|| {
                    ProjectPlacementError::Invalid("Project binding is unavailable".to_string())
                })?;
            return tokio::task::spawn_blocking(move || {
                rua_project::inspect_project_git(binding.primary_root())
            })
            .await
            .map_err(|error| {
                ProjectPlacementError::Unavailable(format!(
                    "inspect Project Git task failed: {error}"
                ))
            })?
            .map_err(ProjectPlacementError::Unavailable);
        }
        match self
            .deliver(
                &project.resource_device_id,
                ProjectDeviceAction::InspectGit {
                    project_id: project.project_id.clone(),
                },
            )
            .await?
        {
            ProjectDeviceResponse::GitStatus { status } => Ok(status),
            _ => Err(ProjectPlacementError::UnexpectedResponse),
        }
    }

    pub async fn git_history(
        &self,
        project: &ProjectDescriptor,
        offset: usize,
        limit: usize,
    ) -> Result<rua_project::ProjectGitHistoryPage, ProjectPlacementError> {
        if project.resource_device_id == self.local_device_id {
            let binding = self
                .bindings
                .get(&project.project_id)
                .map_err(map_project_error)?
                .ok_or_else(|| {
                    ProjectPlacementError::Invalid("Project binding is unavailable".to_string())
                })?;
            return tokio::task::spawn_blocking(move || {
                rua_project::inspect_project_git_history(binding.primary_root(), offset, limit)
            })
            .await
            .map_err(|error| {
                ProjectPlacementError::Unavailable(format!(
                    "inspect Project Git history task failed: {error}"
                ))
            })?
            .map_err(ProjectPlacementError::Unavailable);
        }
        match self
            .deliver(
                &project.resource_device_id,
                ProjectDeviceAction::InspectGitHistory {
                    project_id: project.project_id.clone(),
                    offset,
                    limit,
                },
            )
            .await?
        {
            ProjectDeviceResponse::GitHistory { history } => Ok(history),
            _ => Err(ProjectPlacementError::UnexpectedResponse),
        }
    }

    pub async fn lifecycle_preview(
        &self,
        project: &ProjectDescriptor,
        state: &AppState,
    ) -> Result<ProjectLifecyclePreview, ProjectPlacementError> {
        if self.resource_device_is_revoked(project) {
            return Ok(ProjectLifecyclePreview {
                project: project.clone(),
                impact: None,
                resource_device_revoked: true,
            });
        }
        if project.resource_device_id == self.local_device_id {
            return state
                .conversation_input_intake
                .inspect_project_lifecycle(&project.project_id)
                .await
                .map_err(ProjectPlacementError::Rejected);
        }
        match self
            .deliver(
                &project.resource_device_id,
                ProjectDeviceAction::InspectLifecycle {
                    project_id: project.project_id.clone(),
                },
            )
            .await?
        {
            ProjectDeviceResponse::LifecyclePreview { preview } => Ok(preview),
            _ => Err(ProjectPlacementError::UnexpectedResponse),
        }
    }

    pub async fn archive(
        &self,
        project: &ProjectDescriptor,
        state: &AppState,
    ) -> Result<ProjectArchiveResult, ProjectPlacementError> {
        if self.resource_device_is_revoked(project) {
            self.catalog
                .begin_lifecycle_transition(&project.project_id)
                .map_err(map_project_error)?;
            let result = (|| {
                let changed = project.lifecycle != rua_project::ProjectLifecycle::Archived;
                let project = self
                    .catalog
                    .set_lifecycle(&project.project_id, rua_project::ProjectLifecycle::Archived)
                    .map_err(map_project_error)?;
                Ok(ProjectArchiveResult {
                    project,
                    impact: None,
                    resource_device_revoked: true,
                    changed,
                })
            })();
            self.catalog.end_lifecycle_transition(&project.project_id);
            let result = result?;
            if result.changed {
                publish_lifecycle_descriptor(state, &result.project).await;
            }
            return Ok(result);
        }
        if project.resource_device_id == self.local_device_id {
            let result = state
                .conversation_input_intake
                .archive_project(&project.project_id)
                .await
                .map_err(ProjectPlacementError::Lifecycle)?;
            if result.changed {
                publish_lifecycle_descriptor(state, &result.project).await;
            }
            return Ok(result);
        }
        match self
            .deliver(
                &project.resource_device_id,
                ProjectDeviceAction::Archive {
                    project_id: project.project_id.clone(),
                },
            )
            .await?
        {
            ProjectDeviceResponse::Archived { result } => {
                self.catalog
                    .apply_descriptor(result.project.clone())
                    .map_err(map_project_error)?;
                Ok(result)
            }
            _ => Err(ProjectPlacementError::UnexpectedResponse),
        }
    }

    pub async fn reopen(
        &self,
        project: &ProjectDescriptor,
        state: &AppState,
    ) -> Result<ProjectReopenResult, ProjectPlacementError> {
        if self.resource_device_is_revoked(project) {
            return Err(ProjectPlacementError::Lifecycle(
                "Reopen requires the original resource Device to be online".to_string(),
            ));
        }
        if project.resource_device_id == self.local_device_id {
            let result = state
                .conversation_input_intake
                .reopen_project(&project.project_id)
                .await
                .map_err(ProjectPlacementError::Lifecycle)?;
            if result.changed {
                publish_lifecycle_descriptor(state, &result.project).await;
            }
            return Ok(result);
        }
        match self
            .deliver(
                &project.resource_device_id,
                ProjectDeviceAction::Reopen {
                    project_id: project.project_id.clone(),
                },
            )
            .await?
        {
            ProjectDeviceResponse::Reopened { result } => {
                self.catalog
                    .apply_descriptor(result.project.clone())
                    .map_err(map_project_error)?;
                Ok(result)
            }
            _ => Err(ProjectPlacementError::UnexpectedResponse),
        }
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
                .map_err(ProjectPlacementError::Lifecycle)?;
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
                return Err(ProjectPlacementError::Lifecycle(format!(
                    "Project delete tombstone publication failed: {}",
                    publication.errors.join("; ")
                )));
            }

            let mut cleanup = state
                .conversation_input_intake
                .cleanup_permanently_deleted_project(project_id, &scope_root, &addresses)
                .await
                .map_err(ProjectPlacementError::Lifecycle)?;
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

    fn resource_device_is_revoked(&self, project: &ProjectDescriptor) -> bool {
        project.resource_device_id != self.local_device_id
            && !resource_device_is_member(&self.data_dir, &project.resource_device_id)
    }

    async fn deliver(
        &self,
        target_device_id: &str,
        action: ProjectDeviceAction,
    ) -> Result<ProjectDeviceResponse, ProjectPlacementError> {
        self.deliver_with_timeout(target_device_id, action, PROJECT_COMMAND_TIMEOUT)
            .await
    }

    async fn deliver_with_timeout(
        &self,
        target_device_id: &str,
        action: ProjectDeviceAction,
        timeout: Duration,
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
            timeout,
        )
        .await
        .map_err(|error| match error {
            crate::services::device_command_bus::DeviceCommandDeliveryError::Rejected {
                reason,
                detail,
            } if matches!(
                reason.as_str(),
                "project_directory_unavailable" | "project_create_failed"
            ) =>
            {
                ProjectPlacementError::Invalid(detail)
            }
            crate::services::device_command_bus::DeviceCommandDeliveryError::Rejected {
                reason,
                detail,
            } if matches!(
                reason.as_str(),
                "project_archive_failed"
                    | "project_reopen_failed"
                    | "project_lifecycle_inspect_failed"
            ) =>
            {
                ProjectPlacementError::Lifecycle(detail)
            }
            crate::services::device_command_bus::DeviceCommandDeliveryError::Rejected {
                detail,
                ..
            } => ProjectPlacementError::Rejected(detail),
            other => ProjectPlacementError::Unavailable(other.to_string()),
        })
    }
}

async fn publish_lifecycle_descriptor(state: &AppState, project: &ProjectDescriptor) {
    let publication =
        crate::sync_publication::publish_project_descriptor_committed(state, project.clone()).await;
    if publication.status == crate::sync_publication::PublicationStatus::AppendFailed {
        tracing::warn!(
            project_id = %project.project_id,
            errors = ?publication.errors,
            "Project lifecycle committed but descriptor publication failed"
        );
    }
}

fn resource_device_is_member(data_dir: &Path, resource_device_id: &str) -> bool {
    let path = data_dir.join("sync").join("device_list.json");
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<rua_sync::device_list::DeviceList>(&bytes).ok())
        .is_some_and(|list| {
            list.devices
                .iter()
                .any(|device| hex::encode(device.device_id) == resource_device_id)
        })
}

fn map_project_error(error: rua_project::ProjectError) -> ProjectPlacementError {
    match error {
        rua_project::ProjectError::InvalidResources(_)
        | rua_project::ProjectError::InvalidPath(_)
        | rua_project::ProjectError::DirectoryUnavailable { .. }
        | rua_project::ProjectError::InvalidResourceDevice
        | rua_project::ProjectError::InvalidDisplayName
        | rua_project::ProjectError::InvalidProjectId => {
            ProjectPlacementError::Invalid(error.to_string())
        }
        rua_project::ProjectError::PermanentDeleteRequiresArchive(_)
        | rua_project::ProjectError::PermanentlyDeleted(_)
        | rua_project::ProjectError::LifecycleTransitionInProgress(_) => {
            ProjectPlacementError::Lifecycle(error.to_string())
        }
        other => ProjectPlacementError::Store(other.to_string()),
    }
}

/// Resolve resource identities only against this Device's authoritative binding.
pub(crate) fn inspect_project_resource(
    binding: &rua_project::ProjectResourceBinding,
    directory_id: &str,
    operation: ProjectResourceOperation,
) -> Result<ProjectDeviceResponse, String> {
    let directory = binding
        .resources
        .directory(directory_id)
        .map_err(|error| error.to_string())?;
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
                device_id: binding.resource_device_id.clone(),
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
