use std::path::PathBuf;

use rua_contract::agent::AgentRequest;
use rua_core::{RuaError, RuntimeEvent};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpRuntimeTurnRequest {
    pub request: AgentRequest,
    pub workspace_dir: PathBuf,
    #[serde(default)]
    pub mcp_config: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AcpRuntimeStreamItem {
    Event { event: Box<RuntimeEvent> },
    Error { error: RuaError },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpRuntimeCancelResponse {
    pub cancelled: bool,
}
