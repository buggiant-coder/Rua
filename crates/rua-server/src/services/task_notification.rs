use std::path::PathBuf;
use std::sync::{Arc, OnceLock, Weak};

use async_trait::async_trait;
use rua_core::ChatScopeStore;
use rua_task::{TaskCompletionNotification, TaskNotificationSink, TaskRunnerError};

use crate::sync_publication::{
    publish_sync_fact_with_runtime, SyncPublicationFact, SyncPublicationRuntime,
};

pub struct ServerTaskNotificationSink {
    data_dir: PathBuf,
    cancellation: tokio_util::sync::CancellationToken,
    tasks: OnceLock<Weak<rua_task::TaskRuntime>>,
    sync_runtime: OnceLock<SyncPublicationRuntime>,
    watch_delivery: OnceLock<crate::services::conversation_input_intake::ConversationInputIntake>,
}

impl ServerTaskNotificationSink {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        let data_dir = data_dir.into();
        Self {
            cancellation: tokio_util::sync::CancellationToken::new(),
            tasks: OnceLock::new(),
            data_dir,
            sync_runtime: OnceLock::new(),
            watch_delivery: OnceLock::new(),
        }
    }

    pub fn install_sync_runtime(&self, runtime: SyncPublicationRuntime) -> bool {
        self.sync_runtime.set(runtime).is_ok()
    }

    pub fn install_tasks(&self, tasks: &Arc<rua_task::TaskRuntime>) {
        let _ = self.tasks.set(Arc::downgrade(tasks));
    }

    pub async fn resume_pending(
        &self,
        owner: Option<&rua_task::TaskOwner>,
    ) -> Result<(), TaskRunnerError> {
        let tasks = self
            .tasks
            .get()
            .and_then(Weak::upgrade)
            .ok_or_else(|| TaskRunnerError::new("Task runtime unavailable"))?;
        for record in tasks
            .pending_terminal_deliveries(owner)
            .await
            .map_err(|e| TaskRunnerError::new(e.to_string()))?
        {
            self.terminal(TaskCompletionNotification { record }).await?;
        }
        Ok(())
    }

    pub fn install_watch_delivery(
        &self,
        intake: crate::services::conversation_input_intake::ConversationInputIntake,
    ) -> bool {
        let installed = self.watch_delivery.set(intake.clone()).is_ok();
        if installed {
            if let Some(tasks) = self.tasks.get().and_then(Weak::upgrade) {
                let mut events = tasks.subscribe_activity();
                let cancel = self.cancellation.child_token();
                tokio::spawn(async move {
                    loop {
                        tokio::select! {
                            _ = cancel.cancelled() => break,
                            event = events.recv() => match event {
                                Ok(event) => intake.publish_background_work(&event.owner.chat_address).await,
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                            }
                        }
                    }
                });
            }
        }
        installed
    }
}

impl Drop for ServerTaskNotificationSink {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

#[async_trait]
impl TaskNotificationSink for ServerTaskNotificationSink {
    async fn watch_output(
        &self,
        notification: rua_task::TaskWatchOutputNotification,
    ) -> Result<(), TaskRunnerError> {
        let intake = self
            .watch_delivery
            .get()
            .ok_or_else(|| TaskRunnerError::new("Task Watch delivery is unavailable"))?;
        intake
            .deliver_task_watch_output(notification)
            .await
            .map_err(TaskRunnerError::new)
    }

    async fn terminal(
        &self,
        notification: TaskCompletionNotification,
    ) -> Result<(), TaskRunnerError> {
        let owner = notification.owner().clone();
        let intake = self.watch_delivery.get().cloned().ok_or_else(|| {
            TaskRunnerError::new(
                "Background result delivery is not initialized; result remains pending",
            )
        })?;
        let tasks = self.tasks.get().and_then(Weak::upgrade).ok_or_else(|| {
            TaskRunnerError::new("Task runtime is unavailable; result remains pending")
        })?;
        let cancel = self.cancellation.child_token();
        tokio::select! {
            _ = cancel.cancelled() => {},
            result = intake.deliver_task_result(notification.record, tasks) => {
                if let Err(error) = result { tracing::warn!(%error, "Task result was not queued"); }
            }
        }

        if let Some(runtime) = self.sync_runtime.get() {
            let scope_store = ChatScopeStore::resolve(&self.data_dir, owner.chat_address.clone());
            let _ = publish_sync_fact_with_runtime(
                runtime,
                SyncPublicationFact::StableConversationState {
                    scope_root: scope_store.root().to_path_buf(),
                    address: owner.chat_address,
                },
            )
            .await;
        }
        Ok(())
    }
}
