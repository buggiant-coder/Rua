use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{normalize_project_root, validate_project_directory, ProjectError, ProjectResult};

/// Stable within a Project, including records migrated from a single root.
pub const INITIAL_PROJECT_DIRECTORY_ID: &str = "initial";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectResourceDirectory {
    pub directory_id: String,
    pub resource_root: PathBuf,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectResources {
    pub directories: Vec<ProjectResourceDirectory>,
    pub primary_directory_id: String,
    pub revision: u64,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl ProjectResources {
    pub fn create(paths: &[PathBuf], primary_index: usize) -> ProjectResult<Self> {
        if paths.is_empty() || primary_index >= paths.len() {
            return Err(ProjectError::InvalidResources(
                "Select at least one directory and a primary from that collection".into(),
            ));
        }
        let mut directories = Vec::with_capacity(paths.len());
        for (index, path) in paths.iter().enumerate() {
            let root = normalize_project_root(&path.to_string_lossy())?;
            validate_project_directory(&root)?;
            directories.push(ProjectResourceDirectory {
                directory_id: if index == 0 {
                    INITIAL_PROJECT_DIRECTORY_ID.to_string()
                } else {
                    uuid::Uuid::new_v4().to_string()
                },
                resource_root: root,
                extra: BTreeMap::new(),
            });
        }
        let resources = Self {
            primary_directory_id: directories[primary_index].directory_id.clone(),
            directories,
            revision: 1,
            extra: BTreeMap::new(),
        };
        resources.validate()?;
        Ok(resources)
    }

    pub fn edited(&self, paths: &[PathBuf], primary_index: usize) -> ProjectResult<Self> {
        self.edited_with_directory_ids(paths, primary_index, &[])
    }

    /// Explicit IDs rebind existing directories without changing Chat references.
    pub fn edited_with_directory_ids(
        &self,
        paths: &[PathBuf],
        primary_index: usize,
        directory_ids: &[Option<String>],
    ) -> ProjectResult<Self> {
        if !directory_ids.is_empty() && directory_ids.len() != paths.len() {
            return Err(ProjectError::InvalidResources(
                "Directory IDs must match the resource directory collection".into(),
            ));
        }
        let mut updated = Self::create(paths, primary_index)?;
        for (index, directory) in updated.directories.iter_mut().enumerate() {
            let existing = match directory_ids.get(index).and_then(Option::as_deref) {
                Some(id) => Some(self.directory(id)?),
                None => self
                    .directories
                    .iter()
                    .find(|old| old.resource_root == directory.resource_root),
            };
            if let Some(existing) = existing {
                directory.directory_id = existing.directory_id.clone();
                directory.extra = existing.extra.clone();
            } else {
                directory.directory_id = uuid::Uuid::new_v4().to_string();
            }
        }
        updated.primary_directory_id = updated.directories[primary_index].directory_id.clone();
        updated.revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| ProjectError::InvalidResources("Resource revision exhausted".into()))?;
        updated.extra = self.extra.clone();
        updated.validate()?;
        Ok(updated)
    }

    /// Migration preserves stored path identity even when the directory is offline.
    pub fn from_legacy_root(resource_root: PathBuf) -> Self {
        Self {
            directories: vec![ProjectResourceDirectory {
                directory_id: INITIAL_PROJECT_DIRECTORY_ID.to_string(),
                resource_root,
                extra: BTreeMap::new(),
            }],
            primary_directory_id: INITIAL_PROJECT_DIRECTORY_ID.to_string(),
            revision: 1,
            extra: BTreeMap::new(),
        }
    }

    pub fn validate(&self) -> ProjectResult<()> {
        if self.revision == 0 || self.directories.is_empty() {
            return Err(ProjectError::InvalidResources(
                "Resource revision and directory collection must not be empty".into(),
            ));
        }
        let mut ids = HashSet::new();
        let mut paths = HashSet::new();
        for directory in &self.directories {
            if directory.directory_id.trim().is_empty()
                || !ids.insert(&directory.directory_id)
                || !paths.insert(&directory.resource_root)
                || !directory.resource_root.is_absolute()
            {
                return Err(ProjectError::InvalidResources(
                    "Directory IDs and absolute paths must be nonempty and unique within a Project"
                        .into(),
                ));
            }
        }
        self.primary()?;
        Ok(())
    }

    pub fn directory(&self, directory_id: &str) -> ProjectResult<&ProjectResourceDirectory> {
        self.directories
            .iter()
            .find(|directory| directory.directory_id == directory_id)
            .ok_or_else(|| {
                ProjectError::InvalidResources(format!(
                    "Project directory does not exist: {directory_id}"
                ))
            })
    }

    pub fn primary(&self) -> ProjectResult<&ProjectResourceDirectory> {
        self.directory(&self.primary_directory_id)
    }
}
