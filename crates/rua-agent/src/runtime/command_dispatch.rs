use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use react_agent::agent_event_to_runtime_event;
use rua_contract::agent::{
    RuntimeCommandDispatcher, RuntimeCommandInput, RuntimeCommandStream, RuntimeInputIntercept,
};
use rua_core::event::RuntimeEvent;
use rua_core::id::{MessageId, SessionId};
use rua_core::message::ContentPart;
use rua_core::{Result, RuaError};

#[derive(Debug, Clone)]
struct PersistedAcpBinding {
    runner: String,
    runner_session_id: Option<String>,
    cwd: Option<String>,
}

enum AcpSessionResolution {
    None,
    Ready(rua_command::AcpChatSession),
    Error(String),
}

pub struct CommandRuntimeDispatcherAdapter {
    daemon_registry: rua_command::DaemonCommandRegistry,
    control_registry: rua_command::ControlCommandRegistry,
    session_canceller: Arc<dyn rua_command::SessionCanceller>,
    approval_service: Arc<dyn rua_command::ApprovalService>,
    queue_clearer: Arc<dyn rua_command::QueueClearer>,
    skills_lister: Arc<dyn rua_command::SkillsLister>,
    workspace_reloader: Arc<dyn rua_command::WorkspaceReloader>,
    acp_chat_manager: Arc<rua_command::AcpChatSessionManager>,
    acp_chat_controller: Arc<dyn rua_command::AcpChatController>,
    acp_service: Arc<rua_acp::service::AcpService>,
    data_dir: PathBuf,
    workspace_dir: PathBuf,
    started_at: std::time::Instant,
    exit_fn: Option<Arc<dyn Fn(i32) + Send + Sync>>,
}

impl CommandRuntimeDispatcherAdapter {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        daemon_registry: rua_command::DaemonCommandRegistry,
        control_registry: rua_command::ControlCommandRegistry,
        session_canceller: Arc<dyn rua_command::SessionCanceller>,
        approval_service: Arc<dyn rua_command::ApprovalService>,
        queue_clearer: Arc<dyn rua_command::QueueClearer>,
        skills_lister: Arc<dyn rua_command::SkillsLister>,
        workspace_reloader: Arc<dyn rua_command::WorkspaceReloader>,
        acp_chat_manager: Arc<rua_command::AcpChatSessionManager>,
        acp_chat_controller: Arc<dyn rua_command::AcpChatController>,
        acp_service: Arc<rua_acp::service::AcpService>,
        data_dir: PathBuf,
        workspace_dir: PathBuf,
        started_at: std::time::Instant,
        exit_fn: Option<Arc<dyn Fn(i32) + Send + Sync>>,
    ) -> Self {
        Self {
            daemon_registry,
            control_registry,
            session_canceller,
            approval_service,
            queue_clearer,
            skills_lister,
            workspace_reloader,
            acp_chat_manager,
            acp_chat_controller,
            acp_service,
            data_dir,
            workspace_dir,
            started_at,
            exit_fn,
        }
    }
}

#[async_trait]
impl RuntimeCommandDispatcher for CommandRuntimeDispatcherAdapter {
    async fn dispatch(&self, input: RuntimeCommandInput) -> Result<Option<RuntimeCommandStream>> {
        if let Some(cmd) = self.daemon_registry.get(&input.name).cloned() {
            let session_id = input.session_id.clone();
            let outcome = cmd
                .execute(
                    &rua_command::DaemonContext {
                        data_dir: self.data_dir.clone(),
                        started_at: self.started_at,
                        workspace_reloader: Arc::clone(&self.workspace_reloader),
                        exit_fn: self.exit_fn.clone(),
                        approval_service: Arc::clone(&self.approval_service),
                        locale: input.locale,
                    },
                    &input.args,
                )
                .await;
            return Ok(Some(command_outcome_to_stream(outcome, &session_id)));
        }

        if let Some(cmd) = self.control_registry.get(&input.name).cloned() {
            let session_id = input.session_id.clone();
            let outcome = cmd
                .execute(
                    &rua_command::ControlContext {
                        session_id: input.session_id,
                        user_id: input.user_id,
                        channel: input.channel,
                        session_canceller: Arc::clone(&self.session_canceller),
                        approval_service: Arc::clone(&self.approval_service),
                        queue_clearer: Arc::clone(&self.queue_clearer),
                        skills_lister: Arc::clone(&self.skills_lister),
                        acp_chat_manager: Arc::clone(&self.acp_chat_manager),
                        acp_chat_controller: Arc::clone(&self.acp_chat_controller),
                        locale: input.locale,
                    },
                    &input.args,
                )
                .await;
            return Ok(Some(command_outcome_to_stream(outcome, &session_id)));
        }

        Ok(None)
    }

