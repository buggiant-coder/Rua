use std::path::PathBuf;
use std::sync::Arc;

use cell_store::{CellStore, CellStoreError, FileSystemCellStore};

use crate::config_store::ToolConfigError;

const AGENT_TOOLS_SECTION: &str = "$.tools";

pub struct AgentToolsCells {
    store: Arc<FileSystemCellStore>,
    workspaces_dir: PathBuf,
}

impl AgentToolsCells {
    pub fn new(store: Arc<FileSystemCellStore>, workspaces_dir: PathBuf) -> Self {
        Self {
            store,
            workspaces_dir,
        }
    }

    pub fn cell<'a>(&'a self, agent_id: &str) -> AgentToolsCell<'a> {
        AgentToolsCell {
            cells: self,
            agent_id: agent_id.to_string(),
        }
    }

    fn agent_json_path(&self, agent_id: &str) -> PathBuf {
        self.workspaces_dir.join(agent_id).join("agent.json")
    }
}

pub struct AgentToolsCell<'a> {
    cells: &'a AgentToolsCells,
    agent_id: String,
}

impl<'a> AgentToolsCell<'a> {
    pub async fn read_document(&self) -> Result<Option<serde_json::Value>, ToolConfigError> {
        self.cells
            .store
            .read::<serde_json::Value>(&self.cells.agent_json_path(&self.agent_id), "$")
            .await
            .map_err(cell_err_to_tool)
    }

    pub async fn compare_exchange_document(
        &self,
        expected: Option<serde_json::Value>,
        replacement: serde_json::Value,
    ) -> Result<bool, ToolConfigError> {
        self.cells
            .store
            .compare_exchange(
                &self.cells.agent_json_path(&self.agent_id),
                "$",
                expected,
                replacement,
            )
            .await
            .map_err(cell_err_to_tool)
    }

    pub async fn read_value(&self) -> Result<Option<serde_json::Value>, ToolConfigError> {
        self.cells
            .store
            .read::<serde_json::Value>(
                &self.cells.agent_json_path(&self.agent_id),
                AGENT_TOOLS_SECTION,
            )
            .await
            .map_err(cell_err_to_tool)
    }

    pub async fn write_value(&self, value: serde_json::Value) -> Result<(), ToolConfigError> {
        self.cells
            .store
            .write(
                &self.cells.agent_json_path(&self.agent_id),
                AGENT_TOOLS_SECTION,
                value,
            )
            .await
            .map_err(cell_err_to_tool)
    }

    pub async fn compare_exchange(
        &self,
        expected: Option<serde_json::Value>,
        replacement: serde_json::Value,
    ) -> Result<bool, ToolConfigError> {
        self.cells
            .store
            .compare_exchange(
                &self.cells.agent_json_path(&self.agent_id),
                AGENT_TOOLS_SECTION,
                expected,
                replacement,
            )
            .await
            .map_err(cell_err_to_tool)
    }

    pub async fn update<F>(&self, mutator: F) -> Result<serde_json::Value, ToolConfigError>
    where
        F: FnOnce(Option<serde_json::Value>) -> serde_json::Value + Send,
    {
        self.cells
            .store
            .update::<serde_json::Value, _>(
                &self.cells.agent_json_path(&self.agent_id),
                AGENT_TOOLS_SECTION,
                mutator,
            )
            .await
            .map_err(cell_err_to_tool)
    }
}

fn cell_err_to_tool(err: CellStoreError) -> ToolConfigError {
    ToolConfigError::Config(format!("tools cell store error: {err}"))
}
