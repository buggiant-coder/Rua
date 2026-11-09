use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone)]
pub struct ChatInfo {
    pub id: String,
    pub updated_at: DateTime<Utc>,
}

#[async_trait]
pub trait SessionCleanupStore: Send + Sync {
    async fn list_all_chats(&self) -> Result<Vec<ChatInfo>, String>;

    async fn delete_chats(&self, ids: &[String]) -> Result<(), String>;
}

#[derive(Debug, Clone, Default)]
pub struct CleanupResult {
    pub files_removed: u64,
    pub bytes_freed: u64,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SessionCleanupConfig {
    pub max_age: Duration,
}

impl Default for SessionCleanupConfig {
    fn default() -> Self {
        Self {
            max_age: Duration::from_secs(7 * 24 * 3600),
        }
    }
}

#[derive(Debug, Clone)]
pub struct WorkspaceMaintenanceConfig {
    pub temp_max_age: Duration,
    pub lock_max_age: Duration,

    pub retention_days: Duration,
}

impl Default for WorkspaceMaintenanceConfig {
    fn default() -> Self {
        Self {
            temp_max_age: Duration::from_secs(24 * 3600),
            lock_max_age: Duration::from_secs(3600),
            retention_days: Duration::from_secs(5 * 24 * 3600),
        }
    }
}

pub async fn run_session_cleanup(
    sessions_dir: &Path,
    config: &SessionCleanupConfig,
    chat_store: &dyn SessionCleanupStore,
) -> CleanupResult {
    let mut result = CleanupResult::default();

    let all_chats = match chat_store.list_all_chats().await {
        Ok(chats) => chats,
        Err(e) => {
            tracing::warn!(
                path = %sessions_dir.display(),
                error = %e,
                "Failed to list chats for cleanup; skipping orphan removal (fail-safe)"
            );
            result.errors.push(format!("list_all_chats: {e}"));
            return result;
        }
    };

    if !sessions_dir.exists() {
        tracing::debug!(path = %sessions_dir.display(), "Sessions directory does not exist, nothing to clean");
        return result;
    }

    if all_chats.is_empty() {
        let has_session_files = match tokio::fs::read_dir(sessions_dir).await {
            Ok(mut entries) => matches!(entries.next_entry().await, Ok(Some(_))),
            Err(_) => false,
        };
        if has_session_files {
            tracing::warn!(
                path = %sessions_dir.display(),
                "Chat store returned empty list while sessions dir has files; skipping cleanup (fail-safe)"
            );
            return result;
        }
    }

    let live_chat_ids: HashSet<String> = all_chats.iter().map(|chat| chat.id.clone()).collect();

    let entries = match tokio::fs::read_dir(sessions_dir).await {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!(path = %sessions_dir.display(), error = %e, "Failed to read sessions directory");
            result.errors.push(format!("read_dir: {e}"));
            return result;
        }
    };

    let now = SystemTime::now();
    let mut entries = entries;

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let fname = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();

        if live_chat_ids.iter().any(|chat_id| fname.contains(chat_id)) {
            continue;
        }

        let metadata = match tokio::fs::metadata(&path).await {
            Ok(m) => m,
            Err(e) => {
                result.errors.push(format!("{}: {e}", path.display()));
                continue;
            }
        };

        let modified = match metadata.modified() {
            Ok(t) => t,
            Err(_) => continue,
        };

        let age = now.duration_since(modified).unwrap_or(Duration::ZERO);

        if age > config.max_age {
            let size = metadata.len();
            if let Err(e) = remove_path(&path).await {
                result.errors.push(format!("{}: {e}", path.display()));
            } else {
                result.files_removed += 1;
                result.bytes_freed += size;
                tracing::debug!(path = %path.display(), age_secs = age.as_secs(), "Removed orphan session path");
            }
        }
    }

    tracing::info!(
        removed = result.files_removed,
        bytes_freed = result.bytes_freed,
        errors = result.errors.len(),
        "Session cleanup complete"
    );

    result
}

