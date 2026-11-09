use std::path::Path;

use serde_json::{Map, Value};

use crate::error::SkillError;
use crate::types::SkillRequirements;

pub const REQUIREMENTS_METADATA_NAMESPACES: &[&str] = &["openclaw", "rua", "clawdbot"];

#[derive(Debug, Clone, Default)]
pub struct ParsedSkillMd {
    pub name: String,
    pub description: String,
    pub version: String,
    pub tags: Vec<String>,
    pub emoji: String,
    pub triggers: Vec<String>,
    pub requirements: SkillRequirements,

    pub frontmatter: Map<String, Value>,
    pub body: String,
}

pub fn parse_skill_md(content: &str) -> Result<ParsedSkillMd, SkillError> {
    let (yaml, body) = split_frontmatter(content);
    let frontmatter = match yaml {
        Some(y) if !y.trim().is_empty() => parse_yaml_to_map(&y)?,
        _ => Map::new(),
    };
    Ok(build_parsed(frontmatter, body))
}

pub fn parse_skill_md_safe(content: &str, fallback_name: &str) -> ParsedSkillMd {
    match parse_skill_md(content) {
        Ok(parsed) => parsed,
        Err(err) => {
            tracing::warn!(
                "Failed to parse SKILL.md frontmatter for '{}': {}. Using fallback values.",
                fallback_name,
                err
            );
            ParsedSkillMd {
                name: fallback_name.to_string(),
                ..ParsedSkillMd::default()
            }
        }
    }
}

pub fn parse_skill_md_file(skill_dir: &Path) -> Result<ParsedSkillMd, SkillError> {
    let skill_md = skill_dir.join("SKILL.md");
    let content = std::fs::read_to_string(&skill_md)?;
    let mut parsed = parse_skill_md(&content)?;
    if parsed.name.is_empty() {
        if let Some(dir_name) = skill_dir.file_name().and_then(|s| s.to_str()) {
            parsed.name = dir_name.to_string();
        }
    }
    Ok(parsed)
}

fn split_frontmatter(content: &str) -> (Option<String>, String) {
    let normalized = if content.contains("\r\n") {
        content.replace("\r\n", "\n")
    } else {
        content.to_string()
    };

    let after_open = match normalized.strip_prefix("---\n") {
        Some(rest) => rest.to_string(),
        None => return (None, normalized),
    };

    if let Some(end_idx) = find_close_fence(&after_open) {
        let yaml = &after_open[..end_idx];
        let body_start = end_idx + close_fence_len(&after_open[end_idx..]);
        let body = after_open.get(body_start..).unwrap_or("");

        let body = body.strip_prefix('\n').unwrap_or(body);
        return (Some(yaml.to_string()), body.to_string());
    }

    (Some(after_open), String::new())
}

fn find_close_fence(after_open: &str) -> Option<usize> {
    let mut offset = 0usize;
    for line in after_open.split_inclusive('\n') {
        let trimmed = line.trim_end_matches('\n');
        if trimmed == "---" {
            return Some(offset);
        }
        offset += line.len();
    }
    None
}

fn close_fence_len(rest: &str) -> usize {
    if rest.starts_with("---\n") {
        4
    } else if rest.starts_with("---") {
        3
    } else {
        0
    }
}

fn parse_yaml_to_map(yaml: &str) -> Result<Map<String, Value>, SkillError> {
    let value: Value = serde_yaml::from_str(yaml).map_err(SkillError::Yaml)?;
    match value {
        Value::Object(m) => Ok(m),
        Value::Null => Ok(Map::new()),
        other => Err(SkillError::InvalidFrontmatter(format!(
            "expected YAML mapping at top level, got {}",
            type_of(&other)
        ))),
    }
}

fn type_of(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn build_parsed(frontmatter: Map<String, Value>, body: String) -> ParsedSkillMd {
    let name = string_field(&frontmatter, "name");
    let description = string_field(&frontmatter, "description");
    let mut emoji = string_field(&frontmatter, "emoji");
    if emoji.is_empty() {
        emoji = extract_namespaced_emoji(&frontmatter);
    }
    let tags = string_array(&frontmatter, "tags");
    let triggers = string_array(&frontmatter, "triggers");
    let version = extract_version(&frontmatter);
    let requirements = extract_requirements(&frontmatter);

    ParsedSkillMd {
        name,
        description,
        version,
        tags,
        emoji,
        triggers,
        requirements,
        frontmatter,
        body,
    }
}

fn extract_version(fm: &Map<String, Value>) -> String {
    if let Some(v) = non_empty_string(fm.get("version")) {
        return v;
    }
    let metadata = fm.get("metadata").and_then(|v| v.as_object());
    if let Some(m) = metadata {
        if let Some(v) = non_empty_string(m.get("version")) {
            return v;
        }
        if let Some(v) = non_empty_string(m.get("builtin_skill_version")) {
            return v;
        }
    }
    String::new()
}

fn extract_namespaced_emoji(fm: &Map<String, Value>) -> String {
    let metadata = match fm.get("metadata").and_then(|v| v.as_object()) {
        Some(m) => m,
        None => return String::new(),
    };
    for ns in REQUIREMENTS_METADATA_NAMESPACES {
        if let Some(provider) = metadata.get(*ns).and_then(|v| v.as_object()) {
            if let Some(s) = non_empty_string(provider.get("emoji")) {
                return s;
            }
        }
    }
    String::new()
}

pub fn extract_requirements(fm: &Map<String, Value>) -> SkillRequirements {
    let metadata = fm.get("metadata").and_then(|v| v.as_object());

    let mut requires: Option<&Value> = None;
    if let Some(m) = metadata {
        for ns in REQUIREMENTS_METADATA_NAMESPACES {
            if let Some(provider) = m.get(*ns).and_then(|v| v.as_object()) {
                if let Some(r) = provider.get("requires") {
                    if !r.is_null() {
                        requires = Some(r);
                        break;
                    }
                }
            }
        }
    }
    if requires.is_none() {
        if let Some(m) = metadata {
            if let Some(r) = m.get("requires") {
                if !r.is_null() {
                    requires = Some(r);
                }
            }
        }
    }
    if requires.is_none() {
        if let Some(r) = fm.get("requires") {
            if !r.is_null() {
                requires = Some(r);
            }
        }
    }

    let Some(requires) = requires else {
        return SkillRequirements::default();
    };

    if let Some(arr) = requires.as_array() {
        return SkillRequirements {
            require_bins: arr.iter().filter_map(value_to_string).collect(),
            require_envs: Vec::new(),
        };
    }
    if let Some(obj) = requires.as_object() {
        let bins = obj
            .get("bins")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(value_to_string).collect::<Vec<_>>())
            .unwrap_or_default();
        let envs = obj
            .get("env")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(value_to_string).collect::<Vec<_>>())
            .unwrap_or_default();
        return SkillRequirements {
            require_bins: bins,
            require_envs: envs,
        };
    }

    SkillRequirements::default()
}

fn string_field(fm: &Map<String, Value>, key: &str) -> String {
    fm.get(key).and_then(value_to_string).unwrap_or_default()
}

fn string_array(fm: &Map<String, Value>, key: &str) -> Vec<String> {
    fm.get(key)
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(value_to_string).collect())
        .unwrap_or_default()
}

fn non_empty_string(v: Option<&Value>) -> Option<String> {
    let s = value_to_string(v?)?;
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn value_to_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Null => None,
        other => Some(other.to_string()),
    }
}
