use std::sync::Arc;

use agent_types::{ContentPart, ModelDialogueItem, Role};
use async_trait::async_trait;
use rua_command::conversation::*;

pub struct MockSession {
    messages: tokio::sync::Mutex<Vec<ModelDialogueItem>>,
    context_summary: tokio::sync::Mutex<Option<String>>,
    pub clear_messages_called: std::sync::atomic::AtomicBool,
    pub publish_context_checkpoint_called: std::sync::atomic::AtomicBool,
}

impl MockSession {
    pub fn new() -> Self {
        Self {
            messages: tokio::sync::Mutex::new(Vec::new()),
            context_summary: tokio::sync::Mutex::new(None),
            clear_messages_called: std::sync::atomic::AtomicBool::new(false),
            publish_context_checkpoint_called: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub fn with_messages(mut self, msgs: Vec<ModelDialogueItem>) -> Self {
        self.messages = tokio::sync::Mutex::new(msgs);
        self
    }

    pub fn with_summary(mut self, summary: &str) -> Self {
        self.context_summary = tokio::sync::Mutex::new(Some(summary.to_string()));
        self
    }
}

#[async_trait]
impl ConversationSessionSeam for MockSession {
    async fn load_messages(&self, _session_id: &str) -> SeamResult<Vec<ModelDialogueItem>> {
        let mut messages = Vec::new();
        if let Some(summary) = self.context_summary.lock().await.clone() {
            messages.push(ModelDialogueItem::text(
                Role::System,
                format!("# Compact Summary of Previous Conversation\n\n{summary}"),
            ));
        }
        messages.extend(self.messages.lock().await.clone());
        Ok(messages)
    }
    async fn clear_messages(&self, _session_id: &str) -> SeamResult<()> {
        self.clear_messages_called
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.messages.lock().await.clear();
        *self.context_summary.lock().await = None;
        Ok(())
    }
    async fn get_active_context_summary(&self, _session_id: &str) -> SeamResult<Option<String>> {
        Ok(self.context_summary.lock().await.clone())
    }
    async fn publish_context_checkpoint(
        &self,
        _session_id: &str,
        summary: String,
    ) -> SeamResult<()> {
        self.publish_context_checkpoint_called
            .store(true, std::sync::atomic::Ordering::SeqCst);
        *self.context_summary.lock().await = Some(summary);
        Ok(())
    }
}

impl MockSession {
    pub async fn active_summary(&self) -> Option<String> {
        self.context_summary.lock().await.clone()
    }
}

pub struct MockCompactor {
    pub result: tokio::sync::Mutex<Option<String>>,
    pub previous_summary: tokio::sync::Mutex<Option<String>>,
    pub compacted_texts: tokio::sync::Mutex<Vec<String>>,
    pub called: std::sync::atomic::AtomicBool,
}

impl MockCompactor {
    pub fn returning(summary: &str) -> Self {
        Self {
            result: tokio::sync::Mutex::new(Some(summary.to_string())),
            previous_summary: tokio::sync::Mutex::new(None),
            compacted_texts: tokio::sync::Mutex::new(Vec::new()),
            called: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl ContextCompactorSeam for MockCompactor {
    async fn compact_context(
        &self,
        messages: &[ModelDialogueItem],
        previous_summary: Option<&str>,
        _extra_instruction: &str,
    ) -> SeamResult<Option<String>> {
        self.called.store(true, std::sync::atomic::Ordering::SeqCst);
        *self.previous_summary.lock().await = previous_summary.map(str::to_string);
        *self.compacted_texts.lock().await = messages
            .iter()
            .map(ModelDialogueItem::text_content)
            .collect();
        Ok(self.result.lock().await.clone())
    }
}

pub struct MockPromptFiles {
    files: std::sync::Mutex<Vec<String>>,
}

impl MockPromptFiles {
    pub fn new(files: Vec<String>) -> Self {
        Self {
            files: std::sync::Mutex::new(files),
        }
    }
}

impl PromptFileSeam for MockPromptFiles {
    fn get_active_prompt_files(&self) -> Vec<String> {
        self.files.lock().unwrap().clone()
    }
    fn add_prompt_file(&self, file: &str) -> bool {
        let mut files = self.files.lock().unwrap();
        if files.iter().any(|f| f == file) {
            return false;
        }
        files.push(file.to_string());
        true
    }
    fn remove_prompt_file(&self, file: &str) -> bool {
        let mut files = self.files.lock().unwrap();
        let len_before = files.len();
        files.retain(|f| f != file);
        files.len() < len_before
    }
    fn reset_prompt_files(&self) {
        self.files.lock().unwrap().clear();
    }
}

pub struct MockPendingAction {
    pending: std::sync::Mutex<Option<PendingActionInfo>>,
}

impl MockPendingAction {
    pub fn with_action(prompt: &str) -> Self {
        Self {
            pending: std::sync::Mutex::new(Some(PendingActionInfo {
                action_type: "test".to_string(),
                payload: "test_payload".to_string(),
                prompt: prompt.to_string(),
            })),
        }
    }

    pub fn empty() -> Self {
        Self {
            pending: std::sync::Mutex::new(None),
        }
    }
}

impl PendingActionSeam for MockPendingAction {
    fn resolve_pending_action(&self, _approved: bool) -> Option<PendingActionInfo> {
        self.pending.lock().unwrap().take()
    }
}

pub fn make_message(role: Role, text: &str) -> ModelDialogueItem {
    ModelDialogueItem {
        id: None,
        role,
        content: vec![ContentPart::Text {
            text: text.to_string(),
        }],
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    }
}

pub fn make_ctx_with_session(session: Arc<dyn ConversationSessionSeam>) -> ConversationContext {
    let mut ctx = ConversationContext::stub("agent-1", "session-1", "user-1", "test");
    ctx.session = Some(session);
    ctx.locale = "en".into();
    ctx
}

pub fn outcome_text(outcome: &rua_command::CommandOutcome) -> String {
    outcome
        .events
        .iter()
        .filter_map(|e| match e {
            agent_types::AgentEvent::TextDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

pub fn outcome_metadata(outcome: &rua_command::CommandOutcome) -> serde_json::Value {
    outcome
        .events
        .iter()
        .find_map(|e| match e {
            agent_types::AgentEvent::TextDelta { metadata, .. } => Some(metadata.clone()),
            _ => None,
        })
        .unwrap_or(serde_json::Value::Null)
}