pub async fn run_workspace_maintenance(
    workspace_dir: &Path,
    config: &WorkspaceMaintenanceConfig,
) -> CleanupResult {
    let mut result = CleanupResult::default();

    if !workspace_dir.exists() {
        tracing::debug!(path = %workspace_dir.display(), "Workspace directory does not exist, nothing to maintain");
        return result;
    }

    let now = SystemTime::now();
    cleanup_dir_recursive(workspace_dir, &now, config, &mut result).await;

    let rua_dirs: &[(&str, &str)] = &[
        ("tool_calls", "json"),
        ("tool_result", "txt"),
        ("dialog", "jsonl"),
    ];
    for (subdir, ext) in rua_dirs {
        let dir = workspace_dir.join(subdir);
        cleanup_rua_subdir(&dir, ext, &now, config.retention_days, &mut result).await;
    }

    tracing::info!(
        path = %workspace_dir.display(),
        removed = result.files_removed,
        bytes_freed = result.bytes_freed,
        errors = result.errors.len(),
        "Workspace maintenance complete"
    );

    result
}

async fn cleanup_rua_subdir(
    dir: &Path,
    extension: &str,
    now: &SystemTime,
    max_age: Duration,
    result: &mut CleanupResult,
) {
    if !dir.exists() {
        return;
    }

    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(e) => e,
        Err(e) => {
            result.errors.push(format!("{}: {e}", dir.display()));
            return;
        }
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();

        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext != extension {
            continue;
        }

        let metadata = match tokio::fs::metadata(&path).await {
            Ok(m) => m,
            Err(_) => continue,
        };

        if metadata.is_dir() {
            continue;
        }

        let modified = match metadata.modified() {
            Ok(t) => t,
            Err(_) => continue,
        };

        let age = now.duration_since(modified).unwrap_or(Duration::ZERO);

        if age > max_age {
            let size = metadata.len();
            if let Err(e) = tokio::fs::remove_file(&path).await {
                result.errors.push(format!("{}: {e}", path.display()));
            } else {
                result.files_removed += 1;
                result.bytes_freed += size;
                tracing::debug!(path = %path.display(), age_secs = age.as_secs(), "Removed expired rua file");
            }
        }
    }
}

fn cleanup_dir_recursive<'a>(
    dir: &'a Path,
    now: &'a SystemTime,
    config: &'a WorkspaceMaintenanceConfig,
    result: &'a mut CleanupResult,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
    Box::pin(async move {
        let mut entries = match tokio::fs::read_dir(dir).await {
            Ok(e) => e,
            Err(e) => {
                result.errors.push(format!("{}: {e}", dir.display()));
                return;
            }
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();

            let metadata = match tokio::fs::metadata(&path).await {
                Ok(m) => m,
                Err(_) => continue,
            };

            if metadata.is_dir() {
                cleanup_dir_recursive(&path, now, config, result).await;
                continue;
            }

            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let max_age = match ext {
                "tmp" | "temp" => config.temp_max_age,
                "lock" => config.lock_max_age,
                _ => continue,
            };

            let modified = match metadata.modified() {
                Ok(t) => t,
                Err(_) => continue,
            };

            let age = now.duration_since(modified).unwrap_or(Duration::ZERO);

            if age > max_age {
                let size = metadata.len();
                if let Err(e) = tokio::fs::remove_file(&path).await {
                    result.errors.push(format!("{}: {e}", path.display()));
                } else {
                    result.files_removed += 1;
                    result.bytes_freed += size;
                    tracing::debug!(path = %path.display(), ext = ext, age_secs = age.as_secs(), "Removed stale file");
                }
            }
        }
    })
}

pub struct NullSessionCleanupStore;

#[async_trait]
impl SessionCleanupStore for NullSessionCleanupStore {
    async fn list_all_chats(&self) -> Result<Vec<ChatInfo>, String> {
        Ok(vec![])
    }

    async fn delete_chats(&self, _ids: &[String]) -> Result<(), String> {
        Ok(())
    }
}

async fn remove_path(path: &Path) -> std::io::Result<()> {
    let metadata = tokio::fs::metadata(path).await?;
    if metadata.is_dir() {
        tokio::fs::remove_dir_all(path).await
    } else {
        tokio::fs::remove_file(path).await
    }
}
