use std::path::{Component, Path, PathBuf};

use crate::error::WorkspaceError;

pub use rua_core::{RuaRoot, WorkspaceRoot};

pub trait WorkspaceFs: Send + Sync {
    fn resolve_user_path(&self, path: &str) -> Result<PathBuf, WorkspaceError>;

    fn is_inside_workspace(&self, path: &Path) -> bool;

    fn is_secret_path(&self, path: &Path) -> bool;

    fn canonicalize_under(&self, root: &Path, relative: &str) -> Result<PathBuf, WorkspaceError>;
}

pub struct DefaultWorkspaceFs {
    root: WorkspaceRoot,
    secret_dir: Option<PathBuf>,
}

impl DefaultWorkspaceFs {
    pub fn new(root: WorkspaceRoot) -> Self {
        Self {
            root,
            secret_dir: None,
        }
    }

    pub fn with_secret_dir(mut self, secret_dir: impl Into<PathBuf>) -> Self {
        self.secret_dir = Some(secret_dir.into());
        self
    }

    fn normalize_path(path: &Path) -> PathBuf {
        let mut result = PathBuf::new();
        for component in path.components() {
            match component {
                Component::ParentDir => result.push(component.as_os_str()),
                Component::CurDir => {}
                other => result.push(other),
            }
        }
        result
    }

    fn is_inside(parent: &Path, child: &Path) -> bool {
        let parent_normalized = Self::normalize_path(parent);
        let child_normalized = Self::normalize_path(child);
        child_normalized.starts_with(&parent_normalized)
    }
}

impl WorkspaceFs for DefaultWorkspaceFs {
    fn resolve_user_path(&self, path: &str) -> Result<PathBuf, WorkspaceError> {
        let normalized = self
            .root
            .resolve(path)
            .map_err(|error| WorkspaceError::path_invalid(error.to_string()))?;

        if self
            .secret_dir
            .as_deref()
            .is_some_and(|secret| Self::is_inside(secret, &normalized))
        {
            return Err(WorkspaceError::path_invalid(
                "Access to secret directory denied",
            ));
        }

        Ok(normalized)
    }

    fn is_inside_workspace(&self, path: &Path) -> bool {
        Self::is_inside(self.root.as_path(), path)
    }

    fn is_secret_path(&self, path: &Path) -> bool {
        self.secret_dir
            .as_deref()
            .is_some_and(|secret| Self::is_inside(secret, path))
    }

    fn canonicalize_under(&self, root: &Path, relative: &str) -> Result<PathBuf, WorkspaceError> {
        let candidate = lexical_resolve_under(root, relative)?;
        let canon_root = root
            .canonicalize()
            .map_err(|e| WorkspaceError::path_invalid(format!("canonicalize root: {e}")))?;
        let canon_candidate = candidate
            .canonicalize()
            .map_err(|e| WorkspaceError::path_invalid(format!("canonicalize target: {e}")))?;

        if !canon_candidate.starts_with(&canon_root) {
            return Err(WorkspaceError::path_invalid(format!(
                "path escapes root after canonicalization: {relative}"
            )));
        }

        Ok(canon_candidate)
    }
}

fn lexical_resolve_under(root: &Path, relative: &str) -> Result<PathBuf, WorkspaceError> {
    if relative.trim().is_empty() || relative.contains('\\') {
        return Err(WorkspaceError::path_invalid(
            "workspace-relative path is empty or contains an invalid separator",
        ));
    }
    let rel = Path::new(relative);
    for component in rel.components() {
        match component {
            Component::ParentDir => {
                return Err(WorkspaceError::path_invalid(format!(
                    "parent-relative path not allowed under workspace: {relative}"
                )));
            }
            Component::Normal(_) => {}
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) => {
                return Err(WorkspaceError::path_invalid(format!(
                    "absolute path not allowed under root: {relative}"
                )));
            }
        }
    }
    Ok(root.join(rel))
}
