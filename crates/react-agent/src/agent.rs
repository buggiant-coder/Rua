use std::pin::Pin;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use agent_types::{AgentEvent, FinishReason, ModelDialogueItem};
use futures::Stream;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::config::ReactConfig;
use crate::context::AgentContext;
use crate::error::AgentError;
use crate::hooks::{AgentHook, NoopHook};
use crate::react_loop::run_react_loop;
use crate::toolkit::AgentToolkit;
use crate::toolkit::AgentToolkitHandle;

pub type AgentEventStream = Pin<Box<dyn Stream<Item = AgentEvent> + Send>>;

pub struct ReactAgent {
    pub ctx: AgentContext,
}

impl ReactAgent {
    pub fn new(ctx: AgentContext) -> Self {
        Self { ctx }
    }

    pub fn builder(model: Arc<dyn llm_multiplex::ChatModel>) -> ReactAgentBuilder {
        ReactAgentBuilder::new(model)
    }

    pub fn run_stream(self, user_messages: Vec<ModelDialogueItem>) -> AgentEventStream {
        let messages = self.ctx.build_initial_messages(user_messages);
        let run_id = self.ctx.run_id.clone();
        let model_name = self.ctx.model_name.clone();
        let cancel = self.ctx.cancel.clone();
        let cancel_for_spawn = cancel.clone();
        let hook = self.ctx.hook.clone();

        let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentEvent>(64);

        let ctx = self.ctx;
        let iteration_counter = Arc::new(AtomicU32::new(0));
        let iteration_counter_for_loop = iteration_counter.clone();
        let loop_handle = tokio::spawn(async move {
            let total_timeout = ctx.config.total_timeout();
            let loop_future =
                run_react_loop(&ctx, messages, tx.clone(), &iteration_counter_for_loop);
            tokio::pin!(loop_future);
            let result = tokio::select! {
                biased;
                result = &mut loop_future => result,
                _ = cancel_for_spawn.cancelled() => {
                    // Let the loop persist buffered output before on_finish publishes the terminal.
                    match tokio::time::timeout(std::time::Duration::from_secs(5), &mut loop_future).await {
                        Ok(result) => result,
                        Err(_) => {
                            tracing::warn!("Agent cancellation cleanup exceeded five seconds");
                            Err(AgentError::Cancelled)
                        }
                    }
                }
                _ = tokio::time::sleep(total_timeout), if !total_timeout.is_zero() => {
                    cancel_for_spawn.cancel();
                    if tokio::time::timeout(std::time::Duration::from_secs(5), &mut loop_future).await.is_err() {
                        tracing::warn!("Agent timeout cleanup exceeded five seconds");
                    }
                    Err(AgentError::Timeout {
                        timeout_secs: total_timeout.as_secs(),
                    })
                }
            };

            let iterations = iteration_counter.load(Ordering::Relaxed);
            hook.on_finish(iterations, &result).await;

            if let Err(ref e) = result {
                if !e.is_cancelled() {
                    let _ = tx.send(agent_error_event(e)).await;
                }
            }

            result
        });

        let cancel_for_drop = ctx_cancel_for_drop(cancel);

        Box::pin(async_stream::stream! {

            yield AgentEvent::Start {
                run_id: run_id.clone(),
                model: model_name.clone(),
                metadata: serde_json::Value::Null,
            };

            let mut terminal_emitted = false;


            while let Some(event) = rx.recv().await {
                if event.is_terminal() {
                    terminal_emitted = true;
                    yield event;
                    break;
                }
                yield event;
            }


            if !terminal_emitted {
                match loop_handle.await {
                    Ok(Ok(())) => {

                        yield AgentEvent::End {
                            finish_reason: FinishReason::Stop,
                            usage: None,
                            metadata: serde_json::Value::Null,
                        };
                    }
                    Ok(Err(AgentError::Cancelled)) => {
                        yield AgentEvent::End {
                            finish_reason: FinishReason::Cancelled,
                            usage: None,
                            metadata: serde_json::Value::Null,
                        };
                    }
                    Ok(Err(e)) => {
                        yield agent_error_event(&e);
                    }
                    Err(e) => {
                        yield AgentEvent::Error {
                            code: None,
                            message: format!("agent task panicked: {e}"),
                            retryable: false,
                            metadata: serde_json::Value::Null,
                        };
                    }
                }
            }


            drop(cancel_for_drop);
        })
    }
}

