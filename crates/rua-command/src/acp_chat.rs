use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub struct AcpChatSession {
    pub chat_id: String,
    pub runner: String,
    pub runner_session_id: Option<String>,
    pub cwd: PathBuf,
    pub connected_at: Instant,
    pub last_active: Instant,

    pub resume_hint_pending: bool,
}

pub struct AcpChatSessionManager {
    chats: RwLock<HashMap<String, AcpChatSession>>,
    idle_timeout: Duration,
}

impl AcpChatSessionManager {
    pub fn new(idle_timeout: Duration) -> Self {
        Self {
            chats: RwLock::new(HashMap::new()),
            idle_timeout,
        }
    }

    pub async fn register(&self, chat_id: String, runner: String, cwd: PathBuf) {
        self.register_inner(chat_id, runner, None, cwd, false).await;
    }

    pub async fn register_with_runner_session(
        &self,
        chat_id: String,
        runner: String,
        runner_session_id: Option<String>,
        cwd: PathBuf,
    ) {
        self.register_inner(chat_id, runner, runner_session_id, cwd, false)
            .await;
    }

    pub async fn register_resumed(&self, chat_id: String, runner: String, cwd: PathBuf) {
        self.register_inner(chat_id, runner, None, cwd, true).await;
    }

    async fn register_inner(
        &self,
        chat_id: String,
        runner: String,
        runner_session_id: Option<String>,
        cwd: PathBuf,
        resume_hint_pending: bool,
    ) {
        let now = Instant::now();
        let chat = AcpChatSession {
            chat_id: chat_id.clone(),
            runner,
            runner_session_id,
            cwd,
            connected_at: now,
            last_active: now,
            resume_hint_pending,
        };
        self.chats.write().await.insert(chat_id, chat);
    }

    pub async fn take_resume_hint(&self, chat_id: &str) -> bool {
        let mut chats = self.chats.write().await;
        match chats.get_mut(chat_id) {
            Some(chat) if chat.resume_hint_pending => {
                chat.resume_hint_pending = false;
                true
            }
            _ => false,
        }
    }

    pub async fn touch(&self, chat_id: &str) {
        if let Some(chat) = self.chats.write().await.get_mut(chat_id) {
            chat.last_active = Instant::now();
        }
    }

    pub async fn unregister(&self, chat_id: &str) {
        self.chats.write().await.remove(chat_id);
    }

    pub async fn contains_chat(&self, chat_id: &str) -> bool {
        let chats = self.chats.read().await;

        chats.contains_key(chat_id)
    }

    pub async fn get(&self, chat_id: &str) -> Option<AcpChatSession> {
        let chats = self.chats.read().await;
        chats.get(chat_id).cloned()
    }

    pub async fn active_count(&self) -> usize {
        let chats = self.chats.read().await;
        chats.len()
    }

    pub async fn evict_idle(&self) {
        let mut chats = self.chats.write().await;
        chats.retain(|_, chat| chat.last_active.elapsed() < self.idle_timeout);
    }

    pub fn idle_timeout(&self) -> Duration {
        self.idle_timeout
    }
}

pub const ACP_RESERVED_COMMANDS: &[&str] = &["stop"];

pub fn is_acp_reserved_command(name: &str) -> bool {
    ACP_RESERVED_COMMANDS.contains(&name)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpChatResumeResult {
    pub message: String,
    pub history_preview: Option<String>,
}

#[async_trait]
pub trait AcpChatController: Send + Sync {
    async fn enter(&self, chat_id: &str, runner: &str, cwd: Option<&str>) -> String;
    async fn exit(&self, chat_id: &str) -> String;

    async fn list_sessions(&self, chat_id: &str, runner: &str, cwd_arg: Option<&str>) -> String;
    async fn resume(
        &self,
        chat_id: &str,
        runner: &str,
        acp_session_id: &str,
        cwd: Option<&str>,
    ) -> String;

    async fn resume_with_history_preview(
        &self,
        chat_id: &str,
        runner: &str,
        acp_session_id: &str,
        cwd: Option<&str>,
    ) -> AcpChatResumeResult {
        AcpChatResumeResult {
            message: self.resume(chat_id, runner, acp_session_id, cwd).await,
            history_preview: None,
        }
    }
}
