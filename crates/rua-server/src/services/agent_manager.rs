use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use rua_core::{Result, RuaError};
use tracing::warn;

use crate::endpoints::dto;
use crate::services::helpers::read_json_file;

pub struct AgentManager {
    data_dir: PathBuf,
    agent_mcp_cells: Option<Arc<rua_mcp::AgentMcpCells>>,
    agent_cells: Option<Arc<rua_agent::AgentCells>>,
    agent_tools_cells: Option<Arc<rua_tool::AgentToolsCells>>,
}

impl AgentManager {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            agent_mcp_cells: None,
            agent_cells: None,
            agent_tools_cells: None,
        }
    }

    pub fn with_agent_mcp_cells(mut self, cells: Arc<rua_mcp::AgentMcpCells>) -> Self {
        self.agent_mcp_cells = Some(cells);
        self
    }

    pub fn with_agent_cells(mut self, cells: Arc<rua_agent::AgentCells>) -> Self {
        self.agent_cells = Some(cells);
        self
    }

    pub fn with_agent_tools_cells(mut self, cells: Arc<rua_tool::AgentToolsCells>) -> Self {
        self.agent_tools_cells = Some(cells);
        self
    }

    fn workspaces_dir(&self) -> PathBuf {
        self.data_dir.join("workspaces")
    }

    fn config_path(&self) -> PathBuf {
        self.data_dir.join("config.json")
    }

    fn load_config(&self) -> Result<rua_config::RootConfig> {
        rua_config::load_config(&self.config_path(), &self.data_dir).map_err(|error| {
            RuaError::Config {
                message: error.to_string(),
            }
        })
    }

    fn save_config(&self, config: &rua_config::RootConfig) -> Result<()> {
        let path = self.config_path();
        let value = serde_json::to_value(config).map_err(|e| RuaError::Config {
            message: format!("Failed to serialize config: {e}"),
        })?;
        rua_config::atomic_write_json(&path, &value).map_err(|e| RuaError::Io {
            message: format!("Failed to write config.json: {e}"),
        })?;
        Ok(())
    }

    fn pool_dir(&self) -> PathBuf {
        self.data_dir.join("skill_pool")
    }

    fn install_skills_from_pool(
        &self,
        ws_dir: &std::path::Path,
        skill_names: &[String],
    ) -> Result<()> {
        if skill_names.is_empty() {
            return Ok(());
        }

        let pool_manifest_path = self.pool_dir().join("skill.json");
        let pool_manifest: rua_config::SkillManifestFile =
            read_json_file(&pool_manifest_path)?.unwrap_or_default();

        let skills_dir = ws_dir.join("skills");
        std::fs::create_dir_all(&skills_dir).map_err(|e| RuaError::Io {
            message: format!("Failed to create skills dir: {e}"),
        })?;

        let ws_manifest_path = ws_dir.join("skill.json");
        let mut ws_manifest: rua_config::SkillManifestFile =
            read_json_file(&ws_manifest_path)?.unwrap_or_default();
        if ws_manifest.schema_version.is_empty() {
            ws_manifest.schema_version = "workspace-skill-manifest.v1".to_string();
        }

        for skill_name in skill_names {
            let pool_entry = match pool_manifest.skills.get(skill_name) {
                Some(e) => e,
                None => continue,
            };
            let pool_skill_dir = self.pool_dir().join(skill_name);
            if !pool_skill_dir.is_dir() {
                continue;
            }

            let target_dir = skills_dir.join(skill_name);
            copy_dir_filtered(&pool_skill_dir, &target_dir)?;

            let source = if pool_entry.source == "builtin" {
                "builtin".to_string()
            } else {
                "customized".to_string()
            };

            let entry = rua_config::SkillEntry {
                enabled: true,
                channels: vec!["all".to_string()],
                source,
                ..Default::default()
            };
            ws_manifest.skills.insert(skill_name.clone(), entry);
        }

        Self::write_skill_manifest(&ws_manifest_path, &ws_manifest)?;
        Ok(())
    }

    fn sync_workspace_skills(
        &self,
        ws_dir: &std::path::Path,
        desired_names: &[String],
    ) -> Result<()> {
        let pool_manifest_path = self.pool_dir().join("skill.json");
        let pool_manifest: rua_config::SkillManifestFile =
            read_json_file(&pool_manifest_path)?.unwrap_or_default();

        let skills_dir = ws_dir.join("skills");
        std::fs::create_dir_all(&skills_dir).map_err(|e| RuaError::Io {
            message: format!("Failed to create skills dir: {e}"),
        })?;

        let ws_manifest_path = ws_dir.join("skill.json");
        let mut ws_manifest: rua_config::SkillManifestFile =
            read_json_file(&ws_manifest_path)?.unwrap_or_default();
        if ws_manifest.schema_version.is_empty() {
            ws_manifest.schema_version = "workspace-skill-manifest.v1".to_string();
        }

        let desired_set: std::collections::HashSet<&str> =
            desired_names.iter().map(|s| s.as_str()).collect();

        let current_names: Vec<String> = ws_manifest.skills.keys().cloned().collect();
        for name in &current_names {
            if !desired_set.contains(name.as_str()) {
                ws_manifest.skills.remove(name);
                let skill_dir = skills_dir.join(name);
                if skill_dir.exists() {
                    std::fs::remove_dir_all(&skill_dir).map_err(|e| RuaError::Io {
                        message: format!("Failed to remove skill dir {}: {e}", skill_dir.display()),
                    })?;
                }
            }
        }

        for skill_name in desired_names {
            if ws_manifest.skills.contains_key(skill_name) {
                continue;
            }
            let pool_entry = match pool_manifest.skills.get(skill_name) {
                Some(e) => e,
                None => continue,
            };
            let pool_skill_dir = self.pool_dir().join(skill_name);
            if !pool_skill_dir.is_dir() {
                continue;
            }

            let target_dir = skills_dir.join(skill_name);
            copy_dir_filtered(&pool_skill_dir, &target_dir)?;

            let source = if pool_entry.source == "builtin" {
                "builtin".to_string()
            } else {
                "customized".to_string()
            };

            let entry = rua_config::SkillEntry {
                enabled: true,
                channels: vec!["all".to_string()],
                source,
                ..Default::default()
            };
            ws_manifest.skills.insert(skill_name.clone(), entry);
        }

        Self::write_skill_manifest(&ws_manifest_path, &ws_manifest)?;
        Ok(())
    }

    fn write_skill_manifest(
        path: &std::path::Path,
        manifest: &rua_config::SkillManifestFile,
    ) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let mut value = serde_json::to_value(manifest).map_err(|e| RuaError::Config {
            message: format!("Failed to serialize skill manifest: {e}"),
        })?;
        if let Some(obj) = value.as_object_mut() {
            let now_millis = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            obj.insert(
                "version".to_string(),
                serde_json::Value::Number(now_millis.into()),
            );
        }
        rua_config::atomic_write_json(path, &value).map_err(|e| RuaError::Io {
            message: format!("Failed to write skill manifest: {e}"),
        })
    }

    fn normalized_agent_order(config: &rua_config::RootConfig) -> Vec<String> {
        let mut profile_ids: Vec<String> = config.agents.profiles.keys().cloned().collect();
        profile_ids.sort();
        let mut ordered: Vec<String> = Vec::new();

        for id in &config.agents.agent_order {
            if config.agents.profiles.contains_key(id) && !ordered.contains(id) {
                ordered.push(id.clone());
            }
        }

        for id in &profile_ids {
            if !ordered.contains(id) {
                ordered.push(id.clone());
            }
        }
        ordered
    }

    fn load_agents_from_profiles(
        &self,
        config: &rua_config::RootConfig,
    ) -> Result<Vec<rua_config::AgentProfileConfig>> {
        Self::normalized_agent_order(config)
            .into_iter()
            .map(|id| {
                self.load_registered_agent(config, &id)?
                    .ok_or_else(|| RuaError::Config {
                        message: format!("Agent '{id}' registration is missing"),
                    })
            })
            .collect()
    }

    fn load_registered_agent(
        &self,
        config: &rua_config::RootConfig,
        id: &str,
    ) -> Result<Option<rua_config::AgentProfileConfig>> {
        let Some(value) = config.agents.profiles.get(id) else {
            return Ok(None);
        };
        let profile_ref: rua_config::AgentProfileRef = serde_json::from_value(value.clone())
            .map_err(|error| RuaError::Config {
                message: format!("Invalid Agent reference '{id}': {error}"),
            })?;
        let path = PathBuf::from(&profile_ref.workspace_dir).join("agent.json");
        let agent: rua_config::AgentProfileConfig =
            read_json_file(&path)?.ok_or_else(|| RuaError::Config {
                message: format!("Agent '{id}' configuration is missing: {}", path.display()),
            })?;
        if agent.id != id || agent.workspace_dir != profile_ref.workspace_dir {
            return Err(RuaError::Config { message: format!("Agent '{id}' configuration does not match its registered identity or workspace") });
        }
        Ok(Some(agent))
    }

    fn agent_to_summary(agent: &rua_config::AgentProfileConfig) -> dto::AgentSummary {
        dto::AgentSummary {
            id: agent.id.clone(),
            name: agent.name.clone(),
            description: agent.description.clone(),
            enabled: true,
            workspace_dir: agent.workspace_dir.clone(),
            extra: serde_json::Map::new(),
        }
    }

    fn agent_to_detail(
        agent: &rua_config::AgentProfileConfig,
        enabled: bool,
        mcp: Option<serde_json::Value>,
        running: Option<serde_json::Value>,
        default_model: Option<serde_json::Value>,
        tools: Option<serde_json::Value>,
        security: Option<serde_json::Value>,
    ) -> dto::AgentDetail {
        let mut extra = agent.extra.clone();
        let llm_routing = extra
            .remove("llm_routing")
            .or_else(|| Some(serde_json::json!({})));
        let plan = extra.remove("plan").or_else(|| Some(serde_json::json!({})));

        dto::AgentDetail {
            id: agent.id.clone(),
            name: agent.name.clone(),
            description: agent.description.clone(),
            enabled,
            workspace_dir: agent.workspace_dir.clone(),
            template_id: agent.template_id.clone(),
            language: agent.language.clone(),
            system_prompt_files: agent.system_prompt_files.clone(),
            running: running.or_else(|| serde_json::to_value(&agent.running).ok()),
            default_model: default_model.or_else(|| {
                agent
                    .default_model
                    .as_ref()
                    .and_then(|m| serde_json::to_value(m).ok())
            }),
            channels: agent.channels.clone(),
            mcp,
            heartbeat: agent.heartbeat.clone(),
            llm_routing,
            tools,
            plan,
            security,
            extra,
        }
    }

    async fn read_mcp_value(&self, agent_id: &str) -> Result<Option<serde_json::Value>> {
        let Some(cells) = self.agent_cells.as_ref() else {
            return Ok(None);
        };
        cells
            .section::<serde_json::Value>(agent_id, "mcp")
            .read()
            .await
            .map_err(|e| RuaError::Config {
                message: e.to_string(),
            })
    }

    async fn read_running_value(&self, agent_id: &str) -> Result<Option<serde_json::Value>> {
        let Some(cells) = self.agent_cells.as_ref() else {
            return Ok(None);
        };
        let value = cells
            .section::<rua_config::AgentsRunningConfig>(agent_id, "running")
            .read()
            .await
            .map_err(|e| RuaError::Config {
                message: e.to_string(),
            })?;
        value
            .map(serde_json::to_value)
            .transpose()
            .map_err(|e| RuaError::Config {
                message: e.to_string(),
            })
    }

    async fn read_default_model_value(&self, agent_id: &str) -> Result<Option<serde_json::Value>> {
        let Some(cells) = self.agent_cells.as_ref() else {
            return Ok(None);
        };
        cells
            .section::<serde_json::Value>(agent_id, "default_model")
            .read()
            .await
            .map_err(|e| RuaError::Config {
                message: e.to_string(),
            })
    }

    async fn read_tools_value(&self, agent_id: &str) -> Result<Option<serde_json::Value>> {
        let Some(cells) = self.agent_tools_cells.as_ref() else {
            return Ok(None);
        };
        cells
            .cell(agent_id)
            .read_value()
            .await
            .map_err(|e| RuaError::Config {
                message: e.to_string(),
            })
    }

    fn read_security_value(&self, agent_id: &str) -> Result<Option<serde_json::Value>> {
        let store = rua_security::SecurityConfigStore::new(self.data_dir.clone());
        let config = store.load(agent_id).map_err(|error| RuaError::Config {
            message: error.to_string(),
        })?;
        serde_json::to_value(config)
            .map(Some)
            .map_err(|error| RuaError::Config {
                message: error.to_string(),
            })
    }

    fn agent_enabled_from_config(config: &rua_config::RootConfig, id: &str) -> bool {
        config
            .agents
            .profiles
            .get(id)
            .and_then(|v| v.get("enabled"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true)
    }
}

