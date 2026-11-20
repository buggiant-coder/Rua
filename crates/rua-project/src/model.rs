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
    pub resources: crate::ProjectResources,
    pub lifecycle: ProjectLifecycle,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectCreateResourcesRequest {
    pub directories: Vec<crate::ProjectResourceDirectoryInput>,
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