fn agent_error_event(error: &AgentError) -> AgentEvent {
    AgentEvent::Error {
        code: None,
        message: error.to_string(),
        retryable: error.is_retryable(),
        metadata: serde_json::Value::Null,
    }
}

fn ctx_cancel_for_drop(cancel: CancellationToken) -> CancelOnDrop {
    CancelOnDrop(cancel)
}

struct CancelOnDrop(CancellationToken);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

pub struct ReactAgentBuilder {
    model: Arc<dyn llm_multiplex::ChatModel>,
    toolkit: Arc<AgentToolkit>,
    toolkit_handle: Option<AgentToolkitHandle>,
    config: ReactConfig,
    hook: Arc<dyn AgentHook>,
    cancel: CancellationToken,
    run_control: Option<watch::Receiver<rua_core::RunControl>>,
    run_id: Option<String>,
    model_name: Option<String>,
    provider_id: Option<String>,
    model_call_sections: Vec<crate::model_call_context::ModelCallSection>,
    model_call_projector: Option<Arc<dyn crate::model_call_context::ModelCallProjector>>,
    tool_before_boundary: Option<Arc<dyn crate::ToolBeforeBoundary>>,
    tool_observer_boundary: Option<Arc<dyn crate::ToolObserverBoundary>>,
    permission_request_boundary: Option<Arc<dyn crate::PermissionRequestBoundary>>,
    tool_ctx: Option<rua_core::tool::ToolContext>,
    disabled_tools: Vec<String>,
    approval_authority: Option<Arc<rua_security::approval_authority::ApprovalAuthority>>,
}

impl ReactAgentBuilder {
    pub fn new(model: Arc<dyn llm_multiplex::ChatModel>) -> Self {
        Self {
            model,
            toolkit: Arc::new(AgentToolkit::new()),
            toolkit_handle: None,
            config: ReactConfig::default(),
            hook: Arc::new(NoopHook),
            cancel: CancellationToken::new(),
            run_control: None,
            run_id: None,
            model_name: None,
            provider_id: None,
            model_call_sections: Vec::new(),
            model_call_projector: None,
            tool_before_boundary: None,
            tool_observer_boundary: None,
            permission_request_boundary: None,
            tool_ctx: None,
            disabled_tools: Vec::new(),
            approval_authority: None,
        }
    }

    pub fn toolkit(mut self, toolkit: Arc<AgentToolkit>) -> Self {
        self.toolkit = toolkit;
        self
    }

    pub fn toolkit_handle(mut self, toolkit_handle: AgentToolkitHandle) -> Self {
        self.toolkit_handle = Some(toolkit_handle);
        self
    }

    pub fn config(mut self, config: ReactConfig) -> Self {
        self.config = config;
        self
    }

    pub fn hook(mut self, hook: Arc<dyn AgentHook>) -> Self {
        self.hook = hook;
        self
    }

    pub fn cancel(mut self, cancel: CancellationToken) -> Self {
        self.cancel = cancel;
        self
    }

    pub fn run_control(
        mut self,
        run_control: Option<watch::Receiver<rua_core::RunControl>>,
    ) -> Self {
        self.run_control = run_control;
        self
    }

    pub fn run_id(mut self, run_id: impl Into<String>) -> Self {
        self.run_id = Some(run_id.into());
        self
    }

    pub fn model_name(mut self, model_name: impl Into<String>) -> Self {
        self.model_name = Some(model_name.into());
        self
    }

