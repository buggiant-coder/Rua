use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::loader::PluginLoader;
use crate::PluginRegistry;

const DEFAULT_DEBOUNCE: Duration = Duration::from_secs(1);

#[derive(Debug, thiserror::Error)]
pub enum WatcherError {
    #[error("notify error: {0}")]
    Notify(#[from] notify::Error),
    #[error("plugins directory does not exist: {0}")]
    DirNotFound(String),
}

#[derive(Debug, Clone)]
enum WatchEvent {
    Upsert(PathBuf),
    Remove(PathBuf),
}

pub struct PluginWatcher {
    _watcher: RecommendedWatcher,
    cancel: CancellationToken,
}

impl PluginWatcher {
    pub fn start(plugins_dir: PathBuf, registry: PluginRegistry) -> Result<Self, WatcherError> {
        Self::start_with_debounce(plugins_dir, registry, DEFAULT_DEBOUNCE)
    }

    pub fn start_with_debounce(
        plugins_dir: PathBuf,
        registry: PluginRegistry,
        debounce: Duration,
    ) -> Result<Self, WatcherError> {
        if !plugins_dir.is_dir() {
            return Err(WatcherError::DirNotFound(plugins_dir.display().to_string()));
        }

        let cancel = CancellationToken::new();
        let (tx, rx) = mpsc::unbounded_channel::<WatchEvent>();

        let mut watcher = RecommendedWatcher::new(
            move |res: Result<notify::Event, notify::Error>| {
                if let Ok(event) = res {
                    for path in &event.paths {
                        // A complete plugin can arrive before recursive watches are attached.
                        // Directory create/rename events must also discover its manifest.
                        if !is_plugin_manifest(path) {
                            if matches!(
                                event.kind,
                                EventKind::Create(_)
                                    | EventKind::Modify(notify::event::ModifyKind::Name(_))
                            ) && path.is_dir()
                            {
                                let _ = tx.send(WatchEvent::Upsert(path.join("plugin.json")));
                            }
                            continue;
                        }
                        let watch_event = match event.kind {
                            EventKind::Create(_) | EventKind::Modify(_) => {
                                WatchEvent::Upsert(path.clone())
                            }
                            EventKind::Remove(_) => WatchEvent::Remove(path.clone()),
                            _ => continue,
                        };
                        let _ = tx.send(watch_event);
                    }
                }
            },
            Config::default(),
        )?;

        watcher.watch(&plugins_dir, RecursiveMode::Recursive)?;

        tracing::info!(
            path = %plugins_dir.display(),
            "Plugin watcher started"
        );

        let task_cancel = cancel.clone();
        tokio::spawn(event_loop(rx, task_cancel, registry, debounce, plugins_dir));

        Ok(Self {
            _watcher: watcher,
            cancel,
        })
    }

    pub fn stop(&self) {
        self.cancel.cancel();
    }
}

impl Drop for PluginWatcher {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

fn is_plugin_manifest(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == "plugin.json")
}

fn plugin_dir_for_manifest(manifest_path: &Path) -> Option<PathBuf> {
    manifest_path
        .parent()
        .map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()))
}

fn plugin_id_from_dir(plugin_dir: &Path) -> Option<String> {
    let manifest_path = plugin_dir.join("plugin.json");
    if manifest_path.exists() {
        if let Ok(manifest) = crate::manifest::PluginManifest::from_file(&manifest_path) {
            return Some(manifest.id);
        }
    }
    None
}

async fn event_loop(
    mut rx: mpsc::UnboundedReceiver<WatchEvent>,
    cancel: CancellationToken,
    registry: PluginRegistry,
    debounce: Duration,
    plugins_dir: PathBuf,
) {
    let mut path_to_plugin_id: HashMap<PathBuf, String> = HashMap::new();

    if let Ok(ids) = registry.list() {
        for id in ids {
            let candidate = plugins_dir.join(&id);
            if candidate.is_dir() {
                let canonical = std::fs::canonicalize(&candidate).unwrap_or(candidate);
                path_to_plugin_id.insert(canonical, id);
            }
        }
    }

    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                tracing::info!("Plugin watcher stopping");
                break;
            }
            msg = rx.recv() => {
                let Some(first_event) = msg else { break };


                let mut pending: HashMap<PathBuf, WatchEvent> = HashMap::new();
                if let Some(dir) = manifest_event_dir(&first_event) {
                    pending.insert(dir, first_event);
                }

                tokio::time::sleep(debounce).await;
                while let Ok(evt) = rx.try_recv() {
                    if let Some(dir) = manifest_event_dir(&evt) {
                        pending.insert(dir, evt);
                    }
                }

                for (plugin_dir, event) in pending {
                    match event {
                        WatchEvent::Upsert(_manifest_path) => {
                            handle_upsert(
                                &plugin_dir,
                                &registry,
                                &mut path_to_plugin_id,
                            );
                        }
                        WatchEvent::Remove(_manifest_path) => {
                            handle_remove(
                                &plugin_dir,
                                &registry,
                                &mut path_to_plugin_id,
                            );
                        }
                    }
                }
            }
        }
    }
}

fn manifest_event_dir(event: &WatchEvent) -> Option<PathBuf> {
    match event {
        WatchEvent::Upsert(p) | WatchEvent::Remove(p) => plugin_dir_for_manifest(p),
    }
}

fn handle_upsert(
    plugin_dir: &Path,
    registry: &PluginRegistry,
    path_map: &mut HashMap<PathBuf, String>,
) {
    if let Some(existing_id) = path_map.get(plugin_dir) {
        let id = existing_id.clone();
        tracing::info!(plugin_id = %id, "Reloading plugin (manifest changed)");
        if let Err(e) = registry.remove(&id) {
            tracing::warn!(plugin_id = %id, error = %e, "Failed to remove plugin for reload");
        }
        path_map.remove(plugin_dir);
    }

    match PluginLoader::load(plugin_dir) {
        Ok(plugin) => {
            let plugin_id = plugin.id.clone();
            match registry.add(plugin) {
                Ok(()) => {
                    tracing::info!(plugin_id = %plugin_id, "Plugin loaded via watcher");
                    path_map.insert(plugin_dir.to_path_buf(), plugin_id);
                }
                Err(e) => {
                    tracing::warn!(
                        plugin_id = %plugin_id,
                        error = %e,
                        "Failed to add plugin via watcher"
                    );
                }
            }
        }
        Err(e) => {
            tracing::warn!(
                dir = %plugin_dir.display(),
                error = %e,
                "Failed to load plugin via watcher"
            );
        }
    }
}

fn handle_remove(
    plugin_dir: &Path,
    registry: &PluginRegistry,
    path_map: &mut HashMap<PathBuf, String>,
) {
    let plugin_id = path_map
        .get(plugin_dir)
        .cloned()
        .or_else(|| plugin_id_from_dir(plugin_dir));

    if let Some(id) = plugin_id {
        tracing::info!(plugin_id = %id, "Removing plugin (manifest deleted)");
        if let Err(e) = registry.remove(&id) {
            tracing::warn!(plugin_id = %id, error = %e, "Failed to remove plugin via watcher");
        }
        path_map.remove(plugin_dir);
    } else {
        tracing::warn!(
            dir = %plugin_dir.display(),
            "Plugin manifest removed but no plugin_id mapping found"
        );
    }
}
