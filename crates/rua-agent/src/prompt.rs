use std::collections::HashMap;
use std::path::Path;

use llm_multiplex::traits::ModelInfo;
use rua_core::runtime_layout::RuntimeLayout;

use crate::error::AgentError;
use crate::profile::AgentProfile;

pub fn build_execution_environment_prompt(
    layout: &RuntimeLayout,
    workspace_dir: &Path,
    host_profile: &str,
    device_id: Option<&str>,
    device_name: Option<&str>,
) -> String {
    let contract = layout.agent_path_contract(workspace_dir);
    let mut lines = vec!["## Execution environment".to_string()];

    match (device_name, device_id) {
        (Some(name), Some(id)) => {
            lines.push(format!("- Execution device: `{name}` (`{id}`)."));
        }
        (Some(name), None) => lines.push(format!("- Execution device: `{name}`.")),
        (None, Some(id)) => lines.push(format!("- Execution device ID: `{id}`.")),
        (None, None) => {}
    }
    lines.push(format!(
        "- Host profile: `{host_profile}`; operating system: `{}`.",
        layout.os_family
    ));

    if layout.host_profile.is_mobile() || layout.os_family == "android" {
        lines.push(
            "- Agent commands, Python, skills, MCP, ACP, and plugins run in the Linux Compatibility Runtime (PRoot Debian)."
                .to_string(),
        );
    } else {
        lines.push("- Agent subprocesses run in the native host runtime.".to_string());
    }

    lines.push(format!("- Agent workspace: `{}`.", contract.workspace_dir));

    if layout.host_profile.is_mobile() || layout.os_family == "android" {
        lines.push(
            "- Phone shared storage is available at /sdcard when Android storage permission is granted."
                .to_string(),
        );
        lines.push(
            "- When the user refers to files on this phone, inspect /sdcard instead of claiming that the phone filesystem is inaccessible."
                .to_string(),
        );
        lines.push("- Use absolute paths. Avoid `~` in tool arguments.".to_string());
        if let Some(home) = contract.aliases.get("linux_guest_home") {
            lines.push(format!(
                "- `{home}` is only the Linux Compatibility Runtime guest HOME for CLI configuration."
            ));
        }
    } else {
        lines.extend(contract.rules.iter().map(|rule| format!("- {rule}")));
    }

    lines.extend(
        contract
            .warnings
            .iter()
            .map(|warning| format!("- Warning: {warning}")),
    );
    lines.push(
        "- Absolute paths from another execution device are unavailable unless the files are transferred."
            .to_string(),
    );

    lines.join("\n")
}

#[derive(Debug, Clone, Default)]
pub struct PromptVariables {
    pub date: String,

    pub timezone: String,

    pub user_id: String,

    pub agent_id: String,

    pub channel: String,

    pub workspace: String,

    pub tools_list: String,

    pub memory_summary: Option<String>,

    pub custom: HashMap<String, String>,

    pub supports_image: Option<bool>,

    pub supports_video: Option<bool>,

    pub supports_multimodal: Option<bool>,
}

impl PromptVariables {
    pub fn new(agent_id: impl Into<String>, channel: impl Into<String>) -> Self {
        let now = chrono::Local::now();
        Self {
            date: now.format("%Y-%m-%d").to_string(),
            timezone: chrono::Local::now().offset().to_string(),
            user_id: String::new(),
            agent_id: agent_id.into(),
            channel: channel.into(),
            workspace: String::new(),
            tools_list: String::new(),
            memory_summary: None,
            custom: HashMap::new(),
            supports_image: None,
            supports_video: None,
            supports_multimodal: None,
        }
    }

    pub fn with_user_id(mut self, user_id: impl Into<String>) -> Self {
        self.user_id = user_id.into();
        self
    }

    pub fn with_workspace(mut self, workspace: impl Into<String>) -> Self {
        self.workspace = workspace.into();
        self
    }

    pub fn with_tools_list(mut self, tools_list: impl Into<String>) -> Self {
        self.tools_list = tools_list.into();
        self
    }

    pub fn with_memory_summary(mut self, summary: impl Into<String>) -> Self {
        self.memory_summary = Some(summary.into());
        self
    }

    pub fn with_custom(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.custom.insert(key.into(), value.into());
        self
    }

    pub fn with_model_info(mut self, model: &ModelInfo) -> Self {
        self.supports_image = model.supports_image;
        self.supports_video = model.supports_video;
        self.supports_multimodal = model.supports_multimodal;
        self
    }

