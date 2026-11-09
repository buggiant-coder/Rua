use std::path::PathBuf;

use crate::migration::atomic::atomic_write_json;
use crate::types::templates::{AgentTemplate, AgentTemplatesFile};

pub struct AgentTemplateStore {
    rua_root: PathBuf,
}

struct Builtin {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    prompt_files: &'static [&'static str],
}

const BUILTINS: &[Builtin] = &[
    Builtin {
        id: "default",
        name: "默认智能体",
        description: "小Rua 的完整提示词配置",
        prompt_files: &[
            "10_AGENT_SAFETY.md",
            "20_AGENT_WORKFLOW.md",
            "30_EXECUTION_REPORTING.md",
            "40_STRUCTURED_MEMORY.md",
            "50_AGENT_RESPONSE.md",
            "60_MARKDOWN_PLUS.md",
            "90_DEFAULT_PROFILE.md",
        ],
    },
    Builtin {
        id: "blank",
        name: "空白智能体",
        description: "无预设提示词，完全自定义",
        prompt_files: &[],
    },
    Builtin {
        id: "qa",
        name: "QA 智能体",
        description: "Rua 安装、配置、使用与故障排查助手",
        prompt_files: &[
            "10_AGENT_SAFETY.md",
            "20_AGENT_WORKFLOW.md",
            "30_EXECUTION_REPORTING.md",
            "40_STRUCTURED_MEMORY.md",
            "50_AGENT_RESPONSE.md",
            "60_MARKDOWN_PLUS.md",
            "91_QA_PROFILE.md",
            "92_QA_WORKFLOW.md",
        ],
    },
];

impl AgentTemplateStore {
    pub fn new(rua_root: PathBuf) -> Self {
        Self { rua_root }
    }

    fn templates_path(&self) -> PathBuf {
        self.rua_root.join("agent_templates.json")
    }

    pub fn load(&self) -> std::io::Result<AgentTemplatesFile> {
        let path = self.templates_path();
        match std::fs::read(&path) {
            Ok(bytes) if bytes.is_empty() => Ok(AgentTemplatesFile::default()),
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(std::io::Error::other),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(AgentTemplatesFile::default()),
            Err(e) => Err(e),
        }
    }

    fn save(&self, file: &AgentTemplatesFile) -> std::io::Result<()> {
        let value = serde_json::to_value(file).map_err(std::io::Error::other)?;
        atomic_write_json(&self.templates_path(), &value)
    }

    pub fn save_file(&self, file: &AgentTemplatesFile) -> std::io::Result<()> {
        self.save(file)
    }

    pub fn list(&self) -> std::io::Result<Vec<(String, AgentTemplate)>> {
        let file = self.load()?;
        let mut out: Vec<(String, AgentTemplate)> = file.templates.into_iter().collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    pub fn get(&self, template_id: &str) -> std::io::Result<Option<AgentTemplate>> {
        Ok(self.load()?.templates.remove(template_id))
    }

    pub fn create(&self, id: &str, template: AgentTemplate) -> std::io::Result<bool> {
        let mut file = self.load()?;
        if file.templates.contains_key(id) {
            return Ok(false);
        }
        file.templates.insert(id.to_string(), template);
        self.save(&file)?;
        Ok(true)
    }

    pub fn update(&self, id: &str, template: AgentTemplate) -> std::io::Result<bool> {
        let mut file = self.load()?;
        if !file.templates.contains_key(id) {
            return Ok(false);
        }
        file.templates.insert(id.to_string(), template);
        self.save(&file)?;
        Ok(true)
    }

    pub fn delete(&self, id: &str) -> std::io::Result<bool> {
        let mut file = self.load()?;
        match file.templates.get(id) {
            None => Ok(false),
            Some(entry) if entry.builtin => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Cannot delete builtin template '{id}'"),
            )),
            Some(_) => {
                file.templates.remove(id);
                self.save(&file)?;
                Ok(true)
            }
        }
    }
}

pub fn builtin_templates() -> AgentTemplatesFile {
    let now = chrono::Utc::now().to_rfc3339();
    AgentTemplatesFile {
        templates: BUILTINS
            .iter()
            .map(|builtin| {
                (
                    builtin.id.to_string(),
                    AgentTemplate {
                        name: builtin.name.to_string(),
                        description: builtin.description.to_string(),
                        prompt_files: builtin
                            .prompt_files
                            .iter()
                            .map(|name| (*name).to_string())
                            .collect(),
                        builtin: true,
                        created_at: now.clone(),
                        updated_at: now.clone(),
                        extra: Default::default(),
                    },
                )
            })
            .collect(),
        ..Default::default()
    }
}
