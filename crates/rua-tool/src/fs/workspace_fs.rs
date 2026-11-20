use std::path::{Path, PathBuf};

use crate::error::ToolError;

const SECRET_DIR_PATTERNS: &[&str] = &[".secret", ".ssh", ".gnupg", ".aws", ".azure"];

#[derive(Debug, Clone)]
pub struct WorkspaceFs {
    root: PathBuf,
    execution: Option<(rua_core::runtime_layout::RuntimeLayout, PathBuf)>,

    check_symlinks: bool,

    deny_secret_dirs: bool,
}

impl WorkspaceFs {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, ToolError> {
        let root = root.as_ref().canonicalize().map_err(|e| {
            ToolError::Internal(format!(
                "failed to canonicalize workspace root '{}': {}",
                root.as_ref().display(),
                e
            ))
        })?;
        Ok(Self {
            root,
            execution: None,
            check_symlinks: true,
            deny_secret_dirs: true,
        })
    }

    pub fn new_unchecked(root: PathBuf) -> Self {
        Self {
            root,
            execution: None,
            check_symlinks: true,
            deny_secret_dirs: true,
        }
    }

    pub fn disable_symlink_check(&mut self) {
        self.check_symlinks = false;
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn for_execution(
        root: &Path,
        layout: rua_core::runtime_layout::RuntimeLayout,
    ) -> Result<Self, ToolError> {
        let mut fs = Self::new(root)?;
        if layout.host_profile.is_mobile() || layout.os_family == "android" {
            let public_root = layout
                .linux_compat
                .as_ref()
                .and_then(|guest| guest.map_host_path(&fs.root))
                .ok_or_else(|| ToolError::PathSecurity {
                    message:
                        "Execution directory is unavailable in the Linux Compatibility Runtime"
                            .into(),
                })?;
            layout
                .execution_host_path(&public_root)
                .map_err(|e| ToolError::PathSecurity {
                    message: e.to_string(),
                })?;
            fs.execution = Some((layout, public_root));
        }
        Ok(fs)
    }

    pub(crate) fn public_path(&self, path: &Path) -> PathBuf {
        if let Some((layout, public_root)) = &self.execution {
            if let (Some(guest), Ok(suffix)) =
                (layout.linux_compat.as_ref(), path.strip_prefix("~"))
            {
                return guest.guest_home.join(suffix);
            }
            return public_root.join(path);
        }
        self.root.join(expand_tilde(path))
    }

    fn backing_path(&self, path: &Path) -> Result<PathBuf, ToolError> {
        let public = self.public_path(path);
        match &self.execution {
            Some((layout, _)) => {
                layout
                    .execution_host_path(&public)
                    .map_err(|e| ToolError::PathSecurity {
                        message: e.to_string(),
                    })
            }
            None => Ok(public),
        }
    }

    pub fn resolve(&self, path: impl AsRef<Path>) -> Result<PathBuf, ToolError> {
        self.resolve_host_path(&self.backing_path(path.as_ref())?)
    }

    pub(crate) fn resolve_host_path(&self, absolute: &Path) -> Result<PathBuf, ToolError> {
        let canonical = if self.check_symlinks {
            absolute.canonicalize().map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    ToolError::FileNotFound {
                        path: absolute.to_path_buf(),
                    }
                } else {
                    ToolError::Io(error)
                }
            })?
        } else {
            normalize_path(absolute)
        };

        if self.deny_secret_dirs {
            Self::check_secret_dir(&canonical)?;
        }

        Ok(canonical)
    }

    pub fn resolve_for_write(&self, path: impl AsRef<Path>) -> Result<PathBuf, ToolError> {
        let path = path.as_ref();
        let absolute = self.backing_path(path)?;

        let parent = absolute.parent().ok_or_else(|| ToolError::PathSecurity {
            message: format!("path '{}' has no parent directory", path.display()),
        })?;

        let canonical_parent = if self.check_symlinks {
            parent.canonicalize().map_err(|_| ToolError::FileNotFound {
                path: parent.to_path_buf(),
            })?
        } else {
            normalize_path(parent)
        };

        let file_name = absolute
            .file_name()
            .ok_or_else(|| ToolError::PathSecurity {
                message: format!("path '{}' has no file name", path.display()),
            })?;

        let target = canonical_parent.join(file_name);

        if self.deny_secret_dirs {
            Self::check_secret_dir(&target)?;
        }

        Ok(target)
    }

    pub fn resolve_for_create(&self, path: impl AsRef<Path>) -> Result<PathBuf, ToolError> {
        let path = path.as_ref();
        let absolute = self.backing_path(path)?;

        let normalized = if self.check_symlinks {
            let ancestor = absolute
                .ancestors()
                .find(|ancestor| std::fs::symlink_metadata(ancestor).is_ok())
                .ok_or_else(|| ToolError::FileNotFound {
                    path: absolute.clone(),
                })?;
            let backing = ancestor
                .canonicalize()
                .map_err(|_| ToolError::FileNotFound {
                    path: ancestor.into(),
                })?;
            normalize_path(&backing.join(absolute.strip_prefix(ancestor).expect("path ancestor")))
        } else {
            normalize_path(&absolute)
        };
        if self.deny_secret_dirs {
            Self::check_secret_dir(&normalized)?;
        }

        Ok(normalized)
    }

    /// Check protected directory names without resolving or opening the path.
    pub fn check_secret_dir(path: &Path) -> Result<(), ToolError> {
        let resolved_secret_name = rua_config::dirs::resolve_secret_dir()
            .file_name()
            .map(|n| n.to_string_lossy().to_string());

        for component in path.components() {
            if let std::path::Component::Normal(name) = component {
                let name_str = name.to_string_lossy();
                for pattern in SECRET_DIR_PATTERNS {
                    if name_str == *pattern {
                        return Err(ToolError::SecretDirDenied {
                            path: path.to_path_buf(),
                        });
                    }
                }
                if let Some(ref secret_name) = resolved_secret_name {
                    if *name_str == **secret_name {
                        return Err(ToolError::SecretDirDenied {
                            path: path.to_path_buf(),
                        });
                    }
                }
            }
        }
        Ok(())
    }
}

fn expand_tilde(path: &Path) -> PathBuf {
    let raw = path.as_os_str().to_string_lossy();
    let Some(home) = dirs::home_dir() else {
        return path.to_path_buf();
    };

    if raw == "~" {
        return home;
    }

    if let Some(rest) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) {
        return if rest.is_empty() {
            home
        } else {
            home.join(rest)
        };
    }

    path.to_path_buf()
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                components.pop();
            }
            std::path::Component::CurDir => {}
            _ => {
                components.push(component);
            }
        }
    }
    components.iter().collect()
}
