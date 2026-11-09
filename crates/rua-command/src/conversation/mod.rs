use std::collections::HashMap;
use std::sync::Arc;

use agent_types::ModelDialogueItem;
use async_trait::async_trait;

use crate::CommandOutcome;

pub type SeamError = Box<dyn std::error::Error + Send + Sync>;
pub type SeamResult<T> = Result<T, SeamError>;

#[async_trait]
pub trait ConversationSessionSeam: Send + Sync {
    async fn load_messages(&self, session_id: &str) -> SeamResult<Vec<ModelDialogueItem>>;
    async fn get_active_context_summary(&self, session_id: &str) -> SeamResult<Option<String>>;
    async fn clear_messages(&self, session_id: &str) -> SeamResult<()>;
    async fn publish_context_checkpoint(&self, session_id: &str, summary: String)
        -> SeamResult<()>;
}

#[async_trait]
pub trait ContextCompactorSeam: Send + Sync {
    async fn compact_context(
        &self,
        messages: &[ModelDialogueItem],
        previous_summary: Option<&str>,
        extra_instruction: &str,
    ) -> SeamResult<Option<String>>;
}

pub trait PromptFileSeam: Send + Sync {
    fn get_active_prompt_files(&self) -> Vec<String>;
    fn add_prompt_file(&self, file: &str) -> bool;
    fn remove_prompt_file(&self, file: &str) -> bool;
    fn reset_prompt_files(&self);
}

pub struct PendingActionInfo {
    pub action_type: String,
    pub payload: String,
    pub prompt: String,
}

pub trait PendingActionSeam: Send + Sync {
    fn resolve_pending_action(&self, approved: bool) -> Option<PendingActionInfo>;
}

#[async_trait]
pub trait ConversationCommand: Send + Sync {
    fn name(&self) -> &str;
    fn help(&self) -> &str;
    fn agent_required(&self) -> bool {
        false
    }
    async fn execute(&self, ctx: &ConversationContext, args: &str) -> CommandOutcome;
}

pub struct ConversationContext {
    pub agent_id: String,
    pub session_id: String,
    pub user_id: String,
    pub channel: String,
    pub workspace_dir: String,
    pub session: Option<Arc<dyn ConversationSessionSeam>>,
    pub context_compactor: Option<Arc<dyn ContextCompactorSeam>>,
    pub prompt_files: Option<Arc<dyn PromptFileSeam>>,
    pub pending_action: Option<Arc<dyn PendingActionSeam>>,
    pub locale: String,
}

impl ConversationContext {
    pub fn stub(
        agent_id: impl Into<String>,
        session_id: impl Into<String>,
        user_id: impl Into<String>,
        channel: impl Into<String>,
    ) -> Self {
        Self {
            agent_id: agent_id.into(),
            session_id: session_id.into(),
            user_id: user_id.into(),
            channel: channel.into(),
            workspace_dir: String::new(),
            session: None,
            context_compactor: None,
            prompt_files: None,
            pending_action: None,
            locale: "zh".into(),
        }
    }
}

pub struct ConversationCommandRegistry {
    commands: HashMap<String, Arc<dyn ConversationCommand>>,
}

impl ConversationCommandRegistry {
    pub fn new() -> Self {
        Self {
            commands: HashMap::new(),
        }
    }

    pub fn register(&mut self, cmd: Arc<dyn ConversationCommand>) {
        let name = cmd.name();
        assert!(
            is_valid_command_name(name),
            "conversation command name must be non-empty and contain no whitespace: {name:?}"
        );
        self.commands.insert(name.to_string(), cmd);
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn ConversationCommand>> {
        self.commands.get(name)
    }

    pub fn is_agent_required(&self, name: &str) -> bool {
        self.commands
            .get(name)
            .is_some_and(|cmd| cmd.agent_required())
    }

    pub fn list(&self) -> Vec<(&str, &str)> {
        let mut cmds: Vec<_> = self
            .commands
            .values()
            .map(|cmd| (cmd.name(), cmd.help()))
            .collect();
        cmds.sort_by_key(|(name, _)| *name);
        cmds
    }
}

impl Default for ConversationCommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn is_valid_command_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && !name.chars().any(char::is_whitespace)
}
