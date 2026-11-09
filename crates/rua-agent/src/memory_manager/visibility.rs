use crate::error::AgentError;

use super::{LlmMemoryManager, MemoryVisibility, MemoryVisibilityUpdateRequest};

impl LlmMemoryManager {
    pub(super) fn get_memory_visibility(
        &self,
        owner_agent_id: &str,
        memory_id: &rua_memory::MemoryId,
    ) -> Result<Option<MemoryVisibility>, AgentError> {
        let Some(ref store_mutex) = self.structured_store else {
            return Ok(None);
        };
        match store_mutex.lock() {
            Ok(store) => store
                .visibility_for_memory(owner_agent_id, memory_id)
                .map_err(|e| AgentError::memory(format!("load memory visibility: {e}"))),
            Err(e) => Err(AgentError::memory(format!(
                "lock structured memory store for visibility: {e}"
            ))),
        }
    }

    pub(super) fn list_memory_visibility(
        &self,
        owner_agent_id: &str,
    ) -> Result<Vec<MemoryVisibility>, AgentError> {
        let Some(ref store_mutex) = self.structured_store else {
            return Ok(Vec::new());
        };
        match store_mutex.lock() {
            Ok(store) => store
                .list_visibility_for_agent(owner_agent_id)
                .map_err(|e| AgentError::memory(format!("list memory visibility: {e}"))),
            Err(e) => Err(AgentError::memory(format!(
                "lock structured memory store for visibility list: {e}"
            ))),
        }
    }

    pub(super) async fn update_memory_visibility(
        &self,
        owner_agent_id: &str,
        memory_id: rua_memory::MemoryId,
        req: MemoryVisibilityUpdateRequest,
    ) -> Result<Option<MemoryVisibility>, AgentError> {
        let Some(ref store_mutex) = self.structured_store else {
            return Ok(None);
        };
        let visibility = match store_mutex.lock() {
            Ok(store) => store
                .replace_visibility(owner_agent_id, &memory_id, req, "user")
                .map_err(|e| AgentError::memory(format!("update memory visibility: {e}"))),
            Err(e) => Err(AgentError::memory(format!(
                "lock structured memory store for visibility update: {e}"
            ))),
        }?;
        if let Some(visibility) = visibility.as_ref() {
            self.publish_visibility(visibility).await;
        }
        Ok(visibility)
    }
}
