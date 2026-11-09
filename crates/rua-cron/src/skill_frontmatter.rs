use std::collections::HashMap;

use rua_core::{Result, RuaError};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillFrontmatter {
    pub name: String,
    pub description: String,
    pub source: String,
    pub version: String,
    pub tags: Vec<String>,
    pub extra: HashMap<String, String>,
}

pub fn parse_frontmatter(content: &str) -> Result<SkillFrontmatter> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return Err(RuaError::Validation {
            message: "SKILL.md must begin with YAML frontmatter (---)".into(),
        });
    }

    let after_open = &trimmed[3..];
    let after_open = after_open.strip_prefix('\n').unwrap_or(after_open);

    let close_pos = after_open
        .find("\n---")
        .ok_or_else(|| RuaError::Validation {
            message: "SKILL.md frontmatter has no closing ---".into(),
        })?;

    let yaml_block = &after_open[..close_pos];
    let body = &after_open[close_pos + 4..];

    let mut fm = SkillFrontmatter::default();
    for line in yaml_block.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim();
            let value = value.trim();
            match key {
                "name" => fm.name = value.to_string(),
                "description" => fm.description = value.to_string(),
                "source" => fm.source = value.to_string(),
                "version" => fm.version = value.to_string(),
                "tags" => {
                    fm.tags = parse_yaml_list(value);
                }
                _ => {
                    fm.extra.insert(key.to_string(), value.to_string());
                }
            }
        }
    }

    if fm.name.is_empty() {
        return Err(RuaError::Validation {
            message: "SKILL.md frontmatter missing required field: name".into(),
        });
    }

    let _ = body;
    Ok(fm)
}

pub fn parse_yaml_list(value: &str) -> Vec<String> {
    let value = value.trim();
    if value.starts_with('[') && value.ends_with(']') {
        let inner = &value[1..value.len() - 1];
        inner
            .split(',')
            .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else if value.is_empty() {
        Vec::new()
    } else {
        vec![value.to_string()]
    }
}
