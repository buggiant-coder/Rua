use crate::types::{SkillInfo, SkillSource};

pub fn plugin_skill_id(plugin_id: &str, skill_name: &str) -> String {
    format!("plugin/{plugin_id}/{skill_name}")
}

pub fn parse_plugin_skill_id(skill_id: &str) -> Option<(&str, &str)> {
    let rest = skill_id.strip_prefix("plugin/")?;
    let (plugin_id, skill_name) = rest.split_once('/')?;
    if plugin_id.is_empty() || skill_name.is_empty() {
        return None;
    }
    Some((plugin_id, skill_name))
}

pub fn build_plugin_skill_info(
    plugin_id: &str,
    name: &str,
    description: &str,
    content: &str,
) -> SkillInfo {
    SkillInfo {
        name: plugin_skill_id(plugin_id, name),
        description: description.to_string(),
        version_text: String::new(),
        content: content.to_string(),
        source: SkillSource::Plugin(plugin_id.to_string()),
        references: serde_json::Value::Null,
        scripts: serde_json::Value::Null,
        tags: vec!["plugin".to_string()],
        emoji: String::new(),
    }
}