impl AgentManager {
    pub async fn list_agents(&self) -> Result<Vec<dto::AgentSummary>> {
        let config = self.load_config()?;
        let agents = self.load_agents_from_profiles(&config)?;
        Ok(agents
            .iter()
            .map(|agent| {
                let mut summary = Self::agent_to_summary(agent);
                summary.enabled = config
                    .agents
                    .profiles
                    .get(&agent.id)
                    .and_then(|v| v.get("enabled"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                summary
            })
            .collect())
    }

    pub async fn get_agent(&self, id: &str) -> Result<Option<dto::AgentDetail>> {
        let config = self.load_config()?;
        let agent = match self.load_registered_agent(&config, id)? {
            Some(a) => a,
            None => return Ok(None),
        };
        let mcp = self.read_mcp_value(id).await?;
        let running = self.read_running_value(id).await?;
        let default_model = self.read_default_model_value(id).await?;
        let tools = self.read_tools_value(id).await?;
        let security = self.read_security_value(id)?;
        let enabled = Self::agent_enabled_from_config(&config, id);
        Ok(Some(Self::agent_to_detail(
            &agent,
            enabled,
            mcp,
            running,
            default_model,
            tools,
            security,
        )))
    }

    pub async fn get_agent_status(&self, id: &str) -> Result<Option<dto::AgentStatus>> {
        let config = self.load_config()?;
        Ok(self
            .load_registered_agent(&config, id)?
            .map(|a| dto::AgentStatus {
                id: a.id.clone(),
                status: "idle".into(),
                active_sessions: 0,
                extra: serde_json::Map::new(),
            }))
    }

    pub async fn create_agent(&self, spec: dto::AgentCreate) -> Result<dto::AgentDetail> {
        let config = self.load_config()?;
        let existing_ids: HashSet<String> = config.agents.profiles.keys().cloned().collect();
        let id = match spec.id.as_deref().map(rua_config::sanitize_agent_id) {
            Some(id) if !id.is_empty() => {
                rua_config::validate_agent_id(&id, &existing_ids)
                    .map_err(|message| RuaError::Validation { message })?;
                id
            }
            _ => self.generate_unique_agent_id(&existing_ids)?,
        };

        let ws_dir = self.workspaces_dir().join(&id);
        if ws_dir.exists() {
            return Err(RuaError::Conflict {
                resource: "agent".into(),
                detail: format!("Agent '{id}' already exists"),
            });
        }

        let template_id = spec
            .template_id
            .clone()
            .unwrap_or_else(|| "default".to_string());
        let template_store = rua_config::AgentTemplateStore::new(self.data_dir.clone());
        let template = template_store
            .get(&template_id)?
            .ok_or_else(|| RuaError::Validation {
                message: format!("Agent template '{template_id}' does not exist"),
            })?;
        let prompt_files = template.prompt_files;
        let library = rua_prompts::PromptLibrary::new(self.data_dir.clone());
        library.validate_template_files(&prompt_files)?;

        std::fs::create_dir_all(&ws_dir).map_err(|e| RuaError::Io {
            message: format!("Failed to create workspace dir: {e}"),
        })?;

        for subdir in &["sessions", "skills", "prompts"] {
            std::fs::create_dir_all(ws_dir.join(subdir)).map_err(|e| RuaError::Io {
                message: format!("Failed to create {subdir} dir: {e}"),
            })?;
        }

        let workspace_dir_str = spec.workspace_dir.unwrap_or_else(|| {
            let data_str = self.data_dir.to_string_lossy();
            let compact = dirs::home_dir()
                .filter(|h| data_str.starts_with(&*h.to_string_lossy()))
                .map(|h| {
                    format!(
                        "~{}/workspaces/{id}",
                        &data_str[h.to_string_lossy().len()..]
                    )
                })
                .unwrap_or_else(|| format!("{data_str}/workspaces/{id}"));
            compact
        });

        if !prompt_files.is_empty() {
            library.copy_files_to_workspace(&prompt_files, &ws_dir)?;
        }

        let default_mcp = serde_json::json!({"clients": {}});
        let agent = rua_config::AgentProfileConfig {
            id: id.clone(),
            name: spec.name,
            description: spec.description,
            workspace_dir: workspace_dir_str.clone(),
            template_id: Some(template_id),
            language: String::new(),
            system_prompt_files: prompt_files,
            running: rua_config::AgentsRunningConfig::default(),
            default_model: None,
            channels: Some(serde_json::json!({})),
            mcp: None,
            heartbeat: None,
            last_dispatch: None,
            tools: Some(
                serde_json::json!({"builtin_tools": crate::services::collaboration_runtime::server_tool_catalog().map_err(|error| rua_core::RuaError::Config { message: error.to_string() })?}),
            ),
            extra: spec.extra,
        };

        self.write_and_emit_agent(&id, &ws_dir, &agent).await?;

        if let Some(ref cells) = self.agent_mcp_cells {
            if let Err(e) = cells.cell(&id).write_value(default_mcp).await {
                warn!(error = %e, agent_id = %id, "failed to write initial mcp via cells");
            }
        }

        if let Some(ref cells) = self.agent_cells {
            if let Err(e) = cells
                .section::<rua_config::AgentsRunningConfig>(&id, "running")
                .write(rua_config::AgentsRunningConfig::default())
                .await
            {
                warn!(error = %e, agent_id = %id, "failed to write initial running via cells");
            }
        }

        if let Some(ref skill_names) = spec.skill_names {
            self.install_skills_from_pool(&ws_dir, skill_names)?;
        }

        let mut config = config;
        let profile_ref = serde_json::json!({
            "id": id,
            "workspace_dir": workspace_dir_str,
            "enabled": true
        });
        config.agents.profiles.insert(id.clone(), profile_ref);
        config.agents.agent_order = Self::normalized_agent_order(&config);
        self.save_config(&config)?;

        let mcp = self.read_mcp_value(&id).await?;
        let running = self.read_running_value(&id).await?;
        let default_model = self.read_default_model_value(&id).await?;
        let tools = self.read_tools_value(&id).await?;
        let security = self.read_security_value(&id)?;
        Ok(Self::agent_to_detail(
            &agent,
            true,
            mcp,
            running,
            default_model,
            tools,
            security,
        ))
    }

    fn generate_unique_agent_id(&self, existing_ids: &HashSet<String>) -> Result<String> {
        for _ in 0..32 {
            let id = rua_config::generate_short_agent_id();
            if existing_ids.contains(&id) || self.workspaces_dir().join(&id).exists() {
                continue;
            }
            return Ok(id);
        }

        Err(RuaError::Runtime {
            message: "failed to generate unique agent id after 32 attempts".into(),
        })
    }

    pub async fn update_agent(
        &self,
        id: &str,
        mut update: dto::AgentUpdate,
    ) -> Result<dto::AgentDetail> {
        let agent_json_path = self.workspaces_dir().join(id).join("agent.json");
        let mut agent: rua_config::AgentProfileConfig = read_json_file(&agent_json_path)?
            .ok_or_else(|| RuaError::NotFound {
                resource: "agent".into(),
                id: id.into(),
            })?;

        let mcp_update = update.mcp.take();
        let running_update = update.running.take();
        let default_model_update = update.default_model.take();
        let tools_update = update.tools.take();
        let channels_update = update.channels.take();
        let heartbeat_update = update.heartbeat.take();
        let llm_routing_update = update.llm_routing.take();
        let plan_update = update.plan.take();

        fn merge_json(base: &mut serde_json::Value, overlay: &serde_json::Value) {
            match (base, overlay) {
                (serde_json::Value::Object(base_map), serde_json::Value::Object(overlay_map)) => {
                    for (key, value) in overlay_map {
                        merge_json(
                            base_map
                                .entry(key.clone())
                                .or_insert(serde_json::Value::Null),
                            value,
                        );
                    }
                }
                (base_slot, overlay_value) => {
                    *base_slot = overlay_value.clone();
                }
            }
        }
        let running_typed_update = match &running_update {
            Some(running_value) => {
                let mut merged =
                    serde_json::to_value(&agent.running).map_err(|e| RuaError::Validation {
                        message: format!("failed to serialize existing running config: {e}"),
                    })?;
                merge_json(&mut merged, running_value);
                let running_typed = serde_json::from_value::<rua_config::AgentsRunningConfig>(
                    merged,
                )
                .map_err(|e| RuaError::Validation {
                    message: format!("invalid running config: {e}"),
                })?;
                running_typed
                    .validate()
                    .map_err(|message| RuaError::Validation { message })?;
                Some(running_typed)
            }
            None => None,
        };
        let default_model_typed_update = match &default_model_update {
            Some(default_model_value) => Some(
                serde_json::from_value::<Option<rua_config::ModelSlotConfig>>(
                    default_model_value.clone(),
                )
                .map_err(|e| RuaError::Validation {
                    message: format!("invalid default_model config: {e}"),
                })?,
            ),
            None => None,
        };

        if let Some(name) = update.name {
            agent.name = name;
        }
        if let Some(description) = update.description {
            agent.description = description;
        }
        if let Some(language) = update.language {
            agent.language = language;
        }
        if let Some(system_prompt_files) = update.system_prompt_files {
            agent.system_prompt_files = system_prompt_files;
        }
        if let Some(heartbeat) = heartbeat_update {
            agent.heartbeat = Some(heartbeat);
        }
        if let Some(llm_routing) = llm_routing_update {
            agent.extra.insert("llm_routing".into(), llm_routing);
        }
        if let Some(plan) = plan_update {
            agent.extra.insert("plan".into(), plan);
        }
        if let Some(ref running_typed) = running_typed_update {
            agent.running = running_typed.clone();
        }
        if let Some(ref default_model_typed) = default_model_typed_update {
            agent.default_model = default_model_typed.clone();
        }
        if let Some(ref channels_value) = channels_update {
            agent.channels = Some(channels_value.clone());
        }
        if let Some(ref mcp_value) = mcp_update {
            agent.mcp = Some(mcp_value.clone());
        }
        if let Some(ref tools_value) = tools_update {
            agent.tools = Some(tools_value.clone());
        }

        for (k, v) in update.extra {
            agent.extra.insert(k, v);
        }

        if let Some(ref skill_names) = update.skill_names {
            let ws_dir = self.workspaces_dir().join(id);
            self.sync_workspace_skills(&ws_dir, skill_names)?;
        }

        if let Some(ref mcp_value) = mcp_update {
            if let Some(ref cells) = self.agent_mcp_cells {
                cells
                    .cell(id)
                    .write_value(mcp_value.clone())
                    .await
                    .map_err(|e| RuaError::Config {
                        message: format!("mcp cells write: {e}"),
                    })?;
            }
        }

        if let Some(ref running_typed) = running_typed_update {
            if let Some(ref cells) = self.agent_cells {
                cells
                    .section::<rua_config::AgentsRunningConfig>(id, "running")
                    .write(running_typed.clone())
                    .await
                    .map_err(|e| RuaError::Config {
                        message: format!("running cells write: {e}"),
                    })?;
            }
        }

        if let Some(ref default_model_typed) = default_model_typed_update {
            if let Some(ref cells) = self.agent_cells {
                cells
                    .section::<Option<rua_config::ModelSlotConfig>>(id, "default_model")
                    .write(default_model_typed.clone())
                    .await
                    .map_err(|e| RuaError::Config {
                        message: format!("default_model cells write: {e}"),
                    })?;
            }
        }

        if let Some(ref tools_value) = tools_update {
            if let Some(ref cells) = self.agent_tools_cells {
                cells
                    .cell(id)
                    .write_value(tools_value.clone())
                    .await
                    .map_err(|e| RuaError::Config {
                        message: format!("tools cells write: {e}"),
                    })?;
            }
        }

        if let Some(ref channels_value) = channels_update {
            if let Some(ref cells) = self.agent_cells {
                let cell = cells.section::<serde_json::Value>(id, "channels");
                cell.write(channels_value.clone())
                    .await
                    .map_err(|e| RuaError::Config {
                        message: format!("channels cells write: {e}"),
                    })?;
            }
        }

        let ws_dir = self.workspaces_dir().join(id);
        self.write_and_emit_agent(id, &ws_dir, &agent).await?;

        let mcp = self.read_mcp_value(id).await?;
        let running = self.read_running_value(id).await?;
        let default_model = self.read_default_model_value(id).await?;
        let tools = self.read_tools_value(id).await?;
        let security = self.read_security_value(id)?;
        let config = self.load_config()?;
        let enabled = Self::agent_enabled_from_config(&config, id);
        Ok(Self::agent_to_detail(
            &agent,
            enabled,
            mcp,
            running,
            default_model,
            tools,
            security,
        ))
    }

    pub async fn delete_agent(&self, id: &str) -> Result<()> {
        if id == "default" {
            return Err(RuaError::Validation {
                message: "Cannot delete the default agent".into(),
            });
        }

        let ws_dir = self.workspaces_dir().join(id);
        if !ws_dir.exists() {
            return Err(RuaError::NotFound {
                resource: "agent".into(),
                id: id.into(),
            });
        }

        let mut config = self.load_config()?;
        config.agents.profiles.remove(id);
        config.agents.agent_order = Self::normalized_agent_order(&config);

        if config.agents.active_agent == id {
            config.agents.active_agent = "default".into();
        }
        self.save_config(&config)?;

        if ws_dir.is_dir() {
            std::fs::remove_dir_all(&ws_dir).map_err(|e| RuaError::Io {
                message: format!("Failed to delete agent workspace: {e}"),
            })?;
        }
        Ok(())
    }

    async fn write_and_emit_agent(
        &self,
        _agent_id: &str,
        ws_dir: &std::path::Path,
        agent: &rua_config::AgentProfileConfig,
    ) -> Result<()> {
        let agent_json_path = ws_dir.join("agent.json");
        let agent_value = serde_json::to_value(agent).map_err(|e| RuaError::Config {
            message: format!("Failed to serialize agent: {e}"),
        })?;

        rua_config::atomic_write_json(&agent_json_path, &agent_value).map_err(|e| {
            RuaError::Io {
                message: format!("Failed to write agent.json: {e}"),
            }
        })?;

        Ok(())
    }
}

fn copy_dir_filtered(src: &std::path::Path, dst: &std::path::Path) -> Result<()> {
    const IGNORED: &[&str] = &[
        "__pycache__",
        "__MACOSX",
        ".DS_Store",
        "Thumbs.db",
        "desktop.ini",
    ];
    if dst.exists() {
        std::fs::remove_dir_all(dst).map_err(|e| RuaError::Io {
            message: format!("Failed to remove dir {}: {e}", dst.display()),
        })?;
    }
    copy_dir_recursive_filtered(src, dst, IGNORED)
}

fn copy_dir_recursive_filtered(
    src: &std::path::Path,
    dst: &std::path::Path,
    ignored: &[&str],
) -> Result<()> {
    std::fs::create_dir_all(dst).map_err(|e| RuaError::Io {
        message: format!("Failed to create dir {}: {e}", dst.display()),
    })?;
    for entry in std::fs::read_dir(src)
        .map_err(|e| RuaError::Io {
            message: format!("Failed to read dir {}: {e}", src.display()),
        })?
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().to_string();
        if ignored.contains(&name.as_str()) {
            continue;
        }
        let src_path = entry.path();
        let dst_path = dst.join(&name);
        if src_path.is_dir() {
            copy_dir_recursive_filtered(&src_path, &dst_path, ignored)?;
        } else {
            std::fs::copy(&src_path, &dst_path).map_err(|e| RuaError::Io {
                message: format!("Failed to copy file: {e}"),
            })?;
        }
    }
    Ok(())
}
