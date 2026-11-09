use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_types::{ToolAnnotations, ToolCall, ToolDefinition, ToolResult};
use async_trait::async_trait;
use rua_core::tool::ToolContext;
use thiserror::Error;

use crate::error::AgentError;
use crate::toolkit::{
    AgentToolkit, DisclosureClass, ToolDisclosureMeta, ToolExecutor, ToolSourceKind,
};

#[derive(Debug, Error)]
pub enum SkillToolError {
    #[error("failed to register skill tool '{name}': {message}")]
    RegisterFailed { name: String, message: String },
}

#[derive(Debug, Clone)]
pub struct SkillInfo {
    pub name: String,

    pub description: String,

    pub body: String,

    pub source: SkillRuntimeSource,

    pub base_dir: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SkillRuntimeSource {
    Agent,
    Project,
}

impl SkillRuntimeSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Project => "project",
        }
    }
}

pub fn register_skill_pseudo_tool(
    toolkit: &mut AgentToolkit,
    skills: Vec<SkillInfo>,
) -> Result<(), SkillToolError> {
    if skills.is_empty() {
        return Ok(());
    }

    let registry = skills
        .iter()
        .map(|skill| SkillEntry {
            name: skill.name.clone(),
            source: skill.source,
            body: skill.body.clone(),
            base_dir: skill.base_dir.clone(),
        })
        .collect();

    let description = pseudo_tool_description(&skills);
    let definition = ToolDefinition {
        name: "skill".to_string(),
        description,
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "A declared skill name, source-qualified selector such as agent:name or project:name, or path-qualified selector such as path:/absolute/skill/dir."
                }
            },
            "required": ["name"],
            "additionalProperties": false
        }),
        strict: None,
        annotations: ToolAnnotations {
            read_only: Some(true),
            ..Default::default()
        },
        metadata: serde_json::Map::new(),
    };

    let executor: Arc<dyn ToolExecutor> = Arc::new(SkillRegistryExecutor { skills: registry });
    toolkit.register_with_disclosure(
        definition,
        executor,
        ToolDisclosureMeta {
            class: DisclosureClass::Direct,
            source_kind: ToolSourceKind::Skill,
            source_name: Some("skill".to_string()),
            category: Some("skill".to_string()),
            schema_tokens: None,
        },
    );
    Ok(())
}

pub fn build_skills_prompt_block(skills: &[SkillInfo]) -> Option<String> {
    if skills.is_empty() {
        return Some("<available_skills>\nNo skills are installed or enabled. Do not attempt to locate, search for, or invoke any skills.\n</available_skills>".to_string());
    }
    let mut s = String::from(
        "<available_skills>\nSkills provide specialized instructions. Load via skill(name=\"...\") when relevant.\n\n",
    );
    for skill in skills {
        push_skill_listing(&mut s, skill);
    }
    s.push_str("</available_skills>");
    Some(s)
}

fn pseudo_tool_description(skills: &[SkillInfo]) -> String {
    let mut s = String::from(
        "Load skill instructions on demand. A unique declared name may be invoked directly. Use source:name or path:/absolute/skill/dir when names are ambiguous.\n\nAvailable skills:\n",
    );
    for skill in skills {
        push_skill_listing(&mut s, skill);
    }
    s
}

struct SkillEntry {
    name: String,
    source: SkillRuntimeSource,
    body: String,
    base_dir: PathBuf,
}

struct SkillRegistryExecutor {
    skills: Vec<SkillEntry>,
}

