use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tracing::debug;

use mcp_manager::manager::McpManager;
use mcp_manager::types::{McpCallContext, McpServerId, McpToolInfo, ToolFilter};
use rua_core::tool::{ToolContext, ToolResult, ToolSchema};
use rua_core::RuaError;
use rua_security::{SecurityModule, SecurityVerdict, ToolCallContext as SecurityCallContext};

use crate::tool_adapter::McpToolAdapter;

const DEFAULT_CALL_TIMEOUT_SECS: u64 = 60;

pub struct McpBridgedTool {
    canonical_name: String,
    server_id: McpServerId,
    tool_info: McpToolInfo,
    manager: Arc<dyn McpManager>,
    security: Option<Arc<SecurityModule>>,
    call_timeout: Duration,
}

impl McpBridgedTool {
    pub fn server_id(&self) -> &McpServerId {
        &self.server_id
    }

    pub fn original_name(&self) -> &str {
        &self.tool_info.name
    }

    pub fn tool_info(&self) -> &McpToolInfo {
        &self.tool_info
    }
}

#[async_trait]
impl rua_contract::Tool for McpBridgedTool {
    fn name(&self) -> &str {
        &self.canonical_name
    }

    fn schema(&self) -> ToolSchema {
        McpToolAdapter::to_core_schema(&self.tool_info, &self.canonical_name)
    }

    async fn call(
        &self,
        args: serde_json::Value,
        ctx: ToolContext,
    ) -> rua_core::Result<ToolResult> {
        if let Some(ref security) = self.security {
            let mut call_ctx = SecurityCallContext::new(
                &ctx.agent_id,
                &ctx.session_id,
                &self.canonical_name,
                args.clone(),
            );
            call_ctx.working_dir = ctx.working_dir.to_string_lossy().to_string();
            call_ctx.metadata = ctx.metadata.clone();
            if let Some(value) = ctx.metadata.get("security_chat_address") {
                call_ctx.approval_owner = rua_security::ApprovalOwner::chat(
                    serde_json::from_value(value.clone()).map_err(|error| {
                        RuaError::Validation {
                            message: format!("invalid security Chat address: {error}"),
                        }
                    })?,
                );
            }
            if let Some(value) = ctx
                .metadata
                .get(rua_security::models::EFFECTIVE_SECURITY_CONFIG_METADATA_KEY)
            {
                call_ctx.security_snapshot =
                    Some(Arc::new(serde_json::from_value(value.clone()).map_err(
                        |error| RuaError::Validation {
                            message: format!("invalid effective security snapshot: {error}"),
                        },
                    )?));
            }
            let verdict = if self.tool_info.raw["ruaBuiltinComputerInteraction"] == true {
                let operations = crate::computer_control::action_calls(&args)
                    .map_err(|error| RuaError::Validation {
                        message: error.to_string(),
                    })?
                    .into_iter()
                    .map(|(name, input)| {
                        let mut operation = call_ctx.clone();
                        operation.tool_name =
                            McpToolAdapter::canonical_name(&self.server_id, &name);
                        operation.tool_input = input;
                        operation
                    })
                    .collect::<Vec<_>>();
                security.enforce_composite(&call_ctx, &operations).await
            } else {
                security.enforce(&call_ctx, &[]).await
            };
            match verdict {
                SecurityVerdict::Allow | SecurityVerdict::AuditOnly { .. } => {}
                SecurityVerdict::Deny { reason } => {
                    return Err(RuaError::Security {
                        message: format!("MCP tool '{}' denied: {reason}", self.canonical_name),
                    });
                }
                SecurityVerdict::RequiresApproval { prompt, .. } => {
                    return Err(RuaError::Security {
                        message: format!(
                            "MCP tool '{}' requires approval: {prompt}",
                            self.canonical_name
                        ),
                    });
                }
            }
        }

        if let Err(msg) = McpToolAdapter::validate_args(&self.tool_info.input_schema, &args) {
            return Err(RuaError::Validation { message: msg });
        }

        let mcp_ctx = McpCallContext {
            event_tx: ctx.event_tx.clone(),
            metadata: ctx.metadata.clone().into_iter().collect(),
            agent_id: ctx.agent_id.clone(),
            session_id: ctx.session_id.clone(),
            cancel: ctx.cancel.clone(),
            timeout: Some(self.call_timeout),
        };

        let call_fut = self
            .manager
            .call_tool(&self.server_id, &self.tool_info.name, args, mcp_ctx);

        let mcp_result = tokio::time::timeout(self.call_timeout, call_fut)
            .await
            .map_err(|_| RuaError::Timeout {
                operation: format!("MCP tool call '{}'", self.canonical_name),
            })?
            .map_err(|e| RuaError::Mcp {
                server: self.server_id.to_string(),
                message: e.to_string(),
            })?;

        Ok(McpToolAdapter::map_call_result(&mcp_result))
    }
}

pub struct McpToolBridge {
    manager: Arc<dyn McpManager>,
    security: Option<Arc<SecurityModule>>,
    default_timeout: Duration,
}

impl McpToolBridge {
    pub fn new(manager: Arc<dyn McpManager>) -> Self {
        Self {
            manager,
            security: None,
            default_timeout: Duration::from_secs(DEFAULT_CALL_TIMEOUT_SECS),
        }
    }

    pub fn with_security(mut self, security: Arc<SecurityModule>) -> Self {
        self.security = Some(security);
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    pub async fn discover_tools(&self) -> rua_core::Result<Vec<McpBridgedTool>> {
        let tools = self
            .manager
            .list_tools(ToolFilter::default())
            .await
            .map_err(|e| RuaError::Mcp {
                server: "all".to_string(),
                message: e.to_string(),
            })?;

        let bridged = self.wrap_tools(tools);
        debug!(count = bridged.len(), "discovered MCP tools for bridge");
        Ok(bridged)
    }

    pub async fn discover_tools_from(
        &self,
        server_id: &McpServerId,
    ) -> rua_core::Result<Vec<McpBridgedTool>> {
        let filter = ToolFilter {
            server_id: Some(server_id.clone()),
            name_prefix: None,
            include_disabled: false,
        };

        let tools = self
            .manager
            .list_tools(filter)
            .await
            .map_err(|e| RuaError::Mcp {
                server: server_id.to_string(),
                message: e.to_string(),
            })?;

        Ok(self.wrap_tools(tools))
    }

    fn wrap_tools(&self, tools: Vec<McpToolInfo>) -> Vec<McpBridgedTool> {
        tools
            .into_iter()
            .map(|info| {
                let canonical_name = McpToolAdapter::canonical_name(&info.server_id, &info.name);
                McpBridgedTool {
                    canonical_name,
                    server_id: info.server_id.clone(),
                    tool_info: info,
                    manager: self.manager.clone(),
                    security: self.security.clone(),
                    call_timeout: self.default_timeout,
                }
            })
            .collect()
    }
}
