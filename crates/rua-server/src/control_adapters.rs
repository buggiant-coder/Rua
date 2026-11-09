use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use rua_core::ChatAddress;
use rua_security::{ApprovalAuthority, ApprovalDecision};
use rua_workspace::{ActiveSessionTracker, WorkspaceRegistry};

use crate::services::conversation_authority::{ConversationActiveTurnPhase, ConversationAuthority};
use crate::streaming::ActiveStreamRegistry;

pub struct StreamCancelAdapter {
    streams: Arc<ActiveStreamRegistry>,
    agent_id: String,
    runtime_sessions: Option<Arc<ActiveSessionTracker>>,
    acp_chat_manager: Option<Arc<rua_command::AcpChatSessionManager>>,
    acp_service: Option<Arc<rua_acp::service::AcpService>>,
}

impl StreamCancelAdapter {
    pub fn new(streams: Arc<ActiveStreamRegistry>, agent_id: impl Into<String>) -> Self {
        Self {
            streams,
            agent_id: agent_id.into(),
            runtime_sessions: None,
            acp_chat_manager: None,
            acp_service: None,
        }
    }

    pub fn with_acp(
        mut self,
        acp_chat_manager: Arc<rua_command::AcpChatSessionManager>,
        acp_service: Arc<rua_acp::service::AcpService>,
    ) -> Self {
        self.acp_chat_manager = Some(acp_chat_manager);
        self.acp_service = Some(acp_service);
        self
    }

    pub fn with_runtime_sessions(mut self, runtime_sessions: Arc<ActiveSessionTracker>) -> Self {
        self.runtime_sessions = Some(runtime_sessions);
        self
    }
}

#[async_trait]
impl rua_command::SessionCanceller for StreamCancelAdapter {
    async fn cancel_session(&self, channel_id: &str, session_id: &str) -> bool {
        if channel_id == "console" {
            if let (Some(mgr), Some(service)) = (&self.acp_chat_manager, &self.acp_service) {
                if let Some(session) = mgr.get(session_id).await {
                    let sid = rua_core::id::SessionId::new(session_id);
                    let _ = service.cancel_turn(&sid, &session.runner).await;
                    return true;
                }
            }
            return self
                .streams
                .cancel_all_priorities(channel_id, &ChatAddress::agent(&self.agent_id, session_id))
                .await
                > 0;
        }
        match &self.runtime_sessions {
            Some(sessions) => sessions.cancel_session(session_id).await,
            None => false,
        }
    }

    async fn active_session_count(&self) -> usize {
        let chat_count = self.streams.active_count().await;
        let runtime_count = match &self.runtime_sessions {
            Some(sessions) => sessions.active_count().await,
            None => 0,
        };
        chat_count + runtime_count
    }
}

#[async_trait]
impl rua_channel::core::control_handlers::SessionCanceller for StreamCancelAdapter {
    async fn cancel_session(&self, session_id: &str) -> bool {
        match &self.runtime_sessions {
            Some(sessions) => sessions.cancel_session(session_id).await,
            None => false,
        }
    }

    async fn active_session_count(&self) -> usize {
        match &self.runtime_sessions {
            Some(sessions) => sessions.active_count().await,
            None => 0,
        }
    }
}

pub struct NoopQueueClearer;

#[async_trait]
impl rua_command::QueueClearer for NoopQueueClearer {
    async fn clear_queue(&self, _channel_id: &str, _session_id: &str) -> usize {
        0
    }
}

pub struct ApprovalAdapter {
    inner: Arc<ApprovalAuthority>,
    agent_id: String,
    conversation_authority: Option<Arc<ConversationAuthority>>,
}

impl ApprovalAdapter {
    pub fn new(inner: Arc<ApprovalAuthority>, agent_id: impl Into<String>) -> Self {
        Self {
            inner,
            agent_id: agent_id.into(),
            conversation_authority: None,
        }
    }

    pub fn with_conversation_authority(
        mut self,
        conversation_authority: Arc<ConversationAuthority>,
    ) -> Self {
        self.conversation_authority = Some(conversation_authority);
        self
    }

    async fn prepare_resume(&self, request: &rua_security::ApprovalRequest) {
        let Some(authority) = self.conversation_authority.as_ref() else {
            return;
        };
        let Some(address) = request.owner.as_chat_address() else {
            return;
        };
        let _ = authority
            .transition_active_turn_phase(
                address,
                Some(&request.turn_id),
                ConversationActiveTurnPhase::RunningTool,
            )
            .await;
    }

    fn address(&self, session_id: &str) -> ChatAddress {
        ChatAddress::agent(&self.agent_id, session_id)
    }

