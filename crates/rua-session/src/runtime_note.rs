use rua_core::ChatAddress;
use serde::{Deserialize, Serialize};

use crate::{MessageLogEntry, NewConversationMessage};

pub const RUNTIME_NOTE_MESSAGE_TYPE: &str = "runtime_note";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuntimeNote {
    pub runtime_note_id: String,
    pub created_at: String,
    #[serde(flatten)]
    pub payload: RuntimeNotePayload,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
pub enum RuntimeNotePayload {
    CapabilityState(CapabilityStateRuntimeNote),
    CollaborationResult(CollaborationResultRuntimeNote),
    ConversationRuntimeTerminal(ConversationRuntimeTerminalRuntimeNote),
    MemorySignal(MemorySignalRuntimeNote),
    TaskAttemptTerminal(TaskAttemptTerminalRuntimeNote),
    TaskWatchOutput(TaskWatchOutputRuntimeNote),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CollaborationResultRuntimeNote {
    pub request_id: String,
    pub result_id: String,
    pub target: ChatAddress,
    pub status: String,
    pub output: String,
    #[serde(default)]
    pub partial: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub results: Vec<CollaborationResultReference>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CollaborationResultReference {
    pub chat: ChatAddress,
    pub result: crate::collaboration::CollaborationResultCursor,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskWatchOutputProjection {
    ModelVisible,
    #[default]
    ActivityOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CapabilityStateRuntimeNote {
    pub previous_generation: u64,
    pub current_generation: u64,
    #[serde(default)]
    pub changes: Vec<CapabilityStateChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilityStateChange {
    pub capability_type: String,
    pub name: String,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConversationRuntimeTerminalRuntimeNote {
    pub agent_id: String,
    pub chat_id: String,
    pub turn_id: String,
    pub terminal_status: String,
    pub reason: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemorySignalRuntimeNote {
    pub signal_kind: String,
    pub confidence: String,
    pub suggested_memory_kind: String,
    pub suggested_scope: String,
    pub evidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskAttemptTerminalRuntimeNote {
    pub task_id: String,
    pub attempt_no: u32,
    pub terminal_status: String,
    pub output: String,
    pub runner_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_result_ref: Option<serde_json::Value>,
    #[serde(default)]
    pub artifacts: serde_json::Value,
    #[serde(default)]
    pub device: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskWatchOutputRecord {
    pub source: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskWatchOutputRuntimeNote {
    pub task_id: String,
    pub attempt_no: u32,
    #[serde(default)]
    pub projection: TaskWatchOutputProjection,
    pub records: Vec<TaskWatchOutputRecord>,
}

impl RuntimeNote {
    pub fn from_message_log_entry(entry: &MessageLogEntry) -> Option<Self> {
        if entry
            .metadata
            .get("message_type")
            .and_then(serde_json::Value::as_str)
            != Some(RUNTIME_NOTE_MESSAGE_TYPE)
        {
            return None;
        }
        entry
            .metadata
            .get("runtime_note")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok())
    }

    pub fn into_new_conversation_message(self) -> rua_core::Result<NewConversationMessage> {
        let rendered = self.render_for_model();
        let model_visible = self.is_model_visible();
        let role = if matches!(
            &self.payload,
            RuntimeNotePayload::CollaborationResult(_)
                | RuntimeNotePayload::TaskAttemptTerminal(_)
                | RuntimeNotePayload::TaskWatchOutput(_)
        ) {
            "runtime"
        } else {
            "system"
        };
        let mut metadata = serde_json::Map::new();
        metadata.insert(
            "message_type".to_string(),
            serde_json::Value::String(RUNTIME_NOTE_MESSAGE_TYPE.to_string()),
        );
        metadata.insert(
            "runtime_note".to_string(),
            serde_json::to_value(&self).map_err(|error| rua_core::RuaError::Io {
                message: format!("serialize Runtime Note: {error}"),
            })?,
        );
        metadata.insert(
            "model_visible".to_string(),
            serde_json::Value::Bool(model_visible),
        );
        Ok(NewConversationMessage {
            message_id: self.runtime_note_id,
            role: role.to_string(),
            created_at: self.created_at,
            status: "completed".to_string(),
            content: serde_json::json!([{"type": "text", "text": rendered}]),
            provider: None,
            tool: None,
            metadata,
        })
    }

    pub fn is_model_visible(&self) -> bool {
        !matches!(
            &self.payload,
            RuntimeNotePayload::TaskWatchOutput(TaskWatchOutputRuntimeNote {
                projection: TaskWatchOutputProjection::ActivityOnly,
                ..
            })
        )
    }

    pub fn render_for_model(&self) -> String {
        match &self.payload {
            RuntimeNotePayload::CapabilityState(state) => {
                let changes = state
                    .changes
                    .iter()
                    .map(|change| {
                        format!(
                            "{}:{}={}{}",
                            change.capability_type,
                            change.name,
                            change.state,
                            change
                                .reason
                                .as_ref()
                                .map(|reason| format!(" ({reason})"))
                                .unwrap_or_default()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                format!(
                    "<capability_state>\nprevious_generation: {}\ncurrent_generation: {}\n{}\n</capability_state>",
                    state.previous_generation, state.current_generation, changes
                )
            }
            RuntimeNotePayload::CollaborationResult(result) if !result.results.is_empty() =>
                serde_json::json!({"status": result.status, "partial": result.partial, "results": result.results}).to_string(),
            RuntimeNotePayload::CollaborationResult(result) => format!(
                "<collaboration_result>\nrequest_id: {}\nresult_id: {}\ntarget: {}\nstatus: {}\npartial: {}\noutput:\n{}\n</collaboration_result>",
                result.request_id,
                result.result_id,
                result.target,
                result.status,
                result.partial,
                result.output
            ),
            RuntimeNotePayload::ConversationRuntimeTerminal(terminal) => format!(
                "<conversation_runtime_terminal>\nagent_id: {}\nchat_id: {}\nturn_id: {}\nterminal_status: {}\nreason: {}\nsummary: {}\n</conversation_runtime_terminal>",
                terminal.agent_id,
                terminal.chat_id,
                terminal.turn_id,
                terminal.terminal_status,
                terminal.reason,
                terminal.summary
            ),
            RuntimeNotePayload::MemorySignal(signal) => {
                let workflow = if matches!(
                    signal.signal_kind.as_str(),
                    "correction" | "missed_recall"
                ) {
                    "Suggested workflow: call memory_search with compact keywords, use memory_get if a candidate needs inspection, then prefer memory_update when an existing record should be revised. Use memory_write only if no related record exists."
                } else {
                    "Suggested workflow: call memory_search with compact keywords first. If a related record exists, use memory_update; otherwise use memory_write only for a stable durable fact."
                };
                format!(
                    "<memory_signal>\nsignal_kind: {}\nconfidence: {}\n{}\nDo not save task progress, temporary TODOs, PR/issue/commit ids, or facts likely stale within 7 days.\nSuggested kind/scope: {}/{}\nEvidence: {}\n</memory_signal>",
                    signal.signal_kind,
                    signal.confidence,
                    workflow,
                    signal.suggested_memory_kind,
                    signal.suggested_scope,
                    signal.evidence
                )
            }
            RuntimeNotePayload::TaskAttemptTerminal(task) => format!(
                "<task_attempt_terminal>\ntask_id: {}\nattempt_no: {}\nterminal_status: {}\nrunner: {}\noutput:\n{}\n</task_attempt_terminal>",
                task.task_id,
                task.attempt_no,
                task.terminal_status,
                task.runner_name,
                task.output
            ),
            RuntimeNotePayload::TaskWatchOutput(task) => {
                let records = task
                    .records
                    .iter()
                    .map(|record| format!("[{}] {}", record.source, record.text))
                    .collect::<Vec<_>>()
                    .join("\n");
                format!(
                    "<task_watch_output>\ntask_id: {}\nattempt_no: {}\nrecords:\n{}\n</task_watch_output>",
                    task.task_id, task.attempt_no, records
                )
            }
        }
    }
}
