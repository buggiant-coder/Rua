use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookPoint {
    BeforeLlmCall,
    AfterLlmCall,
    BeforeToolCall,
    AfterToolCall,
    Startup,
    Shutdown,
    OnConversationEnd,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HookSpec {
    pub hook_point: HookPoint,
    pub hook_name: String,
    pub callback: String,
    #[serde(default = "default_priority")]
    pub priority: i32,
}

fn default_priority() -> i32 {
    100
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HookResult {
    pub plugin_id: String,
    pub hook_name: String,
    pub output: String,
}

#[derive(Debug, thiserror::Error)]
pub enum HookError {
    #[error(
        "hook '{hook_name}' already registered for plugin '{plugin_id}' at point '{hook_point:?}'"
    )]
    AlreadyRegistered {
        plugin_id: String,
        hook_name: String,
        hook_point: HookPoint,
    },
    #[error("plugin '{plugin_id}' not found")]
    PluginNotFound { plugin_id: String },
    #[error("invalid hook spec: {0}")]
    InvalidSpec(#[from] serde_json::Error),
    #[error("hook invocation failed for plugin '{plugin_id}': {message}")]
    InvocationFailed { plugin_id: String, message: String },
    #[error("mutex poisoned")]
    Poisoned,
}

#[derive(Debug, Clone)]
pub struct HookRegistration {
    pub plugin_id: String,
    pub spec: HookSpec,
    pub registration_id: String,
}

pub struct HookRegistry {
    hooks: tokio::sync::Mutex<HashMap<HookPoint, Vec<HookRegistration>>>,
    next_id: AtomicU64,
}

impl Default for HookRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl HookRegistry {
    pub fn new() -> Self {
        Self {
            hooks: tokio::sync::Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        }
    }

    fn gen_id(&self) -> String {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        format!("hook-{id}")
    }

    pub async fn register(&self, plugin_id: &str, spec: HookSpec) -> Result<String, HookError> {
        let mut map = self.hooks.lock().await;
        let entries = map.entry(spec.hook_point).or_default();

        let duplicate = entries
            .iter()
            .any(|r| r.plugin_id == plugin_id && r.spec.hook_name == spec.hook_name);
        if duplicate {
            return Err(HookError::AlreadyRegistered {
                plugin_id: plugin_id.to_string(),
                hook_name: spec.hook_name,
                hook_point: spec.hook_point,
            });
        }

        let registration_id = self.gen_id();
        entries.push(HookRegistration {
            plugin_id: plugin_id.to_string(),
            spec,
            registration_id: registration_id.clone(),
        });
        entries.sort_by_key(|r| r.spec.priority);
        Ok(registration_id)
    }

    pub async fn unregister(&self, registration_id: &str) -> bool {
        let mut map = self.hooks.lock().await;
        for entries in map.values_mut() {
            let before = entries.len();
            entries.retain(|r| r.registration_id != registration_id);
            if entries.len() < before {
                return true;
            }
        }
        false
    }

    pub async fn get_for_point(&self, point: HookPoint) -> Vec<HookRegistration> {
        let map = self.hooks.lock().await;
        map.get(&point).cloned().unwrap_or_default()
    }

    pub async fn remove_by_plugin(&self, plugin_id: &str) {
        let mut map = self.hooks.lock().await;
        for entries in map.values_mut() {
            entries.retain(|r| r.plugin_id != plugin_id);
        }
    }

    pub async fn list_for_plugin(&self, plugin_id: &str) -> Vec<HookRegistration> {
        let map = self.hooks.lock().await;
        map.values()
            .flatten()
            .filter(|r| r.plugin_id == plugin_id)
            .cloned()
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum DispatchOutcome {
    Continue,
    Aborted { reason: String },
}

pub struct DispatchResult {
    pub results: Vec<HookResult>,
    pub outcome: DispatchOutcome,
}
