pub mod bootstrap;
pub mod plugin_hook;

use agent_types::{AgentEvent, ModelDialogueItem, ToolCall, ToolResult};
use async_trait::async_trait;
use react_agent::hooks::{AgentHook, ApprovalResult};
use react_agent::{AgentError, ToolDecision};
use std::sync::Arc;
use tokio::sync::Mutex;

pub const SECURITY_CHAT_ADDRESS_METADATA_KEY: &str = "security_chat_address";
pub const PROJECT_INSTRUCTIONS_METADATA_KEY: &str = "project_instructions";
pub const PROJECT_SKILLS_ROOT_METADATA_KEY: &str = "project_skills_root";

pub struct SecurityAgentHook {
    security: Arc<rua_security::SecurityModule>,
    approval_required: Vec<String>,
    enabled: bool,
    session_id: Option<String>,
    agent_id: Option<String>,
    chat_address: Option<rua_core::ChatAddress>,
    turn_id: Option<String>,
    working_dir: Option<String>,
    project_resource_roots: Option<Vec<std::path::PathBuf>>,
    approval_bypass: bool,
    security_snapshot: Option<Arc<rua_config::types::security::SecurityConfig>>,
}

impl SecurityAgentHook {
    pub fn new(security: Arc<rua_security::SecurityModule>, enabled: bool) -> Self {
        Self {
            security,
            approval_required: Vec::new(),
            enabled,
            session_id: None,
            agent_id: None,
            chat_address: None,
            turn_id: None,
            working_dir: None,
            project_resource_roots: None,
            approval_bypass: false,
            security_snapshot: None,
        }
    }

    pub fn with_approval_required(mut self, tools: Vec<String>) -> Self {
        self.approval_required = tools;
        self
    }

    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn with_agent_id(mut self, agent_id: impl Into<String>) -> Self {
        self.agent_id = Some(agent_id.into());
        self
    }

    pub fn with_chat_address(mut self, chat_address: rua_core::ChatAddress) -> Self {
        self.chat_address = Some(chat_address);
        self
    }

    pub fn with_turn_id(mut self, turn_id: impl Into<String>) -> Self {
        self.turn_id = Some(turn_id.into());
        self
    }

    pub fn with_working_dir(mut self, working_dir: impl Into<String>) -> Self {
        self.working_dir = Some(working_dir.into());
        self
    }

    pub fn with_project_resource_roots(mut self, roots: Vec<std::path::PathBuf>) -> Self {
        self.project_resource_roots = Some(roots);
        self
    }

    pub fn with_approval_bypass(mut self, approval_bypass: bool) -> Self {
        self.approval_bypass = approval_bypass;
        self
    }

    pub fn with_security_snapshot(
        mut self,
        config: Arc<rua_config::types::security::SecurityConfig>,
    ) -> Self {
        self.security_snapshot = Some(config);
        self
    }
}

#[async_trait]
impl AgentHook for SecurityAgentHook {
    async fn before_tool_call(
        &self,
        call: &ToolCall,
        _iteration: u32,
    ) -> Result<ToolDecision, AgentError> {
        if !self.enabled {
            return Ok(ToolDecision::Allow);
        }

        let agent_id = self.agent_id.as_deref().unwrap_or("");
        let session_id = self.session_id.as_deref().unwrap_or("");
        let mut ctx = rua_security::ToolCallContext::new(
            agent_id,
            session_id,
            &call.name,
            call.arguments.clone(),
        );
        ctx.security_snapshot = self.security_snapshot.clone();
        if let Some(chat_address) = self.chat_address.as_ref() {
            ctx.approval_owner = rua_security::ApprovalOwner::chat(chat_address.clone());
        }
        ctx.metadata.insert(
            "tool_call_id".into(),
            serde_json::Value::String(call.id.clone()),
        );
        if let Some(ref turn_id) = self.turn_id {
            ctx.metadata
                .insert("turn_id".into(), serde_json::Value::String(turn_id.clone()));
        }
        if let Some(ref wd) = self.working_dir {
            ctx.working_dir = wd.clone();
        }

        if let Some(roots) = &self.project_resource_roots {
            ctx.metadata
                .insert("project_resource_roots".into(), serde_json::json!(roots));
        }
        let verdict = self.security.enforce(&ctx, &self.approval_required).await;
        if self.approval_bypass
            && matches!(
                verdict,
                ToolDecision::RequiresApproval { .. } | ToolDecision::AuditOnly { .. }
            )
        {
            return Ok(ToolDecision::Allow);
        }

        Ok(verdict)
    }

