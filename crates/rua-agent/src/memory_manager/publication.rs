use std::sync::Arc;

use super::{LlmMemoryManager, MemoryPublicationHook, MemoryRecord, MemoryVisibility};

impl LlmMemoryManager {
    fn publication_hook(&self) -> Option<Arc<dyn MemoryPublicationHook>> {
        self.publication_hook.clone().or_else(|| {
            self.publication_hook_cell
                .as_ref()
                .and_then(|cell| cell.get().cloned())
        })
    }

    pub(super) async fn publish_committed_records(&self, records: &[MemoryRecord]) {
        if records.is_empty() {
            return;
        }
        if let Some(hook) = self.publication_hook() {
            if let Err(e) = hook.on_memory_records_committed(records).await {
                tracing::warn!(error = %e, "structured Memory publication failed after local commit");
            }
        }
    }

    pub(super) async fn publish_deleted_records(
        &self,
        agent_id: &str,
        ids: &[rua_memory::MemoryId],
    ) {
        if ids.is_empty() {
            return;
        }
        if let Some(hook) = self.publication_hook() {
            if let Err(e) = hook.on_memory_records_deleted(agent_id, ids).await {
                tracing::warn!(error = %e, "structured Memory delete publication failed after local commit");
            }
        }
    }

    pub(super) async fn publish_visibility(&self, visibility: &MemoryVisibility) {
        if let Some(hook) = self.publication_hook() {
            if let Err(e) = hook.on_memory_visibility_committed(visibility).await {
                tracing::warn!(
                    error = %e,
                    memory_id = %visibility.memory_id,
                    "structured Memory visibility publication failed after local commit"
                );
            }
        }
    }
}
