use crate::{
    MessageLogEntry, NewConversationMessage, TurnIndexRecord, TurnLifecycle, TurnLifecycleStatus,
    WorkspaceSessionStore,
};
use rua_core::Result;

pub struct TurnWriter<'a> {
    store: &'a WorkspaceSessionStore,
    chat_id: &'a str,
    turn_id: &'a str,
}

impl<'a> TurnWriter<'a> {
    pub fn new(store: &'a WorkspaceSessionStore, chat_id: &'a str, turn_id: &'a str) -> Self {
        Self {
            store,
            chat_id,
            turn_id,
        }
    }

    pub async fn append_in_progress_output(
        &self,
        messages: Vec<NewConversationMessage>,
        assistant_started_at: Option<&str>,
    ) -> Result<Option<TurnIndexRecord>> {
        if messages.is_empty() {
            return Ok(None);
        }
        let active_turn = self.turn_record().await?;
        if TurnLifecycleStatus::parse(&active_turn.lifecycle.status)
            != TurnLifecycleStatus::InProgress
        {
            return Err(rua_core::RuaError::Io {
                message: format!(
                    "cannot append in-progress output to terminal turn {}/{}",
                    self.chat_id, self.turn_id
                ),
            });
        }
        let assistant_started_at = assistant_started_at
            .map(str::to_string)
            .or_else(|| {
                messages
                    .iter()
                    .find(|message| message.role == "assistant")
                    .map(|message| message.created_at.clone())
            })
            .or(active_turn.lifecycle.assistant_started_at);
        let selected_variant_id = last_assistant_id(&messages);
        let record = self
            .store
            .append_messages_to_turn(
                self.chat_id,
                self.turn_id,
                TurnLifecycle {
                    user_submitted_at: active_turn.lifecycle.user_submitted_at,
                    assistant_started_at,
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id,
                messages,
            )
            .await?;
        Ok(Some(record))
    }

    pub async fn append_new_variant_in_progress(
        &self,
        messages: Vec<NewConversationMessage>,
        assistant_started_at: Option<&str>,
    ) -> Result<Option<TurnIndexRecord>> {
        if messages.is_empty() {
            return Ok(None);
        }
        let existing_turn = self.turn_record().await?;
        let assistant_started_at = assistant_started_at
            .map(str::to_string)
            .or_else(|| {
                messages
                    .iter()
                    .find(|message| message.role == "assistant")
                    .map(|message| message.created_at.clone())
            })
            .or(existing_turn.lifecycle.assistant_started_at);
        let selected_variant_id = last_assistant_id(&messages);
        let record = self
            .store
            .append_messages_to_turn_as_new_variant(
                self.chat_id,
                self.turn_id,
                TurnLifecycle {
                    user_submitted_at: existing_turn.lifecycle.user_submitted_at,
                    assistant_started_at,
                    assistant_completed_at: None,
                    status: "in_progress".to_string(),
                },
                selected_variant_id,
                messages,
            )
            .await?;
        Ok(Some(record))
    }

    pub async fn append_output(
        &self,
        messages: Vec<NewConversationMessage>,
        force_in_progress: bool,
        assistant_started_at: Option<&str>,
    ) -> Result<TurnIndexRecord> {
        let turns = self
            .store
            .read_turn_range(self.chat_id, 1, usize::MAX)
            .await?;
        let active_turn = turns
            .into_iter()
            .find(|turn| turn.turn_id == self.turn_id)
            .ok_or_else(|| rua_core::RuaError::Io {
                message: format!(
                    "turn index record not found for {}/{}",
                    self.chat_id, self.turn_id
                ),
            })?;
        let assistant_started_at = assistant_started_at
            .map(str::to_string)
            .or_else(|| {
                messages
                    .iter()
                    .find(|message| message.role == "assistant")
                    .map(|message| message.created_at.clone())
            })
            .or_else(|| active_turn.lifecycle.assistant_started_at.clone());
        let terminal_already_recorded = matches!(
            active_turn.lifecycle.status.as_str(),
            "interrupted" | "interjected" | "failed"
        );
        let has_open_tool_call = has_unmatched_tool_use(&messages);
        let assistant_completed_at =
            if force_in_progress || has_open_tool_call || terminal_already_recorded {
                None
            } else {
                messages
                    .iter()
                    .rev()
                    .find(|message| message.role == "assistant")
                    .map(|message| message.created_at.clone())
            };
        let lifecycle = TurnLifecycle {
            user_submitted_at: active_turn.lifecycle.user_submitted_at.clone(),
            assistant_started_at,
            assistant_completed_at: if terminal_already_recorded {
                active_turn.lifecycle.assistant_completed_at.clone()
            } else {
                assistant_completed_at.clone()
            },
            status: if terminal_already_recorded {
                active_turn.lifecycle.status.clone()
            } else if assistant_completed_at.is_some() {
                "completed".to_string()
            } else {
                "in_progress".to_string()
            },
        };

        if messages.is_empty() {
            self.store
                .set_turn_lifecycle(self.chat_id, self.turn_id, lifecycle.clone())
                .await?;
            let mut record = active_turn;
            record.lifecycle = lifecycle;
            return Ok(record);
        }

        self.store
            .append_messages_to_turn(
                self.chat_id,
                self.turn_id,
                lifecycle,
                last_assistant_id(&messages),
                messages,
            )
            .await
    }

    pub async fn finalize(
        &self,
        force_in_progress: bool,
        assistant_started_at: Option<&str>,
    ) -> Result<TurnIndexRecord> {
        let turns = self
            .store
            .read_turn_range(self.chat_id, 1, usize::MAX)
            .await?;
        let active_turn = turns
            .into_iter()
            .find(|turn| turn.turn_id == self.turn_id)
            .ok_or_else(|| rua_core::RuaError::Io {
                message: format!(
                    "turn index record not found for {}/{}",
                    self.chat_id, self.turn_id
                ),
            })?;
        let existing_messages = self
            .store
            .read_turn_messages(self.chat_id, self.turn_id)
            .await?;
        let terminal_already_recorded = matches!(
            active_turn.lifecycle.status.as_str(),
            "interrupted" | "interjected" | "failed"
        );
        let last_assistant = existing_messages
            .iter()
            .rev()
            .find(|message| message.role == "assistant");
        let has_open_tool_call = has_unmatched_tool_use_entries(&existing_messages);
        let lifecycle = TurnLifecycle {
            user_submitted_at: active_turn.lifecycle.user_submitted_at.clone(),
            assistant_started_at: assistant_started_at
                .map(str::to_string)
                .or_else(|| active_turn.lifecycle.assistant_started_at.clone())
                .or_else(|| last_assistant.map(|message| message.created_at.clone())),
            assistant_completed_at: if terminal_already_recorded {
                active_turn.lifecycle.assistant_completed_at.clone()
            } else if force_in_progress || has_open_tool_call {
                None
            } else {
                last_assistant.map(|message| message.created_at.clone())
            },
            status: if terminal_already_recorded {
                active_turn.lifecycle.status.clone()
            } else if force_in_progress || has_open_tool_call {
                "in_progress".to_string()
            } else if last_assistant.is_some() {
                "completed".to_string()
            } else {
                "in_progress".to_string()
            },
        };
        self.store
            .set_turn_lifecycle(self.chat_id, self.turn_id, lifecycle.clone())
            .await?;
        let mut record = active_turn;
        record.lifecycle = lifecycle;
        Ok(record)
    }

    pub async fn is_complete_model_call_boundary(&self) -> Result<bool> {
        let messages = self
            .store
            .read_turn_messages(self.chat_id, self.turn_id)
            .await?;
        Ok(!messages.is_empty() && !has_unmatched_tool_use_entries(&messages))
    }

    async fn turn_record(&self) -> Result<TurnIndexRecord> {
        let turns = self
            .store
            .read_turn_range(self.chat_id, 1, usize::MAX)
            .await?;
        let active_turn = turns
            .into_iter()
            .find(|turn| turn.turn_id == self.turn_id)
            .ok_or_else(|| rua_core::RuaError::Io {
                message: format!(
                    "turn index record not found for {}/{}",
                    self.chat_id, self.turn_id
                ),
            })?;
        Ok(active_turn)
    }
}

fn last_assistant_id(messages: &[NewConversationMessage]) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|message| message.role == "assistant")
        .map(|message| message.message_id.clone())
}

fn has_unmatched_tool_use(messages: &[NewConversationMessage]) -> bool {
    has_unmatched_tool_use_content(messages.iter().map(|message| &message.content))
}

fn has_unmatched_tool_use_entries(messages: &[MessageLogEntry]) -> bool {
    has_unmatched_tool_use_content(messages.iter().map(|message| &message.content))
}

fn has_unmatched_tool_use_content<'a>(
    contents: impl IntoIterator<Item = &'a serde_json::Value>,
) -> bool {
    let mut open_tool_call_ids = std::collections::HashSet::new();
    for content in contents {
        let Some(parts) = content.as_array() else {
            continue;
        };
        for part in parts {
            match part.get("type").and_then(serde_json::Value::as_str) {
                Some("tool_use") => {
                    if let Some(id) = part.get("id").and_then(serde_json::Value::as_str) {
                        open_tool_call_ids.insert(id.to_string());
                    }
                }
                Some("tool_result") => {
                    if let Some(id) = part
                        .get("id")
                        .or_else(|| part.get("call_id"))
                        .and_then(serde_json::Value::as_str)
                    {
                        open_tool_call_ids.remove(id);
                    }
                }
                _ => {}
            }
        }
    }
    !open_tool_call_ids.is_empty()
}
