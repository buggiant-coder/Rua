use std::path::Path;

use rua_config::AgentTemplate;
use serde_json::{Map, Value};

use crate::endpoints::dto::{
    AgentTemplateDetailResponse, AgentTemplateResponse, PromptFileStatus, SubagentTemplateResponse,
};

pub fn agent_template_response(id: String, template: AgentTemplate) -> AgentTemplateResponse {
    let mut extra = template.extra;
    extra.insert(
        "prompt_file_count".to_string(),
        Value::Number(template.prompt_files.len().into()),
    );
    AgentTemplateResponse {
        id,
        name: template.name,
        description: template.description,
        prompt_files: template.prompt_files,
        builtin: template.builtin,
        created_at: option_string(template.created_at),
        updated_at: option_string(template.updated_at),
        extra,
    }
}

pub fn agent_template_detail(
    id: String,
    template: AgentTemplate,
    prompt_library_dir: &Path,
) -> AgentTemplateDetailResponse {
    let prompt_file_status = template
        .prompt_files
        .iter()
        .map(|name| PromptFileStatus {
            name: name.clone(),
            available: prompt_library_dir.join(name).is_file(),
            extra: Map::new(),
        })
        .collect();

    AgentTemplateDetailResponse {
        id,
        name: template.name,
        description: template.description,
        prompt_files: template.prompt_files,
        builtin: template.builtin,
        created_at: option_string(template.created_at),
        updated_at: option_string(template.updated_at),
        prompt_file_status,
        extra: template.extra,
    }
}

pub fn parse_subagent_template(value: Value) -> Option<SubagentTemplateResponse> {
    serde_json::from_value::<SubagentTemplateResponse>(value).ok()
}

fn option_string(s: String) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}
