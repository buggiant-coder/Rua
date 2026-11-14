use std::path::{Path, PathBuf};

use rua_core::{Result, RuaError};

use super::{SeedCoordinator, SeedProducer, SeedRunReport};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentSeedToolPreset {
    Default,
    Named(&'static str),
    ReadOnlyResearch,
    Implementation,
}

const DEFAULT_PROMPT_FILES: &[&str] = &[
    "10_AGENT_SAFETY.md",
    "20_AGENT_WORKFLOW.md",
    "30_EXECUTION_REPORTING.md",
    "40_STRUCTURED_MEMORY.md",
    "50_AGENT_RESPONSE.md",
    "60_MARKDOWN_PLUS.md",
    "90_DEFAULT_PROFILE.md",
];
const QA_PROMPT_FILES: &[&str] = &[
    "10_AGENT_SAFETY.md",
    "20_AGENT_WORKFLOW.md",
    "30_EXECUTION_REPORTING.md",
    "40_STRUCTURED_MEMORY.md",
    "50_AGENT_RESPONSE.md",
    "60_MARKDOWN_PLUS.md",
    "91_QA_PROFILE.md",
    "92_QA_WORKFLOW.md",
];
const RESEARCH_PROMPT_FILES: &[&str] = &[
    "10_AGENT_SAFETY.md",
    "20_AGENT_WORKFLOW.md",
    "30_EXECUTION_REPORTING.md",
    "50_AGENT_RESPONSE.md",
    "60_MARKDOWN_PLUS.md",
    "93_RESEARCH_PROFILE.md",
];

const IMPLEMENTATION_PROMPT_FILES: &[&str] = &[
    "10_AGENT_SAFETY.md",
    "20_AGENT_WORKFLOW.md",
    "30_EXECUTION_REPORTING.md",
    "50_AGENT_RESPONSE.md",
    "60_MARKDOWN_PLUS.md",
];
#[derive(Debug, Clone)]
pub struct BuiltinAgentSeedSpec {
    pub entry_id: &'static str,
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub prompt_files: &'static [&'static str],
    pub tool_preset: AgentSeedToolPreset,
    pub initial_workspace_skills: &'static [&'static str],
    pub order_hint: i32,
    pub enabled: bool,
}

pub fn builtin_agent_seed_catalog() -> Vec<BuiltinAgentSeedSpec> {
    vec![
        BuiltinAgentSeedSpec {
            entry_id: "agent:default",
            id: "default",
            name: "小Rua",
            description: "",
            prompt_files: DEFAULT_PROMPT_FILES,
            tool_preset: AgentSeedToolPreset::Default,
            initial_workspace_skills: &[],
            order_hint: 0,
            enabled: true,
        },
        BuiltinAgentSeedSpec {
            entry_id: "agent:rua_qa",
            id: "rua_qa",
            name: "QA Agent",
            description:
                "Builtin Q&A helper for Rua setup, local configuration, and documentation.",
            prompt_files: QA_PROMPT_FILES,
            tool_preset: AgentSeedToolPreset::Named("qa_support"),
            initial_workspace_skills: &["guidance", "rua_source_index"],
            order_hint: 100,
            enabled: true,
        },
        BuiltinAgentSeedSpec {
            entry_id: "agent:research",
            id: "research",
            name: "Research Agent",
            description: "Read-only research and review Agent for independent collaboration Chats.",
            prompt_files: RESEARCH_PROMPT_FILES,
            tool_preset: AgentSeedToolPreset::ReadOnlyResearch,
            initial_workspace_skills: &[],
            order_hint: 50,
            enabled: true,
        },
        BuiltinAgentSeedSpec {
            entry_id: "agent:implementation",
            id: "implementation",
            name: "Implementation Agent",
            description: "Scoped implementation and verification Agent; writing remains subject to source Chat authorization and approvals.",
            prompt_files: IMPLEMENTATION_PROMPT_FILES,
            tool_preset: AgentSeedToolPreset::Implementation,
            initial_workspace_skills: &[],
            order_hint: 60,
            enabled: true,
        },
    ]
}

pub fn ensure_builtin_agents_seeded(data_dir: &Path) -> SeedRunReport {
    super::prompts::ensure_builtin_prompt_library_seeded(data_dir);
    super::skills::ensure_builtin_skill_pool_seeded(data_dir);

    let mut catalog = builtin_agent_seed_catalog();
    catalog.sort_by_key(|spec| spec.order_hint);
    let mut coordinator = SeedCoordinator::new(data_dir);
    for spec in catalog {
        coordinator = coordinator.with_producer(Box::new(BuiltinAgentSeedProducer::new(spec)));
    }
    coordinator.run()
}

struct BuiltinAgentSeedProducer {
    spec: BuiltinAgentSeedSpec,
}

impl BuiltinAgentSeedProducer {
    fn new(spec: BuiltinAgentSeedSpec) -> Self {
        Self { spec }
    }

    fn workspace_dir(&self, data_dir: &Path) -> PathBuf {
        data_dir.join("workspaces").join(self.spec.id)
    }

