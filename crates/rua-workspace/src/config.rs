use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::SystemTime;

use serde_json::Value;

use crate::error::WorkspaceError;
use crate::root::WorkspaceRoot;

const LAYER_DEFAULT: &str = "default";
const LAYER_GLOBAL: &str = "global";
const LAYER_WORKSPACE: &str = "workspace";
const LAYER_ENV: &str = "env";

const PRIORITY_DEFAULT: u32 = 0;
const PRIORITY_GLOBAL: u32 = 10;
const PRIORITY_WORKSPACE: u32 = 20;
const PRIORITY_ENV: u32 = 30;

#[derive(Debug, Clone)]
pub struct ConfigLayer {
    pub name: String,

    pub priority: u32,

    pub values: Value,
}

pub fn deep_merge(base: &mut Value, overlay: &Value) {
    match (base, overlay) {
        (Value::Object(base_map), Value::Object(overlay_map)) => {
            for (key, overlay_val) in overlay_map {
                let entry = base_map.entry(key.clone()).or_insert(Value::Null);
                deep_merge(entry, overlay_val);
            }
        }
        (base_slot, overlay_val) => {
            *base_slot = overlay_val.clone();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Mtime(SystemTime);

fn file_mtime(path: &Path) -> Option<Mtime> {
    fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .map(Mtime)
}

struct CacheEntry {
    merged: Value,
    sources: Vec<(PathBuf, Option<Mtime>)>,
    env_override: Option<String>,
}

pub struct LayeredConfig {
    default_path: PathBuf,
    global_path: PathBuf,
    workspace_path: PathBuf,
    cache: RwLock<Option<CacheEntry>>,
}

impl LayeredConfig {
    pub fn for_workspace(rua_root: &Path, workspace_root: &WorkspaceRoot) -> Self {
        let default_path = rua_root.join("defaults").join("config.json");
        let global_path = rua_root.join("config.json");
        let workspace_path = workspace_root.as_path().join("agent.json");
        Self {
            default_path,
            global_path,
            workspace_path,
            cache: RwLock::new(None),
        }
    }

    pub fn invalidate(&self) {
        if let Ok(mut guard) = self.cache.write() {
            *guard = None;
        }
    }

    pub fn merged(&self) -> Result<Value, WorkspaceError> {
        if let Some(cached) = self.read_cached_if_fresh() {
            return Ok(cached);
        }

        let mut layers = self.build_layers()?;
        let merged = merge_layers(&mut layers);
        let env_override = current_env_override();

        let sources = vec![
            (self.default_path.clone(), file_mtime(&self.default_path)),
            (self.global_path.clone(), file_mtime(&self.global_path)),
            (
                self.workspace_path.clone(),
                file_mtime(&self.workspace_path),
            ),
        ];

        if let Ok(mut guard) = self.cache.write() {
            *guard = Some(CacheEntry {
                merged: merged.clone(),
                sources,
                env_override,
            });
        }

        Ok(merged)
    }

    fn read_cached_if_fresh(&self) -> Option<Value> {
        let guard = self.cache.read().ok()?;
        let entry = guard.as_ref()?;
        if entry.env_override != current_env_override() {
            return None;
        }
        for (path, recorded) in &entry.sources {
            if file_mtime(path) != *recorded {
                return None;
            }
        }
        Some(entry.merged.clone())
    }

    fn build_layers(&self) -> Result<Vec<ConfigLayer>, WorkspaceError> {
        let mut layers = Vec::new();

        if self.default_path.exists() {
            layers.push(ConfigLayer {
                name: LAYER_DEFAULT.into(),
                priority: PRIORITY_DEFAULT,
                values: read_json_layer(&self.default_path)?,
            });
        }

        if self.global_path.exists() {
            layers.push(ConfigLayer {
                name: LAYER_GLOBAL.into(),
                priority: PRIORITY_GLOBAL,
                values: read_json_layer(&self.global_path)?,
            });
        }

        if self.workspace_path.exists() {
            layers.push(ConfigLayer {
                name: LAYER_WORKSPACE.into(),
                priority: PRIORITY_WORKSPACE,
                values: read_json_layer(&self.workspace_path)?,
            });
        }

        if let Some(env_json) = current_env_override() {
            match serde_json::from_str::<Value>(&env_json) {
                Ok(val) => {
                    layers.push(ConfigLayer {
                        name: LAYER_ENV.into(),
                        priority: PRIORITY_ENV,
                        values: val,
                    });
                }
                Err(e) => {
                    tracing::warn!("RUA_CONFIG_OVERRIDE is not valid JSON, skipping: {e}");
                }
            }
        }

        Ok(layers)
    }
}

fn current_env_override() -> Option<String> {
    match std::env::var("RUA_CONFIG_OVERRIDE") {
        Ok(val) if !val.is_empty() => Some(val),
        _ => None,
    }
}

fn read_json_layer(path: &Path) -> Result<Value, WorkspaceError> {
    let content = fs::read_to_string(path).map_err(|e| {
        WorkspaceError::config_invalid(format!("read config {}: {e}", path.display()))
    })?;
    serde_json::from_str(&content).map_err(|e| {
        WorkspaceError::config_invalid(format!("parse config {}: {e}", path.display()))
    })
}

pub fn merge_layers(layers: &mut [ConfigLayer]) -> Value {
    layers.sort_by_key(|l| l.priority);
    let mut merged = Value::Object(serde_json::Map::new());
    for layer in layers.iter() {
        deep_merge(&mut merged, &layer.values);
    }
    merged
}
