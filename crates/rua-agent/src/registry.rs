use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use rua_config::types::agents::AgentsRunningConfig;
use tokio::sync::RwLock;

use crate::cells::AgentCells;
use crate::error::AgentError;
use crate::profile::AgentProfile;
use crate::profile_store::AgentProfileStore;

#[derive(Debug, Clone)]
pub struct ResolvedAgent {
    pub profile: AgentProfile,
    pub running: Arc<AgentsRunningConfig>,
}

impl ResolvedAgent {
    pub fn from_config(config: &rua_config::AgentProfileConfig) -> Self {
        Self {
            profile: AgentProfile::from_config(config),
            running: Arc::new(config.running.clone()),
        }
    }

    pub fn from_profile(profile: AgentProfile) -> Self {
        Self {
            running: Arc::new(AgentsRunningConfig::default()),
            profile,
        }
    }
}

#[async_trait]
pub trait AgentRegistry: Send + Sync {
    async fn list_agents(&self) -> Result<Vec<ResolvedAgent>, AgentError>;

    async fn get_agent(&self, agent_id: &str) -> Result<ResolvedAgent, AgentError>;

    async fn active_agent(&self) -> Result<ResolvedAgent, AgentError>;

    async fn reload(&self) -> Result<(), AgentError>;
}

pub struct FileAgentRegistry {
    agents: RwLock<HashMap<String, ResolvedAgent>>,

    active_id: RwLock<String>,

    persistence: Option<CellsPersistence>,
}

struct CellsPersistence {
    cells: Arc<AgentCells>,
    profile_store: Arc<AgentProfileStore>,
}

impl FileAgentRegistry {
    pub fn new(profiles: Vec<AgentProfile>, active_id: impl Into<String>) -> Self {
        let active_id_str = active_id.into();
        let mut map = HashMap::new();
        for p in profiles {
            map.insert(p.id.clone(), ResolvedAgent::from_profile(p));
        }
        Self {
            agents: RwLock::new(map),
            active_id: RwLock::new(active_id_str),
            persistence: None,
        }
    }

    pub fn empty() -> Self {
        Self {
            agents: RwLock::new(HashMap::new()),
            active_id: RwLock::new(String::new()),
            persistence: None,
        }
    }

    pub fn with_cells(
        cells: Arc<AgentCells>,
        profile_store: Arc<AgentProfileStore>,
        active_id: impl Into<String>,
    ) -> Self {
        Self {
            agents: RwLock::new(HashMap::new()),
            active_id: RwLock::new(active_id.into()),
            persistence: Some(CellsPersistence {
                cells,
                profile_store,
            }),
        }
    }

    pub async fn set_active(&self, agent_id: &str) -> Result<(), AgentError> {
        let in_cache = self.agents.read().await.contains_key(agent_id);
        if !in_cache {
            if let Some(pers) = self.persistence.as_ref() {
                let exists = pers.cells.full(agent_id).read().await?.is_some();
                if !exists {
                    return Err(AgentError::not_found(agent_id));
                }
            } else {
                return Err(AgentError::not_found(agent_id));
            }
        }
        *self.active_id.write().await = agent_id.to_string();
        Ok(())
    }

    pub async fn upsert(&self, profile: AgentProfile) -> Result<(), AgentError> {
        if let Some(pers) = self.persistence.as_ref() {
            let config = profile_to_config(&profile);
            pers.cells.full(&profile.id).write(config).await?;
        }
        let agent_id = profile.id.clone();
        self.agents
            .write()
            .await
            .insert(agent_id, ResolvedAgent::from_profile(profile));
        Ok(())
    }

    pub async fn remove(&self, agent_id: &str) -> Result<Option<AgentProfile>, AgentError> {
        if let Some(pers) = self.persistence.as_ref() {
            pers.cells.full(agent_id).delete().await?;
        }
        Ok(self
            .agents
            .write()
            .await
            .remove(agent_id)
            .map(|resolved| resolved.profile))
    }

    pub async fn len(&self) -> usize {
        self.agents.read().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.agents.read().await.is_empty()
    }

    async fn resolve_from_disk(&self, agent_id: &str) -> Result<Option<ResolvedAgent>, AgentError> {
        let Some(pers) = self.persistence.as_ref() else {
            return Ok(None);
        };
        let config = pers
            .profile_store
            .load_agent_config_via_cells(&pers.cells, Some(agent_id))
            .await?;

        let direct = pers.cells.full(agent_id).read().await?;
        if agent_id != "default" && direct.is_none() {
            return Ok(None);
        }
        let resolved = ResolvedAgent::from_config(&config);
        self.agents
            .write()
            .await
            .insert(resolved.profile.id.clone(), resolved.clone());
        Ok(Some(resolved))
    }
}

