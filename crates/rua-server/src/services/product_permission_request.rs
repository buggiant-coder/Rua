use std::sync::Arc;

use async_trait::async_trait;
use react_agent::{
    PermissionHookDecision, PermissionHookFailure, PermissionRequestBoundary,
    PermissionRequestInvocation, ToolBeforeInvocation,
};
use rua_core::ChatAddress;
use tokio_util::sync::CancellationToken;

use crate::services::product_hook_execution::HandlerExecutionResult;
use crate::services::product_hooks::HookExecutionRequirement;
use crate::services::product_hooks::{ProductHookContributionView, ProductHookService};
use crate::services::product_tool_before::{
    HandlerExecutorProductInvoker, ProductToolBeforeInvoker,
};

pub struct ProductPermissionRequestBoundary {
    hooks: Arc<ProductHookService>,
    invoker: Arc<dyn ProductPermissionInvoker>,
}

#[async_trait]
pub trait ProductPermissionInvoker: Send + Sync {
    async fn invoke(
        &self,
        requirement: &HookExecutionRequirement,
        invocation: &ToolBeforeInvocation,
        cancellation: CancellationToken,
    ) -> Result<HandlerExecutionResult, String>;
}

#[async_trait]
impl ProductPermissionInvoker for HandlerExecutorProductInvoker {
    async fn invoke(
        &self,
        requirement: &HookExecutionRequirement,
        invocation: &ToolBeforeInvocation,
        cancellation: CancellationToken,
    ) -> Result<HandlerExecutionResult, String> {
        ProductToolBeforeInvoker::invoke(self, requirement, invocation, cancellation).await
    }
}

impl ProductPermissionRequestBoundary {
    pub fn new(hooks: Arc<ProductHookService>, invoker: Arc<dyn ProductPermissionInvoker>) -> Self {
        Self { hooks, invoker }
    }
}

#[async_trait]
impl PermissionRequestBoundary for ProductPermissionRequestBoundary {
    async fn decide(
        &self,
        invocation: PermissionRequestInvocation,
        cancellation: CancellationToken,
    ) -> Result<PermissionHookDecision, PermissionHookFailure> {
        let Some((address, turn_id)) = identity(&invocation)? else {
            return Ok(PermissionHookDecision::NoDecision);
        };
        let Some(frozen) = self
            .hooks
            .frozen_turn_configuration(&address, &turn_id)
            .await
        else {
            return Ok(PermissionHookDecision::NoDecision);
        };
        let mut aggregate = PermissionHookDecision::NoDecision;
        for (resolved_order, hook) in frozen.effective_hooks.iter().enumerate() {
            if hook.event != "permission.request" {
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
                    hook,
                    &source,
                    resolved_order,
                    PermissionHookDecision::NoDecision,
                    Some("missing execution requirement".to_string()),
                )
                .await;
                continue;
            };
            let mut metadata = invocation.metadata.clone();
            metadata.insert(
                "permission_request".to_string(),
                serde_json::json!({
                    "request_id": invocation.request_id,
                    "prompt": invocation.prompt,
                    "argument_digest": invocation.argument_digest,
                }),
            );
            let before = ToolBeforeInvocation {
                call: invocation.call.clone(),
                schema: serde_json::Value::Null,
                agent_id: invocation.agent_id.clone(),
                session_id: invocation.session_id.clone(),
                working_directory: invocation.working_directory.clone(),
                metadata,
            };
            let (decision, diagnostic) = match self
                .invoker
                .invoke(requirement, &before, cancellation.clone())
                .await
            {
                Ok(result) => match result
                    .outcome
                    .get("decision")
                    .and_then(serde_json::Value::as_str)
                {
                    Some("deny") => (
                        PermissionHookDecision::Deny {
                            reason: result
                                .outcome
                                .get("reason")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("denied without a reason")
                                .to_string(),
                        },
                        None,
                    ),
                    Some("allow") => (
                        PermissionHookDecision::Allow {
                            reason: result
                                .outcome
                                .get("reason")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_string),
                        },
                        None,
                    ),
                    Some("no_decision") => (PermissionHookDecision::NoDecision, None),
                    _ => (
                        PermissionHookDecision::NoDecision,
                        Some("malformed permission Hook outcome".to_string()),
                    ),
                },
                Err(error) => (PermissionHookDecision::NoDecision, Some(error)),
            };
            self.record(
                &address,
                &turn_id,
                &invocation,
                hook,
                &source,
                resolved_order,
                decision.clone(),
                diagnostic,
            )
            .await;
            aggregate = aggregate_decision(aggregate, decision);
        }
        Ok(aggregate)
    }
}

impl ProductPermissionRequestBoundary {
    #[allow(clippy::too_many_arguments)]
    async fn record(
        &self,
        address: &ChatAddress,
        turn_id: &str,
        invocation: &PermissionRequestInvocation,
        hook: &crate::services::scope_composition::ResolvedHookBinding,
        source: &str,
        resolved_order: usize,
        decision: PermissionHookDecision,
        diagnostic: Option<String>,
    ) {
        self.hooks
            .record_contribution(ProductHookContributionView {
                chat_address: address.clone(),
                turn_id: turn_id.to_string(),
                tool_call_id: invocation.call.id.clone(),
                event: "permission.request".to_string(),
                definition_revision: hook.definition_revision.clone(),
                source: source.to_string(),
                resolved_order,
                kind: "permission_decision".to_string(),
                content: serde_json::json!({
                    "request_id": invocation.request_id,
                    "argument_digest": invocation.argument_digest,
                    "decision": decision,
                }),
                diagnostic,
            })
            .await;
    }
}

fn aggregate_decision(
    current: PermissionHookDecision,
    next: PermissionHookDecision,
) -> PermissionHookDecision {
    match (current, next) {
        (deny @ PermissionHookDecision::Deny { .. }, _) => deny,
        (_, deny @ PermissionHookDecision::Deny { .. }) => deny,
        (allow @ PermissionHookDecision::Allow { .. }, _) => allow,
        (_, allow @ PermissionHookDecision::Allow { .. }) => allow,
        _ => PermissionHookDecision::NoDecision,
    }
}

fn identity(
    invocation: &PermissionRequestInvocation,
) -> Result<Option<(ChatAddress, String)>, PermissionHookFailure> {
    let Some(value) = invocation
        .metadata
        .get(rua_agent::hooks::SECURITY_CHAT_ADDRESS_METADATA_KEY)
        .cloned()
    else {
        return Ok(None);
    };
    let address = serde_json::from_value(value).map_err(|error| PermissionHookFailure {
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
