use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use react_agent::{ToolBeforeBoundary, ToolBeforeFailure, ToolBeforeInvocation, ToolBeforeOutcome};
use rua_core::{ChatAddress, ChatScope};
use tokio_util::sync::CancellationToken;

use crate::services::product_hook_execution::{
    CommandExecutionResource, CommandHandler, HandlerAdapterError, HandlerAdapterInvocation,
    HandlerAdapterResult, HandlerExecutionResult, HandlerExecutor, HandlerExposure,
    HandlerInvocation, HandlerKind, NativeHandler, PluginHandler, StrictCommandHandler,
};
use crate::services::product_hooks::{HookExecutionRequirement, ProductHookService};

#[async_trait]
pub trait ProductToolBeforeInvoker: Send + Sync {
    async fn invoke(
        &self,
        requirement: &HookExecutionRequirement,
        invocation: &ToolBeforeInvocation,
        cancellation: CancellationToken,
    ) -> Result<HandlerExecutionResult, String>;
}

pub struct ProductToolBeforeBoundary {
    hooks: Arc<ProductHookService>,
    invoker: Arc<dyn ProductToolBeforeInvoker>,
}

pub struct HandlerExecutorProductInvoker {
    active_plugins: Arc<rua_plugin_runtime::ActivePluginRegistry>,
    local_device_id: String,
}

impl HandlerExecutorProductInvoker {
    pub fn new(
        active_plugins: Arc<rua_plugin_runtime::ActivePluginRegistry>,
        local_device_id: impl Into<String>,
    ) -> Self {
        Self {
            active_plugins,
            local_device_id: local_device_id.into(),
        }
    }
}

struct ActivePluginHandler {
    registry: Arc<rua_plugin_runtime::ActivePluginRegistry>,
}

#[async_trait]
impl PluginHandler for ActivePluginHandler {
    async fn execute(
        &self,
        plugin_id: &str,
        invocation: HandlerAdapterInvocation,
    ) -> Result<HandlerAdapterResult, HandlerAdapterError> {
        let callback = invocation.required_callback("plugin")?.to_string();
        let payload = serde_json::to_string(&invocation.exposure.data)
            .map_err(|error| HandlerAdapterError::MalformedOutcome(error.to_string()))?;
        let result = self
            .registry
            .invoke_hook(
                plugin_id,
                rua_plugin_runtime::protocol::InvokeHookParams {
                    hook_point: callback.clone(),
                    payload,
                },
            )
            .await
            .map_err(|error| HandlerAdapterError::Launch(error.to_string()))?;
        let entry = result
            .results
            .into_iter()
            .find(|entry| entry.hook_name == callback)
            .ok_or_else(|| {
                HandlerAdapterError::UnavailableRequirements(vec![format!(
                    "plugin callback '{}'",
                    callback
                )])
            })?;
        let outcome = serde_json::from_str(&entry.output)
            .map_err(|error| HandlerAdapterError::MalformedOutcome(error.to_string()))?;
        Ok(HandlerAdapterResult {
            outcome,
            diagnostics: Vec::new(),
        })
    }
}

struct UnavailableNativeHandler;

#[async_trait]
impl NativeHandler for UnavailableNativeHandler {
    async fn execute(
        &self,
        invocation: HandlerAdapterInvocation,
    ) -> Result<HandlerAdapterResult, HandlerAdapterError> {
        Err(HandlerAdapterError::UnavailableRequirements(vec![format!(
            "native Handler '{}'",
            invocation.executable.as_deref().unwrap_or("unknown")
        )]))
    }
}

#[async_trait]
impl ProductToolBeforeInvoker for HandlerExecutorProductInvoker {
    async fn invoke(
        &self,
        requirement: &HookExecutionRequirement,
        invocation: &ToolBeforeInvocation,
        cancellation: CancellationToken,
    ) -> Result<HandlerExecutionResult, String> {
        if requirement.authority_device_id != self.local_device_id {
            return Err(format!(
                "Authority Device '{}' is unavailable on this Device",
                requirement.authority_device_id
            ));
        }
        let (definition_id, _) = requirement
            .definition_revision
            .rsplit_once('@')
            .ok_or_else(|| "invalid Definition revision identity".to_string())?;
        let (owner_namespace, _) = definition_id
            .rsplit_once('/')
            .ok_or_else(|| "invalid Definition identity".to_string())?;
        let scope = scope_for_owner(owner_namespace, invocation)?;
        let resource_id = format!("{}:{}", scope.kind(), scope.owner_id());
        let working_directory = requirement
            .working_directory
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| invocation.working_directory.clone());
        let command: Arc<dyn CommandHandler> = Arc::new(StrictCommandHandler::new(
            &self.local_device_id,
            vec![CommandExecutionResource {
                scope: scope.clone(),
                resource_id: resource_id.clone(),
                root: working_directory.clone(),
            }],
        ));
        let executor = HandlerExecutor::new(
            command,
            Arc::new(UnavailableNativeHandler),
            Arc::new(ActivePluginHandler {
                registry: Arc::clone(&self.active_plugins),
            }),
        );
        let kind = match requirement.handler_kind.as_str() {
            "command" => HandlerKind::Command,
            "plugin" => HandlerKind::Plugin {
                plugin_id: owner_namespace
                    .strip_prefix("plugin:")
                    .ok_or_else(|| "Plugin Handler has a non-Plugin owner".to_string())?
                    .to_string(),
            },
            "native" => HandlerKind::Native,
            other => return Err(format!("unsupported Handler kind: {other}")),
        };
        executor
            .execute(HandlerInvocation {
                invocation_id: format!("tool-before-{}", uuid::Uuid::new_v4()),
                kind,
                authority_device_id: requirement.authority_device_id.clone(),
                chat_address: chat_address(invocation)?,
                scope,
                resource_id,
                executable: requirement.executable.clone(),
                program: requirement.program.clone(),
                args: requirement.args.clone(),
                working_directory,
                timeout: Duration::from_secs(requirement.timeout_seconds.unwrap_or(30)),
                exposure: HandlerExposure {
                    data: serde_json::json!({
                        "tool_call": invocation.call,
                        "agent_id": invocation.agent_id,
                        "session_id": invocation.session_id,
                    }),
                    capabilities: requirement.capabilities.clone(),
                    sensitive_values: Vec::new(),
                },
                cancellation,
                remaining_budget: Duration::from_secs(30),
                available_resources: if requirement.available {
                    requirement.required_resources.clone()
                } else {
                    Vec::new()
                },
                required_resources: requirement.required_resources.clone(),
            })
            .await
            .map_err(|failure| failure.diagnostic)
    }
}

