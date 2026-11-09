use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rua_agent::cells::AgentCells;
use rua_agent::profile_store::AgentProfileStore;
use rua_config::AgentProfileConfig;
use rua_core::{Result, RuaError};
use rua_prompts::agent_files::{
    AgentPromptFileChange, AgentPromptFilePublisher, AgentPromptFileStore,
};
use serde::{Deserialize, Serialize};
use tracing::warn;

const PROMPTS_DIR: &str = "prompts";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRecord {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub modified: String,
    pub created_time: String,
    pub builtin: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileContentRecord {
    pub name: String,
    pub path: String,
    pub content: String,
    pub size: u64,
}

#[derive(Clone)]
pub struct FileManager {
    workspace_dir: PathBuf,
    data_dir: PathBuf,
    agent_id: Option<String>,
    agent_cells: Arc<AgentCells>,
    publisher: Option<Arc<dyn AgentPromptFilePublisher>>,
}

impl FileManager {
    pub fn new(
        workspace_dir: PathBuf,
        data_dir: PathBuf,
        agent_id: Option<String>,
        agent_cells: Arc<AgentCells>,
    ) -> Self {
        Self {
            workspace_dir,
            data_dir,
            agent_id,
            agent_cells,
            publisher: None,
        }
    }

    pub fn with_publisher(mut self, publisher: Arc<dyn AgentPromptFilePublisher>) -> Self {
        self.publisher = Some(publisher);
        self
    }

    async fn publish(&self, changes: Vec<AgentPromptFileChange>) {
        if let Some(publisher) = &self.publisher {
            publisher.publish(changes).await;
        }
    }

    async fn relative_file(
        &self,
        subdir: Option<&str>,
        name: &str,
        create: bool,
    ) -> Result<String> {
        Self::validate_path_component(name)?;
        let name = Self::ensure_md_extension(name);
        match subdir {
            None => Ok(name),
            Some(subdir) => {
                if create {
                    tokio::fs::create_dir_all(self.prompt_dir())
                        .await
                        .map_err(file_error)?;
                }
                let dir = Self::resolve_subdir(&self.prompt_dir(), subdir, create).await?;
                Ok(format!(
                    "{}/{}",
                    dir.file_name()
                        .expect("Prompt subdirectory")
                        .to_string_lossy(),
                    name
                ))
            }
        }
    }

    fn agent_id_str(&self) -> &str {
        self.agent_id.as_deref().unwrap_or("default")
    }

    fn prompt_dir(&self) -> PathBuf {
        self.workspace_dir.join(PROMPTS_DIR)
    }

    fn validate_path_component(component: &str) -> Result<()> {
        if component.contains("..")
            || component.contains('/')
            || component.contains('\\')
            || component.starts_with('/')
            || component.starts_with('\\')
            || component.is_empty()
        {
            return Err(RuaError::Validation {
                message: format!("Invalid path component: {component}"),
            });
        }
        Ok(())
    }

    fn ensure_md_extension(name: &str) -> String {
        if name.ends_with(".md") {
            name.to_string()
        } else {
            format!("{name}.md")
        }
    }

    async fn file_record_from_path(path: &Path) -> Result<FileRecord> {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let metadata = tokio::fs::metadata(path).await.map_err(|e| RuaError::Io {
            message: format!("Failed to stat {}: {e}", path.display()),
        })?;
        let epoch_zero = "1970-01-01T00:00:00+00:00".to_string();
        let modified = metadata
            .modified()
            .ok()
            .and_then(|t| {
                t.duration_since(std::time::UNIX_EPOCH).ok().map(|d| {
                    chrono::DateTime::from_timestamp(d.as_secs() as i64, d.subsec_nanos())
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_else(|| epoch_zero.clone())
                })
            })
            .unwrap_or_else(|| epoch_zero.clone());
        let created_time = metadata
            .created()
            .ok()
            .and_then(|t| {
                t.duration_since(std::time::UNIX_EPOCH).ok().map(|d| {
                    chrono::DateTime::from_timestamp(d.as_secs() as i64, d.subsec_nanos())
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_else(|| epoch_zero.clone())
                })
            })
            .unwrap_or(epoch_zero);
        Ok(FileRecord {
            name: name.clone(),
            path: name,
            size: metadata.len(),
            modified,
            created_time,
            builtin: false,
        })
    }

    async fn resolve_subdir(workspace: &Path, subdir: &str, create: bool) -> Result<PathBuf> {
        Self::validate_path_component(subdir)?;
        let target_upper = subdir.to_uppercase();

        if let Ok(mut entries) = tokio::fs::read_dir(workspace).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let name = entry.file_name().to_string_lossy().to_string();
                let ft = entry.file_type().await.ok();
                if ft.map(|t| t.is_dir()).unwrap_or(false) && name.to_uppercase() == target_upper {
                    return Ok(entry.path());
                }
            }
        }

        let dir_path = workspace.join(&target_upper);
        if create {
            tokio::fs::create_dir_all(&dir_path)
                .await
                .map_err(|e| RuaError::Io {
                    message: format!("Failed to create directory {}: {e}", dir_path.display()),
                })?;
        }
        Ok(dir_path)
    }

    pub async fn list_files(&self) -> Result<Vec<FileRecord>> {
        let prompt_dir = self.prompt_dir();
        if tokio::fs::metadata(&prompt_dir).await.is_err() {
            return Ok(vec![]);
        }
        let mut result = Vec::new();
        let mut entries = tokio::fs::read_dir(&prompt_dir)
            .await
            .map_err(|e| RuaError::Io {
                message: format!("Failed to read directory {}: {e}", prompt_dir.display()),
            })?;
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let is_file = entry
                .file_type()
                .await
                .map(|t| t.is_file())
                .unwrap_or(false);
            if is_file && name.ends_with(".md") {
                match Self::file_record_from_path(&path).await {
                    Ok(fe) => result.push(fe),
                    Err(e) => warn!("Skipping file {}: {e}", path.display()),
                }
            }
        }
        result.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(result)
    }

    pub async fn get_file(&self, path: &str) -> Result<Option<FileContentRecord>> {
        Self::validate_path_component(path)?;
        let md_name = Self::ensure_md_extension(path);
        let file_path = self.prompt_dir().join(&md_name);
        if tokio::fs::metadata(&file_path).await.is_err() {
            return Ok(None);
        }
        let content = tokio::fs::read_to_string(&file_path)
            .await
            .map_err(|e| RuaError::Io {
                message: format!("Failed to read {}: {e}", file_path.display()),
            })?;
        let metadata = tokio::fs::metadata(&file_path)
            .await
            .map_err(|e| RuaError::Io {
                message: format!("Failed to stat {}: {e}", file_path.display()),
            })?;
        Ok(Some(FileContentRecord {
            name: md_name.clone(),
            path: md_name,
            content: content.trim().to_string(),
            size: metadata.len(),
        }))
    }

    pub async fn write_file(&self, path: &str, content: String) -> Result<FileContentRecord> {
        let guard = AgentPromptFileStore::new(&self.workspace_dir).lock().await;
        let path = self.relative_file(None, path, true).await?;
        let change = guard.write(&path, &content, false).map_err(file_error)?;
        let record = committed_record(&change);
        self.publish(vec![change]).await;
        Ok(record)
    }

    pub async fn delete_file(&self, path: &str) -> Result<()> {
        let guard = AgentPromptFileStore::new(&self.workspace_dir).lock().await;
        let path = self.relative_file(None, path, false).await?;
        let change = guard.delete(&path).map_err(file_error)?;
        self.publish(vec![change]).await;
        Ok(())
    }

    pub async fn rename_file(&self, path: &str, new_name: &str) -> Result<FileContentRecord> {
        let guard = AgentPromptFileStore::new(&self.workspace_dir).lock().await;
        let old = self.relative_file(None, path, false).await?;
        let new = self.relative_file(None, new_name, false).await?;
        let changes = guard.rename(&old, &new).map_err(file_error)?;
        let record = committed_record(changes.last().expect("rename destination"));
        self.publish(changes).await;
        Ok(record)
    }

    pub async fn list_subdirs_info(&self) -> Result<HashMap<String, usize>> {
        let prompt_dir = self.prompt_dir();
        if tokio::fs::metadata(&prompt_dir).await.is_err() {
            return Ok(HashMap::new());
        }
        let mut subdirs: HashMap<String, PathBuf> = HashMap::new();
        if let Ok(mut entries) = tokio::fs::read_dir(&prompt_dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
                if is_dir {
                    let name = entry.file_name().to_string_lossy().to_string();
                    subdirs.insert(name.to_uppercase(), path);
                }
            }
        }
        let mut result = HashMap::new();
        if let Ok(mut entries) = tokio::fs::read_dir(&prompt_dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let name = entry.file_name().to_string_lossy().to_string();
                let is_file = entry
                    .file_type()
                    .await
                    .map(|t| t.is_file())
                    .unwrap_or(false);
                if is_file && name.ends_with(".md") {
                    let stem = name.trim_end_matches(".md");
                    let stem_upper = stem.to_uppercase();
                    if let Some(sub_path) = subdirs.get(&stem_upper) {
                        let count = Self::count_md_in_dir(sub_path).await;
                        if count > 0 {
                            result.insert(name, count);
                        }
                    }
                }
            }
        }
        Ok(result)
    }

    async fn count_md_in_dir(dir: &Path) -> usize {
        let mut count = 0;
        if let Ok(mut entries) = tokio::fs::read_dir(dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let name = entry.file_name().to_string_lossy().to_string();
                let is_file = entry
                    .file_type()
                    .await
                    .map(|t| t.is_file())
                    .unwrap_or(false);
                if is_file && name.ends_with(".md") {
                    count += 1;
                }
            }
        }
        count
    }

    pub async fn list_subdir_files(&self, subdir: &str) -> Result<Vec<FileRecord>> {
        let prompt_dir = self.prompt_dir();
        let dir_path = Self::resolve_subdir(&prompt_dir, subdir, false).await?;
        if tokio::fs::metadata(&dir_path).await.is_err() {
            return Ok(vec![]);
        }
        let mut result = Vec::new();
        let mut entries = tokio::fs::read_dir(&dir_path)
            .await
            .map_err(|e| RuaError::Io {
                message: format!("Failed to read directory {}: {e}", dir_path.display()),
            })?;
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let is_file = entry
                .file_type()
                .await
                .map(|t| t.is_file())
                .unwrap_or(false);
            if is_file && name.ends_with(".md") {
                match Self::file_record_from_path(&path).await {
                    Ok(fe) => result.push(fe),
                    Err(e) => warn!("Skipping file {}: {e}", path.display()),
                }
            }
        }
        result.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(result)
    }

    pub async fn create_subdir_file(
        &self,
        subdir: &str,
        req_filename: Option<String>,
        req_content: String,
    ) -> Result<FileContentRecord> {
        let guard = AgentPromptFileStore::new(&self.workspace_dir).lock().await;
        let name = req_filename
            .filter(|s| !s.is_empty())
            .or_else(|| (!req_content.is_empty()).then_some(req_content))
            .unwrap_or_else(|| "new.md".into());
        let path = self.relative_file(Some(subdir), &name, true).await?;
        let change = guard.write(&path, "", true).map_err(file_error)?;
        let record = committed_record(&change);
        self.publish(vec![change]).await;
        Ok(record)
    }

    pub async fn get_subdir_file(
        &self,
        subdir: &str,
        md_name: &str,
    ) -> Result<Option<FileContentRecord>> {
        Self::validate_path_component(md_name)?;
        let md_name_ext = Self::ensure_md_extension(md_name);
        let prompt_dir = self.prompt_dir();
        let dir_path = Self::resolve_subdir(&prompt_dir, subdir, false).await?;
        let file_path = dir_path.join(&md_name_ext);
        if tokio::fs::metadata(&file_path).await.is_err() {
            return Ok(None);
        }
        let content = tokio::fs::read_to_string(&file_path)
            .await
            .map_err(|e| RuaError::Io {
                message: format!("Failed to read {}: {e}", file_path.display()),
            })?;
        let metadata = tokio::fs::metadata(&file_path)
            .await
            .map_err(|e| RuaError::Io {
                message: format!("Failed to stat {}: {e}", file_path.display()),
            })?;
        Ok(Some(FileContentRecord {
            name: md_name_ext.clone(),
            path: format!("{subdir}/{md_name_ext}"),
            content: content.trim().to_string(),
            size: metadata.len(),
        }))
    }

    pub async fn write_subdir_file(
        &self,
        subdir: &str,
        md_name: &str,
        content: String,
    ) -> Result<FileContentRecord> {
        let guard = AgentPromptFileStore::new(&self.workspace_dir).lock().await;
        let path = self.relative_file(Some(subdir), md_name, true).await?;
        let change = guard.write(&path, &content, false).map_err(file_error)?;
        let record = committed_record(&change);
        self.publish(vec![change]).await;
        Ok(record)
    }

    pub async fn rename_subdir_file(
        &self,
        subdir: &str,
        md_name: &str,
        new_name: &str,
    ) -> Result<FileContentRecord> {
        let guard = AgentPromptFileStore::new(&self.workspace_dir).lock().await;
        let old = self.relative_file(Some(subdir), md_name, false).await?;
        let new = self.relative_file(Some(subdir), new_name, false).await?;
        let changes = guard.rename(&old, &new).map_err(file_error)?;
        let record = committed_record(changes.last().expect("rename destination"));
        self.publish(changes).await;
        Ok(record)
    }

    pub async fn delete_subdir_file(&self, subdir: &str, md_name: &str) -> Result<()> {
        let guard = AgentPromptFileStore::new(&self.workspace_dir).lock().await;
        let path = self.relative_file(Some(subdir), md_name, false).await?;
        let change = guard.delete(&path).map_err(file_error)?;
        self.publish(vec![change]).await;
        Ok(())
    }

    pub async fn load_agent_config(&self) -> Result<AgentProfileConfig> {
        let store = AgentProfileStore::new(self.data_dir.clone());
        store
            .load_agent_config_via_cells(&self.agent_cells, self.agent_id.as_deref())
            .await
            .map_err(RuaError::from)
    }

    pub async fn list_system_prompt_files(&self) -> Result<Vec<String>> {
        let files: Option<Vec<String>> = self
            .agent_cells
            .section::<Vec<String>>(self.agent_id_str(), "system_prompt_files")
            .read()
            .await
            .map_err(RuaError::from)?;
        Ok(files.unwrap_or_default())
    }

    pub async fn update_system_prompt_files(&self, files: Vec<String>) -> Result<Vec<String>> {
        self.agent_cells
            .section::<Vec<String>>(self.agent_id_str(), "system_prompt_files")
            .write(files.clone())
            .await
            .map_err(RuaError::from)?;
        Ok(files)
    }
}

fn committed_record(change: &AgentPromptFileChange) -> FileContentRecord {
    let content = change
        .content
        .as_ref()
        .expect("committed live Prompt file")
        .clone();
    FileContentRecord {
        name: Path::new(&change.path)
            .file_name()
            .expect("Prompt filename")
            .to_string_lossy()
            .into(),
        path: change.path.clone(),
        size: content.len() as u64,
        content,
    }
}

fn file_error(error: std::io::Error) -> RuaError {
    match error.kind() {
        std::io::ErrorKind::NotFound => RuaError::NotFound {
            resource: "file".into(),
            id: error.to_string(),
        },
        std::io::ErrorKind::AlreadyExists => RuaError::Conflict {
            resource: "file".into(),
            detail: error.to_string(),
        },
        std::io::ErrorKind::InvalidInput => RuaError::Validation {
            message: error.to_string(),
        },
        _ => RuaError::Io {
            message: error.to_string(),
        },
    }
}
