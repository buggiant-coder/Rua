use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use agent_types::{ModelDialogueItem, ToolDefinition};
use llm_multiplex::ChatModel;
use rua_core::tool::ToolContext;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::config::ReactConfig;
use crate::hooks::AgentHook;
use crate::model_call_context::{ModelCallProjector, ModelCallSection};
use crate::permission_request::PermissionRequestBoundary;
use crate::tool_before::ToolBeforeBoundary;
use crate::tool_disclosure::assemble_tool_definitions;
use crate::tool_observer::ToolObserverBoundary;
use crate::toolkit::{AgentToolkit, AgentToolkitHandle};

pub struct AgentContext {
    pub model: Arc<dyn ChatModel>,

    pub toolkit: Arc<AgentToolkit>,

    pub toolkit_handle: AgentToolkitHandle,

    pub config: ReactConfig,

    pub hook: Arc<dyn AgentHook>,

    pub cancel: CancellationToken,

    pub run_control: Option<watch::Receiver<rua_core::RunControl>>,

    pub tool_ctx: ToolContext,

    pub run_id: String,

    pub model_name: Option<String>,

    pub provider_id: Option<String>,

    pub model_call_sections: Vec<ModelCallSection>,

    pub model_call_projector: Option<Arc<dyn ModelCallProjector>>,

    pub tool_before_boundary: Option<Arc<dyn ToolBeforeBoundary>>,
    pub tool_observer_boundary: Option<Arc<dyn ToolObserverBoundary>>,
    pub permission_request_boundary: Option<Arc<dyn PermissionRequestBoundary>>,

    pub approval_authority: Option<Arc<rua_security::approval_authority::ApprovalAuthority>>,

    pub(crate) memory_operation_recent_tools: Mutex<VecDeque<(String, bool)>>,
}

impl AgentContext {
    pub fn new(
        model: Arc<dyn ChatModel>,
        toolkit: Arc<AgentToolkit>,
        config: ReactConfig,
        hook: Arc<dyn AgentHook>,
        mut tool_ctx: ToolContext,
    ) -> Self {
        let info = model.model_info();
        tool_ctx.metadata.insert(
            rua_core::tool::MODEL_SUPPORTS_IMAGE_METADATA_KEY.into(),
            serde_json::json!(
                info.supports_multimodal != Some(false)
                    && info.supports_image.or(info.supports_multimodal) == Some(true)
            ),
        );
        let toolkit_handle = AgentToolkitHandle::new(Arc::clone(&toolkit));
        Self {
            model,
            toolkit,
            toolkit_handle,
            config,
            hook,
            cancel: CancellationToken::new(),
            run_control: None,
            tool_ctx,
            run_id: uuid::Uuid::new_v4().to_string(),
            model_name: None,
            provider_id: None,
            model_call_sections: Vec::new(),
            model_call_projector: None,
            tool_before_boundary: None,
            tool_observer_boundary: None,
            permission_request_boundary: None,
            approval_authority: None,
            memory_operation_recent_tools: Mutex::new(VecDeque::new()),
        }
    }

    pub fn with_run_id(mut self, run_id: impl Into<String>) -> Self {
        self.run_id = run_id.into();
        self
    }

    pub fn with_model_name(mut self, model_name: impl Into<String>) -> Self {
        self.model_name = Some(model_name.into());
        self
    }

    pub fn with_toolkit_handle(mut self, toolkit_handle: AgentToolkitHandle) -> Self {
        self.toolkit = toolkit_handle.current();
        self.toolkit_handle = toolkit_handle;
        self
    }

    pub fn with_provider_id(mut self, provider_id: impl Into<String>) -> Self {
        self.provider_id = Some(provider_id.into());
        self
    }

    pub fn with_model_call_sections(mut self, sections: Vec<ModelCallSection>) -> Self {
        self.model_call_sections = sections;
        self
    }

    pub fn with_model_call_projector(mut self, projector: Arc<dyn ModelCallProjector>) -> Self {
        self.model_call_projector = Some(projector);
        self
    }

    pub fn with_tool_before_boundary(mut self, boundary: Arc<dyn ToolBeforeBoundary>) -> Self {
        self.tool_before_boundary = Some(boundary);
        self
    }

    pub fn with_tool_observer_boundary(mut self, boundary: Arc<dyn ToolObserverBoundary>) -> Self {
        self.tool_observer_boundary = Some(boundary);
        self
    }

    pub fn with_permission_request_boundary(
        mut self,
        boundary: Arc<dyn PermissionRequestBoundary>,
    ) -> Self {
        self.permission_request_boundary = Some(boundary);
        self
    }

    pub fn with_approval_authority(
        mut self,
        authority: Arc<rua_security::approval_authority::ApprovalAuthority>,
    ) -> Self {
        self.approval_authority = Some(authority);
        self
    }

    pub fn with_cancel(mut self, cancel: CancellationToken) -> Self {
        self.cancel = cancel;
        self
    }

    pub fn with_run_control(
        mut self,
        run_control: Option<watch::Receiver<rua_core::RunControl>>,
    ) -> Self {
        self.run_control = run_control;
        self
    }

    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        let context_window_tokens = self
            .model
            .model_info()
            .context_window
            .map(|value| value as usize);
        assemble_tool_definitions(
            &self.current_toolkit(),
            &self.config.tool_disclosure,
            context_window_tokens,
        )
        .tools
    }

    pub fn current_toolkit(&self) -> Arc<AgentToolkit> {
        self.toolkit_handle.current()
    }

    pub fn llm_request_metadata(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut metadata = serde_json::Map::new();
        for (key, value) in &self.tool_ctx.metadata {
            metadata.insert(key.clone(), value.clone());
        }
        metadata
            .entry("agent_id".to_string())
            .or_insert_with(|| serde_json::Value::String(self.tool_ctx.agent_id.clone()));
        metadata
            .entry("session_id".to_string())
            .or_insert_with(|| serde_json::Value::String(self.tool_ctx.session_id.clone()));
        metadata
            .entry("chat_id".to_string())
            .or_insert_with(|| serde_json::Value::String(self.tool_ctx.session_id.clone()));
        metadata
            .entry("run_id".to_string())
            .or_insert_with(|| serde_json::Value::String(self.run_id.clone()));
        metadata
            .entry("source".to_string())
            .or_insert_with(|| serde_json::Value::String("react_agent".to_string()));
        metadata
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    pub fn is_interjected(&self) -> bool {
        self.run_control
            .as_ref()
            .map(|rx| rx.borrow().is_interject())
            .unwrap_or(false)
    }

    pub fn should_exit_at_boundary(&self) -> bool {
        self.run_control
            .as_ref()
            .map(|rx| rx.borrow().should_exit_at_boundary())
            .unwrap_or(false)
    }

    pub fn build_initial_messages(
        &self,
        user_messages: Vec<ModelDialogueItem>,
    ) -> Vec<ModelDialogueItem> {
        user_messages
    }
}
