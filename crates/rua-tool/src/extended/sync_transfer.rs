use std::sync::Arc;

use async_trait::async_trait;
use rua_contract::Tool;
use rua_core::tool::{ToolContext, ToolResult, ToolSchema};
use serde::Deserialize;

use crate::annotations::{ApprovalPolicy, ExtendedToolAnnotations};
use crate::schema::ToolSpec;

#[derive(Debug, Clone)]
pub enum SyncTransferRequest {
    List {
        target_device_id: String,
        path: String,
    },
    Send {
        source_device_id: String,
        source_paths: Vec<String>,
        target_device_id: String,
        target_dir: String,
        conflict: Option<String>,
    },
}

#[async_trait]
pub trait SyncTransferRuntime: Send + Sync {
    async fn execute(&self, request: SyncTransferRequest) -> Result<serde_json::Value, String>;
}

pub struct SyncTransferRuntimeHandle {
    pub runtime: Arc<dyn SyncTransferRuntime>,
}

impl SyncTransferRuntimeHandle {
    pub fn new(runtime: Arc<dyn SyncTransferRuntime>) -> Self {
        Self { runtime }
    }
}

#[derive(Clone, Copy)]
enum Operation {
    List,
    Send,
}

pub struct SyncTransferTool {
    operation: Operation,
}

#[derive(Deserialize)]
struct ListArgs {
    target_device_id: String,
    path: String,
}

#[derive(Deserialize)]
struct SendArgs {
    source_device_id: String,
    source_paths: Vec<String>,
    target_device_id: String,
    target_dir: String,
    #[serde(default)]
    conflict: Option<String>,
}

impl SyncTransferTool {
    fn spec(operation: Operation) -> ToolSpec {
        let (name, description, parameters, read_only) = match operation {
            Operation::List => (
                "sync_ls",
                "List files and directories on a paired device.",
                serde_json::json!({
                    "type":"object",
                    "properties":{
                        "target_device_id":{"type":"string"},
                        "path":{"type":"string"}
                    },
                    "required":["target_device_id","path"]
                }),
                true,
            ),
            Operation::Send => (
                "sync_send",
                "Transfer files or directories between paired devices.",
                serde_json::json!({
                    "type":"object",
                    "properties":{
                        "source_device_id":{"type":"string"},
                        "source_paths":{"type":"array","items":{"type":"string"}},
                        "target_device_id":{"type":"string"},
                        "target_dir":{"type":"string"},
                        "conflict":{"type":"string","enum":["rename","overwrite","fail"]}
                    },
                    "required":["source_device_id","source_paths","target_device_id","target_dir"]
                }),
                false,
            ),
        };
        ToolSpec {
            name: name.into(),
            description: description.into(),
            parameters,
            annotations: ExtendedToolAnnotations {
                read_only,
                requires_approval: ApprovalPolicy::ToolGuardDecides,
                ..Default::default()
            },
            namespace: "builtin".into(),
        }
    }

    fn runtime(context: &ToolContext) -> Result<Arc<dyn SyncTransferRuntime>, String> {
        context
            .sync_transfer_runtime
            .as_ref()
            .and_then(|handle| handle.downcast_ref::<SyncTransferRuntimeHandle>())
            .map(|handle| Arc::clone(&handle.runtime))
            .ok_or_else(|| "Sync Transfer Runtime is unavailable".to_string())
    }
}

#[async_trait]
impl Tool for SyncTransferTool {
    fn name(&self) -> &str {
        match self.operation {
            Operation::List => "sync_ls",
            Operation::Send => "sync_send",
        }
    }

    fn schema(&self) -> ToolSchema {
        let spec = Self::spec(self.operation);
        ToolSchema {
            name: spec.name,
            description: spec.description,
            parameters: spec.parameters,
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        context: ToolContext,
    ) -> rua_core::Result<ToolResult> {
        let tool_name = self.name().to_string();
        let error = |message: String| rua_core::RuaError::Tool {
            tool: tool_name.clone(),
            message,
        };
        let request = match self.operation {
            Operation::List => {
                let args: ListArgs =
                    serde_json::from_value(args).map_err(|e| error(e.to_string()))?;
                SyncTransferRequest::List {
                    target_device_id: args.target_device_id,
                    path: args.path,
                }
            }
            Operation::Send => {
                let args: SendArgs =
                    serde_json::from_value(args).map_err(|e| error(e.to_string()))?;
                SyncTransferRequest::Send {
                    source_device_id: args.source_device_id,
                    source_paths: args.source_paths,
                    target_device_id: args.target_device_id,
                    target_dir: args.target_dir,
                    conflict: args.conflict,
                }
            }
        };
        let value = Self::runtime(&context)
            .map_err(error)?
            .execute(request)
            .await
            .map_err(error)?;
        Ok(ToolResult::text(&tool_name, value.to_string()))
    }
}

pub fn register_sync_transfer_tools(
    registry: &mut crate::ToolRegistry,
) -> Result<(), crate::ToolError> {
    for operation in [Operation::List, Operation::Send] {
        registry.register(
            Arc::new(SyncTransferTool { operation }),
            SyncTransferTool::spec(operation),
            crate::ToolNamespace::Builtin,
        )?;
    }
    Ok(())
}
