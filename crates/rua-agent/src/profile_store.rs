use std::path::PathBuf;

use rua_config::AgentProfileConfig;

use crate::cells::AgentCells;
use crate::error::AgentError;

#[derive(Debug, Clone)]
pub struct AgentProfileStore {
    data_dir: PathBuf,
}

impl AgentProfileStore {
    pub fn new(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    pub fn resolve_agent_id(agent_id: Option<&str>) -> &str {
        agent_id.unwrap_or("default")
    }

    pub fn agent_json_path(&self, agent_id: Option<&str>) -> PathBuf {
        let id = Self::resolve_agent_id(agent_id);
        self.data_dir.join("workspaces").join(id).join("agent.json")
    }

    pub async fn load_agent_config_via_cells(
        &self,
        cells: &AgentCells,
        agent_id: Option<&str>,
    ) -> Result<AgentProfileConfig, AgentError> {
        let id = Self::resolve_agent_id(agent_id);

        let value =
            cells.full(id).read_value().await?.ok_or_else(|| {
                AgentError::config(format!("Agent '{id}' configuration is missing"))
            })?;
        let config: AgentProfileConfig = serde_json::from_value(value).map_err(|error| {
            AgentError::config(format!("Invalid Agent '{id}' configuration: {error}"))
        })?;
        if config.id != id {
            return Err(AgentError::config(format!(
                "Agent '{id}' configuration has a different id"
            )));
        }
        Ok(config)
    }
}
