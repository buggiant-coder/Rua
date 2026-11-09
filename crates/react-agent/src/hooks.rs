use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use agent_types::{AgentEvent, ModelDialogueItem, TokenUsage, ToolCall, ToolResult};

use crate::error::AgentError;
use crate::ToolDecision;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ApprovalResult {
    Allow,

    Deny { reason: String },

    AllowWithModifiedArgs { args: serde_json::Value },
}

#[async_trait]
pub trait AgentHook: Send + Sync {
    async fn before_model_call(
        &self,
        messages: &[ModelDialogueItem],
        iteration: u32,
        _event_tx: &tokio::sync::mpsc::Sender<AgentEvent>,
    ) -> Result<(), AgentError> {
        let _ = (messages, iteration);
        Ok(())
    }

    async fn after_model_event(&self, event: &AgentEvent) -> bool {
        let _ = event;
        true
    }

    async fn after_model_call(
        &self,
        messages: &[ModelDialogueItem],
        iteration: u32,
        total_usage: &TokenUsage,
        last_call_usage: Option<&TokenUsage>,
        terminal: bool,
    ) -> Result<(), AgentError> {
        let _ = (messages, iteration, total_usage, last_call_usage, terminal);
        Ok(())
    }

    async fn before_tool_call(
        &self,
        call: &ToolCall,
        iteration: u32,
    ) -> Result<ToolDecision, AgentError> {
        let _ = (call, iteration);
        Ok(ToolDecision::Allow)
    }

    async fn after_tool_call(
        &self,
        call: &ToolCall,
        result: &ToolResult,
        duration_ms: u64,
    ) -> Result<(), AgentError> {
        let _ = (call, result, duration_ms);
        Ok(())
    }

    async fn on_finish(&self, iterations: u32, result: &Result<(), AgentError>) {
        let _ = (iterations, result);
    }

    async fn on_iteration_limit(&self, _max_iterations: u32) -> Result<(), AgentError> {
        Ok(())
    }

    async fn wait_for_approval(
        &self,
        tool_use_id: &str,
        tool_name: &str,
        prompt: &str,
        call: &ToolCall,
    ) -> ApprovalResult {
        let _ = (tool_use_id, tool_name, prompt, call);
        ApprovalResult::Deny {
            reason: "no approval handler configured".into(),
        }
    }

    async fn record_permission_hook_decision(
        &self,
        _request_id: &str,
        _prompt: &str,
        _call: &ToolCall,
        _decision: &crate::PermissionHookDecision,
    ) -> ApprovalResult {
        ApprovalResult::Deny {
            reason: "no permission Hook decision handler configured".into(),
        }
    }
}

pub struct NoopHook;

#[async_trait]
impl AgentHook for NoopHook {}

#[derive(Default)]
pub struct RecordingHook {
    events: tokio::sync::Mutex<Vec<HookEvent>>,
}

#[derive(Debug, Clone)]
pub enum HookEvent {
    BeforeModelCall {
        iteration: u32,
    },
    AfterModelEvent {
        event_type: String,
    },
    AfterModelCall {
        iteration: u32,
        message_count: usize,
        terminal: bool,
    },
    BeforeToolCall {
        tool_name: String,
        iteration: u32,
    },
    AfterToolCall {
        tool_name: String,
        duration_ms: u64,
    },
    Finish {
        iterations: u32,
        success: bool,
    },
    ApprovalRequested {
        tool_name: String,
        prompt: String,
    },
    ApprovalResult {
        tool_name: String,
        result: ApprovalResult,
    },
}

impl RecordingHook {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn events(&self) -> Vec<HookEvent> {
        self.events.lock().await.clone()
    }

    pub async fn event_count(&self) -> usize {
        self.events.lock().await.len()
    }
}

#[async_trait]
impl AgentHook for RecordingHook {
    async fn before_model_call(
        &self,
        _messages: &[ModelDialogueItem],
        iteration: u32,
        _event_tx: &tokio::sync::mpsc::Sender<AgentEvent>,
    ) -> Result<(), AgentError> {
        self.events
            .lock()
            .await
            .push(HookEvent::BeforeModelCall { iteration });
        Ok(())
    }

    async fn after_model_event(&self, event: &AgentEvent) -> bool {
        let event_type = format!("{:?}", std::mem::discriminant(event));
        self.events
            .lock()
            .await
            .push(HookEvent::AfterModelEvent { event_type });
        true
    }

    async fn after_model_call(
        &self,
        messages: &[ModelDialogueItem],
        iteration: u32,
        _total_usage: &TokenUsage,
        _last_call_usage: Option<&TokenUsage>,
        terminal: bool,
    ) -> Result<(), AgentError> {
        self.events.lock().await.push(HookEvent::AfterModelCall {
            iteration,
            message_count: messages.len(),
            terminal,
        });
        Ok(())
    }

    async fn before_tool_call(
        &self,
        call: &ToolCall,
        iteration: u32,
    ) -> Result<ToolDecision, AgentError> {
        self.events.lock().await.push(HookEvent::BeforeToolCall {
            tool_name: call.name.clone(),
            iteration,
        });
        Ok(ToolDecision::Allow)
    }

    async fn after_tool_call(
        &self,
        call: &ToolCall,
        result: &ToolResult,
        duration_ms: u64,
    ) -> Result<(), AgentError> {
        let _ = result;
        self.events.lock().await.push(HookEvent::AfterToolCall {
            tool_name: call.name.clone(),
            duration_ms,
        });
        Ok(())
    }

    async fn on_finish(&self, iterations: u32, result: &Result<(), AgentError>) {
        self.events.lock().await.push(HookEvent::Finish {
            iterations,
            success: result.is_ok(),
        });
    }

    async fn wait_for_approval(
        &self,
        _tool_use_id: &str,
        tool_name: &str,
        prompt: &str,
        _call: &ToolCall,
    ) -> ApprovalResult {
        self.events.lock().await.push(HookEvent::ApprovalRequested {
            tool_name: tool_name.to_string(),
            prompt: prompt.to_string(),
        });
        ApprovalResult::Deny {
            reason: "recording hook: no approval handler".into(),
        }
    }
}
