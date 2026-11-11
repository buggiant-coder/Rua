use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use chrono::{DateTime, Utc};
use rua_core::ProjectId;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    propose_project_display_name, validate_project_directory, ProjectCreateRequest,
    ProjectCreateResourcesRequest, ProjectCreationOutcome, ProjectDescriptor, ProjectLifecycle,
    ProjectOverlapRelation, ProjectOverlapWarning, ProjectResourceBinding, ProjectResources,
};

const PROJECT_RECORD_SCHEMA_VERSION: u32 = 3;
const PROJECT_TOMBSTONE_SCHEMA_VERSION: u32 = 1;
const LEGACY_PROJECT_RUNNING_CONFIG_FIELD: &str = "running_config";

pub type ProjectResult<T> = Result<T, ProjectError>;

#[derive(Debug, Error)]
pub enum ProjectError {
    #[error("invalid Project resources: {0}")]
    InvalidResources(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid project path: {0}")]
    InvalidPath(String),
    #[error("project directory is unavailable at {path}: {detail}")]
    DirectoryUnavailable { path: PathBuf, detail: String },
    #[error("resource Device id must not be empty")]
    InvalidResourceDevice,
    #[error("project display name must not be empty")]
    InvalidDisplayName,
    #[error("project id must not be empty")]
    InvalidProjectId,
    #[error("project not found: {0}")]
    NotFound(ProjectId),
    #[error("project binding is already owned by another Project")]
    DuplicateBinding { owner: Box<ProjectDescriptor> },
    #[error("duplicate Project id in catalog: {0}")]
    DuplicateProjectId(ProjectId),
    #[error("Project resource Device and display path are immutable: {0}")]
    ImmutableIdentityChange(ProjectId),
    #[error("Project record lock is poisoned")]
    LockPoisoned,
    #[error("Project lifecycle transition is already in progress: {0}")]
    LifecycleTransitionInProgress(ProjectId),
    #[error("Project must be Archived before Permanent Delete: {0}")]
    PermanentDeleteRequiresArchive(ProjectId),
    #[error("Project was permanently deleted: {0}")]
    PermanentlyDeleted(ProjectId),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredProjectRecord {
    #[serde(default = "default_schema_version")]
    schema_version: u32,
    descriptor: ProjectDescriptor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    binding: Option<ProjectResourceBinding>,
    #[serde(default, flatten)]
    extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectTombstone {
    #[serde(default = "default_tombstone_schema_version")]
    pub schema_version: u32,
    pub project_id: ProjectId,
    pub deleted_at: DateTime<Utc>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectDeletePreparation {
    Pending {
        descriptor: Box<ProjectDescriptor>,
        tombstone: ProjectTombstone,
        newly_tombstoned: bool,
    },
    Complete {
        tombstone: ProjectTombstone,
    },
}

fn default_schema_version() -> u32 {
    PROJECT_RECORD_SCHEMA_VERSION
}

fn default_tombstone_schema_version() -> u32 {
    PROJECT_TOMBSTONE_SCHEMA_VERSION
}

struct ProjectStoreCore {
    records_dir: PathBuf,
    tombstones_dir: PathBuf,
    records: Mutex<HashMap<ProjectId, StoredProjectRecord>>,
    tombstones: Mutex<HashMap<ProjectId, ProjectTombstone>>,
}

impl ProjectStoreCore {
    fn open(root: impl Into<PathBuf>) -> ProjectResult<Self> {
        let root = root.into();
        let records_dir = root.join("records");
        let tombstones_dir = root.join("tombstones");
        std::fs::create_dir_all(&records_dir)?;
        std::fs::create_dir_all(&tombstones_dir)?;
        let mut records = HashMap::new();
        for entry in std::fs::read_dir(&records_dir)? {
            let entry = entry?;
            let path = entry.path();
            if !entry.file_type()?.is_file()
                || path.extension().and_then(|value| value.to_str()) != Some("json")
            {
                continue;
            }
            let mut raw: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
            let schema = raw
                .get("schema_version")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(1);
            if schema > u64::from(PROJECT_RECORD_SCHEMA_VERSION) {
                return Err(ProjectError::InvalidResources(format!(
                    "Unsupported Project record schema: {schema}"
                )));
            }
            if schema < 3 {
                if let Some(binding) = raw
                    .get_mut("binding")
                    .and_then(serde_json::Value::as_object_mut)
                {
                    if let Some(root) = binding.remove("resource_root") {
                        let root: PathBuf = serde_json::from_value(root)?;
                        binding.insert(
                            "resources".into(),
                            serde_json::to_value(ProjectResources::from_legacy_root(root))?,
                        );
                    }
                }
            }
            let mut record: StoredProjectRecord = serde_json::from_value(raw)?;
            if let Some(binding) = record.binding.as_ref() {
                binding.resources.validate()?;
                if record.descriptor.resources.is_none() {
                    record.descriptor.resources = Some((&binding.resources).into());
                }
            }
            let removed_legacy_running_config =
                remove_legacy_project_running_config(&mut record.descriptor);
            let upgraded_schema = record.schema_version < PROJECT_RECORD_SCHEMA_VERSION;
            if removed_legacy_running_config || upgraded_schema {
                record.schema_version = PROJECT_RECORD_SCHEMA_VERSION;
                let bytes = serde_json::to_vec_pretty(&record)?;
                rua_config::atomic_write(&path, &bytes)?;
            }
            let project_id = record.descriptor.project_id.clone();
            if project_id.as_str().trim().is_empty() {
                return Err(ProjectError::InvalidProjectId);
            }
            if records.insert(project_id.clone(), record).is_some() {
                return Err(ProjectError::DuplicateProjectId(project_id));
            }
        }
        let mut tombstones = HashMap::new();
        for entry in std::fs::read_dir(&tombstones_dir)? {
            let entry = entry?;
            let path = entry.path();
            if !entry.file_type()?.is_file()
                || path.extension().and_then(|value| value.to_str()) != Some("json")
            {
                continue;
            }
            let tombstone: ProjectTombstone = serde_json::from_slice(&std::fs::read(&path)?)?;
            let project_id = tombstone.project_id.clone();
            if project_id.as_str().trim().is_empty() {
                return Err(ProjectError::InvalidProjectId);
            }
            if tombstones.insert(project_id.clone(), tombstone).is_some() {
                return Err(ProjectError::DuplicateProjectId(project_id));
            }
        }
        Ok(Self {
            records_dir,
            tombstones_dir,
            records: Mutex::new(records),
            tombstones: Mutex::new(tombstones),
        })
    }

    fn lock(&self) -> ProjectResult<MutexGuard<'_, HashMap<ProjectId, StoredProjectRecord>>> {
        self.records.lock().map_err(|_| ProjectError::LockPoisoned)
    }

    fn write_record(&self, record: &StoredProjectRecord) -> ProjectResult<()> {
        let path = self.record_path(&record.descriptor.project_id);
        let bytes = serde_json::to_vec_pretty(record)?;
        rua_config::atomic_write(&path, &bytes)?;
        Ok(())
    }

    fn lock_tombstones(
        &self,
    ) -> ProjectResult<MutexGuard<'_, HashMap<ProjectId, ProjectTombstone>>> {
        self.tombstones
            .lock()
            .map_err(|_| ProjectError::LockPoisoned)
    }

    fn write_tombstone(&self, tombstone: &ProjectTombstone) -> ProjectResult<()> {
        let path = self.tombstone_path(&tombstone.project_id);
        let bytes = serde_json::to_vec_pretty(tombstone)?;
        rua_config::atomic_write(&path, &bytes)?;
        Ok(())
    }

    fn remove_record(&self, project_id: &ProjectId) -> ProjectResult<()> {
        remove_file_and_sync_parent(&self.record_path(project_id))
    }

    fn remove_tombstone(&self, project_id: &ProjectId) -> ProjectResult<()> {
        remove_file_and_sync_parent(&self.tombstone_path(project_id))
    }

    fn record_path(&self, project_id: &ProjectId) -> PathBuf {
        self.records_dir.join(format!(
            "{}.json",
            encode_file_component(project_id.as_str())
        ))
    }

    fn tombstone_path(&self, project_id: &ProjectId) -> PathBuf {
        self.tombstones_dir.join(format!(
            "{}.json",
            encode_file_component(project_id.as_str())
        ))
    }
}

pub struct ProjectStores {
    pub catalog: Arc<ProjectCatalog>,
    pub bindings: Arc<ProjectResourceBindingStore>,
}

impl ProjectStores {
    pub fn open(root: impl Into<PathBuf>) -> ProjectResult<Self> {
        let core = Arc::new(ProjectStoreCore::open(root)?);
        Ok(Self {
            catalog: Arc::new(ProjectCatalog {
                core: Arc::clone(&core),
                lifecycle_transitions: Mutex::new(ProjectAdmissions::default()),
            }),
            bindings: Arc::new(ProjectResourceBindingStore { core }),
        })
    }
}

pub struct ProjectCatalog {
    core: Arc<ProjectStoreCore>,
    lifecycle_transitions: Mutex<ProjectAdmissions>,
}

#[derive(Default)]
struct ProjectAdmissions {
    transitions: HashSet<ProjectId>,
    pending: HashMap<ProjectId, usize>,
}

/// Pins resources until an input has become visible to active-work inspection.
pub struct ProjectAdmissionGuard<'a> {
    catalog: &'a ProjectCatalog,
    project_id: ProjectId,
}

impl Drop for ProjectAdmissionGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut state) = self.catalog.lifecycle_transitions.lock() {
            if let Some(count) = state.pending.get_mut(&self.project_id) {
                *count -= 1;
                if *count == 0 {
                    state.pending.remove(&self.project_id);
                }
            }
        }
    }
}

