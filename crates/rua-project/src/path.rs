use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use crate::{
    ProjectDirectoryEntry, ProjectDirectoryEntryKind, ProjectDirectoryListing, ProjectError,
    ProjectResult,
};

pub fn normalize_project_root(input: &str) -> ProjectResult<PathBuf> {
    normalize_project_root_with(input, dirs::home_dir().as_deref(), std::env::current_dir())
}

fn normalize_project_root_with(
    input: &str,
    home: Option<&Path>,
    current_dir: std::io::Result<PathBuf>,
) -> ProjectResult<PathBuf> {
    let input = input.trim();
    if input.is_empty() {
        return Err(ProjectError::InvalidPath(
            "project path must not be empty".to_string(),
        ));
    }

    let expanded = if input == "~" {
        home.ok_or_else(|| ProjectError::InvalidPath("home directory is unavailable".to_string()))?
            .to_path_buf()
    } else if let Some(rest) = input.strip_prefix("~/") {
        home.ok_or_else(|| ProjectError::InvalidPath("home directory is unavailable".to_string()))?
            .join(rest)
    } else {
        PathBuf::from(input)
    };

    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        current_dir?.join(expanded)
    };
    Ok(lexically_normalize(&absolute))
}

pub fn validate_project_directory(path: &Path) -> ProjectResult<()> {
    std::fs::read_dir(path)
        .map(|_| ())
        .map_err(|error| ProjectError::DirectoryUnavailable {
            path: path.to_path_buf(),
            detail: error.to_string(),
        })
}

pub fn list_project_directory(input: &str) -> ProjectResult<ProjectDirectoryListing> {
    let path = normalize_project_root(input)?;
    let read_dir =
        std::fs::read_dir(&path).map_err(|error| ProjectError::DirectoryUnavailable {
            path: path.clone(),
            detail: error.to_string(),
        })?;
    let mut entries = Vec::new();
    for entry in read_dir {
        let entry = entry?;
        let entry_path = entry.path();
        let Ok(link_metadata) = std::fs::symlink_metadata(&entry_path) else {
            continue;
        };
        let Ok(target_metadata) = std::fs::metadata(&entry_path) else {
            continue;
        };
        let kind = if target_metadata.is_dir() {
            ProjectDirectoryEntryKind::Directory
        } else if target_metadata.is_file() {
            ProjectDirectoryEntryKind::File
        } else {
            ProjectDirectoryEntryKind::Other
        };
        entries.push(ProjectDirectoryEntry {
            name: entry.file_name().to_string_lossy().to_string(),
            path: entry_path,
            kind,
            is_symlink: link_metadata.file_type().is_symlink(),
        });
    }
    entries.sort_by(|left, right| {
        directory_rank(left.kind)
            .cmp(&directory_rank(right.kind))
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok(ProjectDirectoryListing {
        path,
        entries,
        roots: project_directory_roots(),
    })
}

fn project_directory_roots() -> Vec<ProjectDirectoryEntry> {
    let mut candidates = Vec::new();
    if let Some(home) = dirs::home_dir() {
        candidates.push(("Home".to_string(), home));
    }
    #[cfg(target_os = "android")]
    {
        candidates.push((
            "Shared storage".to_string(),
            PathBuf::from("/storage/emulated/0"),
        ));
        if let Ok(storage) = std::fs::read_dir("/storage") {
            for entry in storage.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name != "self" && name != "emulated" && entry.path().is_dir() {
                    candidates.push((format!("SD card ({name})"), entry.path()));
                }
            }
        }
    }
    candidates
        .into_iter()
        .map(|(name, path)| ProjectDirectoryEntry {
            name,
            path,
            kind: ProjectDirectoryEntryKind::Directory,
            is_symlink: false,
        })
        .collect()
}

pub fn create_project_directory(
    parent: &str,
    name: &str,
) -> ProjectResult<ProjectDirectoryListing> {
    if name.trim().is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
        return Err(ProjectError::InvalidPath(
            "folder name must be a single directory name".to_string(),
        ));
    }
    let parent = normalize_project_root(parent)?;
    validate_project_directory(&parent)?;
    let path = parent.join(name);
    std::fs::create_dir(&path).map_err(|error| ProjectError::DirectoryUnavailable {
        path: path.clone(),
        detail: error.to_string(),
    })?;
    #[cfg(unix)]
    {
        std::fs::File::open(&path)?.sync_all()?;
        std::fs::File::open(&parent)?.sync_all()?;
    }
    list_project_directory(&path.to_string_lossy())
}

pub fn propose_project_display_name(path: &Path) -> String {
    path.file_name()
        .filter(|name| !name.is_empty())
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

fn directory_rank(kind: ProjectDirectoryEntryKind) -> u8 {
    match kind {
        ProjectDirectoryEntryKind::Directory => 0,
        ProjectDirectoryEntryKind::File => 1,
        ProjectDirectoryEntryKind::Other => 2,
    }
}

fn lexically_normalize(path: &Path) -> PathBuf {
    let mut prefix: Option<OsString> = None;
    let mut rooted = false;
    let mut parts: Vec<OsString> = Vec::new();

    for component in path.components() {
        match component {
            Component::Prefix(value) => prefix = Some(value.as_os_str().to_os_string()),
            Component::RootDir => {
                rooted = true;
                parts.clear();
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if parts.last().is_some_and(|part| part != "..") {
                    parts.pop();
                } else if !rooted {
                    parts.push(OsString::from(".."));
                }
            }
            Component::Normal(value) => parts.push(value.to_os_string()),
        }
    }

    let mut normalized = PathBuf::new();
    if let Some(prefix) = prefix {
        normalized.push(prefix);
    }
    if rooted {
        normalized.push(Path::new(std::path::MAIN_SEPARATOR_STR));
    }
    for part in parts {
        normalized.push(part);
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_expands_home_and_cleans_components_lexically() {
        let result = normalize_project_root_with(
            "~/repo/./crates/../libs",
            Some(Path::new("/home/tester")),
            Ok(PathBuf::from("/work")),
        )
        .unwrap();
        assert_eq!(result, PathBuf::from("/home/tester/repo/libs"));
    }

    #[test]
    fn normalization_keeps_distinct_symlink_alias_text() {
        let current = PathBuf::from("/work");
        let alias =
            normalize_project_root_with("/repos/current", None, Ok(current.clone())).unwrap();
        let target = normalize_project_root_with("/repos/release", None, Ok(current)).unwrap();
        assert_ne!(alias, target);
    }
}
