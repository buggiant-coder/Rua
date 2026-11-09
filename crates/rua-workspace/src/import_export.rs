use std::path::{Path, PathBuf};

use rua_core::{Result, RuaError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceInfoRecord {
    pub workspace_dir: String,
    pub total_size: u64,
    pub file_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceImportSummary {
    pub success: bool,
    pub agents_imported: u32,
    pub message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct WorkspaceImportExport {
    workspace_dir: PathBuf,
    agent_id: String,
}

impl WorkspaceImportExport {
    pub fn new(workspace_dir: PathBuf, agent_id: impl Into<String>) -> Self {
        Self {
            workspace_dir,
            agent_id: agent_id.into(),
        }
    }

    pub fn from_runtime(runtime: &crate::WorkspaceRuntime) -> Self {
        Self::new(runtime.workspace_dir(), runtime.agent_id.clone())
    }

    pub fn export(&self) -> Result<Vec<u8>> {
        if !self.workspace_dir.is_dir() {
            return Err(RuaError::NotFound {
                resource: "workspace".into(),
                id: self.agent_id.clone(),
            });
        }
        zip_directory(&self.workspace_dir)
    }

    pub fn import(&self, data: &[u8]) -> Result<WorkspaceImportSummary> {
        validate_zip_data(data, &self.workspace_dir)?;
        extract_and_merge_zip(data, &self.workspace_dir)?;
        Ok(WorkspaceImportSummary {
            success: true,
            agents_imported: 1,
            message: Some("Imported successfully".into()),
        })
    }

    pub fn workspace_info(&self) -> Option<WorkspaceInfoRecord> {
        if !self.workspace_dir.exists() {
            return None;
        }
        let (file_count, total_size) = dir_stats(&self.workspace_dir);
        Some(WorkspaceInfoRecord {
            workspace_dir: self.workspace_dir.to_string_lossy().to_string(),
            total_size,
            file_count,
        })
    }
}

fn dir_stats(root: &Path) -> (u32, u64) {
    let mut count: u32 = 0;
    let mut size: u64 = 0;
    if root.is_dir() {
        for entry in walkdir::WalkDir::new(root)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() {
                count += 1;
                if let Ok(meta) = entry.metadata() {
                    size += meta.len();
                }
            }
        }
    }
    (count, size)
}

fn zip_directory(root: &Path) -> Result<Vec<u8>> {
    use std::io::{Cursor, Write};
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    let buf = Vec::new();
    let cursor = Cursor::new(buf);
    let mut zip = ZipWriter::new(cursor);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    let mut entries: Vec<walkdir::DirEntry> = walkdir::WalkDir::new(root)
        .sort_by_file_name()
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path() != root)
        .collect();
    entries.sort_by(|a, b| a.path().cmp(b.path()));

    for entry in &entries {
        let rel_path = entry.path().strip_prefix(root).map_err(|e| RuaError::Io {
            message: format!("Failed to compute relative path: {e}"),
        })?;
        let arcname = rel_path.to_string_lossy().replace('\\', "/");

        if entry.file_type().is_file() {
            let content = match std::fs::read(entry.path()) {
                Ok(content) => content,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    continue;
                }
                Err(e) => {
                    return Err(RuaError::Io {
                        message: format!("Failed to read {}: {e}", entry.path().display()),
                    });
                }
            };
            zip.start_file(&arcname, options)
                .map_err(|e| RuaError::Io {
                    message: format!("Failed to start zip file entry {arcname}: {e}"),
                })?;
            zip.write_all(&content).map_err(|e| RuaError::Io {
                message: format!("Failed to write zip entry {arcname}: {e}"),
            })?;
        } else if entry.file_type().is_dir() {
            zip.add_directory(format!("{arcname}/"), options)
                .map_err(|e| RuaError::Io {
                    message: format!("Failed to add zip directory {arcname}: {e}"),
                })?;
        }
    }

    let cursor = zip.finish().map_err(|e| RuaError::Io {
        message: format!("Failed to finalize zip: {e}"),
    })?;
    Ok(cursor.into_inner())
}