pub struct ProjectResourceEditGuard<'a> {
    catalog: &'a ProjectCatalog,
    project_id: ProjectId,
}
impl ProjectResourceEditGuard<'_> {
    /// Commit exactly the candidate validated by the Project resource editor.
    /// This guard proves exclusion from admission, not completion of business checks.
    pub fn commit_resources(
        self,
        expected_revision: u64,
        resources: ProjectResources,
    ) -> ProjectResult<ProjectCreationOutcome> {
        self.catalog
            .commit_resources(&self.project_id, expected_revision, resources)
    }

    pub fn set_primary_directory(
        self,
        expected_revision: u64,
        directory_id: &str,
    ) -> ProjectResult<ProjectDescriptor> {
        self.catalog
            .set_primary_directory(&self.project_id, expected_revision, directory_id)
    }
}

impl Drop for ProjectResourceEditGuard<'_> {
    fn drop(&mut self) {
        self.catalog.end_lifecycle_transition(&self.project_id);
    }
}

impl ProjectCatalog {
    pub fn begin_resource_edit(
        &self,
        project_id: &ProjectId,
    ) -> ProjectResult<ProjectResourceEditGuard<'_>> {
        self.begin_lifecycle_transition(project_id)?;
        Ok(ProjectResourceEditGuard {
            catalog: self,
            project_id: project_id.clone(),
        })
    }

    pub fn begin_admission(
        &self,
        project_id: &ProjectId,
    ) -> ProjectResult<ProjectAdmissionGuard<'_>> {
        let mut state = self
            .lifecycle_transitions
            .lock()
            .map_err(|_| ProjectError::LockPoisoned)?;
        if state.transitions.contains(project_id) {
            return Err(ProjectError::LifecycleTransitionInProgress(
                project_id.clone(),
            ));
        }
        *state.pending.entry(project_id.clone()).or_default() += 1;
        Ok(ProjectAdmissionGuard {
            catalog: self,
            project_id: project_id.clone(),
        })
    }

    pub fn create_bound(
        &self,
        request: ProjectCreateRequest,
    ) -> ProjectResult<ProjectCreationOutcome> {
        self.create_resources(ProjectCreateResourcesRequest {
            resource_device_id: request.resource_device_id,
            resource_roots: vec![PathBuf::from(request.resource_root)],
            primary_index: 0,
            display_name: request.display_name,
        })
    }

    pub fn create_resources(
        &self,
        request: ProjectCreateResourcesRequest,
    ) -> ProjectResult<ProjectCreationOutcome> {
        let resource_device_id = request.resource_device_id.trim();
        if resource_device_id.is_empty() {
            return Err(ProjectError::InvalidResourceDevice);
        }
        let resources = ProjectResources::create(&request.resource_roots, request.primary_index)?;
        let resource_root = resources.primary()?.resource_root.clone();
        let display_name = request
            .display_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| propose_project_display_name(&resource_root));
        if display_name.trim().is_empty() {
            return Err(ProjectError::InvalidDisplayName);
        }

        let mut records = self.core.lock()?;
        let mut overlap_warnings = resources
            .directories
            .iter()
            .flat_map(|directory| {
                overlap_warnings(&records, resource_device_id, &directory.resource_root)
            })
            .collect::<Vec<_>>();
        overlap_warnings
            .sort_by(|left, right| left.project_id.as_str().cmp(right.project_id.as_str()));
        overlap_warnings.dedup_by(|left, right| left.project_id == right.project_id);
        let project_id = ProjectId::new(uuid::Uuid::new_v4().to_string());
        let now = Utc::now();
        let descriptor = ProjectDescriptor {
            project_id: project_id.clone(),
            display_name,
            resource_device_id: resource_device_id.to_string(),
            display_path: resource_root.to_string_lossy().to_string(),
            lifecycle: ProjectLifecycle::Active,
            resources: Some((&resources).into()),
            created_at: now,
            updated_at: now,
            extra: BTreeMap::new(),
        };
        let binding = ProjectResourceBinding {
            project_id: project_id.clone(),
            resource_device_id: resource_device_id.to_string(),
            resources,
            created_at: now,
            extra: BTreeMap::new(),
        };
        let record = StoredProjectRecord {
            schema_version: PROJECT_RECORD_SCHEMA_VERSION,
            descriptor: descriptor.clone(),
            binding: Some(binding),
            extra: BTreeMap::new(),
        };
        self.core.write_record(&record)?;
        records.insert(project_id, record);
        Ok(ProjectCreationOutcome {
            project: descriptor,
            overlap_warnings,
        })
    }

    pub fn list(&self) -> ProjectResult<Vec<ProjectDescriptor>> {
        let records = self.core.lock()?;
        let tombstones = self.core.lock_tombstones()?;
        Ok(sorted_descriptors(
            records
                .iter()
                .filter(|(project_id, _)| !tombstones.contains_key(*project_id))
                .map(|(_, record)| &record.descriptor),
        ))
    }

    pub fn list_active(&self) -> ProjectResult<Vec<ProjectDescriptor>> {
        self.list_by_lifecycle(ProjectLifecycle::Active)
    }

    pub fn list_archived(&self) -> ProjectResult<Vec<ProjectDescriptor>> {
        self.list_by_lifecycle(ProjectLifecycle::Archived)
    }

    pub fn get(&self, project_id: &ProjectId) -> ProjectResult<Option<ProjectDescriptor>> {
        let records = self.core.lock()?;
        let tombstones = self.core.lock_tombstones()?;
        if tombstones.contains_key(project_id) {
            return Ok(None);
        }
        Ok(records
            .get(project_id)
            .map(|record| record.descriptor.clone()))
    }

    pub fn rename(
        &self,
        project_id: &ProjectId,
        display_name: &str,
    ) -> ProjectResult<ProjectDescriptor> {
        let display_name = display_name.trim();
        if display_name.is_empty() {
            return Err(ProjectError::InvalidDisplayName);
        }
        let mut records = self.core.lock()?;
        let tombstones = self.core.lock_tombstones()?;
        if tombstones.contains_key(project_id) {
            return Err(ProjectError::PermanentlyDeleted(project_id.clone()));
        }
        let record = records
            .get_mut(project_id)
            .ok_or_else(|| ProjectError::NotFound(project_id.clone()))?;
        let mut updated = record.clone();
        updated.descriptor.display_name = display_name.to_string();
        updated.descriptor.updated_at = Utc::now();
        self.core.write_record(&updated)?;
        *record = updated;
        Ok(record.descriptor.clone())
    }

    fn commit_resources(
        &self,
        project_id: &ProjectId,
        expected_revision: u64,
        resources: ProjectResources,
    ) -> ProjectResult<ProjectCreationOutcome> {
        let mut records = self.core.lock()?;
        if self.core.lock_tombstones()?.contains_key(project_id) {
            return Err(ProjectError::PermanentlyDeleted(project_id.clone()));
        }
        let record = records
            .get(project_id)
            .ok_or_else(|| ProjectError::NotFound(project_id.clone()))?;
        let mut updated = record.clone();
        let binding = updated.binding.as_mut().ok_or_else(|| {
            ProjectError::InvalidResources("Resource Device binding unavailable".into())
        })?;
        if binding.resources.revision != expected_revision {
            return Err(ProjectError::InvalidResources(
                "Project resources changed; reload before editing".into(),
            ));
        }
        resources.validate()?;
        if expected_revision.checked_add(1) != Some(resources.revision) {
            return Err(ProjectError::InvalidResources(
                "Invalid resource revision increment".into(),
            ));
        }
        binding.resources = resources;
        updated.descriptor.display_path = binding.primary_root().to_string_lossy().into_owned();
        updated.descriptor.resources = Some((&binding.resources).into());
        updated.descriptor.updated_at = Utc::now();
        let mut overlap_warnings = binding
            .resources
            .directories
            .iter()
            .flat_map(|directory| {
                overlap_warnings(
                    &records,
                    &binding.resource_device_id,
                    &directory.resource_root,
                )
            })
            .filter(|warning| warning.project_id != *project_id)
            .collect::<Vec<_>>();
        overlap_warnings
            .sort_by(|left, right| left.project_id.as_str().cmp(right.project_id.as_str()));
        overlap_warnings.dedup_by(|left, right| left.project_id == right.project_id);
        self.core.write_record(&updated)?;
        let project = updated.descriptor.clone();
        records.insert(project_id.clone(), updated);
        Ok(ProjectCreationOutcome {
            project,
            overlap_warnings,
        })
    }

    fn set_primary_directory(
        &self,
        project_id: &ProjectId,
        expected_revision: u64,
        directory_id: &str,
    ) -> ProjectResult<ProjectDescriptor> {
        let mut records = self.core.lock()?;
        if self.core.lock_tombstones()?.contains_key(project_id) {
            return Err(ProjectError::PermanentlyDeleted(project_id.clone()));
        }
        let record = records
            .get_mut(project_id)
            .ok_or_else(|| ProjectError::NotFound(project_id.clone()))?;
        let mut updated = record.clone();
        let binding = updated.binding.as_mut().ok_or_else(|| {
            ProjectError::InvalidResources("Resource Device binding is unavailable".into())
        })?;
        if binding.resources.revision != expected_revision {
            return Err(ProjectError::InvalidResources(
                "Project resources changed; reload before editing".into(),
            ));
        }
        let selected = binding.resources.directory(directory_id)?;
        validate_project_directory(&selected.resource_root)?;
        if binding.resources.primary_directory_id == directory_id {
            return Ok(record.descriptor.clone());
        }
        binding.resources.primary_directory_id = directory_id.into();
        binding.resources.revision += 1;
        updated.descriptor.display_path = binding.primary_root().to_string_lossy().into_owned();
        updated.descriptor.resources = Some((&binding.resources).into());
        updated.descriptor.updated_at = Utc::now();
        self.core.write_record(&updated)?;
        *record = updated;
        Ok(record.descriptor.clone())
    }

    pub fn set_lifecycle(
        &self,
        project_id: &ProjectId,
        lifecycle: ProjectLifecycle,
    ) -> ProjectResult<ProjectDescriptor> {
        let mut records = self.core.lock()?;
        let tombstones = self.core.lock_tombstones()?;
        if tombstones.contains_key(project_id) {
            return Err(ProjectError::PermanentlyDeleted(project_id.clone()));
        }
        let record = records
            .get_mut(project_id)
            .ok_or_else(|| ProjectError::NotFound(project_id.clone()))?;
        if record.descriptor.lifecycle == lifecycle {
            return Ok(record.descriptor.clone());
        }
        let mut updated = record.clone();
        updated.descriptor.lifecycle = lifecycle;
        updated.descriptor.updated_at = Utc::now();
        self.core.write_record(&updated)?;
        *record = updated;
        Ok(record.descriptor.clone())
    }

    pub fn begin_lifecycle_transition(&self, project_id: &ProjectId) -> ProjectResult<()> {
        let records = self.core.lock()?;
        let tombstones = self.core.lock_tombstones()?;
        if !records.contains_key(project_id) && !tombstones.contains_key(project_id) {
            return Err(ProjectError::NotFound(project_id.clone()));
        }
        drop(tombstones);
        drop(records);
        let mut transitions = self
            .lifecycle_transitions
            .lock()
            .map_err(|_| ProjectError::LockPoisoned)?;
        if transitions
            .pending
            .get(project_id)
            .copied()
            .unwrap_or_default()
            > 0
            || !transitions.transitions.insert(project_id.clone())
        {
            return Err(ProjectError::LifecycleTransitionInProgress(
                project_id.clone(),
            ));
        }
        Ok(())
    }

    pub fn end_lifecycle_transition(&self, project_id: &ProjectId) {
        if let Ok(mut transitions) = self.lifecycle_transitions.lock() {
            transitions.transitions.remove(project_id);
        }
    }

    pub fn lifecycle_transition_in_progress(&self, project_id: &ProjectId) -> bool {
        self.lifecycle_transitions
            .lock()
            .map(|transitions| transitions.transitions.contains(project_id))
            .unwrap_or(true)
    }

    pub fn apply_descriptor(
        &self,
        mut descriptor: ProjectDescriptor,
    ) -> ProjectResult<ProjectDescriptor> {
        remove_legacy_project_running_config(&mut descriptor);
        validate_descriptor(&descriptor)?;
        let project_id = descriptor.project_id.clone();
        let mut records = self.core.lock()?;
        let tombstones = self.core.lock_tombstones()?;
        if tombstones.contains_key(&project_id) {
            return Err(ProjectError::PermanentlyDeleted(project_id));
        }
        let updated = match records.get(&project_id) {
            Some(existing) => {
                if existing.descriptor.resource_device_id != descriptor.resource_device_id
                    || existing.descriptor.created_at != descriptor.created_at
                {
                    return Err(ProjectError::ImmutableIdentityChange(project_id));
                }
                if let Some(binding) = existing.binding.as_ref() {
                    descriptor.resources = Some((&binding.resources).into());
                    descriptor.display_path = binding.primary_root().to_string_lossy().into_owned();
                } else if let Some(resources) = existing.descriptor.resources.as_ref() {
                    if descriptor
                        .resources
                        .as_ref()
                        .map(|incoming| incoming.revision)
                        .unwrap_or_default()
                        < resources.revision
                    {
                        descriptor.resources = Some(resources.clone());
                        descriptor.display_path = existing.descriptor.display_path.clone();
                    }
                } else if descriptor.resources.is_none()
                    && existing.descriptor.display_path != descriptor.display_path
                {
                    return Err(ProjectError::ImmutableIdentityChange(project_id));
                }
                StoredProjectRecord {
                    schema_version: existing.schema_version,
                    descriptor: descriptor.clone(),
                    binding: existing.binding.clone(),
                    extra: existing.extra.clone(),
                }
            }
            None => StoredProjectRecord {
                schema_version: PROJECT_RECORD_SCHEMA_VERSION,
                descriptor: descriptor.clone(),
                binding: None,
                extra: BTreeMap::new(),
            },
        };
        self.core.write_record(&updated)?;
        records.insert(project_id, updated);
        Ok(descriptor)
    }

    pub fn export_descriptors(&self) -> ProjectResult<Vec<ProjectDescriptor>> {
        self.list()
    }

    pub fn export_tombstones(&self) -> ProjectResult<Vec<ProjectTombstone>> {
        let tombstones = self.core.lock_tombstones()?;
        let mut tombstones = tombstones.values().cloned().collect::<Vec<_>>();
        tombstones.sort_by(|left, right| left.project_id.as_str().cmp(right.project_id.as_str()));
        Ok(tombstones)
    }

    pub fn is_deleted(&self, project_id: &ProjectId) -> ProjectResult<bool> {
        Ok(self.core.lock_tombstones()?.contains_key(project_id))
    }

    pub fn begin_permanent_delete(
        &self,
        project_id: &ProjectId,
    ) -> ProjectResult<ProjectDeletePreparation> {
        let records = self.core.lock()?;
        let mut tombstones = self.core.lock_tombstones()?;
        if let Some(tombstone) = tombstones.get(project_id).cloned() {
            return Ok(match records.get(project_id) {
                Some(record) => ProjectDeletePreparation::Pending {
                    descriptor: Box::new(record.descriptor.clone()),
                    tombstone,
                    newly_tombstoned: false,
                },
                None => ProjectDeletePreparation::Complete { tombstone },
            });
        }
        let record = records
            .get(project_id)
            .ok_or_else(|| ProjectError::NotFound(project_id.clone()))?;
        if record.descriptor.lifecycle != ProjectLifecycle::Archived {
            return Err(ProjectError::PermanentDeleteRequiresArchive(
                project_id.clone(),
            ));
        }
        let tombstone = ProjectTombstone {
            schema_version: PROJECT_TOMBSTONE_SCHEMA_VERSION,
            project_id: project_id.clone(),
            deleted_at: Utc::now(),
            extra: BTreeMap::new(),
        };
        self.core.write_tombstone(&tombstone)?;
        tombstones.insert(project_id.clone(), tombstone.clone());
        Ok(ProjectDeletePreparation::Pending {
            descriptor: Box::new(record.descriptor.clone()),
            tombstone,
            newly_tombstoned: true,
        })
    }

    pub fn rollback_permanent_delete(&self, project_id: &ProjectId) -> ProjectResult<bool> {
        let records = self.core.lock()?;
        let mut tombstones = self.core.lock_tombstones()?;
        if !records.contains_key(project_id) || !tombstones.contains_key(project_id) {
            return Ok(false);
        }
        self.core.remove_tombstone(project_id)?;
        tombstones.remove(project_id);
        Ok(true)
    }

    pub fn finalize_permanent_delete(&self, project_id: &ProjectId) -> ProjectResult<bool> {
        let mut records = self.core.lock()?;
        let tombstones = self.core.lock_tombstones()?;
        if !tombstones.contains_key(project_id) {
            return Err(ProjectError::NotFound(project_id.clone()));
        }
        if !records.contains_key(project_id) {
            return Ok(false);
        }
        self.core.remove_record(project_id)?;
        records.remove(project_id);
        Ok(true)
    }

    pub fn apply_tombstone(
        &self,
        project_id: ProjectId,
        deleted_at: DateTime<Utc>,
    ) -> ProjectResult<bool> {
        let records = self.core.lock()?;
        let mut tombstones = self.core.lock_tombstones()?;
        if tombstones
            .get(&project_id)
            .is_some_and(|existing| existing.deleted_at >= deleted_at)
        {
            return Ok(false);
        }
        let tombstone = ProjectTombstone {
            schema_version: PROJECT_TOMBSTONE_SCHEMA_VERSION,
            project_id: project_id.clone(),
            deleted_at,
            extra: BTreeMap::new(),
        };
        self.core.write_tombstone(&tombstone)?;
        tombstones.insert(project_id.clone(), tombstone);
        drop(records);
        Ok(true)
    }

    fn list_by_lifecycle(
        &self,
        lifecycle: ProjectLifecycle,
    ) -> ProjectResult<Vec<ProjectDescriptor>> {
        let records = self.core.lock()?;
        let tombstones = self.core.lock_tombstones()?;
        Ok(sorted_descriptors(
            records
                .iter()
                .filter(|(project_id, _)| !tombstones.contains_key(*project_id))
                .map(|(_, record)| &record.descriptor)
                .filter(|descriptor| descriptor.lifecycle == lifecycle),
        ))
    }
}

