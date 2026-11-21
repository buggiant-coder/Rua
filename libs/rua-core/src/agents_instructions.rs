use agent_types::ToolCall;
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::tool::ToolContext;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentsInstructionDecision {
    Continue,
    Reconsider { reason: String },
}

/// One Turn's captured directory instructions, shared by model calls and tools.
#[async_trait]
pub trait AgentsInstructions: Send + Sync {
    /// Return the captured context and mark only this snapshot as model-visible.
    fn snapshot_for_model(&self) -> Option<String>;

    /// Discover explicit locations before execution. An unseen instruction scope
    /// must return Reconsider for every affected call until a model snapshot includes it.
    /// Missing or unreadable files are best-effort discovery, not permission denials.
    async fn before_tool(
        &self,
        call: &ToolCall,
        ctx: &ToolContext,
        cancellation: CancellationToken,
    ) -> crate::Result<AgentsInstructionDecision>;
}
