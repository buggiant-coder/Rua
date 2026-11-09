use std::sync::Arc;

use async_trait::async_trait;
use rua_command::*;

struct NoopCanceller;
#[async_trait]
impl SessionCanceller for NoopCanceller {
    async fn cancel_session(&self, _: &str, _: &str) -> bool {
        false
    }
    async fn active_session_count(&self) -> usize {
        0
    }
}

struct NoopApproval;
#[async_trait]
impl ApprovalService for NoopApproval {
    async fn approve_pending(&self, _: &str) -> bool {
        false
    }
    async fn reject_pending(&self, _: &str) -> bool {
        false
    }
}

struct NoopQueueClearer;
#[async_trait]
impl QueueClearer for NoopQueueClearer {
    async fn clear_queue(&self, _: &str, _: &str) -> usize {
        0
    }
}

struct FakeSkillsLister {
    response: String,
}

#[async_trait]
impl SkillsLister for FakeSkillsLister {
    async fn list_skills(&self, _channel_id: &str, _locale: &str) -> String {
        self.response.clone()
    }
}

struct ChannelAwareSkillsLister;

#[async_trait]
impl SkillsLister for ChannelAwareSkillsLister {
    async fn list_skills(&self, channel_id: &str, _locale: &str) -> String {
        format!("skills for channel: {channel_id}")
    }
}

struct MockAcpChatController;
#[async_trait]
impl AcpChatController for MockAcpChatController {
    async fn enter(&self, _chat_id: &str, _runner: &str, _cwd: Option<&str>) -> String {
        String::new()
    }
    async fn exit(&self, _chat_id: &str) -> String {
        String::new()
    }
    async fn list_sessions(&self, _chat_id: &str, _runner: &str, _cwd: Option<&str>) -> String {
        String::new()
    }
    async fn resume(
        &self,
        _chat_id: &str,
        _runner: &str,
        _acp_session_id: &str,
        _cwd: Option<&str>,
    ) -> String {
        String::new()
    }
}

fn extract_text(outcome: &CommandOutcome) -> String {
    outcome
        .events
        .iter()
        .filter_map(|e| match e {
            agent_types::AgentEvent::TextDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn skills_command_returns_lister_output() {
    let ctx = ControlContext {
        session_id: "s".to_string(),
        user_id: "u".to_string(),
        channel: "console".to_string(),
        session_canceller: Arc::new(NoopCanceller),
        approval_service: Arc::new(NoopApproval),
        queue_clearer: Arc::new(NoopQueueClearer),
        skills_lister: Arc::new(FakeSkillsLister {
            response: "**test-skill**: A test skill".to_string(),
        }),
        acp_chat_manager: Arc::new(rua_command::AcpChatSessionManager::new(
            std::time::Duration::from_secs(300),
        )),
        acp_chat_controller: Arc::new(MockAcpChatController),
        locale: "zh".into(),
    };

    let cmd = SkillsCommand;
    let outcome = cmd.execute(&ctx, "").await;

    let text = extract_text(&outcome);
    assert_eq!(text, "**test-skill**: A test skill");
}

#[tokio::test]
async fn skills_command_returns_empty_notice() {
    let ctx = ControlContext {
        session_id: "s".to_string(),
        user_id: "u".to_string(),
        channel: "console".to_string(),
        session_canceller: Arc::new(NoopCanceller),
        approval_service: Arc::new(NoopApproval),
        queue_clearer: Arc::new(NoopQueueClearer),
        skills_lister: Arc::new(FakeSkillsLister {
            response: "No skills are currently enabled for this channel.".to_string(),
        }),
        acp_chat_manager: Arc::new(rua_command::AcpChatSessionManager::new(
            std::time::Duration::from_secs(300),
        )),
        acp_chat_controller: Arc::new(MockAcpChatController),
        locale: "zh".into(),
    };

    let cmd = SkillsCommand;
    let outcome = cmd.execute(&ctx, "").await;

    let text = extract_text(&outcome);
    assert!(text.contains("No skills"));
}

#[tokio::test]
async fn skills_command_passes_channel_to_lister() {
    let ctx = ControlContext {
        session_id: "s".to_string(),
        user_id: "u".to_string(),
        channel: "wechat-123".to_string(),
        session_canceller: Arc::new(NoopCanceller),
        approval_service: Arc::new(NoopApproval),
        queue_clearer: Arc::new(NoopQueueClearer),
        skills_lister: Arc::new(ChannelAwareSkillsLister),
        acp_chat_manager: Arc::new(rua_command::AcpChatSessionManager::new(
            std::time::Duration::from_secs(300),
        )),
        acp_chat_controller: Arc::new(MockAcpChatController),
        locale: "zh".into(),
    };

    let cmd = SkillsCommand;
    let outcome = cmd.execute(&ctx, "").await;

    let text = extract_text(&outcome);
    assert_eq!(text, "skills for channel: wechat-123");
}

#[tokio::test]
async fn skills_command_registered_in_registry() {
    let mut registry = ControlCommandRegistry::new();
    registry.register(Arc::new(SkillsCommand));

    assert!(registry.get("skills").is_some());
}