impl ProductToolBeforeBoundary {
    pub fn new(hooks: Arc<ProductHookService>, invoker: Arc<dyn ProductToolBeforeInvoker>) -> Self {
        Self { hooks, invoker }
    }
}

#[async_trait]
impl ToolBeforeBoundary for ProductToolBeforeBoundary {
    async fn apply(
        &self,
        mut invocation: ToolBeforeInvocation,
        cancellation: CancellationToken,
    ) -> Result<ToolBeforeOutcome, ToolBeforeFailure> {
        let Some(address) = invocation
            .metadata
            .get(rua_agent::hooks::SECURITY_CHAT_ADDRESS_METADATA_KEY)
            .cloned()
            .map(serde_json::from_value::<ChatAddress>)
            .transpose()
            .map_err(|error| failure(format!("invalid Chat Address: {error}")))?
        else {
            return Ok(ToolBeforeOutcome::Continue(Box::new(invocation.call)));
        };
        let Some(turn_id) = invocation
            .metadata
            .get("turn_id")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
        else {
            return Ok(ToolBeforeOutcome::Continue(Box::new(invocation.call)));
        };
        let Some(frozen) = self
            .hooks
            .frozen_turn_configuration(&address, turn_id)
            .await
        else {
            return Ok(ToolBeforeOutcome::Continue(Box::new(invocation.call)));
        };

        for hook in frozen
            .effective_hooks
            .iter()
            .filter(|hook| hook.event == "tool.before")
        {
            if cancellation.is_cancelled() {
                return Err(failure("tool.before cancelled"));
            }
            let requirement = frozen
                .execution_requirements
                .iter()
                .find(|requirement| requirement.definition_revision == hook.definition_revision)
                .ok_or_else(|| {
                    failure(format!(
                        "missing execution requirement for {}",
                        hook.definition_revision
                    ))
                })?;
            let result = self
                .invoker
                .invoke(requirement, &invocation, cancellation.clone())
                .await
                .map_err(|diagnostic| {
                    failure(format!(
                        "{} execution failed: {diagnostic}",
                        hook.definition_revision
                    ))
                })?;
            let action = result
                .outcome
                .get("action")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    failure(format!(
                        "{} returned a malformed tool.before outcome",
                        hook.definition_revision
                    ))
                })?;
            match action {
                "continue" => {
                    let arguments = result.outcome.get("arguments").cloned().ok_or_else(|| {
                        failure(format!(
                            "{} omitted complete Tool arguments",
                            hook.definition_revision
                        ))
                    })?;
                    rua_tool::validate_args(&invocation.call.name, &arguments, &invocation.schema)
                        .map_err(|error| {
                            failure(format!(
                                "{} produced invalid Tool arguments: {error}",
                                hook.definition_revision
                            ))
                        })?;
                    invocation.call.arguments = arguments;
                }
                "reject" => {
                    return Ok(ToolBeforeOutcome::Reject {
                        reason: result
                            .outcome
                            .get("reason")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("rejected without a reason")
                            .to_string(),
                    });
                }
                other => {
                    return Err(failure(format!(
                        "{} returned unsupported action '{other}'",
                        hook.definition_revision
                    )));
                }
            }
        }
        Ok(ToolBeforeOutcome::Continue(Box::new(invocation.call)))
    }
}

fn failure(diagnostic: impl Into<String>) -> ToolBeforeFailure {
    ToolBeforeFailure {
        diagnostic: diagnostic.into(),
    }
}

fn chat_address(invocation: &ToolBeforeInvocation) -> Result<ChatAddress, String> {
    invocation
        .metadata
        .get(rua_agent::hooks::SECURITY_CHAT_ADDRESS_METADATA_KEY)
        .cloned()
        .ok_or_else(|| "missing Chat Address".to_string())
        .and_then(|value| serde_json::from_value(value).map_err(|error| error.to_string()))
}

fn scope_for_owner(
    owner_namespace: &str,
    invocation: &ToolBeforeInvocation,
) -> Result<ChatScope, String> {
    if let Some(agent_id) = owner_namespace.strip_prefix("agent:") {
        return Ok(ChatScope::agent(agent_id));
    }
    if let Some(project_id) = owner_namespace.strip_prefix("project:") {
        return Ok(ChatScope::project(project_id));
    }
    if owner_namespace.starts_with("plugin:") {
        return Ok(chat_address(invocation)?.scope().clone());
    }
    Err(format!("unsupported Handler owner: {owner_namespace}"))
}
