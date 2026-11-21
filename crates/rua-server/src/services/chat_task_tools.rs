use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use rua_core::tool::{ToolContext, ToolResult};
use rua_tool::extended::task_tools::TaskToolRouter;
use serde_json::Value;

use super::chat_task::ChatTaskService;

/// Binds the existing Task tools to the Chat's shared local/remote router.
pub struct RuntimeTaskToolRouter {
    service: Arc<OnceLock<Arc<ChatTaskService>>>,
}

impl RuntimeTaskToolRouter {
    pub fn new(service: Arc<OnceLock<Arc<ChatTaskService>>>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl TaskToolRouter for RuntimeTaskToolRouter {
    async fn execute(
        &self,
        tool_name: &str,
        arguments: Value,
        ctx: &ToolContext,
    ) -> Result<Option<ToolResult>, String> {
        self.service
            .get()
            .ok_or("Chat Task router unavailable")?
            .execute_tool(tool_name, arguments, ctx)
            .await
    }
}
