use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Result, RuaError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuaRoot {
    pub rua_root: PathBuf,
    pub secret_dir: PathBuf,
}

impl RuaRoot {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let parent = path.parent().unwrap_or(Path::new("/"));
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("rua");
        Self {
            secret_dir: parent.join(format!("{name}.secret")),
            rua_root: path,
        }
    }

    pub fn from_paths(path: impl Into<PathBuf>, secret_root: impl Into<PathBuf>) -> Self {
        Self {
            rua_root: path.into(),
            secret_dir: secret_root.into(),
        }
    }

    pub fn as_path(&self) -> &Path {
        &self.rua_root
    }

    pub fn secret_root(&self) -> &Path {
        &self.secret_dir
    }

    pub fn with_secret_root(mut self, secret_root: impl Into<PathBuf>) -> Self {
        self.secret_dir = secret_root.into();
        self
    }

    pub fn agent_workspace(&self, agent_id: &str) -> Result<WorkspaceRoot> {
        validate_owner_id("Agent", agent_id)?;
        Ok(WorkspaceRoot::new(
            self.rua_root.join("workspaces").join(agent_id),
        ))
    }

    pub fn project_workspace(&self, project_id: &str) -> Result<WorkspaceRoot> {
        validate_owner_id("Project", project_id)?;
        Ok(WorkspaceRoot::new(
            self.rua_root.join("projects").join(project_id),
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceRoot {
    path: PathBuf,
}

impl WorkspaceRoot {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn as_path(&self) -> &Path {
        &self.path
    }

    pub fn resolve(&self, relative: &str) -> Result<PathBuf> {
        if relative.trim().is_empty() || relative.contains('\\') {
            return Err(path_error(
                "workspace-relative path is empty or contains an invalid separator",
            ));
        }
        let path = Path::new(relative);
        for component in path.components() {
            match component {
                Component::ParentDir => {
                    return Err(path_error(format!(
                        "parent-relative path not allowed under Workspace: {relative}"
                    )));
                }
                Component::RootDir | Component::Prefix(_) => {
                    return Err(path_error(format!(
                        "absolute path not allowed under Workspace: {relative}"
                    )));
                }
                Component::CurDir | Component::Normal(_) => {}
            }
        }
        Ok(self.path.join(path))
    }

    pub fn media_dir(&self) -> PathBuf {
        self.path.join("media")
    }

    pub fn sessions_dir(&self) -> PathBuf {
        self.path.join("sessions")
    }

    pub fn chat_index_path(&self) -> PathBuf {
        self.path.join("chat-index.sqlite")
    }

    pub fn ensure(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.path)
    }
}

fn validate_owner_id(kind: &str, id: &str) -> Result<()> {
    if id.is_empty()
        || id == "."
        || id == ".."
        || id.contains('/')
        || id.contains('\\')
        || !id
            .chars()
            .all(|value| value.is_alphanumeric() || matches!(value, '-' | '_' | '.'))
    {
        return Err(path_error(format!(
            "{kind} ID contains invalid path characters: {id}"
        )));
    }
    Ok(())
}

fn path_error(message: impl Into<String>) -> RuaError {
    RuaError::Validation {
        message: message.into(),
    }
}