    pub fn provider_id(mut self, provider_id: impl Into<String>) -> Self {
        self.provider_id = Some(provider_id.into());
        self
    }

    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.model_call_sections.retain(|section| {
            section.kind != crate::model_call_context::ModelCallSectionKind::AgentPromptContext
        });
        self.model_call_sections
            .push(crate::model_call_context::ModelCallSection::new(
                crate::model_call_context::ModelCallSectionKind::AgentPromptContext,
                prompt,
            ));
        self
    }

    pub fn model_call_section(
        mut self,
        section: crate::model_call_context::ModelCallSection,
    ) -> Self {
        self.model_call_sections.push(section);
        self
    }

    pub fn model_call_projector(
        mut self,
        projector: Arc<dyn crate::model_call_context::ModelCallProjector>,
    ) -> Self {
        self.model_call_projector = Some(projector);
        self
    }

    pub fn tool_before_boundary(mut self, boundary: Arc<dyn crate::ToolBeforeBoundary>) -> Self {
        self.tool_before_boundary = Some(boundary);
        self
    }

    pub fn tool_observer_boundary(
        mut self,
        boundary: Arc<dyn crate::ToolObserverBoundary>,
    ) -> Self {
        self.tool_observer_boundary = Some(boundary);
        self
    }

    pub fn permission_request_boundary(
        mut self,
        boundary: Arc<dyn crate::PermissionRequestBoundary>,
    ) -> Self {
        self.permission_request_boundary = Some(boundary);
        self
    }

    pub fn tool_ctx(mut self, tool_ctx: rua_core::tool::ToolContext) -> Self {
        self.tool_ctx = Some(tool_ctx);
        self
    }

    pub fn disabled_tools(mut self, tools: Vec<String>) -> Self {
        self.disabled_tools = tools;
        self
    }

    pub fn approval_authority(
        mut self,
        authority: Arc<rua_security::approval_authority::ApprovalAuthority>,
    ) -> Self {
        self.approval_authority = Some(authority);
        self
    }

    pub fn build(self) -> ReactAgent {
        let tool_ctx = self
            .tool_ctx
            .unwrap_or_else(|| rua_core::tool::ToolContext {
                agent_id: "default".into(),
                session_id: uuid::Uuid::new_v4().to_string(),
                working_dir: std::path::PathBuf::from("."),
                metadata: Default::default(),
                cancel: None,
                task_runtime: None,
                ..Default::default()
            });

        let hook: Arc<dyn AgentHook> = self.hook;

        let toolkit = if self.disabled_tools.is_empty() {
            self.toolkit
        } else {
            Arc::new(self.toolkit.create_filtered(&self.disabled_tools))
        };
        let toolkit_handle = self
            .toolkit_handle
            .unwrap_or_else(|| AgentToolkitHandle::new(Arc::clone(&toolkit)));
        if !self.disabled_tools.is_empty() {
            toolkit_handle.replace(Arc::clone(&toolkit));
        }

        let mut ctx = AgentContext::new(self.model, toolkit, self.config, hook, tool_ctx)
            .with_toolkit_handle(toolkit_handle)
            .with_cancel(self.cancel)
            .with_run_control(self.run_control);

        if let Some(run_id) = self.run_id {
            ctx = ctx.with_run_id(run_id);
        }
        if let Some(model_name) = self.model_name {
            ctx = ctx.with_model_name(model_name);
        }
        if let Some(provider_id) = self.provider_id {
            ctx = ctx.with_provider_id(provider_id);
        }
        if !self.model_call_sections.is_empty() {
            ctx = ctx.with_model_call_sections(self.model_call_sections);
        }
        if let Some(projector) = self.model_call_projector {
            ctx = ctx.with_model_call_projector(projector);
        }
        if let Some(boundary) = self.tool_before_boundary {
            ctx = ctx.with_tool_before_boundary(boundary);
        }
        if let Some(boundary) = self.tool_observer_boundary {
            ctx = ctx.with_tool_observer_boundary(boundary);
        }
        if let Some(boundary) = self.permission_request_boundary {
            ctx = ctx.with_permission_request_boundary(boundary);
        }
        if let Some(authority) = self.approval_authority {
            ctx = ctx.with_approval_authority(authority);
        }

        ReactAgent::new(ctx)
    }
}
