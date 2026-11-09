mod compressor;
mod output_processor;
mod tool_call_logger;
mod truncation;

pub use compressor::{
    compress_output, compress_output_tokens, estimate_text_tokens, format_byte_size,
    format_token_count,
};
pub use output_processor::ToolResultOutputProcessor;
pub use tool_call_logger::{recall_key, ToolCallArchive};
pub use truncation::{
    continuation_meta, format_truncation_warning, hard_cap_meta, recall_meta, truncation_kind,
    TruncationKind,
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub fn archive_dir_for_context(
    ctx: &rua_core::tool::ToolContext,
) -> rua_core::Result<std::path::PathBuf> {
    ctx.tool_result_store_dir
        .clone()
        .ok_or_else(|| rua_core::RuaError::Validation {
            message: "tool result storage requires conversation ownership".to_string(),
        })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultBlob {
    pub id: String,

    pub tool_name: String,

    pub content: String,

    pub mime_type: String,

    pub created_at: chrono::DateTime<chrono::Utc>,

    #[serde(default)]
    pub input: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct CleanupPolicy {
    pub max_age: std::time::Duration,

    pub max_total_bytes: u64,
}

impl Default for CleanupPolicy {
    fn default() -> Self {
        Self {
            max_age: std::time::Duration::from_secs(24 * 3600),
            max_total_bytes: 100 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CleanupReport {
    pub files_removed: usize,
    pub bytes_freed: u64,
}

#[async_trait]
pub trait ToolResultStore: Send + Sync {
    async fn save(&self, blob: ToolResultBlob) -> Result<(), crate::error::ToolError>;

    async fn read(&self, id: &str) -> Result<ToolResultBlob, crate::error::ToolError>;

    async fn cleanup(
        &self,
        policy: CleanupPolicy,
    ) -> Result<CleanupReport, crate::error::ToolError>;
}

pub struct NoOpResultStore {
    store: tokio::sync::Mutex<std::collections::HashMap<String, ToolResultBlob>>,
}

impl NoOpResultStore {
    pub fn new() -> Self {
        Self {
            store: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }
}

impl Default for NoOpResultStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolResultStore for NoOpResultStore {
    async fn save(&self, blob: ToolResultBlob) -> Result<(), crate::error::ToolError> {
        let id = blob.id.clone();
        self.store.lock().await.insert(id, blob);
        Ok(())
    }

    async fn read(&self, id: &str) -> Result<ToolResultBlob, crate::error::ToolError> {
        self.store.lock().await.get(id).cloned().ok_or_else(|| {
            crate::error::ToolError::ResultStore {
                message: format!("result not found: {id}"),
            }
        })
    }

    async fn cleanup(
        &self,
        _policy: CleanupPolicy,
    ) -> Result<CleanupReport, crate::error::ToolError> {
        Ok(CleanupReport {
            files_removed: 0,
            bytes_freed: 0,
        })
    }
}
