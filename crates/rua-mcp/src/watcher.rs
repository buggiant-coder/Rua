use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use mcp_manager::manager::McpManager;
use mcp_manager::types::McpConfig;

use crate::config::McpConfigLoader;

const POLL_INTERVAL: Duration = Duration::from_secs(2);
const MAX_RETRIES: u32 = 3;

#[derive(Debug)]
struct FileState {
    mtime: Option<SystemTime>,
    content_hash: u64,
    consecutive_failures: u32,
}

pub struct McpConfigWatcher {
    cancel_tx: watch::Sender<bool>,
    handle: Option<JoinHandle<()>>,
}

impl McpConfigWatcher {
    pub fn start_watching(paths: Vec<PathBuf>, manager: Arc<dyn McpManager>) -> Self {
        let (cancel_tx, cancel_rx) = watch::channel(false);

        let handle = tokio::spawn(Self::poll_loop(paths, manager, cancel_rx));

        Self {
            cancel_tx,
            handle: Some(handle),
        }
    }

    pub async fn stop_watching(mut self) {
        let _ = self.cancel_tx.send(true);
        if let Some(handle) = self.handle.take() {
            let _ = handle.await;
        }
    }

    async fn poll_loop(
        paths: Vec<PathBuf>,
        manager: Arc<dyn McpManager>,
        mut cancel_rx: watch::Receiver<bool>,
    ) {
        let mut states: HashMap<PathBuf, FileState> = HashMap::new();
        let mut last_config: Option<McpConfig> = None;

        for path in &paths {
            let state = Self::snapshot(path);
            states.insert(path.clone(), state);
        }

        for path in &paths {
            if let Ok(content) = std::fs::read_to_string(path) {
                if let Ok(config) = McpConfigLoader::load_from_str(&content) {
                    last_config = Some(config);
                }
            }
        }

        info!(
            paths = ?paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
            "MCP config watcher started"
        );

        loop {
            tokio::select! {
                _ = tokio::time::sleep(POLL_INTERVAL) => {}
                result = cancel_rx.changed() => {
                    if result.is_err() || *cancel_rx.borrow() {
                        break;
                    }
                }
            }

            for path in &paths {
                let state = states
                    .get_mut(path)
                    .expect("state initialized for all paths");

                let current_mtime = Self::file_mtime(path);

                if current_mtime.is_none() {
                    debug!(path = %path.display(), "config file not found, skipping");
                    continue;
                }

                if current_mtime == state.mtime {
                    continue;
                }

                let content = match std::fs::read_to_string(path) {
                    Ok(c) => c,
                    Err(e) => {
                        warn!(path = %path.display(), error = %e, "failed to read config file");
                        continue;
                    }
                };

                let new_hash = Self::hash_content(&content);

                if new_hash == state.content_hash {
                    state.mtime = current_mtime;
                    debug!(path = %path.display(), "mtime changed but content unchanged");
                    continue;
                }

                info!(path = %path.display(), "config change detected");
                state.consecutive_failures = 0;

                let config = match McpConfigLoader::load_from_str(&content) {
                    Ok(cfg) => cfg,
                    Err(e) => {
                        state.consecutive_failures += 1;
                        error!(
                            path = %path.display(),
                            error = %e,
                            failures = state.consecutive_failures,
                            "failed to parse config"
                        );
                        if state.consecutive_failures >= MAX_RETRIES {
                            error!(
                                path = %path.display(),
                                "max retries reached, stopping watch for this file"
                            );
                        }
                        continue;
                    }
                };

                let result = if let Some(ref old_config) = last_config {
                    let (added, removed, updated, _unchanged) =
                        McpConfigLoader::diff_configs(old_config, &config);

                    if added.is_empty() && removed.is_empty() && updated.len() == 1 {
                        let server_id = &updated[0];
                        let server_config = config.clients.get(server_id).unwrap().clone();
                        info!(
                            path = %path.display(),
                            server = %server_id,
                            "single server changed, using replace_client"
                        );
                        manager
                            .replace_client(server_id, server_config)
                            .await
                            .map(|()| (0, 0, 1))
                    } else {
                        info!(
                            path = %path.display(),
                            added = added.len(),
                            removed = removed.len(),
                            updated = updated.len(),
                            "multiple changes, using full reload"
                        );
                        manager
                            .reload(config.clone())
                            .await
                            .map(|r| (r.added.len(), r.removed.len(), r.updated.len()))
                    }
                } else {
                    manager
                        .reload(config.clone())
                        .await
                        .map(|r| (r.added.len(), r.removed.len(), r.updated.len()))
                };

                match result {
                    Ok((added, removed, updated)) => {
                        info!(
                            path = %path.display(),
                            added,
                            removed,
                            updated,
                            "MCP config applied successfully"
                        );
                        state.mtime = current_mtime;
                        state.content_hash = new_hash;
                        state.consecutive_failures = 0;
                        last_config = Some(config);
                    }
                    Err(e) => {
                        state.consecutive_failures += 1;
                        error!(
                            path = %path.display(),
                            error = %e,
                            failures = state.consecutive_failures,
                            "config apply failed"
                        );
                        if state.consecutive_failures >= MAX_RETRIES {
                            error!(
                                path = %path.display(),
                                "max retries reached, stopping watch for this file"
                            );
                        }
                    }
                }
            }
        }

        info!("MCP config watcher stopped");
    }

    fn snapshot(path: &Path) -> FileState {
        let mtime = Self::file_mtime(path);
        let content_hash = match std::fs::read_to_string(path) {
            Ok(content) => Self::hash_content(&content),
            Err(_) => 0,
        };
        FileState {
            mtime,
            content_hash,
            consecutive_failures: 0,
        }
    }

    fn file_mtime(path: &Path) -> Option<SystemTime> {
        std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
    }

    fn hash_content(content: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        content.hash(&mut hasher);
        hasher.finish()
    }
}