    async fn intercept_input(
        &self,
        input: RuntimeInputIntercept,
    ) -> Result<Option<RuntimeCommandStream>> {
        self.try_intercept_acp_chat(&input).await
    }
}

impl CommandRuntimeDispatcherAdapter {
    async fn try_intercept_acp_chat(
        &self,
        input: &RuntimeInputIntercept,
    ) -> Result<Option<RuntimeCommandStream>> {
        let session = match self.resolve_acp_session(input).await {
            AcpSessionResolution::None => return Ok(None),
            AcpSessionResolution::Ready(session) => session,
            AcpSessionResolution::Error(message) => {
                return Ok(Some(acp_error_stream(input.session_id.clone(), message)));
            }
        };

        if let Some((cmd_name, _args)) = rua_command::dispatch::parse_command(&input.text) {
            if rua_command::is_acp_reserved_command(cmd_name) {
                return Ok(None);
            }
        }
        self.acp_chat_manager.touch(&input.session_id).await;

        let show_resume_hint = self
            .acp_chat_manager
            .take_resume_hint(&input.session_id)
            .await;

        let sid = SessionId::new(input.session_id.clone());
        let runner = session.runner.clone();
        let acp_service = Arc::clone(&self.acp_service);
        let prompt = render_acp_prompt(&input.content);
        let text_message_id = MessageId::new(format!("msg-acp-{}-text", uuid::Uuid::new_v4()));
        let reasoning_message_id =
            MessageId::new(format!("msg-acp-{}-reasoning", uuid::Uuid::new_v4()));

        let (tx, rx) =
            tokio::sync::mpsc::channel::<std::result::Result<RuntimeEvent, RuaError>>(64);

        let sid_start = sid.clone();
        let tx_run = tx.clone();
        tokio::spawn(async move {
            let _ = tx_run
                .send(Ok(RuntimeEvent::Start {
                    session_id: sid_start.clone(),
                    agent_id: None,
                }))
                .await;

            if show_resume_hint {
                let banner = format!(
                    "> ℹ️ 已恢复 `{runner}` 会话。runner 可能先回放之前的对话再接上新的回复,若开头出现历史内容属于正常现象。\n\n---\n\n"
                );
                let _ = tx_run
                    .send(Ok(RuntimeEvent::TextDelta {
                        delta: banner,
                        message_id: Some(text_message_id.clone()),
                    }))
                    .await;
            }

            let mut event_rx = match acp_service
                .run_turn(
                    sid_start.clone(),
                    &runner,
                    &prompt,
                    rua_acp::service::TurnMode::Message,
                    None,
                )
                .await
            {
                Ok(rx) => rx,
                Err(e) => {
                    let _ = tx_run
                        .send(Ok(RuntimeEvent::Error {
                            error: RuaError::Runtime {
                                message: format!("ACP turn failed: {e}"),
                            },
                        }))
                        .await;
                    let _ = tx_run
                        .send(Ok(RuntimeEvent::End {
                            finish_reason: Some("acp_error".into()),
                        }))
                        .await;
                    return;
                }
            };

            let mut saw_runner_event = false;
            while let Some(mut event) = event_rx.recv().await {
                if matches!(event, RuntimeEvent::End { .. }) {
                    break;
                }
                match &mut event {
                    RuntimeEvent::TextDelta { message_id, .. } => {
                        *message_id = Some(text_message_id.clone());
                    }
                    RuntimeEvent::ThinkingDelta { message_id, .. } => {
                        *message_id = Some(reasoning_message_id.clone());
                    }
                    _ => {}
                }
                saw_runner_event = true;
                if tx_run.send(Ok(event)).await.is_err() {
                    break;
                }
            }

            if !saw_runner_event {
                let _ = tx_run
                    .send(Ok(RuntimeEvent::Error {
                        error: RuaError::Runtime {
                            message: "ACP turn failed before producing output".to_string(),
                        },
                    }))
                    .await;
                let _ = tx_run
                    .send(Ok(RuntimeEvent::End {
                        finish_reason: Some("acp_error".into()),
                    }))
                    .await;
                return;
            }

            let _ = tx_run
                .send(Ok(RuntimeEvent::End {
                    finish_reason: Some("acp_chat".into()),
                }))
                .await;
        });

        Ok(Some(Box::pin(futures::stream::unfold(
            rx,
            |mut rx| async move { rx.recv().await.map(|event| (event, rx)) },
        ))))
    }

