use std::sync::Arc;

use async_trait::async_trait;
use rua_contract::Tool;
use rua_core::tool::{ToolContext, ToolResult, ToolSchema};
use serde::Deserialize;

use crate::annotations::{ApprovalPolicy, ExtendedToolAnnotations};
use crate::schema::ToolSpec;

#[derive(Debug, Clone)]
pub enum SyncTransferRequest {
    Send {
        arguments: serde_json::Value,
        source_device_id: String,
        source_paths: Vec<String>,
        target_device_id: String,
        target_dir: String,
        conflict: Option<String>,
    },
}

#[async_trait]
pub trait SyncTransferRuntime: Send + Sync {
    async fn execute(
        &self,
        request: SyncTransferRequest,
        context: &ToolContext,
    ) -> Result<serde_json::Value, String>;
}

pub struct SyncTransferRuntimeHandle {
    pub runtime: Arc<dyn SyncTransferRuntime>,
}

impl SyncTransferRuntimeHandle {
    pub fn new(runtime: Arc<dyn SyncTransferRuntime>) -> Self {
        Self { runtime }
    }
}

pub struct SyncTransferTool;

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
    fn spec() -> ToolSpec {
        ToolSpec {
            name: "sync_send".into(),
            description: "Explicitly copy selected files or directories between devices. Both endpoints use their owning device's absolute filesystem paths. Waits for completion evidence and returns actual destination paths; never deletes the source.".into(),
            parameters: serde_json::json!({
                    "type":"object",
                    "properties":{
                        "source_device_id":{"type":"string","description":"Persistent ID of the device owning the source paths."},
                        "source_paths":{"type":"array","minItems":1,"items":{"type":"string"},"description":"Absolute real filesystem paths on source_device_id; only these selected files or directories are copied."},
                        "target_device_id":{"type":"string","description":"Persistent ID of the device receiving the files."},
                        "target_dir":{"type":"string","description":"Absolute real path of the destination directory on target_device_id."},
                        "conflict":{"type":"string","enum":["rename","overwrite","fail"],"default":"rename","description":"How to handle existing destinations. Defaults to rename; overwrite must be explicit."}
                    },
                    "required":["source_device_id","source_paths","target_device_id","target_dir"]
            }),
            annotations: ExtendedToolAnnotations {
                read_only: false,
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
        "sync_send"
    }

    fn schema(&self) -> ToolSchema {
        let spec = Self::spec();
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
        if context
            .cancel
            .as_ref()
            .is_some_and(|cancel| *cancel.borrow())
        {
            return Err(rua_core::RuaError::Cancelled);
        }
        let tool_name = self.name().to_string();
        let error = |message: String| rua_core::RuaError::Tool {
            tool: tool_name.clone(),
            message,
        };
        let arguments = args;
        let args: SendArgs =
            serde_json::from_value(arguments.clone()).map_err(|e| error(e.to_string()))?;
        for device in [&args.source_device_id, &args.target_device_id] {
            if device.trim().is_empty() || device.trim() != device {
                return Err(error(
                    "source_device_id and target_device_id must be nonempty Device IDs".into(),
                ));
            }
        }
        if args.source_paths.is_empty() {
            return Err(error("source_paths must not be empty".into()));
        }
        if args
            .source_paths
            .iter()
            .chain(std::iter::once(&args.target_dir))
            .any(|path| !rua_core::execution_directory::is_device_absolute_path(path))
        {
            return Err(error("source_paths and target_dir must be absolute filesystem paths on their owning devices".into()));
        }
        if args
            .conflict
            .as_deref()
            .is_some_and(|policy| !matches!(policy, "rename" | "overwrite" | "fail"))
        {
            return Err(error("conflict must be rename, overwrite, or fail".into()));
        }
        let request = SyncTransferRequest::Send {
            arguments,
            source_device_id: args.source_device_id,
            source_paths: args.source_paths,
            target_device_id: args.target_device_id,
            target_dir: args.target_dir,
            conflict: args.conflict,
        };
        let value = Self::runtime(&context)
            .map_err(error)?
            .execute(request, &context)
            .await
            .map_err(error)?;
        let mut result = ToolResult::text(&tool_name, value.to_string());
        if value["status"] == "not_executed" && value["agents_instructions"] == "reconsider" {
            result
                .metadata
                .insert("execution_status".into(), "not_executed".into());
            result
                .metadata
                .insert("agents_instructions".into(), "reconsider".into());
        }
        Ok(result)
    }
}

pub fn register_sync_transfer_tools(
    registry: &mut crate::ToolRegistry,
) -> Result<(), crate::ToolError> {
    registry.register(
        Arc::new(SyncTransferTool),
        SyncTransferTool::spec(),
        crate::ToolNamespace::Builtin,
    )
}