fn validate_zip_data(data: &[u8], workspace_dir: &Path) -> Result<()> {
    use std::io::Cursor;
    let cursor = Cursor::new(data);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|_| RuaError::Validation {
        message: "Uploaded file is not a valid zip archive".into(),
    })?;
    let workspace_root = workspace_dir
        .canonicalize()
        .unwrap_or_else(|_| workspace_dir.to_path_buf());
    for i in 0..archive.len() {
        let file = archive.by_index(i).map_err(|e| RuaError::Io {
            message: format!("Failed to read zip entry: {e}"),
        })?;
        let name = file.name().to_string();
        let Some(enclosed_name) = file.enclosed_name() else {
            return Err(RuaError::Validation {
                message: format!("Zip contains unsafe path: {name}"),
            });
        };
        let resolved = workspace_root.join(enclosed_name);
        let resolved = resolved.canonicalize().unwrap_or(resolved);
        if !resolved.starts_with(&workspace_root) {
            return Err(RuaError::Validation {
                message: format!("Zip contains unsafe path: {name}"),
            });
        }
    }
    Ok(())
}

fn extract_and_merge_zip(data: &[u8], workspace_dir: &Path) -> Result<()> {
    use std::io::Cursor;

    let tmp_dir = tempfile::tempdir().map_err(|e| RuaError::Io {
        message: format!("Failed to create temp dir: {e}"),
    })?;

    let cursor = Cursor::new(data);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|e| RuaError::Io {
        message: format!("Failed to open zip: {e}"),
    })?;
    archive.extract(tmp_dir.path()).map_err(|e| RuaError::Io {
        message: format!("Failed to extract zip: {e}"),
    })?;

    let top_entries: Vec<_> = std::fs::read_dir(tmp_dir.path())
        .map_err(|e| RuaError::Io {
            message: format!("Failed to read temp dir: {e}"),
        })?
        .filter_map(|e| e.ok())
        .collect();

    let extract_root = if top_entries.len() == 1
        && top_entries[0]
            .file_type()
            .map(|t| t.is_dir())
            .unwrap_or(false)
        && should_unwrap_top_level_dir(&top_entries[0].file_name().to_string_lossy())
    {
        top_entries[0].path()
    } else {
        tmp_dir.path().to_path_buf()
    };

    std::fs::create_dir_all(workspace_dir).map_err(|e| RuaError::Io {
        message: format!("Failed to create workspace dir: {e}"),
    })?;

    for entry in std::fs::read_dir(&extract_root).map_err(|e| RuaError::Io {
        message: format!("Failed to read extract root: {e}"),
    })? {
        let entry = entry.map_err(|e| RuaError::Io {
            message: format!("Failed to read entry: {e}"),
        })?;
        let dest = workspace_dir.join(entry.file_name());
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            std::fs::copy(entry.path(), &dest).map_err(|e| RuaError::Io {
                message: format!("Failed to copy file: {e}"),
            })?;
        } else if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            if dest.exists() && dest.is_file() {
                std::fs::remove_file(&dest).map_err(|e| RuaError::Io {
                    message: format!("Failed to remove file before merge: {e}"),
                })?;
            }
            copy_dir_recursive(&entry.path(), &dest)?;
        }
    }

    Ok(())
}

fn should_unwrap_top_level_dir(name: &str) -> bool {
    !matches!(
        name,
        ".rua"
            | "MEMORY"
            | "memory"
            | "prompts"
            | "sessions"
            | "skills"
            | "media"
            | "prompt-state"
            | "backup"
            | "tool_call_logs"
    )
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst).map_err(|e| RuaError::Io {
        message: format!("Failed to create dir {}: {e}", dst.display()),
    })?;
    for entry in std::fs::read_dir(src).map_err(|e| RuaError::Io {
        message: format!("Failed to read dir {}: {e}", src.display()),
    })? {
        let entry = entry.map_err(|e| RuaError::Io {
            message: format!("Failed to read dir entry: {e}"),
        })?;
        let dest = dst.join(entry.file_name());
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            std::fs::copy(entry.path(), &dest).map_err(|e| RuaError::Io {
                message: format!("Failed to copy {}: {e}", entry.path().display()),
            })?;
        } else if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            copy_dir_recursive(&entry.path(), &dest)?;
        }
    }
    Ok(())
}
