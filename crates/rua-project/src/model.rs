use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use rua_core::ProjectId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectLifecycle {
    Active,
    Archived,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectDescriptor {
    pub project_id: ProjectId,
    pub display_name: String,
    pub resource_device_id: String,
    pub display_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<ProjectResourcesDescriptor>,
    pub lifecycle: ProjectLifecycle,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectResourceBinding {
    pub project_id: ProjectId,
    pub resource_device_id: String,
    pub resources: crate::ProjectResources,
    pub created_at: DateTime<Utc>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl ProjectResourceBinding {
    /// Store creation and loading validate the collection before exposing a binding.
    pub fn primary_root(&self) -> &std::path::Path {
        &self
            .resources
            .primary()
            .expect("validated Project resources")
            .resource_root
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectCreateResourcesRequest {
    pub resource_device_id: String,
    pub resource_roots: Vec<PathBuf>,
    pub primary_index: usize,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectCreateRequest {
    pub resource_device_id: String,
    pub resource_root: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectOverlapRelation {
    SharedDirectory,
    ExistingAncestor,
    ExistingDescendant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectOverlapWarning {
    pub project_id: ProjectId,
    pub display_name: String,
    pub display_path: String,
    pub lifecycle: ProjectLifecycle,
    pub relation: ProjectOverlapRelation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectCreationOutcome {
    pub project: ProjectDescriptor,
    #[serde(default)]
    pub overlap_warnings: Vec<ProjectOverlapWarning>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectDirectoryEntryKind {
    Directory,
    File,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectDirectoryEntry {
    pub name: String,
    pub path: PathBuf,
    pub kind: ProjectDirectoryEntryKind,
    pub is_symlink: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectDirectoryListing {
    pub path: PathBuf,
    pub entries: Vec<ProjectDirectoryEntry>,
    pub roots: Vec<ProjectDirectoryEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectResourcesDescriptor {
    pub revision: u64,
    pub primary_directory_id: String,
    pub directories: Vec<ProjectDirectoryDescriptor>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectDirectoryDescriptor {
    pub directory_id: String,
    pub display_path: String,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl From<&crate::ProjectResources> for ProjectResourcesDescriptor {
    fn from(resources: &crate::ProjectResources) -> Self {
        Self {
            revision: resources.revision,
            primary_directory_id: resources.primary_directory_id.clone(),
            directories: resources
                .directories
                .iter()
                .map(|directory| ProjectDirectoryDescriptor {
                    directory_id: directory.directory_id.clone(),
                    display_path: directory.resource_root.to_string_lossy().to_string(),
                    extra: BTreeMap::new(),
                })
                .collect(),
            extra: BTreeMap::new(),
        }
    }
}