    async fn resolve_acp_session(&self, input: &RuntimeInputIntercept) -> AcpSessionResolution {
        if let Some(session) = self.acp_chat_manager.get(&input.session_id).await {
            return AcpSessionResolution::Ready(session);
        }

        let Some(binding) = self.load_persisted_acp_binding(&input.session_id).await else {
            return AcpSessionResolution::None;
        };

        if let Some((cmd_name, _args)) = rua_command::dispatch::parse_command(&input.text) {
            if rua_command::is_acp_reserved_command(cmd_name) {
                return AcpSessionResolution::None;
            }
        }

        let Some(runner_session_id) = binding.runner_session_id.clone() else {
            return AcpSessionResolution::Error(format!(
                "ACP passthrough binding for this chat is stale: runner `{}` did not persist a resumable session id. Exit ACP mode and enter again.",
                binding.runner
            ));
        };

        let sid = SessionId::new(input.session_id.clone());
        let acp_sid = rua_acp::AcpSessionId::from(runner_session_id.clone());
        match self
            .acp_service
            .open_session_resume(sid, &binding.runner, acp_sid, binding.cwd.clone())
            .await
        {
            Ok(opened) => {
                self.acp_chat_manager
                    .register_with_runner_session(
                        input.session_id.clone(),
                        binding.runner,
                        Some(opened.session_id),
                        binding
                            .cwd
                            .map(PathBuf::from)
                            .unwrap_or_else(|| self.workspace_dir.clone()),
                    )
                    .await;
                match self.acp_chat_manager.get(&input.session_id).await {
                    Some(session) => AcpSessionResolution::Ready(session),
                    None => AcpSessionResolution::Error(
                        "ACP passthrough resume succeeded but session registration was not visible"
                            .to_string(),
                    ),
                }
            }
            Err(error) => AcpSessionResolution::Error(format!(
                "ACP passthrough session `{runner_session_id}` on runner `{}` is no longer connected and automatic resume failed: {error}. Exit ACP mode or resume a valid ACP session.",
                binding.runner
            )),
        }
    }

    async fn load_persisted_acp_binding(&self, chat_id: &str) -> Option<PersistedAcpBinding> {
        let store = rua_session::WorkspaceSessionStore::new(&self.workspace_dir);
        let header = match store.load_chat_header(chat_id).await {
            Ok(Some(header)) => header,
            Ok(None) => return None,
            Err(error) => {
                tracing::warn!(
                    chat_id,
                    error = %error,
                    "failed to load chat header while resolving ACP passthrough binding"
                );
                return None;
            }
        };
        persisted_acp_binding_from_extra(&header.extra)
    }
}

