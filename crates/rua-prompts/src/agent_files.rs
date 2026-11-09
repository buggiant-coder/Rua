//! Agent Prompt file facade over the shared durable file store.
use crate::file_store::PromptFileStore;
pub use crate::file_store::{
    PromptFileChange as AgentPromptFileChange, PromptFileGuard as AgentPromptFileGuard,
    PromptFileVersion,
};
use std::path::PathBuf;

#[async_trait::async_trait]
pub trait AgentPromptFilePublisher: Send + Sync {
    /// Post-commit, best effort. Failure must not roll back local file changes.
    async fn publish(&self, changes: Vec<AgentPromptFileChange>);
}

#[derive(Clone)]
pub struct AgentPromptFileStore(PromptFileStore);
impl AgentPromptFileStore {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self(PromptFileStore::for_agent(workspace))
    }
    pub async fn lock(&self) -> AgentPromptFileGuard {
        self.0.lock().await
    }
}
