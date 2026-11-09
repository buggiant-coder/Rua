use async_trait::async_trait;

use crate::error::McpResult;
use crate::status::McpServerStatus;
use crate::types::{
    McpCallContext, McpCallResult, McpConfig, McpReloadReport, McpServerConfig, McpServerId,
    McpShutdownReport, McpToolInfo, ToolFilter,
};

#[async_trait]
pub trait McpManager: Send + Sync {
    async fn load_config(&self, config: McpConfig) -> McpResult<McpReloadReport>;

    /// Read the accepted configuration without connecting, checking health or
    /// discovering tools. Transport credentials are internal and must not be
    /// copied into a portable Chat capability selection.
    async fn configuration(&self) -> McpResult<McpConfig>;

    async fn list_servers(&self) -> McpResult<Vec<McpServerStatus>>;

    async fn list_tools(&self, filter: ToolFilter) -> McpResult<Vec<McpToolInfo>>;

    async fn call_tool(
        &self,
        server: &McpServerId,
        tool: &str,
        args: serde_json::Value,
        ctx: McpCallContext,
    ) -> McpResult<McpCallResult>;

    async fn reload(&self, new_config: McpConfig) -> McpResult<McpReloadReport>;

    async fn suspend_new_calls(&self) -> McpResult<()>;

    async fn invalidate_tool_cache(&self, server: &McpServerId) -> McpResult<()>;

    async fn connect_server(&self, server: &McpServerId) -> McpResult<()>;

    async fn disconnect_server(&self, server: &McpServerId) -> McpResult<()>;

    async fn replace_client(
        &self,
        server_id: &McpServerId,
        config: McpServerConfig,
    ) -> McpResult<()>;

    async fn shutdown_all(&self) -> McpResult<McpShutdownReport>;
}