    async fn oldest_pending(&self, session_id: &str) -> Option<rua_security::ApprovalRequest> {
        let mut pending = self
            .inner
            .list_pending_for_address(&self.address(session_id))
            .await;
        pending.extend(
            self.inner
                .list_pending_for_runtime_session(&self.agent_id, session_id)
                .await,
        );
        pending.into_iter().min_by_key(|request| request.created_at)
    }
}

#[async_trait]
impl rua_command::ApprovalService for ApprovalAdapter {
    async fn approve_pending(&self, session_id: &str) -> bool {
        let Some(request) = self.oldest_pending(session_id).await else {
            return false;
        };
        self.prepare_resume(&request).await;
        self.inner
            .decide(
                &request.id,
                ApprovalDecision::RunOnce {
                    decided_by: "daemon-command".into(),
                },
            )
            .await
            .is_ok()
    }

    async fn reject_pending(&self, session_id: &str) -> bool {
        let Some(request) = self.oldest_pending(session_id).await else {
            return false;
        };
        self.prepare_resume(&request).await;
        self.inner
            .decide(
                &request.id,
                ApprovalDecision::Reject {
                    decided_by: "daemon-command".into(),
                },
            )
            .await
            .is_ok()
    }

    async fn approve_request(&self, request_id: &str, decided_by: &str) -> bool {
        if let Some(request) = self.inner.request(request_id).await {
            self.prepare_resume(&request).await;
        }
        self.inner
            .decide(
                request_id,
                ApprovalDecision::RunOnce {
                    decided_by: decided_by.to_string(),
                },
            )
            .await
            .is_ok()
    }

    async fn reject_request(&self, request_id: &str, decided_by: &str) -> bool {
        if let Some(request) = self.inner.request(request_id).await {
            self.prepare_resume(&request).await;
        }
        self.inner
            .decide(
                request_id,
                ApprovalDecision::Reject {
                    decided_by: decided_by.to_string(),
                },
            )
            .await
            .is_ok()
    }
}

#[async_trait]
impl rua_channel::core::control_handlers::ApprovalService for ApprovalAdapter {
    async fn approve_pending(&self, session_id: &str) -> bool {
        <Self as rua_command::ApprovalService>::approve_pending(self, session_id).await
    }

    async fn reject_pending(&self, session_id: &str) -> bool {
        <Self as rua_command::ApprovalService>::reject_pending(self, session_id).await
    }
}

pub struct WorkspaceReloadAdapter {
    registry: Arc<dyn WorkspaceRegistry>,
}

impl WorkspaceReloadAdapter {
    pub fn new(registry: Arc<dyn WorkspaceRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl rua_command::WorkspaceReloader for WorkspaceReloadAdapter {
    async fn reload_all(&self) -> rua_core::Result<()> {
        let runtimes = self
            .registry
            .list()
            .await
            .map_err(|e| rua_core::RuaError::Runtime {
                message: format!("workspace reload failed: {e}"),
            })?;
        let mut failed = Vec::new();
        for summary in &runtimes {
            if let Err(e) = self.registry.reload_agent(&summary.agent_id).await {
                failed.push(summary.agent_id.clone());
                tracing::warn!(
                    agent_id = %summary.agent_id,
                    error = %e,
                    "failed to reload agent during workspace reload"
                );
            }
        }
        if !failed.is_empty() {
            return Err(rua_core::RuaError::Runtime {
                message: format!("Workspace rebuild failed for Agents: {}", failed.join(", ")),
            });
        }
        tracing::info!("workspace reload completed ({} agents)", runtimes.len());
        Ok(())
    }
}

#[async_trait]
impl rua_channel::core::control_handlers::WorkspaceReloader for WorkspaceReloadAdapter {
    async fn reload_all(&self) -> rua_core::Result<()> {
        <Self as rua_command::WorkspaceReloader>::reload_all(self).await
    }
}

pub struct SkillsListerAdapter {
    workspace_dir: Option<PathBuf>,
}

impl SkillsListerAdapter {
    pub fn new(workspace_dir: Option<PathBuf>) -> Self {
        Self { workspace_dir }
    }
}

#[async_trait]
impl rua_command::SkillsLister for SkillsListerAdapter {
    async fn list_skills(&self, channel_id: &str, locale: &str) -> String {
        let ctx = rua_session::commands::SkillsContext {
            workspace_dir: self.workspace_dir.clone(),
            channel_id: channel_id.to_string(),
            locale: locale.to_string(),
        };
        rua_session::commands::handle_skills_command(&ctx)
    }
}
