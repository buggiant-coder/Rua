use async_trait::async_trait;
use serde_json::{Map, Value};

use rua_skills::InstallMeta;

use crate::types::HubError;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NormalizedBundle {
    pub name: String,
    pub content: String,
    pub references: Map<String, Value>,
    pub scripts: Map<String, Value>,
    pub extra_files: Map<String, Value>,
}

#[async_trait]
pub trait BundleInstaller: Send + Sync {
    async fn create_skill(
        &self,
        name: &str,
        content: &str,
        references: &Map<String, Value>,
        scripts: &Map<String, Value>,
        extra_files: &Map<String, Value>,
        install_meta: Option<&InstallMeta>,
    ) -> Result<String, HubError>;

    async fn enable_skill(&self, name: &str) -> Result<bool, HubError>;
}

#[async_trait]
pub trait BundlePoolInstaller: Send + Sync {
    async fn create_pool_skill(
        &self,
        name: &str,
        content: &str,
        references: &Map<String, Value>,
        scripts: &Map<String, Value>,
        extra_files: &Map<String, Value>,
        install_meta: Option<&InstallMeta>,
    ) -> Result<String, HubError>;
}

pub fn normalize_bundle(value: &Value) -> Result<NormalizedBundle, HubError> {
    let payload = match value {
        Value::Object(map) => match map.get("skill") {
            Some(Value::Object(_)) => map.get("skill").unwrap(),
            _ => value,
        },
        _ => {
            return Err(HubError::InvalidBundle(
                "hub bundle is not a JSON object".into(),
            ));
        }
    };
    let payload = payload
        .as_object()
        .ok_or_else(|| HubError::InvalidBundle("hub bundle inner payload not object".into()))?;

    let mut content = payload
        .get("content")
        .or_else(|| payload.get("skill_md"))
        .or_else(|| payload.get("skillMd"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let mut references = sanitize_tree(payload.get("references"));
    let mut scripts = sanitize_tree(payload.get("scripts"));
    let mut extra_files: Map<String, Value> = Map::new();

    if let Some(Value::Object(files)) = payload.get("files") {
        let (ref2, scr2) = files_to_tree(files);
        if references.is_empty() {
            references = ref2;
        }
        if scripts.is_empty() {
            scripts = scr2;
        }
        for (rel, body) in files {
            let rel_str = rel.as_str();
            let Some(body_str) = body.as_str() else {
                continue;
            };
            if rel_str == "SKILL.md" {
                continue;
            }
            let Some(parts) = safe_path_parts(rel_str) else {
                continue;
            };
            if matches!(
                parts.first().map(String::as_str),
                Some("references") | Some("scripts")
            ) {
                continue;
            }
            tree_insert(&mut extra_files, &parts, body_str);
        }
        if content.is_empty() {
            if let Some(skill_md) = files.get("SKILL.md").and_then(|v| v.as_str()) {
                content = skill_md.to_string();
            }
        }
    }

    if content.is_empty() {
        return Err(HubError::InvalidBundle(
            "hub bundle missing SKILL.md".into(),
        ));
    }

    let mut name = payload
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if name.is_empty() {
        if let Some(parsed) = parse_frontmatter_name(&content) {
            name = parsed;
        }
    }
    if name.is_empty() {
        return Err(HubError::InvalidBundle(
            "hub bundle missing skill name".into(),
        ));
    }

    let description = payload
        .get("description")
        .or_else(|| payload.get("desc"))
        .or_else(|| payload.get("summary"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| fallback_description(&content, &name));
    content = ensure_required_frontmatter(&content, &name, &description)?;

    Ok(NormalizedBundle {
        name,
        content,
        references,
        scripts,
        extra_files,
    })
}

fn parse_frontmatter_name(content: &str) -> Option<String> {
    let trimmed = content.trim_start_matches('\u{feff}');
    let body = trimmed.strip_prefix("---")?;
    let (yaml, _rest) = body.split_once("\n---")?;
    let yaml = yaml.trim_start_matches('\n');
    let parsed: serde_yaml::Value = serde_yaml::from_str(yaml).ok()?;
    parsed
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
}

fn ensure_required_frontmatter(
    content: &str,
    name: &str,
    description: &str,
) -> Result<String, HubError> {
    let (frontmatter, body) = split_frontmatter(content);
    let mut map = match frontmatter.as_deref() {
        Some(yaml) => serde_yaml::from_str::<serde_yaml::Mapping>(yaml).map_err(|e| {
            HubError::InvalidBundle(format!("failed to parse SKILL.md frontmatter: {e}"))
        })?,
        None => serde_yaml::Mapping::new(),
    };

    let name_key = serde_yaml::Value::String("name".to_string());
    let desc_key = serde_yaml::Value::String("description".to_string());
    let has_name = yaml_string(&map, "name")
        .map(str::trim)
        .is_some_and(|s| !s.is_empty());
    let has_description = yaml_string(&map, "description")
        .map(str::trim)
        .is_some_and(|s| !s.is_empty());

    if has_name && has_description {
        return Ok(content.to_string());
    }

    if !has_name {
        map.insert(name_key, serde_yaml::Value::String(name.to_string()));
    }
    if !has_description {
        map.insert(desc_key, serde_yaml::Value::String(description.to_string()));
    }

    let yaml = serde_yaml::to_string(&map).map_err(|e| {
        HubError::InvalidBundle(format!("failed to write SKILL.md frontmatter: {e}"))
    })?;
    let body = body.trim_start_matches('\n');
    Ok(format!("---\n{}\n---\n{}", yaml.trim_end(), body))
}

fn split_frontmatter(content: &str) -> (Option<String>, String) {
    let trimmed = content.trim_start_matches('\u{feff}');
    let Some(rest) = trimmed.strip_prefix("---") else {
        return (None, content.to_string());
    };
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    let Some((yaml, body)) = rest.split_once("\n---") else {
        return (None, content.to_string());
    };
    (Some(yaml.to_string()), body.to_string())
}

fn yaml_string<'a>(map: &'a serde_yaml::Mapping, key: &str) -> Option<&'a str> {
    map.get(serde_yaml::Value::String(key.to_string()))
        .and_then(serde_yaml::Value::as_str)
}

fn fallback_description(content: &str, name: &str) -> String {
    content
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty()
                && *line != "---"
                && !line.starts_with("name:")
                && !line.starts_with("description:")
        })
        .map(|line| line.trim_start_matches('#').trim())
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(180).collect::<String>())
        .unwrap_or_else(|| format!("Imported skill {name}"))
}

