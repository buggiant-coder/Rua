use std::sync::{Arc, Weak};
use std::time::Duration;

use agent_types::ToolCall;
use async_trait::async_trait;
use react_agent::hooks::AgentHook;
use rua_tool::{ToolTaskExecution, ToolTaskExecutionRequest};

use crate::registry::WorkspaceRegistry;

pub struct WorkspaceToolTaskExecution {
    registry: Weak<dyn WorkspaceRegistry>,
}

struct TaskToolWaitStateHook {
    inner: Arc<dyn AgentHook>,
    runtime: Arc<rua_task::TaskRuntime>,
    owner: rua_task::TaskOwner,
    task_id: String,
    attempt_no: u32,
}

#[async_trait]
impl AgentHook for TaskToolWaitStateHook {
    async fn before_tool_call(
        &self,
        call: &ToolCall,
        iteration: u32,
    ) -> Result<react_agent::ToolDecision, react_agent::error::AgentError> {
        self.inner.before_tool_call(call, iteration).await
    }

    async fn after_tool_call(
        &self,
        call: &ToolCall,
        result: &agent_types::ToolResult,
        duration_ms: u64,
    ) -> Result<(), react_agent::error::AgentError> {
        self.inner.after_tool_call(call, result, duration_ms).await
    }

    async fn wait_for_approval(
        &self,
        tool_use_id: &str,
        tool_name: &str,
        prompt: &str,
        call: &ToolCall,
    ) -> react_agent::hooks::ApprovalResult {
        match self
            .runtime
            .set_waiting_approval(&self.owner, &self.task_id, self.attempt_no, true)
            .await
        {
            Ok(true) => {}
            result => {
                return react_agent::hooks::ApprovalResult::Deny {
                    reason: format!("Task cannot enter approval wait: {result:?}"),
                }
            }
        }
        let decision = self
            .inner
            .wait_for_approval(tool_use_id, tool_name, prompt, call)
            .await;
        match self
            .runtime
            .set_waiting_approval(&self.owner, &self.task_id, self.attempt_no, false)
            .await
        {
            Ok(true) => decision,
            result => react_agent::hooks::ApprovalResult::Deny {
                reason: format!("Task is no longer awaiting this approval: {result:?}"),
            },
        }
    }

    async fn record_permission_hook_decision(
        &self,
        request_id: &str,
        prompt: &str,
        call: &ToolCall,
        decision: &react_agent::PermissionHookDecision,
    ) -> react_agent::hooks::ApprovalResult {
        self.inner
            .record_permission_hook_decision(request_id, prompt, call, decision)
            .await
    }
}