    fn agent_json_rel_path(&self) -> String {
        format!("workspaces/{}/agent.json", self.spec.id)
    }

    fn workspace_prompt_rel_path(&self, filename: &str) -> String {
        format!("workspaces/{}/prompts/{filename}", self.spec.id)
    }

    fn workspace_skill_rel_dir(&self, skill_name: &str) -> String {
        format!("workspaces/{}/skills/{skill_name}", self.spec.id)
    }
}

impl SeedProducer for BuiltinAgentSeedProducer {
    fn entry_id(&self) -> String {
        self.spec.entry_id.to_string()
    }

    fn producer_name(&self) -> &str {
        "agent_seed_catalog"
    }

    fn target_paths(&self) -> Vec<String> {
        let mut paths = vec![self.agent_json_rel_path()];
        paths.extend(
            self.spec
                .prompt_files
                .iter()
                .map(|filename| self.workspace_prompt_rel_path(filename)),
        );
        paths.extend(
            self.spec
                .initial_workspace_skills
                .iter()
                .map(|skill| self.workspace_skill_rel_dir(skill)),
        );
        paths
    }

    fn materialize(&self, data_dir: &Path) -> Result<Vec<String>> {
        let workspace_dir = self.workspace_dir(data_dir);
        create_workspace_dirs(&workspace_dir)?;
        let workspace_dir_str = workspace_dir.to_string_lossy().into_owned();

        let mut agent = rua_config::AgentProfileConfig {
            id: self.spec.id.to_string(),
            name: self.spec.name.to_string(),
            description: self.spec.description.to_string(),
            workspace_dir: workspace_dir_str.clone(),
            template_id: None,
            language: String::new(),
            system_prompt_files: self
                .spec
                .prompt_files
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            running: rua_config::AgentsRunningConfig::default(),
            default_model: None,
            channels: Some(serde_json::json!({})),
            mcp: Some(serde_json::json!({"clients": {}})),
            heartbeat: None,
            last_dispatch: None,
            tools: Some(tools_for_preset(self.spec.tool_preset)),
            extra: serde_json::Map::new(),
        };
        if self.spec.tool_preset == AgentSeedToolPreset::ReadOnlyResearch {
            agent.tools = Some(tools_for_preset(AgentSeedToolPreset::ReadOnlyResearch));
        }
        if self.spec.tool_preset == AgentSeedToolPreset::Implementation {
            agent.extra.insert("systemPrompt".into(), serde_json::json!(
                "You are Rua's implementation and verification Agent. Execute only the delegated, user-authorized scope. Inspect the current code and contracts before editing. Respect the specified working directory, file ownership, forbidden changes, and other contributors' work. Clarify missing requirements or conflicting ownership instead of guessing. Run focused verification and report actual changes, test evidence, limitations, and blockers. Writing capability is not authorization to deploy, publish, delete data, bypass approval, or broaden the task. Do not delegate to another Agent or ACP runner. Use background Tool Tasks only for permitted tool work; read and verify their real results before claiming completion."
            ));
        }

        let agent_value = serde_json::to_value(&agent).map_err(|e| RuaError::Config {
            message: format!("Failed to serialize builtin agent seed: {e}"),
        })?;
        rua_config::atomic_write_json(&workspace_dir.join("agent.json"), &agent_value).map_err(
            |e| RuaError::Io {
                message: format!("Failed to write builtin agent.json: {e}"),
            },
        )?;
        upsert_root_agent_profile(data_dir, &self.spec, &workspace_dir_str)?;

        let mut materialized_paths = vec![self.agent_json_rel_path()];
        materialized_paths.extend(copy_workspace_prompts(
            data_dir,
            &workspace_dir,
            self.spec.id,
            self.spec.prompt_files,
        )?);
        materialized_paths.extend(install_initial_workspace_skills(
            data_dir,
            &workspace_dir,
            self.spec.id,
            self.spec.initial_workspace_skills,
        )?);

        Ok(materialized_paths)
    }
}

fn create_workspace_dirs(workspace_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(workspace_dir).map_err(|e| RuaError::Io {
        message: format!("Failed to create agent workspace dir: {e}"),
    })?;
    for subdir in ["sessions", "skills", "prompts"] {
        std::fs::create_dir_all(workspace_dir.join(subdir)).map_err(|e| RuaError::Io {
            message: format!("Failed to create {subdir} dir: {e}"),
        })?;
    }
    Ok(())
}

fn upsert_root_agent_profile(
    data_dir: &Path,
    spec: &BuiltinAgentSeedSpec,
    workspace_dir: &str,
) -> Result<()> {
    let config_path = data_dir.join("config.json");
    let mut config: rua_config::RootConfig =
        read_json_file(&config_path)?.unwrap_or_else(rua_config::RootConfig::default);

    config.agents.profiles.insert(
        spec.id.to_string(),
        serde_json::json!({
            "id": spec.id,
            "name": spec.name,
            "workspace_dir": workspace_dir,
            "enabled": spec.enabled,
        }),
    );
    let id_value = spec.id.to_string();
    if !config.agents.agent_order.iter().any(|id| id == &id_value) {
        config.agents.agent_order.push(id_value);
    }
    if config.agents.active_agent.is_empty() {
        config.agents.active_agent = "default".to_string();
    }

    let value = serde_json::to_value(&config).map_err(|e| RuaError::Config {
        message: format!("Failed to serialize root config after agent seed: {e}"),
    })?;
    rua_config::atomic_write_json(&config_path, &value).map_err(|e| RuaError::Io {
        message: format!("Failed to write config.json after agent seed: {e}"),
    })
}

