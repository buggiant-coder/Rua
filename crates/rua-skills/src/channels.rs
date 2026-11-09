use std::path::{Path, PathBuf};

use crate::manifest::WorkspaceManifest;

pub const ALL_SKILL_ROUTING_CHANNELS: &[&str] = &[
    "console",
    "discord",
    "telegram",
    "dingtalk",
    "feishu",
    "imessage",
    "qq",
    "mattermost",
    "wecom",
    "mqtt",
];

pub fn channel_matches(channels: &[String], channel_name: &str) -> bool {
    if channels.is_empty() {
        return true;
    }
    channels.iter().any(|c| c == "all" || c == channel_name)
}

pub fn resolve_effective_skills(workspace_dir: &Path, channel_name: &str) -> Vec<String> {
    let manifest_path = workspace_dir.join("skill.json");
    let bytes = match std::fs::read(&manifest_path) {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };
    let manifest: WorkspaceManifest = match serde_json::from_slice(&bytes) {
        Ok(m) => m,
        Err(_) => return Vec::new(),
    };
    let skills_root = workspace_skills_dir(workspace_dir);

    manifest
        .skills
        .iter()
        .filter(|(_, entry)| entry.enabled)
        .filter(|(_, entry)| channel_matches(&entry.channels, channel_name))
        .filter(|(name, _)| skills_root.join(name).exists())
        .map(|(name, _)| name.clone())
        .collect()
}

pub(crate) fn workspace_skills_dir(workspace_dir: &Path) -> PathBuf {
    let preferred = workspace_dir.join("skills");
    let legacy = workspace_dir.join("skill");
    if preferred.exists() {
        return preferred;
    }
    if legacy.exists() {
        if std::fs::rename(&legacy, &preferred).is_ok() {
            return preferred;
        }
        return legacy;
    }
    preferred
}
