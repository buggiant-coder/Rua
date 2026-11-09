use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::services::conversation_authority::ConversationTurnTerminalFact;
use crate::services::product_hook_execution::{
    CommandExecutionResource, CommandHandler, HandlerAdapterError, HandlerAdapterInvocation,
    HandlerAdapterResult, HandlerExecutionResult, HandlerExecutor, HandlerExposure,
    HandlerInvocation, HandlerKind, NativeHandler, PluginHandler, StrictCommandHandler,
};
use crate::services::product_hooks::{HookExecutionRequirement, ProductHookService};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalObserverDiagnostic {
    pub definition_revision: String,
    pub resolved_order: usize,
    pub status: String,
    pub diagnostic: Option<String>,
}

#[async_trait]
pub trait TerminalObserverInvoker: Send + Sync {
    async fn invoke(
        &self,
        requirement: &HookExecutionRequirement,
        terminal: &ConversationTurnTerminalFact,
        cancellation: CancellationToken,
    ) -> Result<HandlerExecutionResult, String>;
}

pub struct TerminalObserverRuntime {
    hooks: Arc<ProductHookService>,
    invoker: Arc<dyn TerminalObserverInvoker>,
}

pub struct HandlerExecutorTerminalObserverInvoker {
    active_plugins: Arc<rua_plugin_runtime::ActivePluginRegistry>,
    local_device_id: String,
}

impl HandlerExecutorTerminalObserverInvoker {
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

struct TerminalPluginHandler {
    registry: Arc<rua_plugin_runtime::ActivePluginRegistry>,
}

#[async_trait]
impl PluginHandler for TerminalPluginHandler {
    async fn execute(
        &self,
        plugin_id: &str,
        invocation: HandlerAdapterInvocation,
    ) -> Result<HandlerAdapterResult, HandlerAdapterError> {
        let callback = invocation.required_callback("plugin")?.to_string();
        let result = self
            .registry
            .invoke_hook(
                plugin_id,
                rua_plugin_runtime::protocol::InvokeHookParams {
                    hook_point: callback.clone(),
                    payload: serde_json::to_string(&invocation.exposure.data).map_err(|error| {
                        HandlerAdapterError::MalformedOutcome(error.to_string())
                    })?,
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
        Ok(HandlerAdapterResult {
            outcome: serde_json::from_str(&entry.output)
                .map_err(|error| HandlerAdapterError::MalformedOutcome(error.to_string()))?,
            diagnostics: Vec::new(),
        })
    }
}

struct UnavailableTerminalNativeHandler;

#[async_trait]
impl NativeHandler for UnavailableTerminalNativeHandler {
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
impl TerminalObserverInvoker for HandlerExecutorTerminalObserverInvoker {
    async fn invoke(
        &self,
        requirement: &HookExecutionRequirement,
        terminal: &ConversationTurnTerminalFact,
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
        let scope = if let Some(agent_id) = owner_namespace.strip_prefix("agent:") {
            rua_core::ChatScope::agent(agent_id)
        } else if let Some(project_id) = owner_namespace.strip_prefix("project:") {
            rua_core::ChatScope::project(project_id)
        } else if owner_namespace.starts_with("plugin:") {
            terminal.address.scope().clone()
        } else {
            return Err(format!("unsupported Handler owner: {owner_namespace}"));
        };
        let resource_id = format!("{}:{}", scope.kind(), scope.owner_id());
        let working_directory = requirement
            .working_directory
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_default();
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
        HandlerExecutor::new(
            Arc::new(StrictCommandHandler::new(
                &self.local_device_id,
                vec![CommandExecutionResource {
                    scope: scope.clone(),
                    resource_id: resource_id.clone(),
                    root: working_directory.clone(),
                }],
            )) as Arc<dyn CommandHandler>,
            Arc::new(UnavailableTerminalNativeHandler),
            Arc::new(TerminalPluginHandler {
                registry: Arc::clone(&self.active_plugins),
            }),
        )
        .execute(HandlerInvocation {
            invocation_id: format!("turn-terminal-{}", uuid::Uuid::new_v4()),
            kind,
            authority_device_id: requirement.authority_device_id.clone(),
            chat_address: terminal.address.clone(),
            scope,
            resource_id,
            executable: requirement.executable.clone(),
            program: requirement.program.clone(),
            args: requirement.args.clone(),
            working_directory,
            timeout: Duration::from_secs(requirement.timeout_seconds.unwrap_or(30)),
            exposure: HandlerExposure {
                data: serde_json::json!({
                    "event": "turn.terminal",
                    "terminal_fact": {
                        "chat_address": terminal.address,
                        "turn_id": terminal.turn_id,
                        "status": terminal.status,
                        "completed_at": terminal.completed_at,
                        "authority_device_id": terminal.authority_device_id,
                        "device_delivery_id": terminal.device_delivery_id,
                    }
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

impl TerminalObserverRuntime {
    pub fn new(hooks: Arc<ProductHookService>, invoker: Arc<dyn TerminalObserverInvoker>) -> Self {
        Self { hooks, invoker }
    }

    pub async fn observe(
        &self,
        terminal: &ConversationTurnTerminalFact,
        cancellation: CancellationToken,
    ) -> Vec<TerminalObserverDiagnostic> {
        let Some(frozen) = self
            .hooks
            .frozen_turn_configuration(&terminal.address, &terminal.turn_id)
            .await
        else {
            return Vec::new();
        };
        self.observe_frozen(terminal, &frozen, cancellation).await
    }

    pub(crate) async fn observe_frozen(
        &self,
        terminal: &ConversationTurnTerminalFact,
        frozen: &crate::services::product_hooks::ChatHookConfiguration,
        cancellation: CancellationToken,
    ) -> Vec<TerminalObserverDiagnostic> {
        let mut diagnostics = Vec::new();
        for (resolved_order, hook) in frozen.effective_hooks.iter().enumerate() {
            if hook.event != "turn.terminal" {
                continue;
            }
            if hook.definition_revision
                == format!(
                    "{}@v1",
                    crate::services::product_hooks::NATIVE_TERMINAL_DEFINITION_ID
                )
            {
                continue;
            }
            if cancellation.is_cancelled() {
                diagnostics.push(TerminalObserverDiagnostic {
                    definition_revision: hook.definition_revision.clone(),
                    resolved_order,
                    status: "cancelled".to_string(),
                    diagnostic: Some("terminal observer cancelled".to_string()),
                });
                continue;
            }
            let Some(requirement) = frozen
                .execution_requirements
                .iter()
                .find(|item| item.definition_revision == hook.definition_revision)
            else {
                diagnostics.push(TerminalObserverDiagnostic {
                    definition_revision: hook.definition_revision.clone(),
                    resolved_order,
                    status: "failed".to_string(),
                    diagnostic: Some("missing execution requirement".to_string()),
                });
                continue;
            };
            match self
                .invoker
                .invoke(requirement, terminal, cancellation.clone())
                .await
            {
                Ok(_) => diagnostics.push(TerminalObserverDiagnostic {
                    definition_revision: hook.definition_revision.clone(),
                    resolved_order,
                    status: "completed".to_string(),
                    diagnostic: None,
                }),
                Err(error) => diagnostics.push(TerminalObserverDiagnostic {
                    definition_revision: hook.definition_revision.clone(),
                    resolved_order,
                    status: "failed".to_string(),
                    diagnostic: Some(error),
                }),
            }
        }
        diagnostics
    }
}
