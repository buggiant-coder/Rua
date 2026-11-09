use std::collections::HashMap;
use std::sync::Mutex;

use rua_skills::types::SkillInfo;
use rua_skills::{build_plugin_skill_info, parse_plugin_skill_id, plugin_skill_id};
use rua_skills::{SkillPoolService, SkillSource};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginSkillSpec {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub content: String,
}

impl PluginSkillSpec {
    fn to_skill_md(&self) -> String {
        if self.content.is_empty() {
            format!(
                "---\nname: {}\ndescription: {}\n---\n",
                self.name, self.description
            )
        } else {
            self.content.clone()
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error("skill '{skill_name}' already registered for plugin '{plugin_id}'")]
    AlreadyRegistered {
        plugin_id: String,
        skill_name: String,
    },
    #[error("plugin '{plugin_id}' not found")]
    PluginNotFound { plugin_id: String },
    #[error("skill source conflict: skill '{skill_name}' owned by {existing_source}, plugin '{plugin_id}' cannot claim it")]
    SourceConflict {
        plugin_id: String,
        skill_name: String,
        existing_source: String,
    },
    #[error("invalid skill spec: {0}")]
    InvalidSpec(#[from] serde_json::Error),
    #[error("pool service error: {0}")]
    Pool(#[from] rua_skills::SkillError),
    #[error("skill registry error: {0}")]
    SkillRegistry(#[from] SkillRegistryError),
    #[error("mutex poisoned")]
    Poisoned,
}

pub struct SkillRegistrar<'a> {
    pub registry: &'a crate::PluginRegistry,
    pub pool: &'a SkillPoolService,
    pub skill_registry: Option<&'a PluginSkillRegistry>,
}

impl<'a> SkillRegistrar<'a> {
    pub fn new(registry: &'a crate::PluginRegistry, pool: &'a SkillPoolService) -> Self {
        Self {
            registry,
            pool,
            skill_registry: None,
        }
    }

    pub fn with_skill_registry(mut self, sr: &'a PluginSkillRegistry) -> Self {
        self.skill_registry = Some(sr);
        self
    }
}

pub fn register_skill(
    registrar: &SkillRegistrar<'_>,
    plugin_id: &str,
    skill_manifest_json: &str,
) -> Result<(), SkillError> {
    let spec: PluginSkillSpec = serde_json::from_str(skill_manifest_json)?;
    let content = spec.to_skill_md();
    let source = SkillSource::Plugin(plugin_id.to_string());

    registrar.registry.track_skill(plugin_id, &spec.name)?;

    if let Some(sr) = registrar.skill_registry {
        if let Err(e) = sr.register(plugin_id, &spec.name, &spec.description, &content) {
            registrar.registry.untrack_plugin_skills(plugin_id);
            return Err(SkillError::SkillRegistry(e));
        }
    }

    match registrar
        .pool
        .create_skill(&spec.name, &content, None, source)
    {
        Ok(_) => Ok(()),
        Err(rua_skills::SkillError::SourceConflict {
            skill_name,
            existing_source,
            ..
        }) => {
            registrar.registry.untrack_plugin_skills(plugin_id);
            if let Some(sr) = registrar.skill_registry {
                sr.unregister_plugin(plugin_id);
            }
            Err(SkillError::SourceConflict {
                plugin_id: plugin_id.to_string(),
                skill_name,
                existing_source,
            })
        }
        Err(e) => {
            registrar.registry.untrack_plugin_skills(plugin_id);
            if let Some(sr) = registrar.skill_registry {
                sr.unregister_plugin(plugin_id);
            }
            Err(SkillError::Pool(e))
        }
    }
}

pub fn remove_plugin_skills(
    registrar: &SkillRegistrar<'_>,
    plugin_id: &str,
) -> Result<Vec<String>, SkillError> {
    if let Some(sr) = registrar.skill_registry {
        sr.unregister_plugin(plugin_id);
    }

    let expected_source = SkillSource::Plugin(plugin_id.to_string());
    let names = registrar.registry.untrack_plugin_skills(plugin_id);
    let mut removed = Vec::new();
    for name in &names {
        if let Ok(Some(info)) = registrar.pool.get_skill(name) {
            if info.source == expected_source {
                let _ = registrar.pool.delete_skill(name);
                removed.push(name.clone());
            }
        }
    }
    Ok(removed)
}

#[derive(Debug, thiserror::Error)]
pub enum SkillRegistryError {
    #[error("skill '{skill_id}' already registered")]
    AlreadyRegistered { skill_id: String },
    #[error("mutex poisoned")]
    Poisoned,
}

#[derive(Debug, Default)]
pub struct PluginSkillRegistry {
    skills: Mutex<HashMap<String, SkillInfo>>,
}

impl PluginSkillRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &self,
        plugin_id: &str,
        name: &str,
        description: &str,
        content: &str,
    ) -> Result<String, SkillRegistryError> {
        let skill_id = plugin_skill_id(plugin_id, name);
        let info = build_plugin_skill_info(plugin_id, name, description, content);
        let mut map = self
            .skills
            .lock()
            .map_err(|_| SkillRegistryError::Poisoned)?;
        if map.contains_key(&skill_id) {
            return Err(SkillRegistryError::AlreadyRegistered {
                skill_id: skill_id.clone(),
            });
        }
        map.insert(skill_id.clone(), info);
        Ok(skill_id)
    }

    pub fn unregister_plugin(&self, plugin_id: &str) -> Vec<String> {
        let prefix = format!("plugin/{plugin_id}/");
        let mut map = match self.skills.lock() {
            Ok(m) => m,
            Err(_) => return Vec::new(),
        };
        let to_remove: Vec<String> = map
            .keys()
            .filter(|k| k.starts_with(&prefix))
            .cloned()
            .collect();
        for key in &to_remove {
            map.remove(key);
        }
        to_remove
    }

    pub fn list_all(&self) -> Vec<SkillInfo> {
        self.skills
            .lock()
            .map(|map| map.values().cloned().collect())
            .unwrap_or_default()
    }

    pub fn list_for_plugin(&self, plugin_id: &str) -> Vec<SkillInfo> {
        let prefix = format!("plugin/{plugin_id}/");
        self.skills
            .lock()
            .map(|map| {
                map.iter()
                    .filter(|(k, _)| k.starts_with(&prefix))
                    .map(|(_, v)| v.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn get(&self, skill_id: &str) -> Option<SkillInfo> {
        self.skills
            .lock()
            .ok()
            .and_then(|map| map.get(skill_id).cloned())
    }

    pub fn resolve(skill_id: &str) -> Option<(&str, &str)> {
        parse_plugin_skill_id(skill_id)
    }

    pub fn is_plugin_skill(skill_id: &str) -> bool {
        skill_id.starts_with("plugin/")
    }
}
