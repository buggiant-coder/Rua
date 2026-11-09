use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};

use rua_core::event::RuntimeEvent;
use rua_core::id::{AgentId, SessionId};
use rua_core::message::ContentPart;
use rua_core::{Result, RuaError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRequest {
    pub session_id: SessionId,
    pub user_id: String,
    pub agent_id: AgentId,
    pub messages: Vec<AgentInputMessage>,

    #[serde(default)]
    pub model_override: Option<String>,

    #[serde(default = "default_tool_mode")]
    pub tool_mode: String,

    #[serde(default)]
    pub attachments: Vec<AttachmentRef>,

    #[serde(default)]
    pub display_preferences: Option<serde_json::Value>,

    #[serde(default)]
    pub disabled_tools: Vec<String>,

    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,

    #[serde(skip)]
    pub cancel: Option<tokio::sync::watch::Receiver<bool>>,

    #[serde(skip)]
    pub run_control: Option<tokio::sync::watch::Receiver<rua_core::RunControl>>,

    #[serde(skip)]
    pub workspace_dir: Option<std::path::PathBuf>,

    #[serde(skip)]
    pub conversation_store_dir: Option<std::path::PathBuf>,

    #[serde(skip)]
    pub working_dir: Option<std::path::PathBuf>,
}

impl AgentRequest {
    pub fn conversation_store_dir(&self) -> Option<&std::path::Path> {
        self.conversation_store_dir
            .as_deref()
            .or(self.workspace_dir.as_deref())
    }

    pub fn working_dir(&self) -> Option<&std::path::Path> {
        self.working_dir
            .as_deref()
            .or(self.workspace_dir.as_deref())
    }
}

fn default_tool_mode() -> String {
    "auto".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInputMessage {
    pub role: String,
    pub content: Vec<ContentPart>,
}

pub use agent_types::AttachmentRef;

#[derive(Debug, Clone)]
pub struct RuntimeWarmupContext {
    pub agent_id: AgentId,
}

#[derive(Debug, Clone)]
pub struct RuntimeCommandInput {
    pub name: String,
    pub args: String,
    pub session_id: String,
    pub user_id: String,
    pub channel: String,
    pub locale: String,
}

#[derive(Debug, Clone)]
pub struct RuntimeInputIntercept {
    pub session_id: String,
    pub text: String,
    pub content: Vec<ContentPart>,
}

pub type RuntimeCommandStream =
    Pin<Box<dyn Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>;

#[async_trait]
pub trait RuntimeCommandDispatcher: Send + Sync {
    async fn dispatch(&self, input: RuntimeCommandInput) -> Result<Option<RuntimeCommandStream>>;

    async fn intercept_input(
        &self,
        _input: RuntimeInputIntercept,
    ) -> Result<Option<RuntimeCommandStream>> {
        Ok(None)
    }
}

pub struct CommandRuntimeConfig {
    pub dispatcher: Arc<dyn RuntimeCommandDispatcher>,
}

pub type CommandRuntimeFactory =
    Arc<dyn Fn(&str, &std::path::Path) -> CommandRuntimeConfig + Send + Sync>;

#[async_trait]
pub trait SessionReplicationHook: Send + Sync {
    async fn on_session_state_changed(
        &self,
        chat_address: &rua_core::ChatAddress,
        state: &serde_json::Value,
    );
}

#[async_trait]
pub trait AgentRuntime: Send + Sync {
    async fn stream(
        &self,
        req: AgentRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = std::result::Result<RuntimeEvent, RuaError>> + Send>>>;

    async fn cancel(&self, session_id: &SessionId) -> Result<()>;

    async fn warmup(&self, context: RuntimeWarmupContext) -> Result<()>;

    async fn shutdown(&self) -> Result<()>;

    fn set_command_runtime(&self, _config: CommandRuntimeConfig) {}

    fn bump_prepared_generation(&self) {}
}