    fn resolve(&self, name: &str) -> Option<String> {
        match name {
            "date" => Some(self.date.clone()),
            "timezone" => Some(self.timezone.clone()),
            "user_id" => Some(self.user_id.clone()),
            "agent_id" => Some(self.agent_id.clone()),
            "channel" => Some(self.channel.clone()),
            "workspace" => Some(self.workspace.clone()),
            "tools_list" => Some(self.tools_list.clone()),
            "memory_summary" => self.memory_summary.clone(),
            _ => self.custom.get(name).cloned(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct UnresolvedVariable {
    pub name: String,

    pub position: usize,
}

#[derive(Debug, Clone)]
pub struct PromptRenderResult {
    pub text: String,

    pub unresolved: Vec<UnresolvedVariable>,
}

pub struct PromptBuilder;

impl PromptBuilder {
    pub fn render_template(
        template: &str,
        vars: &PromptVariables,
        profile: &AgentProfile,
    ) -> PromptRenderResult {
        let mut result = String::with_capacity(template.len());
        let mut unresolved = Vec::new();
        let mut chars = template.char_indices().peekable();

        while let Some((pos, ch)) = chars.next() {
            if ch == '{' {
                if let Some(&(_, '{')) = chars.peek() {
                    chars.next();

                    let mut var_name = String::new();
                    let mut found_close = false;
                    while let Some((_, c)) = chars.next() {
                        if c == '}' {
                            if let Some(&(_, '}')) = chars.peek() {
                                chars.next();
                                found_close = true;
                                break;
                            }
                        }
                        var_name.push(c);
                    }

                    if found_close {
                        let var_name = var_name.trim();

                        let resolved = if var_name == "system_prompt" {
                            Some(profile.system_prompt.clone())
                        } else {
                            vars.resolve(var_name)
                        };

                        match resolved {
                            Some(value) => result.push_str(&value),
                            None => {
                                result.push_str("{{");
                                result.push_str(var_name);
                                result.push_str("}}");
                                unresolved.push(UnresolvedVariable {
                                    name: var_name.to_string(),
                                    position: pos,
                                });
                            }
                        }
                    } else {
                        result.push_str("{{");
                        result.push_str(&var_name);
                    }
                } else {
                    result.push(ch);
                }
            } else {
                result.push(ch);
            }
        }

        PromptRenderResult {
            text: result,
            unresolved,
        }
    }

    pub fn format_tools_list(tools: &[(String, String)]) -> String {
        if tools.is_empty() {
            return "No tools available.".to_string();
        }
        tools
            .iter()
            .map(|(name, desc)| format!("- {name}: {desc}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub const AGENT_PROMPT_FILES_METADATA_KEY: &str = "agent_prompt_files";
pub const AGENT_CUSTOM_PROMPT_METADATA_KEY: &str = "agent_custom_prompt";
pub const WORKSPACE_CONTEXT_METADATA_KEY: &str = "workspace_context";
pub const AGENT_PROMPT_CONTEXT_METADATA_KEY: &str = "agent_prompt_context";
pub const LEGACY_CONSOLE_BUILTIN_PROMPT_FILE: &str = "__RUA_BUILTIN_SYSTEM_PROMPT__.md";

const WORKSPACE_PROMPTS_DIR: &str = "prompts";
const RESERVED_PROMPT_FILES: &[&str] = &[
    "BOOTSTRAP.md",
    "AGENT_MEMORY.md",
    LEGACY_CONSOLE_BUILTIN_PROMPT_FILE,
];

pub fn strip_frontmatter(content: &str) -> &str {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return content;
    }

    let after_open = &trimmed[3..];
    if let Some(close_pos) = after_open.find("\n---") {
        let remainder = &after_open[close_pos + 4..];

        remainder.strip_prefix('\n').unwrap_or(remainder)
    } else {
        content
    }
}

pub fn parse_frontmatter(
    content: &str,
) -> (Option<serde_json::Map<String, serde_json::Value>>, String) {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (None, content.to_string());
    }
    let after_open = &trimmed[3..];
    let Some(close_pos) = after_open.find("\n---") else {
        return (None, content.to_string());
    };
    let yaml_str = &after_open[..close_pos];
    let remainder = &after_open[close_pos + 4..];
    let body = remainder
        .strip_prefix('\n')
        .unwrap_or(remainder)
        .to_string();

    let mut map = serde_json::Map::new();
    let mut current_key: Option<String> = None;
    let mut current_list: Vec<serde_json::Value> = Vec::new();

    for line in yaml_str.lines() {
        let trimmed_line = line.trim();
        if trimmed_line.is_empty() || trimmed_line.starts_with('#') {
            continue;
        }

        if trimmed_line.starts_with("- ") && current_key.is_some() {
            let item = trimmed_line[2..].trim();
            current_list.push(serde_json::Value::String(
                item.trim_matches('"').trim_matches('\'').to_string(),
            ));
            continue;
        }

        if let Some(ref key) = current_key {
            if !current_list.is_empty() {
                map.insert(
                    key.clone(),
                    serde_json::Value::Array(std::mem::take(&mut current_list)),
                );
                current_key = None;
            }
        }

        if let Some((key, value)) = trimmed_line.split_once(':') {
            let key = key.trim().to_string();
            let value = value.trim();

            if value.is_empty() {
                current_key = Some(key);
                current_list.clear();
                continue;
            }

            let parsed_value = parse_yaml_value(value);
            current_key = None;
            map.insert(key, parsed_value);
        }
    }

    if let Some(key) = current_key {
        if !current_list.is_empty() {
            map.insert(key, serde_json::Value::Array(current_list));
        }
    }

    let result = if map.is_empty() { None } else { Some(map) };
    (result, body)
}

fn parse_yaml_value(value: &str) -> serde_json::Value {
    if value.starts_with('[') && value.ends_with(']') {
        let inner = &value[1..value.len() - 1];
        let items: Vec<serde_json::Value> = inner
            .split(',')
            .map(|s| {
                serde_json::Value::String(s.trim().trim_matches('"').trim_matches('\'').to_string())
            })
            .filter(|v| !v.as_str().unwrap_or("").is_empty())
            .collect();
        return serde_json::Value::Array(items);
    }

    match value {
        "true" | "True" | "yes" | "Yes" => return serde_json::Value::Bool(true),
        "false" | "False" | "no" | "No" => return serde_json::Value::Bool(false),
        _ => {}
    }

    if let Ok(n) = value.parse::<i64>() {
        return serde_json::Value::Number(n.into());
    }

    let s = value.trim_matches('"').trim_matches('\'');
    serde_json::Value::String(s.to_string())
}

pub fn build_system_prompt_from_working_dir(
    workspace_dir: &Path,
    active_prompt_files: Option<&[String]>,
) -> Result<String, AgentError> {
    let prompt_dir = workspace_dir.join(WORKSPACE_PROMPTS_DIR);
    let files_to_load: Vec<String> = if let Some(active) = active_prompt_files {
        active
            .iter()
            .filter(|filename| !is_reserved_prompt_file(filename))
            .cloned()
            .collect()
    } else {
        let mut found = Vec::new();
        collect_prompt_files(&prompt_dir, &prompt_dir, &mut found);
        found.retain(|filename| !is_reserved_prompt_file(filename));
        found.sort();
        found
    };

    let mut sections = Vec::new();
    for filename in &files_to_load {
        let path = prompt_dir.join(filename);
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                let stripped = strip_frontmatter(&content);
                let trimmed = stripped.trim();
                if !trimmed.is_empty() {
                    sections.push(trimmed.to_string());
                }
            }
            Err(_) => {
                tracing::debug!(file = %filename, "Prompt file not found, skipping");
            }
        }
    }

    let joined = sections.join("\n\n");
    Ok(strip_heartbeat(&joined).trim().to_string())
}

fn is_reserved_prompt_file(filename: &str) -> bool {
    let name = Path::new(filename)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(filename);
    RESERVED_PROMPT_FILES
        .iter()
        .any(|reserved| name.eq_ignore_ascii_case(reserved))
}

fn collect_prompt_files(root: &Path, current: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(current) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_prompt_files(root, &path, out);
            continue;
        }
        if !path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| {
                ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown")
            })
        {
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        if !relative.is_empty() {
            out.push(relative);
        }
    }
}

pub fn inject_multimodal_hint(
    prompt: &str,
    supports_image: Option<bool>,
    supports_video: Option<bool>,
    supports_multimodal: Option<bool>,
) -> String {
    if supports_image == Some(true) || supports_video == Some(true) {
        return prompt.to_string();
    }
    if supports_multimodal.is_none() {
        return prompt.to_string();
    }
    let hint = "It appears that you can only understand text content. \
 Please honestly inform the user about this when \
 their input includes multimodal information.";
    format!("{prompt}\n\n{hint}\n")
}

pub fn strip_heartbeat(input: &str) -> String {
    const START_MARKER: &str = "<!-- heartbeat:start -->";
    const END_MARKER: &str = "<!-- heartbeat:end -->";

    let mut result = input.to_string();
    while let Some(start) = result.find(START_MARKER) {
        let after_start = start + START_MARKER.len();
        let Some(end_offset) = result[after_start..].find(END_MARKER) else {
            break;
        };
        let end = after_start + end_offset + END_MARKER.len();
        result = format!("{}{}", &result[..start], &result[end..]);
    }
    result
}
