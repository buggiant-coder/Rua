use agent_types::ToolCall;
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct ToolBeforeInvocation {
    pub call: ToolCall,
    pub schema: serde_json::Value,
    pub agent_id: String,
    pub session_id: String,
    pub working_directory: PathBuf,
    pub metadata: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ToolBeforeOutcome {
    Continue(Box<ToolCall>),
    Reject { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolBeforeFailure {
    pub diagnostic: String,
}

#[async_trait]
pub trait ToolBeforeBoundary: Send + Sync {
    async fn apply(
        &self,
        invocation: ToolBeforeInvocation,
        cancellation: CancellationToken,
    ) -> Result<ToolBeforeOutcome, ToolBeforeFailure>;
}