#[async_trait]
impl ToolExecutor for SkillRegistryExecutor {
    async fn execute(&self, call: &ToolCall, _ctx: &ToolContext) -> Result<ToolResult, AgentError> {
        match call.arguments.get("name") {
            Some(serde_json::Value::String(requested)) => match resolve_skill(
                &self.skills,
                requested,
            ) {
                SkillResolution::Found(entry) => {
                    Ok(ToolResult::text(
                        call.id.clone(),
                        format_skill_result(entry),
                    ))
                }
                SkillResolution::Ambiguous(matches) => Ok(ToolResult::error(
                    call.id.clone(),
                    format!(
                        "Ambiguous skill '{}'. Matches: {}. Use a source-qualified selector when unique within that source, or an exact path-qualified selector.",
                        requested,
                        path_selectors(matches),
                    ),
                )),
                SkillResolution::Missing => Ok(ToolResult::error(
                        call.id.clone(),
                        format!(
                            "Unknown skill '{}'. Available skills: {}.",
                            requested,
                            available_selectors(&self.skills),
                        ),
                    )),
            },
            Some(_non_string) => Ok(ToolResult::error(
                call.id.clone(),
                format!(
                    "Invalid 'name' argument: expected a string. Available skills: {}.",
                    available_selectors(&self.skills),
                ),
            )),
            None => Ok(ToolResult::error(
                call.id.clone(),
                format!(
                    "Unknown skill '<missing>'. Available skills: {}.",
                    available_selectors(&self.skills),
                ),
            )),
        }
    }
}

const MAX_SKILL_FILES: usize = 50;

fn push_skill_listing(output: &mut String, skill: &SkillInfo) {
    output.push_str(&format!(
        "- `{}` (source `{}`, path `{}`): {}\n",
        skill.name,
        skill.source.as_str(),
        skill.base_dir.display(),
        skill.description
    ));
}

enum SkillResolution<'a> {
    Found(&'a SkillEntry),
    Ambiguous(Vec<&'a SkillEntry>),
    Missing,
}

fn resolve_skill<'a>(skills: &'a [SkillEntry], requested: &str) -> SkillResolution<'a> {
    let matches = if let Some(path) = requested.strip_prefix("path:") {
        skills
            .iter()
            .filter(|skill| skill.base_dir == Path::new(path))
            .collect::<Vec<_>>()
    } else if let Some((source, name)) = requested.split_once(':') {
        let source = match source {
            "agent" => Some(SkillRuntimeSource::Agent),
            "project" => Some(SkillRuntimeSource::Project),
            _ => None,
        };
        source
            .into_iter()
            .flat_map(|source| {
                skills
                    .iter()
                    .filter(move |skill| skill.source == source && skill.name == name)
            })
            .collect::<Vec<_>>()
    } else {
        skills
            .iter()
            .filter(|skill| skill.name == requested)
            .collect::<Vec<_>>()
    };
    match matches.as_slice() {
        [entry] => SkillResolution::Found(entry),
        [] => SkillResolution::Missing,
        _ => SkillResolution::Ambiguous(matches),
    }
}

fn format_skill_result(entry: &SkillEntry) -> String {
    let mut output = entry.body.to_string();
    output.push_str(&format!(
        "\n\nSource: {}\nBase directory: {}",
        entry.source.as_str(),
        entry.base_dir.display()
    ));

    let files = list_skill_files(&entry.base_dir);
    if !files.is_empty() {
        output.push_str("\n\n<skill_files>");
        for f in &files {
            output.push_str(&format!("\n{}", f.display()));
        }
        output.push_str("\n</skill_files>");
    }

    output
}

fn list_skill_files(base_dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let skill_md = base_dir.join("SKILL.md");
    let mut visited = HashSet::new();
    collect_files_recursive(base_dir, &skill_md, &mut files, &mut visited);
    files.sort();
    files.truncate(MAX_SKILL_FILES);
    files
}

fn collect_files_recursive(
    dir: &Path,
    exclude: &Path,
    out: &mut Vec<PathBuf>,
    visited: &mut HashSet<PathBuf>,
) {
    if out.len() >= MAX_SKILL_FILES {
        return;
    }
    let canonical = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    if !visited.insert(canonical) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= MAX_SKILL_FILES {
            return;
        }
        let path = entry.path();
        if path.is_file() {
            if path == exclude {
                continue;
            }
            out.push(path);
        } else if path.is_dir() {
            collect_files_recursive(&path, exclude, out, visited);
        }
    }
}

fn available_selectors(skills: &[SkillEntry]) -> String {
    skills
        .iter()
        .flat_map(|skill| {
            [
                format!("{}:{}", skill.source.as_str(), skill.name),
                format!("path:{}", skill.base_dir.display()),
            ]
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join(", ")
}

fn path_selectors(skills: Vec<&SkillEntry>) -> String {
    skills
        .into_iter()
        .map(|skill| format!("path:{}", skill.base_dir.display()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join(", ")
}
