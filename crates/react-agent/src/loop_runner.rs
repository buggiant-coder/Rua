use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::warn;

use agent_types::{AgentEvent, ModelDialogueItem, TokenUsage, ToolCall, ToolResult};
use rua_skills::SkillEnvOverrides;

use crate::error::AgentError;
use crate::hooks::AgentHook;
use crate::hooks::ApprovalResult;
use crate::ToolDecision;

pub struct SkillEnvHook {
    inner: Arc<dyn AgentHook>,
    workspace_dir: PathBuf,
    channel: String,
    active_guards: Mutex<HashMap<String, SkillEnvOverrides>>,
}

impl SkillEnvHook {
    pub fn new(
        inner: Arc<dyn AgentHook>,
        workspace_dir: PathBuf,
        channel: impl Into<String>,
    ) -> Self {
        Self {
            inner,
            workspace_dir,
            channel: channel.into(),
            active_guards: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl AgentHook for SkillEnvHook {
    async fn before_model_call(
        &self,
        messages: &[agent_types::ModelDialogueItem],
        iteration: u32,
        event_tx: &tokio::sync::mpsc::Sender<AgentEvent>,
    ) -> Result<(), AgentError> {
        self.inner
            .before_model_call(messages, iteration, event_tx)
            .await
    }

    async fn after_model_event(&self, event: &AgentEvent) -> bool {
        self.inner.after_model_event(event).await
    }

    async fn after_model_call(
        &self,
        messages: &[ModelDialogueItem],
        iteration: u32,
        total_usage: &TokenUsage,
        last_call_usage: Option<&TokenUsage>,
        terminal: bool,
    ) -> Result<(), AgentError> {
        self.inner
            .after_model_call(messages, iteration, total_usage, last_call_usage, terminal)
            .await
    }

    async fn before_tool_call(
        &self,
        call: &ToolCall,
        iteration: u32,
    ) -> Result<ToolDecision, AgentError> {
        let decision = self.inner.before_tool_call(call, iteration).await?;

        if matches!(
            decision,
            ToolDecision::Allow | ToolDecision::AuditOnly { .. }
        ) {
            match SkillEnvOverrides::activate(&self.workspace_dir, &self.channel) {
                Ok(guard) => {
                    self.active_guards
                        .lock()
                        .await
                        .insert(call.id.clone(), guard);
                }
                Err(e) => {
                    warn!(
                        tool = %call.name,
                        "skill env overrides activation failed: {e}",
                    );
                }
            }
        }

        Ok(decision)
    }

    async fn after_tool_call(
        &self,
        call: &ToolCall,
        result: &ToolResult,
        duration_ms: u64,
    ) -> Result<(), AgentError> {
        let inner_result = self.inner.after_tool_call(call, result, duration_ms).await;

        self.active_guards.lock().await.remove(&call.id);
        inner_result
    }

    async fn on_finish(&self, iterations: u32, result: &Result<(), AgentError>) {
        self.inner.on_finish(iterations, result).await;
    }

    async fn on_iteration_limit(&self, max_iterations: u32) -> Result<(), AgentError> {
        self.inner.on_iteration_limit(max_iterations).await
    }

    async fn wait_for_approval(
        &self,
        tool_use_id: &str,
        tool_name: &str,
        prompt: &str,
        call: &ToolCall,
    ) -> ApprovalResult {
        self.inner
            .wait_for_approval(tool_use_id, tool_name, prompt, call)
            .await
    }
}
