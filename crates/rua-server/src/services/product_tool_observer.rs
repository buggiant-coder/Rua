use std::sync::Arc;

use async_trait::async_trait;
use react_agent::{
    ToolBeforeInvocation, ToolObservationEvent, ToolObservationInvocation, ToolObserverBoundary,
    ToolObserverContribution, ToolObserverFailure,
};
use rua_core::ChatAddress;
use tokio_util::sync::CancellationToken;

use crate::services::product_hook_execution::HandlerExecutionResult;
use crate::services::product_hooks::{
    HookExecutionRequirement, ProductHookContributionView, ProductHookService,
};
use crate::services::product_tool_before::{
    HandlerExecutorProductInvoker, ProductToolBeforeInvoker,
};

#[async_trait]
pub trait ProductToolObserverInvoker: Send + Sync {
    async fn invoke(
        &self,
        requirement: &HookExecutionRequirement,
        invocation: &ToolObservationInvocation,
        cancellation: CancellationToken,
    ) -> Result<HandlerExecutionResult, String>;
}

pub struct HandlerExecutorProductObserverInvoker {
    inner: Arc<HandlerExecutorProductInvoker>,
}

impl HandlerExecutorProductObserverInvoker {
    pub fn new(inner: Arc<HandlerExecutorProductInvoker>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl ProductToolObserverInvoker for HandlerExecutorProductObserverInvoker {
    async fn invoke(
        &self,
        requirement: &HookExecutionRequirement,
        invocation: &ToolObservationInvocation,
        cancellation: CancellationToken,
    ) -> Result<HandlerExecutionResult, String> {
        let mut metadata = invocation.metadata.clone();
        metadata.insert(
            "tool_observation".to_string(),
            serde_json::json!({
                "event": invocation.event,
                "canonical_result": invocation.result,
            }),
        );
        self.inner
            .invoke(
                requirement,
                &ToolBeforeInvocation {
                    call: invocation.call.clone(),
                    schema: serde_json::Value::Null,
                    agent_id: invocation.agent_id.clone(),
                    session_id: invocation.session_id.clone(),
                    working_directory: invocation.working_directory.clone(),
                    metadata,
                },
                cancellation,
            )
            .await
    }
}

pub struct ProductToolObserverBoundaryImpl {
    hooks: Arc<ProductHookService>,
    invoker: Arc<dyn ProductToolObserverInvoker>,
}

impl ProductToolObserverBoundaryImpl {
    pub fn new(
        hooks: Arc<ProductHookService>,
        invoker: Arc<dyn ProductToolObserverInvoker>,
    ) -> Self {
        Self { hooks, invoker }
    }
}

#[async_trait]
impl ToolObserverBoundary for ProductToolObserverBoundaryImpl {
    async fn observe(
        &self,
        invocation: ToolObservationInvocation,
        cancellation: CancellationToken,
    ) -> Result<Vec<ToolObserverContribution>, ToolObserverFailure> {
        let Some((address, turn_id)) = observation_identity(&invocation)? else {
            return Ok(Vec::new());
        };
        let Some(frozen) = self
            .hooks
            .frozen_turn_configuration(&address, &turn_id)
            .await
        else {
            return Ok(Vec::new());
        };
        let event = match invocation.event {
            ToolObservationEvent::After => "tool.after",
            ToolObservationEvent::Failed => "tool.failed",
        };
        let mut contributions = Vec::new();
        for (resolved_order, hook) in frozen.effective_hooks.iter().enumerate() {
            if hook.event != event {
                continue;
            }
            let source = serde_json::to_string(&hook.selected_by).unwrap_or_default();
            let Some(requirement) = frozen
                .execution_requirements
                .iter()
                .find(|item| item.definition_revision == hook.definition_revision)
            else {
                self.record(
                    &address,
                    &turn_id,
                    &invocation,
                    event,
                    hook,
                    &source,
                    resolved_order,
                    "diagnostic",
                    serde_json::Value::Null,
                    Some("missing execution requirement".to_string()),
                )
                .await;
                continue;
            };
            match self
                .invoker
                .invoke(requirement, &invocation, cancellation.clone())
                .await
            {
                Ok(result) => {
                    let Some(items) = result
                        .outcome
                        .get("contributions")
                        .and_then(serde_json::Value::as_array)
                    else {
                        self.record(
                            &address,
                            &turn_id,
                            &invocation,
                            event,
                            hook,
                            &source,
                            resolved_order,
                            "diagnostic",
                            serde_json::Value::Null,
                            Some("malformed observer outcome".to_string()),
                        )
                        .await;
                        continue;
                    };
                    for item in items {
                        let Some(kind) = item.get("kind").and_then(serde_json::Value::as_str)
                        else {
                            continue;
                        };
                        if !matches!(kind, "feedback" | "context") {
                            continue;
                        }
                        let content = item
                            .get("content")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null);
                        self.record(
                            &address,
                            &turn_id,
                            &invocation,
                            event,
                            hook,
                            &source,
                            resolved_order,
                            kind,
                            content.clone(),
                            None,
                        )
                        .await;
                        contributions.push(ToolObserverContribution {
                            definition_revision: hook.definition_revision.clone(),
                            source: source.clone(),
                            resolved_order,
                            kind: kind.to_string(),
                            content,
                        });
                    }
                }
                Err(diagnostic) => {
                    self.record(
                        &address,
                        &turn_id,
                        &invocation,
                        event,
                        hook,
                        &source,
                        resolved_order,
                        "diagnostic",
                        serde_json::Value::Null,
                        Some(diagnostic),
                    )
                    .await;
                }
            }
        }
        Ok(contributions)
    }
}

impl ProductToolObserverBoundaryImpl {
    #[allow(clippy::too_many_arguments)]
    async fn record(
        &self,
        address: &ChatAddress,
        turn_id: &str,
        invocation: &ToolObservationInvocation,
        event: &str,
        hook: &crate::services::scope_composition::ResolvedHookBinding,
        source: &str,
        resolved_order: usize,
        kind: &str,
        content: serde_json::Value,
        diagnostic: Option<String>,
    ) {
        self.hooks
            .record_contribution(ProductHookContributionView {
                chat_address: address.clone(),
                turn_id: turn_id.to_string(),
                tool_call_id: invocation.call.id.clone(),
                event: event.to_string(),
                definition_revision: hook.definition_revision.clone(),
                source: source.to_string(),
                resolved_order,
                kind: kind.to_string(),
                content,
                diagnostic,
            })
            .await;
    }
}

fn observation_identity(
    invocation: &ToolObservationInvocation,
) -> Result<Option<(ChatAddress, String)>, ToolObserverFailure> {
    let Some(value) = invocation
        .metadata
        .get(rua_agent::hooks::SECURITY_CHAT_ADDRESS_METADATA_KEY)
        .cloned()
    else {
        return Ok(None);
    };
    let address = serde_json::from_value(value).map_err(|error| ToolObserverFailure {
        diagnostic: format!("invalid Chat Address: {error}"),
    })?;
    let Some(turn_id) = invocation
        .metadata
        .get("turn_id")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    Ok(Some((address, turn_id.to_string())))
}