pub fn normalize_skill_key(text: &str) -> String {
    let lower = text.to_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut prev_dash = false;
    for ch in lower.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

pub fn safe_fallback_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_dash = false;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            out.push(ch);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_matches(|c| c == '-' || c == '_');
    if trimmed.is_empty() {
        "imported-skill".to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn sanitize_skill_dir_name(name: &str) -> String {
    if name.is_empty() {
        return "imported-skill".to_string();
    }
    if name.contains('/') || name.contains('\\') {
        let key = normalize_skill_key(name);
        if !key.is_empty() {
            return key;
        }
        return safe_fallback_name(name);
    }
    name.to_string()
}

fn safe_path_parts(path: &str) -> Option<Vec<String>> {
    if path.is_empty() || path.starts_with('/') {
        return None;
    }
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return None;
    }
    for part in &parts {
        if *part == "." || *part == ".." {
            return None;
        }
    }
    Some(parts.into_iter().map(String::from).collect())
}

fn tree_insert(tree: &mut Map<String, Value>, parts: &[String], content: &str) {
    if parts.is_empty() {
        return;
    }
    if parts.len() == 1 {
        tree.insert(parts[0].clone(), Value::String(content.to_string()));
        return;
    }
    let head = &parts[0];
    let entry = tree
        .entry(head.clone())
        .or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    if let Value::Object(child) = entry {
        tree_insert(child, &parts[1..], content);
    }
}

fn sanitize_tree(value: Option<&Value>) -> Map<String, Value> {
    let Some(Value::Object(map)) = value else {
        return Map::new();
    };
    let mut out = Map::new();
    for (key, value) in map {
        if key == "." || key == ".." || key.contains('/') || key.contains('\\') {
            continue;
        }
        match value {
            Value::Object(_) => {
                out.insert(key.clone(), Value::Object(sanitize_tree(Some(value))));
            }
            Value::String(s) => {
                out.insert(key.clone(), Value::String(s.clone()));
            }
            _ => {}
        }
    }
    out
}

fn files_to_tree(files: &Map<String, Value>) -> (Map<String, Value>, Map<String, Value>) {
    let mut references: Map<String, Value> = Map::new();
    let mut scripts: Map<String, Value> = Map::new();
    for (rel, content) in files {
        let Some(body) = content.as_str() else {
            continue;
        };
        let Some(parts) = safe_path_parts(rel) else {
            continue;
        };
        if parts.first().map(String::as_str) == Some("references") && parts.len() > 1 {
            tree_insert(&mut references, &parts[1..], body);
        } else if parts.first().map(String::as_str) == Some("scripts") && parts.len() > 1 {
            tree_insert(&mut scripts, &parts[1..], body);
        }
    }
    (references, scripts)
}