pub struct ProjectResourceBindingStore {
    core: Arc<ProjectStoreCore>,
}

impl ProjectResourceBindingStore {
    pub fn get(&self, project_id: &ProjectId) -> ProjectResult<Option<ProjectResourceBinding>> {
        let records = self.core.lock()?;
        Ok(records
            .get(project_id)
            .and_then(|record| record.binding.clone()))
    }

    pub fn list(&self) -> ProjectResult<Vec<ProjectResourceBinding>> {
        let records = self.core.lock()?;
        let mut bindings = records
            .values()
            .filter_map(|record| record.binding.clone())
            .collect::<Vec<_>>();
        bindings.sort_by(|left, right| left.project_id.as_str().cmp(right.project_id.as_str()));
        Ok(bindings)
    }

    pub fn validate_resource_root(&self, project_id: &ProjectId) -> ProjectResult<()> {
        let binding = self
            .get(project_id)?
            .ok_or_else(|| ProjectError::NotFound(project_id.clone()))?;
        validate_project_directory(binding.primary_root())
    }
}

fn validate_descriptor(descriptor: &ProjectDescriptor) -> ProjectResult<()> {
    if descriptor.project_id.as_str().trim().is_empty() {
        return Err(ProjectError::InvalidProjectId);
    }
    if descriptor.resource_device_id.trim().is_empty() {
        return Err(ProjectError::InvalidResourceDevice);
    }
    if descriptor.display_name.trim().is_empty() {
        return Err(ProjectError::InvalidDisplayName);
    }
    Ok(())
}