impl WorkspaceToolTaskExecution {
    pub fn new(registry: Weak<dyn WorkspaceRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl ToolTaskExecution for WorkspaceToolTaskExecution {
    async fn execute(
        &self,
        request: ToolTaskExecutionRequest,
    ) -> Result<agent_types::ToolResult, String> {
        let registry = self
            .registry
            .upgrade()
            .ok_or_else(|| "workspace registry is unavailable".to_string())?;
        let execution_agent_id = request.authorization_agent_id.clone();
        let chat_id = request.owner.chat_id().to_string();
        let runtime = registry
            .get_or_create(&execution_agent_id)
            .await
            .map_err(|error| error.to_string())?;
        if !runtime.can_accept_requests().await {
            return Err(
                "Agent runtime is shutting down and cannot execute new Tool Tasks".to_string(),
            );
        }
        let scope = rua_core::ChatScopeStore::resolve(
            &runtime.rua_root(),
            request.owner.chat_address.clone(),
        );
        let store = rua_session::WorkspaceSessionStore::new(scope.root());
        let epoch = request
            .configuration_epoch
            .as_deref()
            .ok_or("Task has no captured Chat configuration epoch")?;
        let snapshot = store
            .load_configuration_snapshot(&chat_id, epoch)
            .await
            .map_err(|error| error.to_string())?
            .ok_or("Task configuration snapshot is unavailable")?;
        let assignment: rua_chat::TurnAgentAssignment = serde_json::from_value(
            snapshot
                .get("agent_assignment")
                .cloned()
                .ok_or("Task configuration has no Agent assignment")?,
        )
        .map_err(|error| format!("invalid Task Agent assignment: {error}"))?;
        if assignment.agent_id != execution_agent_id {
            return Err("Task Agent does not match the captured Chat configuration".into());
        }
        let configuration: rua_agent::runtime::ChatCapabilityConfiguration =
            serde_json::from_value(
                snapshot
                    .get("capability_configuration")
                    .cloned()
                    .ok_or("Task configuration has no Tool selection policy")?,
            )
            .map_err(|error| format!("invalid Task capability configuration: {error}"))?;
        let project_root = match request.owner.chat_address.project_id() {
            Some(project_id) => {
                let stores = rua_project::ProjectStores::open(runtime.rua_root().join("projects"))
                    .map_err(|error| error.to_string())?;
                let binding = stores
                    .bindings
                    .get(project_id)
                    .map_err(|error| error.to_string())?
                    .ok_or("Task Project Resource Binding is unavailable")?;
                let directory_id = configuration.extra.get("project_working_directory_id")
                    .and_then(serde_json::Value::as_str)
                    .ok_or("Task configuration has no captured Project working directory; update this Chat configuration")?;
                let directory = binding
                    .resources
                    .directory(directory_id)
                    .map_err(|error| error.to_string())?;
                rua_project::validate_project_directory(&directory.resource_root)
                    .map_err(|error| error.to_string())?;
                Some(directory.resource_root.clone())
            }
            None => None,
        };
        let working_dir = project_root
            .clone()
            .unwrap_or_else(|| runtime.workspace_dir());
        let toolkit = runtime
            .task_toolkit_for_configuration(
                scope.root(),
                project_root.as_deref(),
                &chat_id,
                request.origin_turn_id.as_deref(),
                &configuration,
            )
            .await
            .map_err(|error| error.to_string())?;
        if !toolkit.has_tool(&request.tool_name) {
            return Err(format!(
                "tool '{}' is not available in the Task configuration",
                request.tool_name
            ));
        }
        let disabled_tools = assignment.disabled_tools;
        if disabled_tools.contains(&request.tool_name) {
            return Err(format!(
                "tool '{}' is disabled in the Task configuration",
                request.tool_name
            ));
        }
        let security_snapshot = Arc::new(
            serde_json::from_value::<rua_config::types::security::SecurityConfig>(
                snapshot
                    .get("security_config")
                    .cloned()
                    .ok_or("Task security snapshot is missing")?,
            )
            .map_err(|error| format!("invalid Task security snapshot: {error}"))?,
        );

        let security = runtime
            .security_module()
            .await
            .ok_or_else(|| "agent security module is unavailable".to_string())?;
        let mut security_hook = rua_agent::hooks::SecurityAgentHook::new(security, true)
            .with_session_id(&chat_id)
            .with_agent_id(&execution_agent_id)
            .with_chat_address(request.owner.chat_address.clone())
            .with_working_dir(working_dir.to_string_lossy().to_string())
            .with_security_snapshot(Arc::clone(&security_snapshot));
        if let Some(turn_id) = request.origin_turn_id.as_ref() {
            security_hook = security_hook.with_turn_id(turn_id);
        }
        let hook: Arc<dyn AgentHook> = Arc::new(TaskToolWaitStateHook {
            inner: Arc::new(security_hook),
            runtime: runtime.task_runtime(),
            owner: request.owner.clone(),
            task_id: request.task_id.clone(),
            attempt_no: request.attempt_no,
        });

        let call = ToolCall {
            id: format!(
                "task:{}:attempt:{}:tool",
                request.task_id, request.attempt_no
            ),
            name: request.tool_name,
            arguments: request.arguments,
            raw_arguments: None,
            provider_call_id: None,
            metadata: Default::default(),
        };
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let tool_result_scope_root = match request.owner.chat_address.project_id() {
            Some(project_id) => runtime
                .rua_root()
                .join("projects")
                .join(project_id.as_str()),
            None => runtime.workspace_dir(),
        };
        let mut metadata = std::collections::HashMap::from([(
            "task_id".to_string(),
            serde_json::Value::String(request.task_id),
        )]);
        let root = runtime.rua_root();
        let config = rua_config::load_config(&root.join("config.json"), &root)
            .map_err(|error| error.to_string())?;
        // Null is an explicit UTC default, not permission to read another instance's config.
        metadata.insert(
            "user_timezone".to_string(),
            serde_json::json!(config.user_timezone),
        );
        metadata.insert(
            rua_agent::runtime::CHAT_CAPABILITY_CONFIGURATION_METADATA_KEY.into(),
            serde_json::to_value(&configuration).map_err(|error| error.to_string())?,
        );
        metadata.insert(
            rua_core::tool::EFFECTIVE_DISABLED_TOOLS_METADATA_KEY.into(),
            serde_json::json!(disabled_tools),
        );
        let running: rua_config::AgentsRunningConfig = serde_json::from_value(
            snapshot
                .get("running_config")
                .cloned()
                .ok_or("Task configuration has no Running snapshot")?,
        )
        .map_err(|error| format!("invalid Task Running snapshot: {error}"))?;
        metadata.insert(
            "memory_search_mode".into(),
            serde_json::json!(running.memory_summary.search_mode),
        );
        metadata.insert(
            rua_agent::hooks::SECURITY_CHAT_ADDRESS_METADATA_KEY.to_string(),
            serde_json::to_value(&request.owner.chat_address).map_err(|error| error.to_string())?,
        );
        if let Some(turn_id) = request.origin_turn_id {
            metadata.insert("turn_id".to_string(), serde_json::json!(turn_id));
        }
        metadata.insert(
            rua_security::models::EFFECTIVE_SECURITY_CONFIG_METADATA_KEY.to_string(),
            serde_json::to_value(security_snapshot.as_ref()).map_err(|error| error.to_string())?,
        );
        if let Some(epoch) = request.configuration_epoch {
            metadata.insert(
                "chat_configuration_epoch".to_string(),
                serde_json::json!(epoch),
            );
        }
        if !request.dependencies.is_empty() {
            metadata.insert(
                "task_dependencies".to_string(),
                serde_json::to_value(&request.dependencies).unwrap_or_default(),
            );
        }
        let tool_context = rua_core::tool::ToolContext {
            agent_id: execution_agent_id,
            session_id: chat_id,
            working_dir,
            tool_result_store_dir: Some(tool_result_scope_root.join("tool_call_logs")),
            metadata,
            cancel: Some(cancel_rx),
            task_runtime: None,
            task_session: Some(Arc::new(
                rua_tool::extended::task_tools::TaskRunnerSessionHandle::new(
                    request.session.clone(),
                ),
            )),
            event_tx: None,
            ..Default::default()
        };
        let cancel = request.cancel;
        let cancel_for_watch = cancel.clone();
        let cancel_bridge = tokio::spawn(async move {
            cancel_for_watch.cancelled().await;
            let _ = cancel_tx.send(true);
        });
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(32);
        let event_drain = tokio::spawn(async move { while event_rx.recv().await.is_some() {} });
        let result = react_agent::react_loop::execute_single_tool(
            &call,
            &toolkit,
            &hook,
            &tool_context,
            &react_agent::ToolDisclosureConfig::default(),
            0,
            &cancel,
            Duration::ZERO,
            &event_tx,
        )
        .await
        .map_err(|error| error.to_string());
        drop(event_tx);
        let _ = event_drain.await;
        cancel_bridge.abort();
        result
    }
}
