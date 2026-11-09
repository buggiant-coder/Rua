use async_trait::async_trait;

use rua_core::tool::{ToolContext, ToolResult, ToolSchema};
use rua_core::Result;

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;

    fn schema(&self) -> ToolSchema;

    async fn call(&self, args: serde_json::Value, ctx: ToolContext) -> Result<ToolResult>;
}
