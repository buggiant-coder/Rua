use rua_core::execution_directory::is_device_absolute_path;
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{ProjectError, ProjectResult};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectResourceDirectoryInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory_id: Option<String>,
    pub device_id: String,
    pub resource_root: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl ProjectResourceDirectoryInput {
    pub fn new(device_id: impl Into<String>, resource_root: impl Into<PathBuf>) -> Self {
        Self {
            directory_id: None,
            device_id: device_id.into(),
            resource_root: resource_root.into(),
            display_name: None,
            extra: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectResourceDirectory {
    pub directory_id: String,
    pub device_id: String,
    pub resource_root: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectResources {
    pub revision: u64,
    pub directories: Vec<ProjectResourceDirectory>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Default for ProjectResources {
    fn default() -> Self {
        Self {
            directories: Vec::new(),
            revision: 1,
            extra: BTreeMap::new(),
        }
    }
}

impl ProjectResources {
    pub fn create(directories: Vec<ProjectResourceDirectoryInput>) -> ProjectResult<Self> {
        Self::default().build(directories, 1)
    }

    pub fn edited(&self, directories: Vec<ProjectResourceDirectoryInput>) -> ProjectResult<Self> {
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| ProjectError::InvalidResources("Resource revision exhausted".into()))?;
        self.build(directories, revision)
    }

    /// Owners validate new paths before this metadata-only operation.
    fn build(
        &self,
        inputs: Vec<ProjectResourceDirectoryInput>,
        revision: u64,
    ) -> ProjectResult<Self> {
        let mut directories = Vec::with_capacity(inputs.len());
        let mut supplied_ids = HashSet::new();
        for input in inputs {
            let same_location = |d: &ProjectResourceDirectory| {
                d.device_id == input.device_id
                    && d.resource_root.as_os_str() == input.resource_root.as_os_str()
            };
            let existing = if let Some(id) = input.directory_id {
                if !supplied_ids.insert(id.clone()) {
                    return Err(ProjectError::InvalidResources(format!(
                        "Repeated directory ID: {id}"
                    )));
                }
                Some(self.directory(&id)?).filter(|d| same_location(d))
            } else {
                self.directories.iter().find(|d| same_location(d))
            };
            let mut extra = existing.map(|d| d.extra.clone()).unwrap_or_default();
            extra.extend(input.extra);
            directories.push(ProjectResourceDirectory {
                directory_id: existing
                    .map(|d| d.directory_id.clone())
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                device_id: input.device_id,
                resource_root: input.resource_root,
                display_name: input
                    .display_name
                    .map(|name| name.trim().to_owned())
                    .filter(|name| !name.is_empty()),
                extra,
            });
        }
        let result = Self {
            revision,
            directories,
            extra: self.extra.clone(),
        };
        result.validate()?;
        Ok(result)
    }

    pub fn validate(&self) -> ProjectResult<()> {
        if self.revision == 0 {
            return Err(ProjectError::InvalidResources(
                "Resource revision must be positive".into(),
            ));
        }
        let mut ids = HashSet::new();
        let mut locations = HashSet::new();
        for directory in &self.directories {
            if directory.device_id.trim().is_empty() {
                return Err(ProjectError::InvalidResourceDevice);
            }
            let path = directory.resource_root.to_str().unwrap_or_default();
            if directory.directory_id.trim().is_empty()
                || !ids.insert(&directory.directory_id)
                || !locations.insert((&directory.device_id, path))
                || !is_device_absolute_path(path)
            {
                return Err(ProjectError::InvalidResources(
                    "Resource IDs and Device/absolute path pairs must be nonempty and unique"
                        .into(),
                ));
            }
        }
        Ok(())
    }

    pub fn directory(&self, directory_id: &str) -> ProjectResult<&ProjectResourceDirectory> {
        self.directories
            .iter()
            .find(|d| d.directory_id == directory_id)
            .ok_or_else(|| {
                ProjectError::InvalidResources(format!(
                    "Project directory does not exist: {directory_id}"
                ))
            })
    }
}
