use async_trait::async_trait;
use rua_agent::memory_manager::MemoryPublicationHook;

use crate::sync_publication::{
    publish_sync_fact_with_runtime, SyncPublicationFact, SyncPublicationRuntime,
};

pub struct SyncMemoryPublicationHook {
    runtime: SyncPublicationRuntime,
}

impl SyncMemoryPublicationHook {
    pub fn new(runtime: SyncPublicationRuntime) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self { runtime })
    }
}

#[async_trait]
impl MemoryPublicationHook for SyncMemoryPublicationHook {
    async fn on_memory_records_committed(
        &self,
        records: &[rua_memory::MemoryRecord],
    ) -> Result<(), rua_agent::AgentError> {
        for record in records {
            let _ = publish_sync_fact_with_runtime(
                &self.runtime,
                SyncPublicationFact::StructuredMemoryRecordCommitted {
                    record: Box::new(record.clone()),
                },
            )
            .await;
        }
        Ok(())
    }

    async fn on_memory_visibility_committed(
        &self,
        visibility: &rua_memory::MemoryVisibility,
    ) -> Result<(), rua_agent::AgentError> {
        let _ = publish_sync_fact_with_runtime(
            &self.runtime,
            SyncPublicationFact::StructuredMemoryVisibilityCommitted {
                visibility: Box::new(visibility.clone()),
            },
        )
        .await;
        Ok(())
    }

    async fn on_memory_records_deleted(
        &self,
        agent_id: &str,
        memory_ids: &[rua_memory::MemoryId],
    ) -> Result<(), rua_agent::AgentError> {
        for memory_id in memory_ids {
            let _ = publish_sync_fact_with_runtime(
                &self.runtime,
                SyncPublicationFact::StructuredMemoryRecordDeleted {
                    agent_id: agent_id.to_string(),
                    memory_id: memory_id.clone(),
                },
            )
            .await;
        }
        Ok(())
    }
}
