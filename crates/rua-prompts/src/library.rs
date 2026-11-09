use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rua_core::{Result, RuaError};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::archive;
use crate::file_store::{PromptFileChange, PromptFileStore};

#[async_trait::async_trait]
pub trait PromptLibraryPublisher: Send + Sync {
    async fn publish(&self, changes: Vec<PromptFileChange>);
}

pub struct SeedEntry {
    pub relative_path: String,
    pub content: Vec<u8>,
}

pub struct RestoreBuiltinResult {
    pub message: String,
    pub seeded: Vec<SeedEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    #[serde(rename = "filename")]
    pub name: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default, rename = "modified_time")]
    pub modified: String,
    #[serde(default)]
    pub created_time: String,
    #[serde(default)]
    pub builtin: bool,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileContent {
    #[serde(rename = "filename")]
    pub name: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub size: u64,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptEntry {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub is_shared: bool,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptCreate {
    pub title: String,
    pub content: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptUpdate {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptBroadcastRequest {
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub agent_ids: Vec<String>,
    #[serde(default = "default_conflict_mode")]
    pub conflict_mode: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn default_conflict_mode() -> String {
    "ask".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BroadcastFileResult {
    pub agent_id: String,
    pub file: String,
    pub final_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflict: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggested_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptBroadcastResponse {
    pub results: Vec<BroadcastFileResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptLibrarySyncRequest {
    #[serde(default)]
    pub agent_ids: Vec<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptLibrarySyncResponse {
    pub success: bool,
    #[serde(default)]
    pub results: Vec<serde_json::Value>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

pub struct PromptLibrary {
    data_dir: PathBuf,
    publisher: Option<Arc<dyn PromptLibraryPublisher>>,
}

impl PromptLibrary {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            publisher: None,
        }
    }

    pub fn with_publisher(mut self, publisher: Arc<dyn PromptLibraryPublisher>) -> Self {
        self.publisher = Some(publisher);
        self
    }

    async fn publish(&self, changes: Vec<PromptFileChange>) {
        if let Some(publisher) = &self.publisher {
            publisher.publish(changes).await;
        }
    }

    fn relative_file(&self, path: &Path) -> Result<String> {
        path.strip_prefix(self.lib_dir())
            .map(|p| {
                p.components()
                    .map(|part| part.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .map_err(|e| RuaError::Validation {
                message: e.to_string(),
            })
    }

    async fn commit_write(
        &self,
        path: &Path,
        content: &str,
        create_only: bool,
    ) -> Result<FileContent> {
        let relative = self.relative_file(path)?;
        let guard = PromptFileStore::for_library(&self.data_dir).lock().await;
        let change = guard
            .write(&relative, content, create_only)
            .map_err(prompt_file_error)?;
        let result = Self::file_content_from_path(path);
        drop(guard);
        self.publish(vec![change]).await;
        result
    }

    async fn commit_delete(&self, path: &Path) -> Result<()> {
        let guard = PromptFileStore::for_library(&self.data_dir).lock().await;
        let change = guard
            .delete(&self.relative_file(path)?)
            .map_err(prompt_file_error)?;
        drop(guard);
        self.publish(vec![change]).await;
        Ok(())
    }

    async fn commit_rename(&self, old: &Path, new: &Path) -> Result<FileContent> {
        let guard = PromptFileStore::for_library(&self.data_dir).lock().await;
        let changes = guard
            .rename(&self.relative_file(old)?, &self.relative_file(new)?)
            .map_err(prompt_file_error)?;
        let result = Self::file_content_from_path(new);
        drop(guard);
        self.publish(changes).await;
        result
    }

    pub fn lib_dir(&self) -> PathBuf {
        self.data_dir.join("prompt_library")
    }

    fn ensure_lib_dir(&self) -> Result<PathBuf> {
        let dir = self.lib_dir();
        std::fs::create_dir_all(&dir).map_err(|e| RuaError::Io {
            message: format!("Failed to create prompt_library dir: {e}"),
        })?;
        Ok(dir)
    }

    fn prompts_json_path(&self) -> PathBuf {
        self.lib_dir().join("prompts.json")
    }

    fn load_prompts(&self) -> Result<Vec<PromptEntry>> {
        let path = self.prompts_json_path();
        match std::fs::read_to_string(&path) {
            Ok(content) => match serde_json::from_str::<Vec<PromptEntry>>(&content) {
                Ok(prompts) => Ok(prompts),
                Err(e) => {
                    warn!("Failed to parse prompts.json: {e}");
                    Ok(Vec::new())
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => {
                warn!("Failed to read prompts.json: {e}");
                Ok(Vec::new())
            }
        }
    }

    fn save_prompts(&self, prompts: &[PromptEntry]) -> Result<()> {
        self.ensure_lib_dir()?;
        let path = self.prompts_json_path();
        let value = serde_json::to_value(prompts).map_err(|e| RuaError::Config {
            message: format!("Failed to serialize prompts.json: {e}"),
        })?;
        rua_config::atomic_write_json(&path, &value).map_err(|e| RuaError::Io {
            message: format!("Failed to write prompts.json: {e}"),
        })?;
        Ok(())
    }

    pub(crate) fn validate_name(name: &str) -> Result<()> {
        if name.contains("..") || name.starts_with('/') || name.starts_with('\\') || name.is_empty()
        {
            return Err(RuaError::Validation {
                message: format!("Invalid name: {name}"),
            });
        }
        Ok(())
    }

    fn ensure_md_ext(name: &str) -> String {
        if name.ends_with(".md") {
            name.to_string()
        } else {
            format!("{name}.md")
        }
    }

    fn compact_path(path: &Path) -> String {
        if let Some(home) = dirs::home_dir() {
            let home_str = home.to_string_lossy();
            let path_str = path.to_string_lossy();
            if path_str.as_ref() == home_str.as_ref() {
                return "~".to_string();
            }
            let prefix = format!("{}/", home_str);
            if let Some(rest) = path_str.strip_prefix(&prefix) {
                return format!("~/{rest}");
            }
        }
        path.to_string_lossy().to_string()
    }

    fn file_entry_from_path(path: &Path) -> Result<FileEntry> {
        let meta = std::fs::metadata(path).map_err(|e| RuaError::Io {
            message: format!("Failed to stat {}: {e}", path.display()),
        })?;
        let epoch_zero = "1970-01-01T00:00:00+00:00".to_string();
        let modified = meta
            .modified()
            .ok()
            .map(|t| {
                chrono::DateTime::<chrono::Utc>::from(t)
                    .format("%Y-%m-%dT%H:%M:%S")
                    .to_string()
            })
            .unwrap_or_else(|| epoch_zero.clone());
        let created_time = meta
            .created()
            .ok()
            .map(|t| {
                chrono::DateTime::<chrono::Utc>::from(t)
                    .format("%Y-%m-%dT%H:%M:%S")
                    .to_string()
            })
            .unwrap_or(epoch_zero);
        Ok(FileEntry {
            name: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            path: Self::compact_path(path),
            size: meta.len(),
            modified,
            created_time,
            builtin: false,
            extra: serde_json::Map::new(),
        })
    }

    fn file_content_from_path(path: &Path) -> Result<FileContent> {
        let content = std::fs::read_to_string(path).map_err(|e| RuaError::Io {
            message: format!("Failed to read {}: {e}", path.display()),
        })?;
        let size = content.len() as u64;
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        Ok(FileContent {
            name,
            path: Self::compact_path(path),
            content: content.trim().to_string(),
            size,
            extra: serde_json::Map::new(),
        })
    }

    fn resolve_subdir(&self, subdir: &str, create: bool) -> Result<PathBuf> {
        Self::validate_name(subdir)?;
        let lib = self.lib_dir();
        let target_upper = subdir.to_uppercase();

        if lib.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&lib) {
                for entry in entries.flatten() {
                    if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                        let name = entry.file_name().to_string_lossy().to_uppercase();
                        if name == target_upper {
                            return Ok(entry.path());
                        }
                    }
                }
            }
        }

        let dir_path = lib.join(&target_upper);
        if create {
            std::fs::create_dir_all(&dir_path).map_err(|e| RuaError::Io {
                message: format!("Failed to create subdir {}: {e}", dir_path.display()),
            })?;
        }
        Ok(dir_path)
    }

    pub async fn list_prompts(&self) -> Result<Vec<PromptEntry>> {
        self.load_prompts()
    }

    pub async fn get_prompt(&self, prompt_id: &str) -> Result<Option<PromptEntry>> {
        let prompts = self.load_prompts()?;
        Ok(prompts.into_iter().find(|p| p.id == prompt_id))
    }

    pub async fn create_prompt(&self, spec: PromptCreate) -> Result<PromptEntry> {
        let mut prompts = self.load_prompts()?;
        let id = uuid::Uuid::new_v4().to_string();
        let entry = PromptEntry {
            id,
            title: spec.title,
            content: spec.content,
            category: spec.category,
            tags: spec.tags,
            is_shared: false,
            extra: spec.extra,
        };
        prompts.push(entry.clone());
        self.save_prompts(&prompts)?;
        Ok(entry)
    }

    pub async fn update_prompt(
        &self,
        prompt_id: &str,
        update: PromptUpdate,
    ) -> Result<PromptEntry> {
        let mut prompts = self.load_prompts()?;
        let prompt = prompts
            .iter_mut()
            .find(|p| p.id == prompt_id)
            .ok_or_else(|| RuaError::NotFound {
                resource: "prompt".into(),
                id: prompt_id.into(),
            })?;
        if let Some(title) = update.title {
            prompt.title = title;
        }
        if let Some(content) = update.content {
            prompt.content = content;
        }
        if update.category.is_some() {
            prompt.category = update.category;
        }
        if let Some(tags) = update.tags {
            prompt.tags = tags;
        }
        for (k, v) in update.extra {
            prompt.extra.insert(k, v);
        }
        let updated = prompt.clone();
        self.save_prompts(&prompts)?;
        Ok(updated)
    }

    pub async fn delete_prompt(&self, prompt_id: &str) -> Result<()> {
        let mut prompts = self.load_prompts()?;
        let len_before = prompts.len();
        prompts.retain(|p| p.id != prompt_id);
        if prompts.len() == len_before {
            return Err(RuaError::NotFound {
                resource: "prompt".into(),
                id: prompt_id.into(),
            });
        }
        self.save_prompts(&prompts)?;
        Ok(())
    }

    pub async fn broadcast<F>(
        &self,
        req: PromptBroadcastRequest,
        mut workspace_for: F,
    ) -> Result<PromptBroadcastResponse>
    where
        F: FnMut(&str) -> Option<PathBuf>,
    {
        let lib = self.lib_dir();
        let mut results: Vec<BroadcastFileResult> = Vec::new();
        for agent_id in &req.agent_ids {
            let dst_dir = match workspace_for(agent_id) {
                Some(d) => d,
                None => {
                    for filename in &req.files {
                        results.push(BroadcastFileResult {
                            agent_id: agent_id.clone(),
                            file: filename.clone(),
                            final_name: None,
                            conflict: None,
                            suggested_name: None,
                            error: Some(format!("agent '{agent_id}' not found")),
                        });
                    }
                    continue;
                }
            };
            if let Err(e) = std::fs::create_dir_all(&dst_dir) {
                for filename in &req.files {
                    results.push(BroadcastFileResult {
                        agent_id: agent_id.clone(),
                        file: filename.clone(),
                        final_name: None,
                        conflict: None,
                        suggested_name: None,
                        error: Some(format!("create workspace dir: {e}")),
                    });
                }
                continue;
            }
            for filename in &req.files {
                let row = copy_md_with_subdir(&lib, &dst_dir, filename, &req.conflict_mode);
                results.push(BroadcastFileResult {
                    agent_id: agent_id.clone(),
                    file: filename.clone(),
                    final_name: row.final_name,
                    conflict: row.conflict,
                    suggested_name: row.suggested_name,
                    error: row.error,
                });
            }
        }
        Ok(PromptBroadcastResponse { results })
    }

    #[allow(dead_code)]
    fn _broadcast_anchor() {}
}

#[derive(Default)]
struct CopyRow {
    final_name: Option<String>,
    conflict: Option<bool>,
    suggested_name: Option<String>,
    error: Option<String>,
}

fn resolve_name_conflict(dst_dir: &Path, filename: &str) -> String {
    let (stem, ext) = match filename.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (filename.to_string(), String::new()),
    };
    for i in 2..10_000 {
        let candidate = format!("{stem}_{i}{ext}");
        if !dst_dir.join(&candidate).exists() {
            return candidate;
        }
    }
    format!("{stem}_conflict{ext}")
}

fn copy_md_with_subdir(
    src_dir: &Path,
    dst_dir: &Path,
    filename: &str,
    conflict_mode: &str,
) -> CopyRow {
    let src = src_dir.join(filename);
    if !src.is_file() {
        return CopyRow {
            error: Some("File not found in prompt library".to_string()),
            ..Default::default()
        };
    }
    let final_name = if dst_dir.join(filename).exists() {
        match conflict_mode {
            "ask" => {
                let suggested = resolve_name_conflict(dst_dir, filename);
                return CopyRow {
                    conflict: Some(true),
                    suggested_name: Some(suggested),
                    ..Default::default()
                };
            }
            "overwrite" => filename.to_string(),
            _ => resolve_name_conflict(dst_dir, filename),
        }
    } else {
        filename.to_string()
    };

    if let Err(e) = std::fs::copy(&src, dst_dir.join(&final_name)) {
        return CopyRow {
            error: Some(format!("copy failed: {e}")),
            ..Default::default()
        };
    }

    let stem = filename.trim_end_matches(".md").trim_end_matches(".MD");
    let src_sub = src_dir.join(stem);
    if src_sub.is_dir() {
        let dst_stem = final_name.trim_end_matches(".md").trim_end_matches(".MD");
        let dst_sub = dst_dir.join(dst_stem);
        if let Err(e) = copy_dir_recursive(&src_sub, &dst_sub) {
            return CopyRow {
                final_name: Some(final_name),
                error: Some(format!("subdir copy failed: {e}")),
                ..Default::default()
            };
        }
    }

    CopyRow {
        final_name: Some(final_name),
        ..Default::default()
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let target = dst.join(&name);
        if path.is_dir() {
            copy_dir_recursive(&path, &target)?;
        } else if path.is_file() {
            std::fs::copy(&path, &target)?;
        }
    }
    Ok(())
}

impl PromptLibrary {
    fn _placeholder_after_helpers(&self) {}

    pub async fn list_files(&self) -> Result<Vec<FileEntry>> {
        let lib = self.lib_dir();
        if !lib.is_dir() {
            return Ok(Vec::new());
        }
        let mut files = Vec::new();
        let entries = std::fs::read_dir(&lib).map_err(|e| RuaError::Io {
            message: format!("Failed to read prompt_library dir: {e}"),
        })?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().map(|e| e == "md").unwrap_or(false) {
                match Self::file_entry_from_path(&path) {
                    Ok(fe) => files.push(fe),
                    Err(e) => warn!("Failed to stat prompt file {}: {e}", path.display()),
                }
            }
        }
        files.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(files)
    }

    pub async fn get_file(&self, md_name: &str) -> Result<Option<FileContent>> {
        Self::validate_name(md_name)?;
        let md_name = Self::ensure_md_ext(md_name);
        let path = self.lib_dir().join(&md_name);
        if !path.is_file() {
            return Ok(None);
        }
        Ok(Some(Self::file_content_from_path(&path)?))
    }

    pub async fn update_file(&self, md_name: &str, content: &str) -> Result<FileContent> {
        Self::validate_name(md_name)?;
        let md_name = Self::ensure_md_ext(md_name);
        let lib = self.ensure_lib_dir()?;
        let path = lib.join(&md_name);
        self.commit_write(&path, content, false).await
    }

    pub async fn delete_file(&self, md_name: &str) -> Result<()> {
        Self::validate_name(md_name)?;
        let md_name = Self::ensure_md_ext(md_name);
        let path = self.lib_dir().join(&md_name);
        if !path.is_file() {
            return Err(RuaError::NotFound {
                resource: "prompt_file".into(),
                id: md_name,
            });
        }
        self.commit_delete(&path).await
    }

    pub async fn rename_file(&self, md_name: &str, new_name: &str) -> Result<FileContent> {
        Self::validate_name(md_name)?;
        Self::validate_name(new_name)?;
        let old_name = Self::ensure_md_ext(md_name);
        let new_name = Self::ensure_md_ext(new_name);
        let lib = self.lib_dir();
        let old_path = lib.join(&old_name);
        let new_path = lib.join(&new_name);
        if !old_path.is_file() {
            return Err(RuaError::NotFound {
                resource: "prompt_file".into(),
                id: old_name,
            });
        }
        if new_path.exists() {
            return Err(RuaError::Validation {
                message: format!("File already exists: {new_name}"),
            });
        }
        self.commit_rename(&old_path, &new_path).await
    }

    pub async fn list_subdirs_info(&self) -> Result<HashMap<String, usize>> {
        let lib = self.lib_dir();
        if !lib.is_dir() {
            return Ok(HashMap::new());
        }

        let mut subdirs: HashMap<String, PathBuf> = HashMap::new();
        if let Ok(entries) = std::fs::read_dir(&lib) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let name_upper = entry.file_name().to_string_lossy().to_uppercase();
                    subdirs.insert(name_upper, path);
                }
            }
        }

        let mut result = HashMap::new();
        if let Ok(entries) = std::fs::read_dir(&lib) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && path.extension().map(|e| e == "md").unwrap_or(false) {
                    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
                    let stem_upper = stem.to_uppercase();
                    if let Some(sub_path) = subdirs.get(&stem_upper) {
                        let count = std::fs::read_dir(sub_path)
                            .map(|entries| {
                                entries
                                    .flatten()
                                    .filter(|e| {
                                        let p = e.path();
                                        p.is_file()
                                            && p.extension().map(|ex| ex == "md").unwrap_or(false)
                                    })
                                    .count()
                            })
                            .unwrap_or(0);
                        if count > 0 {
                            let md_name = entry.file_name().to_string_lossy().to_string();
                            result.insert(md_name, count);
                        }
                    }
                }
            }
        }
        Ok(result)
    }

    pub async fn list_subdir_files(&self, subdir: &str) -> Result<Vec<FileEntry>> {
        let dir_path = self.resolve_subdir(subdir, false)?;
        if !dir_path.is_dir() {
            return Ok(Vec::new());
        }
        let mut files = Vec::new();
        let entries = std::fs::read_dir(&dir_path).map_err(|e| RuaError::Io {
            message: format!("Failed to read subdir: {e}"),
        })?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().map(|e| e == "md").unwrap_or(false) {
                match Self::file_entry_from_path(&path) {
                    Ok(fe) => files.push(fe),
                    Err(e) => warn!("Failed to stat subdir file {}: {e}", path.display()),
                }
            }
        }
        files.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(files)
    }

    pub async fn create_subdir_file(
        &self,
        subdir: &str,
        filename: Option<&str>,
        content: &str,
    ) -> Result<FileContent> {
        let md_name = filename
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .or_else(|| {
                if content.is_empty() {
                    None
                } else {
                    Some(content.to_string())
                }
            })
            .unwrap_or_else(|| "NEW.md".to_string());
        let md_name = Self::ensure_md_ext(&md_name);
        let dir_path = self.resolve_subdir(subdir, true)?;
        let file_path = dir_path.join(&md_name);
        if file_path.exists() {
            return Err(RuaError::Validation {
                message: format!("File already exists: {subdir}/{md_name}"),
            });
        }
        self.commit_write(&file_path, "", true).await
    }

    pub async fn get_subdir_file(
        &self,
        subdir: &str,
        md_name: &str,
    ) -> Result<Option<FileContent>> {
        Self::validate_name(md_name)?;
        let md_name = Self::ensure_md_ext(md_name);
        let dir_path = self.resolve_subdir(subdir, false)?;
        let file_path = dir_path.join(&md_name);
        if !file_path.is_file() {
            return Ok(None);
        }
        Ok(Some(Self::file_content_from_path(&file_path)?))
    }

    pub async fn write_subdir_file(
        &self,
        subdir: &str,
        md_name: &str,
        content: &str,
    ) -> Result<FileContent> {
        Self::validate_name(md_name)?;
        let md_name = Self::ensure_md_ext(md_name);
        let dir_path = self.resolve_subdir(subdir, true)?;
        let file_path = dir_path.join(&md_name);
        self.commit_write(&file_path, content, false).await
    }

    pub async fn rename_subdir_file(
        &self,
        subdir: &str,
        md_name: &str,
        new_name: &str,
    ) -> Result<FileContent> {
        Self::validate_name(md_name)?;
        Self::validate_name(new_name)?;
        let old_name = Self::ensure_md_ext(md_name);
        let new_name_md = Self::ensure_md_ext(new_name);
        let dir_path = self.resolve_subdir(subdir, false)?;
        let old_path = dir_path.join(&old_name);
        let new_path = dir_path.join(&new_name_md);
        if !old_path.is_file() {
            return Err(RuaError::NotFound {
                resource: "subdir_file".into(),
                id: format!("{subdir}/{old_name}"),
            });
        }
        if new_path.exists() {
            return Err(RuaError::Validation {
                message: format!("File already exists: {subdir}/{new_name_md}"),
            });
        }
        self.commit_rename(&old_path, &new_path).await
    }

    pub async fn delete_subdir_file(&self, subdir: &str, md_name: &str) -> Result<()> {
        Self::validate_name(md_name)?;
        let md_name = Self::ensure_md_ext(md_name);
        let dir_path = self.resolve_subdir(subdir, false)?;
        let file_path = dir_path.join(&md_name);
        if !file_path.is_file() {
            return Err(RuaError::NotFound {
                resource: "subdir_file".into(),
                id: format!("{subdir}/{md_name}"),
            });
        }
        self.commit_delete(&file_path).await?;
        // Only remove an empty directory; other files have independent identities.
        let _ = std::fs::remove_dir(&dir_path);
        Ok(())
    }

    pub async fn sync(&self, _req: PromptLibrarySyncRequest) -> Result<PromptLibrarySyncResponse> {
        Ok(PromptLibrarySyncResponse {
            success: true,
            results: vec![],
            extra: serde_json::Map::new(),
        })
    }

    pub async fn download(&self) -> Result<Vec<u8>> {
        archive::download(&self.lib_dir())
    }

    pub async fn upload(&self, data: &[u8]) -> Result<String> {
        let lib = self.ensure_lib_dir()?;
        let prepared = archive::prepare_upload(data)?;
        for (index, (relative, bytes)) in prepared.files.iter().enumerate() {
            let path = lib.join(relative);
            let result = if relative.to_ascii_lowercase().ends_with(".md") {
                self.commit_write(
                    &path,
                    std::str::from_utf8(bytes).expect("validated Markdown"),
                    false,
                )
                .await
                .map(|_| ())
            } else {
                rua_config::migration::atomic::atomic_write(&path, bytes).map_err(RuaError::from)
            };
            result.map_err(|error| RuaError::Io {
                message: format!(
                    "Import stopped at {relative} after {index} committed file(s): {error}"
                ),
            })?;
        }
        Ok(prepared.message.into())
    }

    pub async fn restore_builtin(&self) -> Result<RestoreBuiltinResult> {
        let mut copied = Vec::new();
        for prompt in crate::builtin::BUILTIN_PROMPTS {
            let path = self.lib_dir().join(prompt.path);
            if path.exists() {
                continue;
            }
            // A concurrent writer can win between the existence check and commit.
            let guard = PromptFileStore::for_library(&self.data_dir).lock().await;
            if guard.file_path(prompt.path)?.try_exists()? {
                continue;
            }
            let change = guard
                .write(prompt.path, prompt.content, true)
                .map_err(|error| RuaError::Io {
                    message: format!(
                        "Restore stopped at {} after {} committed file(s): {error}",
                        prompt.path,
                        copied.len()
                    ),
                })?;
            drop(guard);
            self.publish(vec![change]).await;
            copied.push(SeedEntry {
                relative_path: format!("prompt_library/{}", prompt.path),
                content: prompt.content.as_bytes().to_vec(),
            });
        }
        let message = if copied.is_empty() {
            "Prompt library already complete, nothing to seed.".into()
        } else {
            format!(
                "Seeded {} missing prompt-library file(s): {}",
                copied.len(),
                copied
                    .iter()
                    .map(|e| e.relative_path.trim_start_matches("prompt_library/"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        Ok(RestoreBuiltinResult {
            message,
            seeded: copied,
        })
    }

    /// Startup-only additive seed. Explicit user restoration uses durable commits.
    pub fn restore_builtin_now(&self) -> Result<RestoreBuiltinResult> {
        let lib = self.ensure_lib_dir()?;
        let mut copied = Vec::new();

        for prompt in crate::builtin::BUILTIN_PROMPTS {
            let dst = lib.join(prompt.path);
            if dst.exists() {
                continue;
            }
            if let Some(parent) = dst.parent() {
                if !parent.exists() {
                    std::fs::create_dir_all(parent).map_err(|e| RuaError::Io {
                        message: format!("Failed to create dir {}: {e}", parent.display()),
                    })?;
                }
            }
            rua_config::migration::atomic::atomic_write(&dst, prompt.content.as_bytes()).map_err(
                |e| RuaError::Io {
                    message: format!("Failed to write builtin {}: {e}", prompt.path),
                },
            )?;
            copied.push(prompt.path);
        }

        let message = if copied.is_empty() {
            "Prompt library already complete, nothing to seed.".to_string()
        } else {
            format!(
                "Seeded {} missing prompt-library file(s): {}",
                copied.len(),
                copied.join(", ")
            )
        };

        let seeded: Vec<SeedEntry> = copied
            .iter()
            .filter_map(|path| {
                crate::builtin::BUILTIN_PROMPTS
                    .iter()
                    .find(|p| p.path == *path)
                    .map(|p| SeedEntry {
                        relative_path: format!("prompt_library/{}", p.path),
                        content: p.content.as_bytes().to_vec(),
                    })
            })
            .collect();

        Ok(RestoreBuiltinResult { message, seeded })
    }

    pub fn validate_template_files(&self, files: &[String]) -> Result<()> {
        let root = self.lib_dir();
        for filename in files {
            Self::validate_name(filename)?;
            let source = root.join(filename);
            if !source.is_file() {
                return Err(RuaError::NotFound {
                    resource: "prompt library file".into(),
                    id: filename.clone(),
                });
            }
            let canonical_root = root.canonicalize()?;
            if !source.canonicalize()?.starts_with(&canonical_root) {
                return Err(RuaError::Validation {
                    message: format!("Prompt file escapes library: {filename}"),
                });
            }
        }
        Ok(())
    }

    pub fn copy_files_to_workspace(
        &self,
        files: &[String],
        workspace_dir: &Path,
    ) -> Result<Vec<String>> {
        self.validate_template_files(files)?;
        let lib = self.lib_dir();
        let prompt_dir = workspace_dir.join("prompts");
        std::fs::create_dir_all(&prompt_dir).map_err(|e| RuaError::Io {
            message: format!("Failed to create workspace dir: {e}"),
        })?;

        let mut copied = Vec::new();
        for filename in files {
            let src = lib.join(filename);

            let dst = prompt_dir.join(filename);
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent).map_err(|e| RuaError::Io {
                    message: format!("Failed to create dir {}: {e}", parent.display()),
                })?;
            }
            rua_config::migration::atomic::atomic_write(&dst, &std::fs::read(&src)?).map_err(
                |e| RuaError::Io {
                    message: format!("Failed to copy {filename} to workspace: {e}"),
                },
            )?;

            let stem = filename.strip_suffix(".md").unwrap_or(filename);
            let src_subdir = lib.join(stem);
            if src_subdir.is_dir() {
                copy_dir_recursive(&src_subdir, &prompt_dir.join(stem)).map_err(|e| {
                    RuaError::Io {
                        message: format!("Failed to copy subdir {stem}: {e}"),
                    }
                })?;
            }
            copied.push(filename.clone());
        }
        Ok(copied)
    }
}

fn prompt_file_error(error: std::io::Error) -> RuaError {
    match error.kind() {
        std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::InvalidInput => {
            RuaError::Validation {
                message: error.to_string(),
            }
        }
        _ => error.into(),
    }
}
