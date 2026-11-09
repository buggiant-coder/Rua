use std::collections::HashMap;
use std::path::PathBuf;

use agent_types::ToolCall;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct PermissionRequestInvocation {
    pub request_id: String,
    pub prompt: String,
    pub call: ToolCall,
    pub argument_digest: String,
    pub agent_id: String,
    pub session_id: String,
    pub working_directory: PathBuf,
    pub metadata: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum PermissionHookDecision {
    Allow { reason: Option<String> },
    Deny { reason: String },
    NoDecision,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionHookFailure {
    pub diagnostic: String,
}

#[async_trait]
pub trait PermissionRequestBoundary: Send + Sync {
    async fn decide(
        &self,
        invocation: PermissionRequestInvocation,
        cancellation: CancellationToken,
    ) -> Result<PermissionHookDecision, PermissionHookFailure>;
}

pub fn argument_digest(call: &ToolCall) -> String {
    let payload = serde_json::to_vec(&serde_json::json!({
        "tool": call.name,
        "arguments": call.arguments,
    }))
    .expect("Tool Call digest payload is serializable");
    blake3::hash(&payload).to_hex().to_string()
}
