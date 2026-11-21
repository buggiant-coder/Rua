use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

pub const AGENTS_INSTRUCTIONS_MAX_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentsInstructionTargetKind {
    #[default]
    Auto,
    File,
    Directory,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentsInstructionDiscoveryRequest {
    pub device_id: String,
    pub target_path: String,
    #[serde(default)]
    pub target_kind: AgentsInstructionTargetKind,
    #[serde(default)]
    pub checked_directories: Vec<String>,
    pub max_bytes: usize,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentsInstructionSource {
    pub device_id: String,
    pub path: String,
    pub scope_directory: String,
    pub content: String,
    pub source_bytes: u64,
    pub truncated: bool,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentsInstructionDiscovery {
    pub target_directory: Option<String>,
    pub checked_directories: Vec<String>,
    pub sources: Vec<AgentsInstructionSource>,
    pub diagnostics: Vec<String>,
    pub truncated: bool,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[async_trait::async_trait]
pub trait AgentsInstructionLoader: Send + Sync {
    async fn discover(
        &self,
        request: &AgentsInstructionDiscoveryRequest,
        ctx: &crate::tool::ToolContext,
        cancellation: CancellationToken,
    ) -> crate::Result<AgentsInstructionDiscovery>;
}
