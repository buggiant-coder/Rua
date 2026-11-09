use crate::state::AppState;
use crate::sync_publication::{
    publish_sync_fact_with_runtime, SyncPublicationFact, SyncPublicationRuntime,
};
use rua_prompts::{file_store::PromptFileChange, library::PromptLibraryPublisher, PromptLibrary};
use std::sync::Arc;

pub fn prompt_library(state: &AppState) -> PromptLibrary {
    PromptLibrary::new(state.rua_root.clone()).with_publisher(Arc::new(LibraryPublication(
        SyncPublicationRuntime::from_app_state(state),
    )))
}

struct LibraryPublication(SyncPublicationRuntime);
#[async_trait::async_trait]
impl PromptLibraryPublisher for LibraryPublication {
    async fn publish(&self, changes: Vec<PromptFileChange>) {
        let _ = publish_sync_fact_with_runtime(
            &self.0,
            SyncPublicationFact::PromptLibraryFilesCommitted { changes },
        )
        .await;
    }
}