    async fn wait_for_approval(
        &self,
        tool_use_id: &str,
        _tool_name: &str,
        prompt: &str,
        call: &ToolCall,
    ) -> ApprovalResult {
        let session_id = match &self.session_id {
            Some(sid) => sid.clone(),
            None => {
                return ApprovalResult::Deny {
                    reason: "no session available for approval".into(),
                };
            }
        };
        let agent_id = self.agent_id.as_deref().unwrap_or("unknown");
        let turn_id = match &self.turn_id {
            Some(turn_id) => turn_id.clone(),
            None => {
                return ApprovalResult::Deny {
                    reason: "no turn available for approval".into(),
                };
            }
        };
        let authority = self.security.approval_authority();
        let approval_owner = self
            .chat_address
            .clone()
            .map(rua_security::ApprovalOwner::chat)
            .unwrap_or_else(|| rua_security::ApprovalOwner::runtime_session(agent_id, &session_id));

        let snapshot = self
            .security
            .approval_authority()
            .snapshot_for_owner(&approval_owner)
            .await;
        let request_id = if let Some(existing) = snapshot
            .pending
            .into_iter()
            .find(|request| request.tool_call_id == tool_use_id)
        {
            existing.id
        } else {
            authority
                .create_request(rua_security::ApprovalRequestInput {
                    agent_id: agent_id.to_string(),
                    owner: approval_owner,
                    turn_id,
                    tool_call_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    trigger: rua_security::ApprovalTrigger::approval_mode(prompt.to_string()),
                    tool_call: rua_security::ToolCallSnapshot {
                        display: format!("{} {}", call.name, call.arguments),
                        args: call.arguments.clone(),
                    },
                })
                .await
                .id
        };

        match authority.wait_for_decision(&request_id).await {
            Ok(request) if request.status == rua_security::AuthorityStatus::Approved => {
                ApprovalResult::Allow
            }
            Ok(request) => ApprovalResult::Deny {
                reason: match request.status {
                    rua_security::AuthorityStatus::Rejected => "rejected by the user".to_string(),
                    other => format!("approval ended as {other:?}"),
                },
            },
            Err(err) => ApprovalResult::Deny {
                reason: err.to_string(),
            },
        }
    }

    async fn record_permission_hook_decision(
        &self,
        request_id: &str,
        prompt: &str,
        call: &ToolCall,
        decision: &react_agent::PermissionHookDecision,
    ) -> ApprovalResult {
        let Some(session_id) = self.session_id.clone() else {
            return ApprovalResult::Deny {
                reason: "no session available for approval".into(),
            };
        };
        let agent_id = self.agent_id.as_deref().unwrap_or("unknown");
        let Some(turn_id) = self.turn_id.clone() else {
            return ApprovalResult::Deny {
                reason: "no turn available for approval".into(),
            };
        };
        let owner = self
            .chat_address
            .clone()
            .map(rua_security::ApprovalOwner::chat)
            .unwrap_or_else(|| rua_security::ApprovalOwner::runtime_session(agent_id, &session_id));
        let authority = self.security.approval_authority();
        let request = authority
            .create_request(rua_security::ApprovalRequestInput {
                agent_id: agent_id.to_string(),
                owner,
                turn_id,
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                trigger: rua_security::ApprovalTrigger::approval_mode(prompt.to_string()),
                tool_call: rua_security::ToolCallSnapshot {
                    display: format!("{} {}", call.name, call.arguments),
                    args: call.arguments.clone(),
                },
            })
            .await;
        let authority_decision = match decision {
            react_agent::PermissionHookDecision::Allow { .. } => {
                rua_security::ApprovalDecision::RunOnce {
                    decided_by: format!("product_hook:{request_id}"),
                }
            }
            react_agent::PermissionHookDecision::Deny { .. } => {
                rua_security::ApprovalDecision::Reject {
                    decided_by: format!("product_hook:{request_id}"),
                }
            }
            react_agent::PermissionHookDecision::NoDecision => {
                return ApprovalResult::Deny {
                    reason: "NoDecision cannot be persisted".to_string(),
                };
            }
        };
        match authority.decide(&request.id, authority_decision).await {
            Ok(request) if request.status == rua_security::AuthorityStatus::Approved => {
                ApprovalResult::Allow
            }
            Ok(_) => ApprovalResult::Deny {
                reason: match decision {
                    react_agent::PermissionHookDecision::Deny { reason } => reason.clone(),
                    _ => "permission Hook rejected the request".to_string(),
                },
            },
            Err(error) => ApprovalResult::Deny {
                reason: error.to_string(),
            },
        }
    }
}

pub struct UsageAgentHook {
    prompt_tokens: Mutex<u64>,

    completion_tokens: Mutex<u64>,

    agent_id: String,
}

impl UsageAgentHook {
    pub fn new(agent_id: impl Into<String>) -> Self {
        Self {
            prompt_tokens: Mutex::new(0),
            completion_tokens: Mutex::new(0),
            agent_id: agent_id.into(),
        }
    }

    pub async fn prompt_tokens(&self) -> u64 {
        *self.prompt_tokens.lock().await
    }

