use std::collections::HashMap;
use std::path::PathBuf;

use agent_types::{ToolCall, ToolResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolObservationEvent {
    After,
    Failed,
}

#[derive(Debug, Clone)]
pub struct ToolObservationInvocation {
    pub event: ToolObservationEvent,
    pub call: ToolCall,
    pub result: ToolResult,
    pub agent_id: String,
    pub session_id: String,
    pub working_directory: PathBuf,
    pub metadata: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolObserverContribution {
    pub definition_revision: String,
    pub source: String,
    pub resolved_order: usize,
    pub kind: String,
    pub content: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolObserverFailure {
    pub diagnostic: String,
}

#[async_trait]
pub trait ToolObserverBoundary: Send + Sync {
    async fn observe(
        &self,
        invocation: ToolObservationInvocation,
        cancellation: CancellationToken,
    ) -> Result<Vec<ToolObserverContribution>, ToolObserverFailure>;
}
