use std::path::Path;

use rua_i18n::t;
use rua_skills::channels::channel_matches;
use rua_skills::frontmatter::parse_skill_md_safe;
use rua_skills::reconcile::reconcile_workspace_manifest;

use crate::commands::SkillsContext;

const DESCRIPTION_LIMIT: usize = 120;

pub struct SkillsCommandHandler;

impl SkillsCommandHandler {
    pub const COMMAND_NAME: &'static str = "/skills";
}

pub fn handle_skills_command(ctx: &SkillsContext) -> String {
    let Some(workspace_dir) = ctx.workspace_dir.as_deref() else {
        return t("skills.error.no_workspace", &ctx.locale);
    };

    let manifest = reconcile_workspace_manifest(workspace_dir).unwrap_or_default();
    let skills_root = workspace_skills_root(workspace_dir);
    let channel_id = ctx.channel_id.as_str();

    let mut lines: Vec<String> = Vec::new();
    let mut found = false;

    for (folder_name, entry) in manifest.skills.iter() {
        if !entry.enabled {
            continue;
        }
        if !channel_matches(&entry.channels, channel_id) {
            continue;
        }
        let skill_dir = skills_root.join(folder_name);
        if !skill_dir.exists() {
            continue;
        }
        found = true;
        let description = resolve_description(&skill_dir, &entry.metadata.description);
        lines.push(format!(
            "**{}**: {}",
            folder_name,
            truncate_description(&description, DESCRIPTION_LIMIT)
        ));
    }

    if !found {
        return t("skills.none_enabled", &ctx.locale);
    }
    lines.push(t("skills.footer_hint", &ctx.locale));
    lines.join("\n\n")
}

fn resolve_description(skill_dir: &Path, manifest_description: &str) -> String {
    let skill_md = skill_dir.join("SKILL.md");
    if skill_md.exists() {
        if let Ok(content) = std::fs::read_to_string(&skill_md) {
            let parsed = parse_skill_md_safe(
                &content,
                skill_dir.file_name().and_then(|s| s.to_str()).unwrap_or(""),
            );
            if !parsed.description.trim().is_empty() {
                return parsed.description;
            }
        }
    }
    let trimmed = manifest_description.trim();
    if trimmed.is_empty() {
        "No description.".to_string()
    } else {
        trimmed.to_string()
    }
}

fn workspace_skills_root(workspace_dir: &Path) -> std::path::PathBuf {
    let preferred = workspace_dir.join("skills");
    if preferred.exists() {
        return preferred;
    }
    let legacy = workspace_dir.join("skill");
    if legacy.exists() {
        return legacy;
    }
    preferred
}

fn truncate_description(text: &str, limit: usize) -> String {
    let normalized: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= limit {
        return normalized;
    }
    let target = limit.saturating_sub(3);
    let mut head: String = normalized.chars().take(target).collect();
    let trimmed_len = head.trim_end().len();
    head.truncate(trimmed_len);
    head.push_str("...");
    head
}