fn render_acp_prompt(content: &[ContentPart]) -> String {
    let mut rendered = Vec::new();
    for (index, part) in content.iter().enumerate() {
        match part {
            ContentPart::Text { text } if !text.is_empty() => rendered.push(text.clone()),
            ContentPart::File { url, name, .. } if !next_text_mentions_url(content, index, url) => {
                rendered.push(format!(
                    "[用户上传文件: {}]\n路径: {}",
                    fallback_label(name, "file"),
                    url
                ));
            }
            ContentPart::Image { url, alt, .. } if !next_text_mentions_url(content, index, url) => {
                rendered.push(format!(
                    "[用户上传图片: {}]\n路径: {}",
                    fallback_label(alt.as_deref().unwrap_or(""), "image"),
                    url
                ));
            }
            ContentPart::Audio { url, .. } if !next_text_mentions_url(content, index, url) => {
                rendered.push(format!("[用户上传音频]\n路径: {url}"));
            }
            ContentPart::Video { url, .. } if !next_text_mentions_url(content, index, url) => {
                rendered.push(format!("[用户上传视频]\n路径: {url}"));
            }
            _ => {}
        }
    }
    rendered.join("\n")
}

fn next_text_mentions_url(content: &[ContentPart], index: usize, url: &str) -> bool {
    content
        .get(index + 1)
        .is_some_and(|part| matches!(part, ContentPart::Text { text } if text.contains(url)))
}

fn fallback_label<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback
    } else {
        trimmed
    }
}

fn persisted_acp_binding_from_extra(
    extra: &serde_json::Map<String, serde_json::Value>,
) -> Option<PersistedAcpBinding> {
    let binding = extra.get("acp_binding")?.as_object()?;
    let runner = binding
        .get("runner")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();
    let runner_session_id = binding
        .get("runner_session_id")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let cwd = binding
        .get("cwd")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    Some(PersistedAcpBinding {
        runner,
        runner_session_id,
        cwd,
    })
}

fn acp_error_stream(session_id: String, message: String) -> RuntimeCommandStream {
    Box::pin(futures::stream::iter(vec![
        Ok(RuntimeEvent::Start {
            session_id: SessionId::new(session_id),
            agent_id: None,
        }),
        Ok(RuntimeEvent::Error {
            error: RuaError::Runtime { message },
        }),
        Ok(RuntimeEvent::End {
            finish_reason: Some("acp_error".into()),
        }),
    ]))
}

fn command_outcome_to_stream(
    outcome: rua_command::CommandOutcome,
    session_id: &str,
) -> RuntimeCommandStream {
    let sid = SessionId::new(session_id);
    let text_message_id = MessageId::new(format!("msg-command-{}-text", uuid::Uuid::new_v4()));
    let reasoning_message_id =
        MessageId::new(format!("msg-command-{}-reasoning", uuid::Uuid::new_v4()));
    let mut events: Vec<std::result::Result<RuntimeEvent, RuaError>> =
        Vec::with_capacity(outcome.events.len() + 2);
    events.push(Ok(RuntimeEvent::Start {
        session_id: sid.clone(),
        agent_id: None,
    }));
    for event in outcome.events {
        if let Some(mut runtime_event) = agent_event_to_runtime_event(event, &sid) {
            match &mut runtime_event {
                RuntimeEvent::TextDelta { message_id, .. } => {
                    *message_id = Some(text_message_id.clone());
                }
                RuntimeEvent::ThinkingDelta { message_id, .. } => {
                    *message_id = Some(reasoning_message_id.clone());
                }
                _ => {}
            }
            events.push(Ok(runtime_event));
        }
    }
    events.push(Ok(RuntimeEvent::End {
        finish_reason: Some("command".into()),
    }));
    Box::pin(futures::stream::iter(events))
}
