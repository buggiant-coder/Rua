use async_trait::async_trait;

use crate::error::McpResult;
use crate::status::McpServerStatus;
use crate::types::{McpCallContext, McpCallResult, McpInitializeResult, McpToolInfo};

#[async_trait]
pub trait McpClient: Send + Sync {
    async fn connect(&self) -> McpResult<()>;

    async fn initialize_result(&self) -> McpResult<McpInitializeResult>;

    async fn list_tools(&self) -> McpResult<Vec<McpToolInfo>>;

    async fn call_tool(
        &self,
        name: &str,
        args: serde_json::Value,
        ctx: McpCallContext,
    ) -> McpResult<McpCallResult>;

    async fn invalidate_tool_cache(&self);

    async fn ping(&self) -> McpResult<()> {
        Ok(())
    }

    async fn shutdown(&self) -> McpResult<()>;

    fn status(&self) -> McpServerStatus;
}