    pub async fn completion_tokens(&self) -> u64 {
        *self.completion_tokens.lock().await
    }

    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }
}

#[async_trait]
impl AgentHook for UsageAgentHook {
    async fn after_model_event(&self, event: &AgentEvent) -> bool {
        if let AgentEvent::Usage { usage } = event {
            *self.prompt_tokens.lock().await += usage.prompt_tokens;
            *self.completion_tokens.lock().await += usage.completion_tokens;
        }
        if let AgentEvent::End {
            usage: Some(usage), ..
        } = event
        {
            let mut pt = self.prompt_tokens.lock().await;
            if usage.prompt_tokens > *pt {
                *pt = usage.prompt_tokens;
            }
            let mut ct = self.completion_tokens.lock().await;
            if usage.completion_tokens > *ct {
                *ct = usage.completion_tokens;
            }
        }
        true
    }
}

pub struct CompositeHook {
    hooks: Vec<Arc<dyn AgentHook>>,
}

impl CompositeHook {
    pub fn new(hooks: Vec<Arc<dyn AgentHook>>) -> Self {
        Self { hooks }
    }

    pub fn empty() -> Self {
        Self { hooks: Vec::new() }
    }

    pub fn add(&mut self, hook: Arc<dyn AgentHook>) {
        self.hooks.push(hook);
    }
}

#[async_trait]
impl AgentHook for CompositeHook {
    async fn before_model_call(
        &self,
        messages: &[ModelDialogueItem],
        iteration: u32,
        event_tx: &tokio::sync::mpsc::Sender<AgentEvent>,
    ) -> Result<(), AgentError> {
        for hook in &self.hooks {
            hook.before_model_call(messages, iteration, event_tx)
                .await?;
        }
        Ok(())
    }

    async fn after_model_event(&self, event: &AgentEvent) -> bool {
        let mut emit = true;
        for hook in &self.hooks {
            if !hook.after_model_event(event).await {
                emit = false;
            }
        }
        emit
    }

    async fn after_model_call(
        &self,
        messages: &[ModelDialogueItem],
        iteration: u32,
        total_usage: &agent_types::TokenUsage,
        last_call_usage: Option<&agent_types::TokenUsage>,
        terminal: bool,
    ) -> Result<(), AgentError> {
        for hook in &self.hooks {
            hook.after_model_call(messages, iteration, total_usage, last_call_usage, terminal)
                .await?;
        }
        Ok(())
    }

    async fn before_tool_call(
        &self,
        call: &ToolCall,
        iteration: u32,
    ) -> Result<ToolDecision, AgentError> {
        for hook in &self.hooks {
            let decision = hook.before_tool_call(call, iteration).await?;
            if !matches!(
                decision,
                ToolDecision::Allow | ToolDecision::AuditOnly { .. }
            ) {
                return Ok(decision);
            }
        }
        Ok(ToolDecision::Allow)
    }

    async fn after_tool_call(
        &self,
        call: &ToolCall,
        result: &ToolResult,
        duration_ms: u64,
    ) -> Result<(), AgentError> {
        for hook in &self.hooks {
            hook.after_tool_call(call, result, duration_ms).await?;
        }
        Ok(())
    }

    async fn on_finish(&self, iterations: u32, result: &Result<(), AgentError>) {
        for hook in &self.hooks {
            hook.on_finish(iterations, result).await;
        }
    }

    async fn on_iteration_limit(&self, max_iterations: u32) -> Result<(), AgentError> {
        for hook in &self.hooks {
            hook.on_iteration_limit(max_iterations).await?;
        }
        Ok(())
    }

    async fn wait_for_approval(
        &self,
        tool_use_id: &str,
        tool_name: &str,
        prompt: &str,
        call: &ToolCall,
    ) -> ApprovalResult {
        for hook in &self.hooks {
            let result = hook
                .wait_for_approval(tool_use_id, tool_name, prompt, call)
                .await;
            match &result {
                ApprovalResult::Allow | ApprovalResult::AllowWithModifiedArgs { .. } => {
                    return result;
                }
                ApprovalResult::Deny { reason } => {
                    if reason != "no approval handler configured" {
                        return result;
                    }
                }
            }
        }
        ApprovalResult::Deny {
            reason: "no approval handler configured".into(),
        }
    }

    async fn record_permission_hook_decision(
        &self,
        request_id: &str,
        prompt: &str,
        call: &ToolCall,
        decision: &react_agent::PermissionHookDecision,
    ) -> ApprovalResult {
        for hook in &self.hooks {
            let result = hook
                .record_permission_hook_decision(request_id, prompt, call, decision)
                .await;
            match &result {
                ApprovalResult::Deny { reason }
                    if reason == "no permission Hook decision handler configured" => {}
                _ => return result,
            }
        }
        ApprovalResult::Deny {
            reason: "no permission Hook decision handler configured".into(),
        }
    }
}
