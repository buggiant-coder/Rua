use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::thread::ThreadId;

use serde_json::Value;
use tracing::warn;

use crate::channels::resolve_effective_skills;
use crate::error::SkillError;
use crate::manifest::WorkspaceManifest;

#[derive(Debug, Clone)]
pub struct EnvEntry {
    pub value: String,
    pub count: usize,
}

static SKILL_ENV_REGISTRY: OnceLock<Mutex<HashMap<String, EnvEntry>>> = OnceLock::new();

fn registry() -> &'static Mutex<HashMap<String, EnvEntry>> {
    SKILL_ENV_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

#[must_use = "SkillEnvOverrides must be held until the env vars are no longer needed; \
              dropping it immediately is almost certainly a bug"]
pub struct SkillEnvOverrides {
    keys: Vec<String>,
}

impl SkillEnvOverrides {
    pub fn activate(workspace_dir: &Path, channel: &str) -> Result<Self, SkillError> {
        record_activate_thread();
        let manifest = read_workspace_manifest(workspace_dir);
        let effective = resolve_effective_skills(workspace_dir, channel);
        let mut keys: Vec<String> = Vec::new();

        for skill_name in effective {
            let Some(entry) = manifest.skills.get(&skill_name) else {
                continue;
            };
            let config = &entry.config;
            if config.is_empty() {
                continue;
            }

            let require_envs: Vec<String> = entry
                .requirements
                .require_envs
                .iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();

            let overrides = build_skill_config_env_overrides(&skill_name, config, &require_envs);

            for (env_key, env_value) in overrides {
                if !acquire_env_key(&env_key, &env_value) {
                    warn!(
                        skill = %skill_name,
                        env = %env_key,
                        "skipped env override: conflict with existing value or pre-set env",
                    );
                    continue;
                }
                keys.push(env_key);
            }
        }

        Ok(SkillEnvOverrides { keys })
    }

    pub fn keys(&self) -> &[String] {
        &self.keys
    }
}

impl Drop for SkillEnvOverrides {
    fn drop(&mut self) {
        for key in self.keys.iter().rev() {
            release_env_key(key);
        }
        release_activate_thread();
    }
}

pub fn is_main_thread() -> bool {
    std::thread::current().name() == Some("main")
}

static ACTIVATE_THREAD: Mutex<Option<(ThreadId, usize)>> = Mutex::new(None);

fn record_activate_thread() {
    let tid = std::thread::current().id();
    let mismatch: Option<ThreadId>;
    {
        let mut slot = ACTIVATE_THREAD.lock().unwrap_or_else(|e| e.into_inner());
        match &mut *slot {
            Some((recorded, count)) => {
                if *recorded == tid {
                    *count += 1;
                    mismatch = None;
                } else {
                    mismatch = Some(*recorded);
                }
            }
            None => {
                *slot = Some((tid, 1));
                mismatch = None;
            }
        }
    }
    if let Some(recorded_tid) = mismatch {
        warn!(
            first_thread = ?recorded_tid,
            current_thread = ?tid,
            "SkillEnvOverrides::activate called from a different thread while another guard is active",
        );
    }
}

fn release_activate_thread() {
    let mut slot = ACTIVATE_THREAD.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, count)) = &mut *slot {
        *count -= 1;
        if *count == 0 {
            *slot = None;
        }
    }
}

fn read_workspace_manifest(workspace_dir: &Path) -> WorkspaceManifest {
    let manifest_path = workspace_dir.join("skill.json");
    std::fs::read(&manifest_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<WorkspaceManifest>(&bytes).ok())
        .unwrap_or_default()
}

fn build_skill_config_env_overrides(
    skill_name: &str,
    config: &serde_json::Map<String, Value>,
    require_envs: &[String],
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::with_capacity(require_envs.len() + 1);
    let required: std::collections::HashSet<&str> =
        require_envs.iter().map(|s| s.as_str()).collect();

    for (key, value) in config.iter() {
        if !required.contains(key.as_str()) {
            continue;
        }
        let Some(stringified) = stringify_skill_env_value(value) else {
            continue;
        };
        out.push((key.clone(), stringified));
    }

    for env_name in require_envs {
        if !out.iter().any(|(k, _)| k == env_name) {
            warn!(
                skill = %skill_name,
                env = %env_name,
                "skill requires env but config does not provide it",
            );
        }
    }

    let json_blob = serde_json::to_string(config).unwrap_or_else(|_| "{}".to_string());
    out.push((skill_config_env_var_name(skill_name), json_blob));
    out
}

fn stringify_skill_env_value(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(s) if s.is_empty() => None,
        Value::String(s) => Some(s.clone()),
        other => serde_json::to_string(other).ok(),
    }
}

pub fn skill_config_env_var_name(skill_name: &str) -> String {
    let normalized: String = skill_name
        .to_uppercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let trimmed = normalized.trim_matches('_');
    if trimmed.is_empty() {
        "RUA_SKILL_CONFIG_DEFAULT".to_string()
    } else {
        format!("RUA_SKILL_CONFIG_{}", trimmed)
    }
}

fn acquire_env_key(key: &str, value: &str) -> bool {
    let mut reg = registry()
        .lock()
        .expect("SKILL_ENV_REGISTRY mutex poisoned");
    if let Some(entry) = reg.get_mut(key) {
        if entry.value != value {
            return false;
        }
        entry.count += 1;

        unsafe {
            std::env::set_var(key, value);
        }
        return true;
    }

    if std::env::var(key).is_ok() {
        return false;
    }

    reg.insert(
        key.to_string(),
        EnvEntry {
            value: value.to_string(),
            count: 1,
        },
    );

    unsafe {
        std::env::set_var(key, value);
    }
    true
}

fn release_env_key(key: &str) {
    let mut reg = registry()
        .lock()
        .expect("SKILL_ENV_REGISTRY mutex poisoned");
    let Some(entry) = reg.get_mut(key) else {
        return;
    };
    entry.count = entry.count.saturating_sub(1);
    if entry.count > 0 {
        let value = entry.value.clone();
        unsafe {
            std::env::set_var(key, value);
        }
        return;
    }
    reg.remove(key);

    unsafe {
        std::env::remove_var(key);
    }
}