fn tools_for_preset(preset: AgentSeedToolPreset) -> serde_json::Value {
    match preset {
        AgentSeedToolPreset::Default | AgentSeedToolPreset::Named(_) => {
            serde_json::json!({
                "selection_policy": rua_config::types::tools::ToolSelectionPolicy::default_enabled(),
                "builtin_tools": {}
            })
        }
        AgentSeedToolPreset::ReadOnlyResearch | AgentSeedToolPreset::Implementation => {
            let mut allowed = vec![
                "read",
                "grep",
                "glob",
                "web_search",
                "web_fetch",
                "conversation_search",
                "conversation_read_span",
            ];
            if preset == AgentSeedToolPreset::Implementation {
                allowed.extend([
                    "edit",
                    "bash",
                    "task_create",
                    "task_read",
                    "task_list",
                    "task_cancel",
                    "task_retry",
                    "task_send_input",
                    "update_plan",
                ]);
            }
            let policy = rua_config::types::tools::ToolSelectionPolicy::restricted(
                allowed
                    .into_iter()
                    .map(rua_config::types::tools::ToolIdentity::builtin)
                    .collect(),
            );
            serde_json::json!({ "selection_policy": policy, "builtin_tools": {} })
        }
    }
}

fn copy_workspace_prompts(
    data_dir: &Path,
    workspace_dir: &Path,
    agent_id: &str,
    prompt_files: &[&str],
) -> Result<Vec<String>> {
    if prompt_files.is_empty() {
        return Ok(Vec::new());
    }
    let lib = rua_prompts::PromptLibrary::new(data_dir.to_path_buf());
    let files: Vec<String> = prompt_files
        .iter()
        .map(|file| (*file).to_string())
        .collect();
    let copied = lib
        .copy_files_to_workspace(&files, workspace_dir)
        .map_err(|e| RuaError::Io {
            message: format!("Failed to copy workspace prompt files for {agent_id}: {e}"),
        })?;
    Ok(copied
        .into_iter()
        .map(|filename| format!("workspaces/{agent_id}/prompts/{filename}"))
        .collect())
}

fn install_initial_workspace_skills(
    data_dir: &Path,
    workspace_dir: &Path,
    agent_id: &str,
    skill_names: &[&str],
) -> Result<Vec<String>> {
    if skill_names.is_empty() {
        return Ok(Vec::new());
    }
    let pool = rua_skills::pool_service::SkillPoolService::new_in(data_dir.join("skill_pool"))
        .map_err(|e| RuaError::Config {
            message: format!("Failed to open skill pool for Agent seed: {e}"),
        })?;

    let mut materialized_paths = Vec::new();
    for skill_name in skill_names {
        pool.download_to_workspace(workspace_dir, skill_name, None, false)
            .map_err(|e| RuaError::Config {
                message: format!(
                    "Failed to install workspace skill {skill_name} for {agent_id}: {e}"
                ),
            })?;
        let skill_rel_dir = format!("workspaces/{agent_id}/skills/{skill_name}");
        let files = collect_relative_files(data_dir, &data_dir.join(&skill_rel_dir))?;
        let borrowed: Vec<&str> = files.iter().map(String::as_str).collect();
        rua_core::seed_manifest::record_seed_entry(
            data_dir,
            &format!("workspace_skill:{agent_id}:{skill_name}"),
            "agent_seed_catalog",
            &borrowed,
        )?;
        materialized_paths.extend(files);
    }
    Ok(materialized_paths)
}

fn collect_relative_files(data_dir: &Path, root: &Path) -> Result<Vec<String>> {
    let mut paths = Vec::new();
    collect_files(root, &mut paths).map_err(|e| RuaError::Io {
        message: format!(
            "Failed to collect Agent seed files under {}: {e}",
            root.display()
        ),
    })?;
    paths.sort();
    let mut rels = Vec::with_capacity(paths.len());
    for path in paths {
        let rel = path
            .strip_prefix(data_dir)
            .map_err(|e| RuaError::Validation {
                message: format!("Failed to relativize Agent seed file: {e}"),
            })?;
        rels.push(rel.to_string_lossy().replace('\\', "/"));
    }
    Ok(rels)
}

fn collect_files(root: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if !root.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, out)?;
        } else if path.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

fn read_json_file<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let parsed = serde_json::from_str(&content).map_err(|e| RuaError::Config {
                message: format!("Failed to parse {}: {e}", path.display()),
            })?;
            Ok(Some(parsed))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(RuaError::Io {
            message: format!("Failed to read {}: {e}", path.display()),
        }),
    }
}
