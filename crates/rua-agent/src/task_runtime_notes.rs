use std::path::PathBuf;

use async_trait::async_trait;
use rua_core::ChatScopeStore;
use rua_session::{RuntimeNote, RuntimeNotePayload, TaskAttemptTerminalRuntimeNote};
use rua_task::{TaskCompletionNotification, TaskNotificationSink, TaskRunnerError, TaskState};

pub struct TaskRuntimeNoteSink {
    data_dir: PathBuf,
}

impl TaskRuntimeNoteSink {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
        }
    }
}

#[async_trait]
impl TaskNotificationSink for TaskRuntimeNoteSink {
    async fn terminal(
        &self,
        notification: TaskCompletionNotification,
    ) -> Result<(), TaskRunnerError> {
        let note = task_terminal_note(&notification.record)?;
        let record = notification.record;
        let scope_store =
            ChatScopeStore::resolve(&self.data_dir, record.spec.owner.chat_address.clone());
        let store = rua_session::WorkspaceSessionStore::new(scope_store.root());
        let chat_id = record.spec.owner.chat_id().to_string();
        if let Some(turn_id) = record.spec.origin.turn_id.as_deref() {
            store.append_runtime_note(&chat_id, turn_id, note).await
        } else {
            store
                .append_runtime_note_to_latest_turn(&chat_id, note)
                .await
        }
        .map_err(|error| TaskRunnerError::new(error.to_string()))?;
        Ok(())
    }
}

pub fn task_terminal_note(record: &rua_task::TaskRecord) -> Result<RuntimeNote, TaskRunnerError> {
    let result = record
        .result
        .as_ref()
        .ok_or_else(|| TaskRunnerError::new("terminal task has no result"))?;
    let terminal_status = match record.state {
        TaskState::Completed => "completed",
        TaskState::Failed => "failed",
        TaskState::Cancelled => "cancelled",
        TaskState::Interrupted => "interrupted",
        other => {
            return Err(TaskRunnerError::new(format!(
                "cannot publish non-terminal task state {other:?}"
            )));
        }
    };
    Ok(RuntimeNote {
        runtime_note_id: format!("task:{}:attempt:{}", record.task_id, record.attempt_no),
        created_at: record.finished_at.unwrap_or(record.updated_at).to_rfc3339(),
        payload: RuntimeNotePayload::TaskAttemptTerminal(TaskAttemptTerminalRuntimeNote {
            task_id: record.task_id.clone(),
            attempt_no: record.attempt_no,
            terminal_status: terminal_status.to_string(),
            output: result.output.clone(),
            runner_name: record.spec.runner.display_name().to_string(),
            tool_call_id: record.spec.origin.tool_call_id.clone(),
            full_result_ref: result
                .full_result_ref
                .as_ref()
                .and_then(|reference| serde_json::to_value(reference).ok()),
            artifacts: serde_json::to_value(&result.artifacts).unwrap_or_default(),
            device: serde_json::to_value(&result.device).unwrap_or_default(),
        }),
    })
}