fn remove_legacy_project_running_config(descriptor: &mut ProjectDescriptor) -> bool {
    descriptor
        .extra
        .remove(LEGACY_PROJECT_RUNNING_CONFIG_FIELD)
        .is_some()
}

fn sorted_descriptors<'a>(
    descriptors: impl Iterator<Item = &'a ProjectDescriptor>,
) -> Vec<ProjectDescriptor> {
    let mut descriptors = descriptors.cloned().collect::<Vec<_>>();
    descriptors.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.project_id.as_str().cmp(right.project_id.as_str()))
    });
    descriptors
}

fn overlap_warnings(
    records: &HashMap<ProjectId, StoredProjectRecord>,
    resource_device_id: &str,
    resource_root: &Path,
) -> Vec<ProjectOverlapWarning> {
    let mut warnings = records
        .values()
        .filter_map(|record| {
            let binding = record.binding.as_ref()?;
            if binding.resource_device_id != resource_device_id {
                return None;
            }
            let relation = binding.resources.directories.iter().find_map(|directory| {
                if directory.resource_root == resource_root {
                    Some(ProjectOverlapRelation::SharedDirectory)
                } else if resource_root.starts_with(&directory.resource_root) {
                    Some(ProjectOverlapRelation::ExistingAncestor)
                } else if directory.resource_root.starts_with(resource_root) {
                    Some(ProjectOverlapRelation::ExistingDescendant)
                } else {
                    None
                }
            })?;
            Some(ProjectOverlapWarning {
                project_id: record.descriptor.project_id.clone(),
                display_name: record.descriptor.display_name.clone(),
                display_path: record.descriptor.display_path.clone(),
                lifecycle: record.descriptor.lifecycle,
                relation,
            })
        })
        .collect::<Vec<_>>();
    warnings.sort_by(|left, right| left.project_id.as_str().cmp(right.project_id.as_str()));
    warnings
}

fn encode_file_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len() * 2);
    for byte in value.as_bytes() {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("write to String cannot fail");
    }
    encoded
}

pub fn project_tombstone_exists(root: &Path, project_id: &ProjectId) -> bool {
    root.join("tombstones")
        .join(format!(
            "{}.json",
            encode_file_component(project_id.as_str())
        ))
        .is_file()
}

fn remove_file_and_sync_parent(path: &Path) -> ProjectResult<()> {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    }
    let parent = path.parent().ok_or_else(|| {
        ProjectError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Project store path has no parent",
        ))
    })?;
    rua_core::fs::sync_directory(parent)?;
    Ok(())
}
