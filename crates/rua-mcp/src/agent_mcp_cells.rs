use std::path::PathBuf;
use std::sync::Arc;

use cell_store::{CellStore, CellStoreError, FileSystemCellStore};
use mcp_manager::error::{McpError, McpResult};
use mcp_manager::types::McpConfig;

const AGENT_MCP_SECTION: &str = "$.mcp";

pub struct AgentMcpCells {
    store: Arc<FileSystemCellStore>,
    workspaces_dir: PathBuf,
}

impl AgentMcpCells {
    pub fn new(store: Arc<FileSystemCellStore>, workspaces_dir: PathBuf) -> Self {
        Self {
            store,
            workspaces_dir,
        }
    }

    pub fn cell<'a>(&'a self, agent_id: &str) -> AgentMcpCell<'a> {
        AgentMcpCell {
            cells: self,
            agent_id: agent_id.to_string(),
        }
    }

    fn agent_json_path(&self, agent_id: &str) -> PathBuf {
        self.workspaces_dir.join(agent_id).join("agent.json")
    }
}

pub struct AgentMcpCell<'a> {
    cells: &'a AgentMcpCells,
    agent_id: String,
}

impl<'a> AgentMcpCell<'a> {
    pub async fn read(&self) -> McpResult<Option<McpConfig>> {
        self.cells
            .store
            .read::<McpConfig>(
                &self.cells.agent_json_path(&self.agent_id),
                AGENT_MCP_SECTION,
            )
            .await
            .map_err(cell_err_to_mcp)
    }

    pub async fn write(&self, config: McpConfig) -> McpResult<()> {
        self.cells
            .store
            .write(
                &self.cells.agent_json_path(&self.agent_id),
                AGENT_MCP_SECTION,
                config,
            )
            .await
            .map_err(cell_err_to_mcp)
    }

    pub async fn update<F>(&self, mutator: F) -> McpResult<McpConfig>
    where
        F: FnOnce(Option<McpConfig>) -> McpConfig + Send,
    {
        self.cells
            .store
            .update::<McpConfig, _>(
                &self.cells.agent_json_path(&self.agent_id),
                AGENT_MCP_SECTION,
                mutator,
            )
            .await
            .map_err(cell_err_to_mcp)
    }

    pub async fn read_value(&self) -> McpResult<Option<serde_json::Value>> {
        self.cells
            .store
            .read::<serde_json::Value>(
                &self.cells.agent_json_path(&self.agent_id),
                AGENT_MCP_SECTION,
            )
            .await
            .map_err(cell_err_to_mcp)
    }

    pub async fn write_value(&self, value: serde_json::Value) -> McpResult<()> {
        self.cells
            .store
            .write(
                &self.cells.agent_json_path(&self.agent_id),
                AGENT_MCP_SECTION,
                value,
            )
            .await
            .map_err(cell_err_to_mcp)
    }

    pub async fn delete(&self) -> McpResult<()> {
        self.cells
            .store
            .delete(
                &self.cells.agent_json_path(&self.agent_id),
                AGENT_MCP_SECTION,
            )
            .await
            .map_err(cell_err_to_mcp)
    }
}

fn cell_err_to_mcp(err: CellStoreError) -> McpError {
    McpError::Internal {
        message: format!("agent MCP cell store error: {err}"),
    }
}
