use std::sync::Arc;

use rua_prompts::agent_files::{AgentPromptFileChange, AgentPromptFilePublisher};
use rua_workspace::FileManager;

use crate::state::AppState;
use crate::sync_publication::{
    publish_sync_fact_with_runtime, SyncPublicationFact, SyncPublicationRuntime,
};

pub fn agent_prompt_files(state: &AppState, agent_id: &str) -> FileManager {
    FileManager::new(
        state.rua_root.join("workspaces").join(agent_id),
        state.rua_root.clone(),
        Some(agent_id.into()),
        Arc::clone(&state.agent_cells),
    )
    .with_publisher(Arc::new(PromptFilePublication {
        runtime: SyncPublicationRuntime::from_app_state(state),
        agent_id: agent_id.into(),
    }))
}

struct PromptFilePublication {
    runtime: SyncPublicationRuntime,
    agent_id: String,
}

#[async_trait::async_trait]
impl AgentPromptFilePublisher for PromptFilePublication {
    async fn publish(&self, changes: Vec<AgentPromptFileChange>) {
        let _ = publish_sync_fact_with_runtime(
            &self.runtime,
            SyncPublicationFact::AgentPromptFilesCommitted {
                agent_id: self.agent_id.clone(),
                changes,
            },
        )
        .await;
    }
}
