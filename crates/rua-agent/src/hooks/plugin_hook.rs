use std::sync::Arc;

use agent_types::{AgentEvent, ModelDialogueItem, ToolCall, ToolResult};
use async_trait::async_trait;
use react_agent::hooks::AgentHook;
use react_agent::AgentError;
use react_agent::ToolDecision;
use rua_plugin_runtime::{DispatchOutcome, HookPoint, HookResult, PluginRegistry};

pub struct PluginHook {
    registry: Arc<PluginRegistry>,
}

impl PluginHook {
    pub fn new(registry: Arc<PluginRegistry>) -> Self {
        Self { registry }
    }

    pub fn parse_hook_outcome(results: &[HookResult]) -> PluginHookOutcome {
        for r in results {
            let output = r.output.trim();
            if output.is_empty() {
                continue;
            }
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(output) {
                if let Some(action) = parsed.get("action").and_then(|a| a.as_str()) {
                    match action {
                        "deny" => {
                            let reason = parsed
                                .get("reason")
                                .and_then(|v| v.as_str())
                                .unwrap_or("denied by plugin")
                                .to_string();
                            return PluginHookOutcome::Deny(reason);
                        }
                        "modify" => {
                            if let Some(patch) = parsed.get("patch") {
                                return PluginHookOutcome::Modify(patch.clone());
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        PluginHookOutcome::Continue
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PluginHookOutcome {
    Continue,
    Modify(serde_json::Value),
    Deny(String),
}

#[async_trait]
impl AgentHook for PluginHook {
    async fn before_model_call(
        &self,
        messages: &[ModelDialogueItem],
        iteration: u32,
        _event_tx: &tokio::sync::mpsc::Sender<AgentEvent>,
    ) -> Result<(), AgentError> {
        let payload = serde_json::json!({
            "message_count": messages.len(),
            "iteration": iteration,
        });

        match self
            .registry
            .dispatch_hook_with_abort(HookPoint::BeforeLlmCall, &payload.to_string())
            .await
        {
            Ok(result) => {
                if let DispatchOutcome::Aborted { reason } = result.outcome {
                    return Err(AgentError::Internal(format!(
                        "pre_message hook aborted: {reason}"
                    )));
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "plugin BeforeLlmCall hook failed, continuing");
            }
        }
        Ok(())
    }

    async fn after_model_event(&self, event: &AgentEvent) -> bool {
        let event_type = match event {
            AgentEvent::TextDelta { .. } => "text_delta",
            AgentEvent::ToolCall { .. } => "tool_call",
            AgentEvent::End { .. } => "end",
            AgentEvent::Error { .. } => "error",
            _ => "other",
        };

        let payload = serde_json::json!({
            "event_type": event_type,
        });

        match self
            .registry
            .dispatch_hook(HookPoint::AfterLlmCall, &payload.to_string())
            .await
        {
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(error = %e, "plugin AfterLlmCall hook failed, continuing");
            }
        }
        true
    }

    async fn before_tool_call(
        &self,
        call: &ToolCall,
        iteration: u32,
    ) -> Result<ToolDecision, AgentError> {
        let payload = serde_json::json!({
            "tool_name": call.name,
            "tool_id": call.id,
            "arguments": call.arguments,
            "iteration": iteration,
        });

        let dispatch_result = match self
            .registry
            .dispatch_hook_with_abort(HookPoint::BeforeToolCall, &payload.to_string())
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "plugin BeforeToolCall hook failed, continuing");
                return Ok(ToolDecision::Allow);
            }
        };

        if let DispatchOutcome::Aborted { reason } = dispatch_result.outcome {
            return Ok(ToolDecision::Deny { reason });
        }

        match Self::parse_hook_outcome(&dispatch_result.results) {
            PluginHookOutcome::Continue => Ok(ToolDecision::Allow),
            PluginHookOutcome::Deny(reason) => Ok(ToolDecision::Deny { reason }),
            PluginHookOutcome::Modify(_patch) => Ok(ToolDecision::Allow),
        }
    }

    async fn after_tool_call(
        &self,
        call: &ToolCall,
        result: &ToolResult,
        duration_ms: u64,
    ) -> Result<(), AgentError> {
        let payload = serde_json::json!({
            "tool_name": call.name,
            "tool_id": call.id,
            "duration_ms": duration_ms,
            "result_id": result.call_id,
        });

        match self
            .registry
            .dispatch_hook(HookPoint::AfterToolCall, &payload.to_string())
            .await
        {
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(error = %e, "plugin AfterToolCall hook failed, continuing");
            }
        }
        Ok(())
    }

    async fn on_finish(&self, iterations: u32, result: &Result<(), AgentError>) {
        let payload = serde_json::json!({
            "iterations": iterations,
            "success": result.is_ok(),
            "error": result.as_ref().err().map(|e| e.to_string()),
        });

        match self
            .registry
            .dispatch_hook(HookPoint::OnConversationEnd, &payload.to_string())
            .await
        {
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(error = %e, "plugin OnConversationEnd hook failed, continuing");
            }
        }

        let shutdown_hooks = self.registry.get_hooks_for_point(HookPoint::Shutdown).await;
        let conv_end_hooks = self
            .registry
            .get_hooks_for_point(HookPoint::OnConversationEnd)
            .await;
        for (pid, _spec) in &shutdown_hooks {
            let has_conv_end = conv_end_hooks.iter().any(|(p, _)| p == pid);
            if !has_conv_end {
                tracing::warn!(
                    plugin_id = %pid,
                    "plugin registers Shutdown but not OnConversationEnd; \
                     Shutdown now only fires on app exit. \
                     Migrate to OnConversationEnd for conversation lifecycle hooks."
                );
            }
        }
    }
}