#[async_trait]
impl AgentRegistry for FileAgentRegistry {
    async fn list_agents(&self) -> Result<Vec<ResolvedAgent>, AgentError> {
        if self.persistence.is_some() {
            self.reload().await?;
        }
        let agents = self.agents.read().await;
        let mut list: Vec<ResolvedAgent> = agents
            .values()
            .filter(|a| a.profile.enabled)
            .cloned()
            .collect();
        list.sort_by(|a, b| a.profile.id.cmp(&b.profile.id));
        Ok(list)
    }

    async fn get_agent(&self, agent_id: &str) -> Result<ResolvedAgent, AgentError> {
        if self.persistence.is_some() {
            if let Some(profile) = self.resolve_from_disk(agent_id).await? {
                return Ok(profile);
            }
            return Err(AgentError::not_found(agent_id));
        }
        let agents = self.agents.read().await;
        agents
            .get(agent_id)
            .cloned()
            .ok_or_else(|| AgentError::not_found(agent_id))
    }

    async fn active_agent(&self) -> Result<ResolvedAgent, AgentError> {
        let active_id = self.active_id.read().await.clone();
        if active_id.is_empty() {
            if self.persistence.is_some() {
                if let Some(profile) = self.resolve_from_disk("default").await? {
                    return Ok(profile);
                }
            }

            let agents = self.agents.read().await;
            if let Some(agent) = agents.get("default") {
                return Ok(agent.clone());
            }
            let mut list: Vec<&ResolvedAgent> =
                agents.values().filter(|a| a.profile.enabled).collect();
            list.sort_by(|a, b| a.profile.id.cmp(&b.profile.id));
            return list
                .first()
                .cloned()
                .cloned()
                .ok_or_else(|| AgentError::config("No agents configured"));
        }
        self.get_agent(&active_id).await
    }

    async fn reload(&self) -> Result<(), AgentError> {
        let Some(pers) = self.persistence.as_ref() else {
            tracing::debug!("Agent registry reload requested on legacy in-memory registry; no-op");
            return Ok(());
        };

        let ids = pers.cells.list().await?;

        let mut refreshed: HashMap<String, ResolvedAgent> = HashMap::new();
        for id in ids {
            let config = pers
                .profile_store
                .load_agent_config_via_cells(&pers.cells, Some(&id))
                .await?;
            refreshed.insert(id.clone(), ResolvedAgent::from_config(&config));
        }

        if !refreshed.contains_key("default") {
            let default_config = pers
                .profile_store
                .load_agent_config_via_cells(&pers.cells, Some("default"))
                .await?;
            refreshed.insert(
                "default".to_string(),
                ResolvedAgent::from_config(&default_config),
            );
        }
        let mut cache = self.agents.write().await;
        *cache = refreshed;
        tracing::debug!(
            agent_count = cache.len(),
            "Agent registry reloaded from cells"
        );
        Ok(())
    }
}

fn profile_to_config(profile: &AgentProfile) -> rua_config::AgentProfileConfig {
    use rua_config::types::agents::ModelSlotConfig;

    let config = rua_config::AgentProfileConfig {
        id: profile.id.clone(),
        name: profile.name.clone(),
        description: profile.description.clone().unwrap_or_default(),
        workspace_dir: profile.workspace_dir.clone(),
        template_id: profile.template_id.clone(),
        language: profile.language.clone(),
        system_prompt_files: profile.system_prompt_files.clone(),
        running: AgentsRunningConfig::default(),
        default_model: profile.model_slot.as_ref().map(|m| ModelSlotConfig {
            provider_id: m.provider_id.clone(),
            model: m.model.clone(),
            extra: m.extra.clone(),
        }),
        channels: None,
        mcp: None,
        heartbeat: None,
        last_dispatch: None,
        tools: None,
        extra: profile.extra.clone(),
    };
    config
}

pub struct MockAgentRegistry {
    inner: FileAgentRegistry,
}

impl MockAgentRegistry {
    pub fn new(profiles: Vec<AgentProfile>, active_id: impl Into<String>) -> Self {
        Self {
            inner: FileAgentRegistry::new(profiles, active_id),
        }
    }

    pub fn default_agent() -> Self {
        let profile = AgentProfile::new("default", "Default Agent");
        Self::new(vec![profile], "default")
    }
}

#[async_trait]
impl AgentRegistry for MockAgentRegistry {
    async fn list_agents(&self) -> Result<Vec<ResolvedAgent>, AgentError> {
        self.inner.list_agents().await
    }

    async fn get_agent(&self, agent_id: &str) -> Result<ResolvedAgent, AgentError> {
        self.inner.get_agent(agent_id).await
    }

    async fn active_agent(&self) -> Result<ResolvedAgent, AgentError> {
        self.inner.active_agent().await
    }

    async fn reload(&self) -> Result<(), AgentError> {
        Ok(())
    }
}
