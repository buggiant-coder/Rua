use std::sync::Arc;

use serde::{Deserialize, Serialize};

pub use agent_types::{ToolAttachment, ToolCall, ToolResult};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,

    pub parameters: serde_json::Value,
}

/// Trusted model capability projected into tool calls by the executing Agent context.
pub const MODEL_SUPPORTS_IMAGE_METADATA_KEY: &str = "model_supports_image";

/// Canonical tool disablements captured by the executing Chat/Task configuration.
pub const EFFECTIVE_DISABLED_TOOLS_METADATA_KEY: &str = "effective_disabled_tools";

pub const DEFAULT_TOOL_EXECUTION_OUTPUT_MAX_TOKENS: usize = 10_000;

pub const DEFAULT_TOOL_RAW_CAPTURE_MAX_BYTES: usize = 1024 * 1024;

pub const DEFAULT_TOOL_RECALL_SLICE_MAX_TOKENS: usize = 10_000;

pub const DEFAULT_TOOL_OLD_MAX_BYTES: usize = 3_000;

pub const DEFAULT_TOKEN_ESTIMATE_DIVISOR: f64 = 4.0;

#[derive(Clone)]
pub struct ToolContext {
    pub agent_id: String,
    pub session_id: String,
    pub working_dir: std::path::PathBuf,

    pub conversation_store_dir: Option<std::path::PathBuf>,

    pub tool_result_store_dir: Option<std::path::PathBuf>,
    pub metadata: std::collections::HashMap<String, serde_json::Value>,

    pub cancel: Option<tokio::sync::watch::Receiver<bool>>,

    pub task_runtime: Option<Arc<dyn std::any::Any + Send + Sync>>,

    pub task_session: Option<Arc<dyn std::any::Any + Send + Sync>>,

    pub collaboration_runtime: Option<Arc<dyn std::any::Any + Send + Sync>>,

    pub sync_transfer_runtime: Option<Arc<dyn std::any::Any + Send + Sync>>,

    pub event_tx: Option<tokio::sync::mpsc::Sender<agent_types::AgentEvent>>,

    pub execution_output_max_tokens: usize,

    pub raw_capture_max_bytes: usize,

    pub recall_slice_max_tokens: usize,

    pub old_max_bytes: usize,

    pub token_estimate_divisor: f64,
}

impl Default for ToolContext {
    fn default() -> Self {
        Self {
            agent_id: String::new(),
            session_id: String::new(),
            working_dir: std::path::PathBuf::new(),
            conversation_store_dir: None,
            tool_result_store_dir: None,
            metadata: std::collections::HashMap::new(),
            cancel: None,
            task_runtime: None,
            task_session: None,
            collaboration_runtime: None,
            sync_transfer_runtime: None,
            event_tx: None,
            execution_output_max_tokens: DEFAULT_TOOL_EXECUTION_OUTPUT_MAX_TOKENS,
            raw_capture_max_bytes: DEFAULT_TOOL_RAW_CAPTURE_MAX_BYTES,
            recall_slice_max_tokens: DEFAULT_TOOL_RECALL_SLICE_MAX_TOKENS,
            old_max_bytes: DEFAULT_TOOL_OLD_MAX_BYTES,
            token_estimate_divisor: DEFAULT_TOKEN_ESTIMATE_DIVISOR,
        }
    }
}
