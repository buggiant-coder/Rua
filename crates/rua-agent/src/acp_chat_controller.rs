use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use rua_acp::client::AcpClientAdapter;
use rua_acp::service::AcpService;
use rua_command::{AcpChatController, AcpChatResumeResult, AcpChatSessionManager};

pub struct AcpChatControllerImpl {
    service: Arc<AcpService>,
    manager: Arc<AcpChatSessionManager>,
    default_cwd: PathBuf,
}

impl AcpChatControllerImpl {
    pub fn new(
        service: Arc<AcpService>,
        manager: Arc<AcpChatSessionManager>,
        default_cwd: PathBuf,
    ) -> Self {
        Self {
            service,
            manager,
            default_cwd,
        }
    }

    fn resolve_cwd(&self, user_cwd: Option<&str>) -> PathBuf {
        match user_cwd {
            Some(cwd) if !cwd.trim().is_empty() => PathBuf::from(cwd.trim()),
            _ => self.default_cwd.clone(),
        }
    }
}

#[async_trait]
impl AcpChatController for AcpChatControllerImpl {
    async fn enter(&self, chat_id: &str, runner: &str, cwd: Option<&str>) -> String {
        let resolved_cwd = self.resolve_cwd(cwd);
        let sid = rua_core::id::SessionId::new(chat_id);

        match self
            .service
            .open_session(
                sid,
                runner,
                Some(resolved_cwd.to_string_lossy().to_string()),
            )
            .await
        {
            Ok(runner_session_id) => {
                self.manager
                    .register_with_runner_session(
                        chat_id.to_string(),
                        runner.to_string(),
                        Some(runner_session_id),
                        resolved_cwd.clone(),
                    )
                    .await;
                format!(
                    "Entered ACP chat mode with `{}` (cwd: {}). Send a message to talk directly to the runner. Use /noacp to exit.",
                    runner,
                    resolved_cwd.display()
                )
            }
            Err(e) => format!("Failed to enter ACP chat mode with `{}`: {}", runner, e),
        }
    }

    async fn exit(&self, chat_id: &str) -> String {
        if !self.manager.contains_chat(chat_id).await {
            return "Not in ACP chat mode".to_string();
        }
        let session = self.manager.get(chat_id).await;
        self.manager.unregister(chat_id).await;
        if let Some(session) = session {
            let sid = rua_core::id::SessionId::new(chat_id);
            let _ = self.service.close_chat_session(&sid, &session.runner).await;
        }
        "Exited ACP chat mode".to_string()
    }

    async fn list_sessions(&self, chat_id: &str, runner: &str, cwd_arg: Option<&str>) -> String {
        let Some(config) = self.service.agent_config(runner) else {
            return format!("ACP runner `{}` not configured", runner);
        };
        if !config.enabled {
            return format!("ACP runner `{}` is disabled", runner);
        }

        let (filter_cwd, scope_label): (Option<String>, String) = match cwd_arg {
            Some("all") => (None, "all".into()),
            Some(path) => (Some(path.to_string()), path.to_string()),
            None => {
                let inferred = match self.manager.get(chat_id).await {
                    Some(session) => session.cwd.to_string_lossy().to_string(),
                    None => self.default_cwd.to_string_lossy().to_string(),
                };
                (Some(inferred.clone()), inferred)
            }
        };

        match AcpClientAdapter::list_sessions(&config, filter_cwd).await {
            Ok(sessions) if sessions.is_empty() => {
                format!(
                    "No ACP sessions reported by `{}` (scope: {}). Use `/acph {} all` to list every session.",
                    runner, scope_label, runner
                )
            }
            Ok(sessions) => {
                let mut lines = vec![format!(
                    "**ACP sessions for `{}`** (scope: {})\n",
                    runner, scope_label
                )];
                for info in &sessions {
                    let title = info.title.clone().unwrap_or_else(|| "(untitled)".into());
                    let updated = info.updated_at.clone().unwrap_or_else(|| "-".into());
                    lines.push(format!(
                        "- `{}` — {} — {} ({})",
                        info.session_id,
                        title,
                        info.cwd.display(),
                        updated
                    ));
                }
                if scope_label != "all" {
                    lines.push(String::new());
                    lines.push(format!(
                        "_Use `/acph {} all` to list every session the runner knows._",
                        runner
                    ));
                }
                lines.join("\n")
            }
            Err(e) => format!(
                "Failed to list ACP sessions from `{}`: {}. Runner may not support session listing.",
                runner, e
            ),
        }
    }

    async fn resume(
        &self,
        chat_id: &str,
        runner: &str,
        acp_session_id: &str,
        cwd: Option<&str>,
    ) -> String {
        self.resume_with_history_preview(chat_id, runner, acp_session_id, cwd)
            .await
            .message
    }

    async fn resume_with_history_preview(
        &self,
        chat_id: &str,
        runner: &str,
        acp_session_id: &str,
        cwd: Option<&str>,
    ) -> AcpChatResumeResult {
        let resolved_cwd = self.resolve_cwd(cwd);
        let sid = rua_core::id::SessionId::new(chat_id);
        let acp_sid = rua_acp::AcpSessionId::from(acp_session_id.to_string());

        match self
            .service
            .open_session_resume(
                sid,
                runner,
                acp_sid,
                Some(resolved_cwd.to_string_lossy().to_string()),
            )
            .await
        {
            Ok(opened) => {
                self.manager
                    .register_with_runner_session(
                        chat_id.to_string(),
                        runner.to_string(),
                        Some(opened.session_id.clone()),
                        resolved_cwd.clone(),
                    )
                    .await;
                AcpChatResumeResult {
                    message: format!(
                        "Resumed ACP session `{}` on `{}` (cwd: {}). Send a message to continue. Use /noacp to exit.",
                        acp_session_id,
                        runner,
                        resolved_cwd.display()
                    ),
                    history_preview: opened.history_preview,
                }
            }
            Err(e) => AcpChatResumeResult {
                message: format_resume_error(runner, &e),
                history_preview: None,
            },
        }
    }
}

fn format_resume_error(runner: &str, error: &rua_acp::core::AcpError) -> String {
    match error {
        rua_acp::core::AcpError::SessionAlreadyExists(_) => format!(
            "Failed to resume ACP session on `{runner}`: this chat already has an active ACP session. Exit ACP mode or try again."
        ),
        _ => format!("Failed to resume ACP session on `{runner}`: {error}."),
    }
}
