use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rua_core::{ChatAddress, ChatScope};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::services::product_hook_execution::{
    CommandExecutionResource, CommandHandler, HandlerAdapterError, HandlerAdapterInvocation,
    HandlerAdapterResult, HandlerExecutionResult, HandlerExecutor, HandlerExposure,
    HandlerInvocation, HandlerKind, NativeHandler, PluginHandler, StrictCommandHandler,
};
use crate::services::product_hooks::{HookExecutionRequirement, ProductHookService};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverCompletionCapability {
    Continuable,
    AcceptOnly,
}

#[derive(Debug, Clone)]
pub struct CandidateCompletion {
    pub address: ChatAddress,
    pub turn_id: String,
    pub driver_capability: DriverCompletionCapability,
    pub candidate: serde_json::Value,
    pub cancellation: CancellationToken,
    pub remaining_budget: Duration,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookContinuationContext {
    pub address: ChatAddress,
    pub turn_id: String,
    pub definition_revision: String,
    pub source: String,
    pub resolved_order: usize,
    pub content: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CompletionDisposition {
    Accept,
    Continue {
        contexts: Vec<HookContinuationContext>,
    },
    Failed {
        diagnostics: Vec<String>,
    },
}

#[async_trait]
pub trait CandidateCompletionInvoker: Send + Sync {
    async fn invoke(
        &self,
        requirement: &HookExecutionRequirement,
        completion: &CandidateCompletion,
    ) -> Result<HandlerExecutionResult, String>;
}

pub struct CandidateCompletionRuntime {
    hooks: Arc<ProductHookService>,
    invoker: Arc<dyn CandidateCompletionInvoker>,
}

pub struct HandlerExecutorCandidateCompletionInvoker {
    active_plugins: Arc<rua_plugin_runtime::ActivePluginRegistry>,
    local_device_id: String,
}

impl HandlerExecutorCandidateCompletionInvoker {
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

struct CandidatePluginHandler {
    registry: Arc<rua_plugin_runtime::ActivePluginRegistry>,
}

#[async_trait]
impl PluginHandler for CandidatePluginHandler {
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

struct UnavailableCandidateNativeHandler;

#[async_trait]
impl NativeHandler for UnavailableCandidateNativeHandler {
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
impl CandidateCompletionInvoker for HandlerExecutorCandidateCompletionInvoker {
    async fn invoke(
        &self,
        requirement: &HookExecutionRequirement,
        completion: &CandidateCompletion,
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
        let scope = scope_for_owner(owner_namespace, &completion.address)?;
        let resource_id = format!("{}:{}", scope.kind(), scope.owner_id());
        let working_directory = requirement
            .working_directory
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_default();
        let command: Arc<dyn CommandHandler> = Arc::new(StrictCommandHandler::new(
            &self.local_device_id,
            vec![CommandExecutionResource {
                scope: scope.clone(),
                resource_id: resource_id.clone(),
                root: working_directory.clone(),
            }],
        ));
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
            command,
            Arc::new(UnavailableCandidateNativeHandler),
            Arc::new(CandidatePluginHandler {
                registry: Arc::clone(&self.active_plugins),
            }),
        )
        .execute(HandlerInvocation {
            invocation_id: format!("candidate-completion-{}", uuid::Uuid::new_v4()),
            kind,
            authority_device_id: requirement.authority_device_id.clone(),
            chat_address: completion.address.clone(),
            scope,
            resource_id,
            executable: requirement.executable.clone(),
            program: requirement.program.clone(),
            args: requirement.args.clone(),
            working_directory,
            timeout: Duration::from_secs(requirement.timeout_seconds.unwrap_or(30)),
            exposure: HandlerExposure {
                data: serde_json::json!({
                    "event": "turn.before_completion",
                    "chat_address": completion.address,
                    "turn_id": completion.turn_id,
                    "candidate": completion.candidate,
                }),
                capabilities: requirement.capabilities.clone(),
                sensitive_values: Vec::new(),
            },
            cancellation: completion.cancellation.clone(),
            remaining_budget: completion.remaining_budget,
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

impl CandidateCompletionRuntime {
    pub fn new(
        hooks: Arc<ProductHookService>,
        invoker: Arc<dyn CandidateCompletionInvoker>,
    ) -> Self {
        Self { hooks, invoker }
    }

    pub async fn has_before_completion_hooks(&self, address: &ChatAddress, turn_id: &str) -> bool {
        self.hooks
            .frozen_turn_configuration(address, turn_id)
            .await
            .is_some_and(|frozen| {
                frozen
                    .effective_hooks
                    .iter()
                    .any(|hook| hook.event == "turn.before_completion")
            })
    }

    pub async fn validate_driver_compatibility(
        &self,
        address: &ChatAddress,
        turn_id: &str,
        driver_capability: DriverCompletionCapability,
    ) -> Result<(), String> {
        let Some(frozen) = self.hooks.frozen_turn_configuration(address, turn_id).await else {
            return Ok(());
        };
        if driver_capability == DriverCompletionCapability::AcceptOnly
            && has_continuation_capable_hook(&frozen)
        {
            return Err(
                "accept-only Driver is incompatible with continuation-capable turn.before_completion Hooks"
                    .to_string(),
            );
        }
        Ok(())
    }

    pub async fn evaluate(&self, completion: CandidateCompletion) -> CompletionDisposition {
        if completion.cancellation.is_cancelled() {
            return CompletionDisposition::Failed {
                diagnostics: vec!["Candidate Completion was cancelled".to_string()],
            };
        }
        if completion.remaining_budget.is_zero() {
            return CompletionDisposition::Failed {
                diagnostics: vec!["shared Turn budget is exhausted".to_string()],
            };
        }
        let Some(frozen) = self
            .hooks
            .frozen_turn_configuration(&completion.address, &completion.turn_id)
            .await
        else {
            return CompletionDisposition::Accept;
        };
        if completion.driver_capability == DriverCompletionCapability::AcceptOnly
            && has_continuation_capable_hook(&frozen)
        {
            return CompletionDisposition::Failed {
                diagnostics: vec![
                    "accept-only Driver is incompatible with continuation-capable turn.before_completion Hooks"
                        .to_string(),
                ],
            };
        }
        let mut contexts = Vec::new();
        let mut diagnostics = Vec::new();
        let started = std::time::Instant::now();

        for hook in frozen
            .effective_hooks
            .iter()
            .filter(|hook| hook.event == "turn.before_completion")
        {
            let Some(requirement) = frozen
                .execution_requirements
                .iter()
                .find(|requirement| requirement.definition_revision == hook.definition_revision)
            else {
                diagnostics.push(format!(
                    "missing execution requirement for {}",
                    hook.definition_revision
                ));
                continue;
            };
            if completion.cancellation.is_cancelled() {
                diagnostics.push("Candidate Completion was cancelled".to_string());
                break;
            }
            let remaining_budget = completion
                .remaining_budget
                .saturating_sub(started.elapsed());
            if remaining_budget.is_zero() {
                diagnostics.push("shared Turn budget is exhausted".to_string());
                break;
            }
            let mut invocation = completion.clone();
            invocation.remaining_budget = remaining_budget;
            let result = match self.invoker.invoke(requirement, &invocation).await {
                Ok(result) => result,
                Err(diagnostic) => {
                    diagnostics.push(format!(
                        "{} execution failed: {diagnostic}",
                        hook.definition_revision
                    ));
                    continue;
                }
            };
            let Some(action) = result
                .outcome
                .get("action")
                .and_then(serde_json::Value::as_str)
            else {
                diagnostics.push(format!(
                    "{} returned a malformed turn.before_completion outcome",
                    hook.definition_revision
                ));
                continue;
            };
            match action {
                "accept" => {}
                "continue" => {
                    let Some(content) = result.outcome.get("context").cloned() else {
                        diagnostics.push(format!(
                            "{} omitted continuation context",
                            hook.definition_revision
                        ));
                        continue;
                    };
                    contexts.push(HookContinuationContext {
                        address: completion.address.clone(),
                        turn_id: completion.turn_id.clone(),
                        definition_revision: hook.definition_revision.clone(),
                        source: hook_source(hook),
                        resolved_order: hook.resolved_order,
                        content,
                    });
                }
                other => diagnostics.push(format!(
                    "{} returned unsupported action '{other}'",
                    hook.definition_revision
                )),
            }
        }

        if !diagnostics.is_empty() {
            CompletionDisposition::Failed { diagnostics }
        } else if contexts.is_empty() {
            CompletionDisposition::Accept
        } else {
            CompletionDisposition::Continue { contexts }
        }
    }
}

fn has_continuation_capable_hook(
    frozen: &crate::services::product_hooks::ChatHookConfiguration,
) -> bool {
    frozen.execution_requirements.iter().any(|requirement| {
        requirement
            .capabilities
            .iter()
            .any(|capability| capability == "candidate_response")
            && frozen.effective_hooks.iter().any(|hook| {
                hook.event == "turn.before_completion"
                    && hook.definition_revision == requirement.definition_revision
            })
    })
}

fn hook_source(hook: &crate::services::scope_composition::ResolvedHookBinding) -> String {
    serde_json::to_value(hook)
        .ok()
        .and_then(|value| {
            value
                .get("selected_by")?
                .as_array()?
                .first()?
                .as_str()
                .map(str::to_string)
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn scope_for_owner(owner_namespace: &str, address: &ChatAddress) -> Result<ChatScope, String> {
    if let Some(agent_id) = owner_namespace.strip_prefix("agent:") {
        return Ok(ChatScope::agent(agent_id));
    }
    if let Some(project_id) = owner_namespace.strip_prefix("project:") {
        return Ok(ChatScope::project(project_id));
    }
    if owner_namespace.starts_with("plugin:") {
        return Ok(address.scope().clone());
    }
    Err(format!("unsupported Handler owner: {owner_namespace}"))
}
