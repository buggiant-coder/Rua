//! Local, non-exported intent for a cooperative Chat writer handoff. Released is
//! the commit intent: recovery may finish it, but must never restore the old writer.

use std::sync::Mutex;

use super::{write_bytes_atomic_sync, ChatSessionHeader, WorkspaceSessionStore};
use rua_core::{Result, RuaError};
use serde::{Deserialize, Serialize};
use serde_json::Value;

// Journal transitions contain only small synchronous file commits. Chat/head
// locks additionally serialize activation against ordinary storage writes.
static JOURNAL_COMMIT: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatHandoffPhase {
    Prepared,
    Released,
    Activated,
    Aborted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatHandoffRecord {
    pub handoff_id: String,
    pub chat_id: String,
    pub old_placement: Value,
    pub new_placement: Value,
    /// The configuration in force when the handoff began. Placement is the only
    /// thing a handoff changes; the configuration only needs to still match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration: Option<Value>,
    /// The configuration this handoff installs on the new writer. A handoff is a
    /// prepared intent, so its candidate travels with the intent instead of an
    /// immutable snapshot that a reader could observe before it is applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_configuration: Option<Value>,
    pub requirements: Value,
    pub phase: ChatHandoffPhase,
    #[serde(default, flatten)]
    pub extra: serde_json::Map<String, Value>,
}

impl ChatHandoffRecord {
    fn validate(&self) -> Result<()> {
        if self.handoff_id.trim().is_empty() || self.chat_id.trim().is_empty() {
            return Err(invalid("Invalid Chat handoff identity"));
        }
        for placement in [&self.old_placement, &self.new_placement] {
            for key in ["device_id", "authority_epoch", "base_manifest_hash"] {
                if placement[key].as_str().is_none_or(str::is_empty) {
                    return Err(invalid("Chat handoff requires complete placements"));
                }
            }
        }
        if self.old_placement["authority_epoch"] == self.new_placement["authority_epoch"] {
            return Err(invalid("Chat handoff requires a new Authority epoch"));
        }
        Ok(())
    }
}

fn invalid(message: impl Into<String>) -> RuaError {
    RuaError::Validation {
        message: message.into(),
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &std::path::Path) -> Result<Option<T>> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| invalid(format!("Invalid {}: {error}", path.display()))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn save_json(path: &std::path::Path, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| invalid(format!("Serialize Chat handoff: {error}")))?;
    write_bytes_atomic_sync(path, &bytes)
}

impl WorkspaceSessionStore {
    /// Deliberately absent from the Chat sync/gap-transfer inventory.
    pub fn chat_handoff_path(&self, chat_id: &str) -> std::path::PathBuf {
        self.chat_dir(chat_id).join("local_handoff.json")
    }

    fn read_handoff(&self, chat_id: &str) -> Result<Option<ChatHandoffRecord>> {
        let record: Option<ChatHandoffRecord> = read_json(&self.chat_handoff_path(chat_id))?;
        if let Some(record) = &record {
            record.validate()?;
            if record.chat_id != chat_id {
                return Err(invalid("Chat handoff belongs to another Chat"));
            }
        }
        Ok(record)
    }

    pub async fn load_chat_handoff(&self, chat_id: &str) -> Result<Option<ChatHandoffRecord>> {
        let store = self.clone();
        let chat_id = chat_id.to_owned();
        tokio::task::spawn_blocking(move || store.read_handoff(&chat_id))
            .await
            .map_err(|error| invalid(format!("Read Chat handoff: {error}")))?
    }

    /// For a publication predicate evaluated while holding this Chat's write lock.
    /// Preparation uses that same lock, so a background result cannot cross the fence.
    pub fn chat_handoff_allows_history_write(
        &self,
        chat_id: &str,
        local_device_id: &str,
    ) -> Result<bool> {
        match self.read_handoff(chat_id)?.map(|record| record.phase) {
            None | Some(ChatHandoffPhase::Aborted) => Ok(true),
            Some(ChatHandoffPhase::Prepared | ChatHandoffPhase::Released) => Ok(false),
            Some(ChatHandoffPhase::Activated) => {
                let header: ChatSessionHeader = read_json(&self.chat_header_path(chat_id))?
                    .ok_or_else(|| invalid("Chat handoff header is unavailable"))?;
                let device = header
                    .extra
                    .get("authority_placement")
                    .and_then(|placement| placement.get("device_id"))
                    .and_then(Value::as_str)
                    .filter(|device| !device.is_empty())
                    .ok_or_else(|| invalid("Chat Authority placement is unavailable"))?;
                Ok(device == local_device_id)
            }
        }
    }

    fn require_handoff(&self, chat_id: &str, handoff_id: &str) -> Result<ChatHandoffRecord> {
        let record = self
            .read_handoff(chat_id)?
            .ok_or_else(|| invalid("Chat handoff not found"))?;
        if record.handoff_id != handoff_id {
            return Err(invalid("Chat handoff operation changed"));
        }
        Ok(record)
    }

    fn validate_handoff_head(
        &self,
        record: &ChatHandoffRecord,
        allow_candidate: bool,
    ) -> Result<()> {
        let header: ChatSessionHeader = read_json(&self.chat_header_path(&record.chat_id))?
            .ok_or_else(|| invalid("Chat handoff header is unavailable"))?;
        let placement = header.extra.get("authority_placement");
        if placement != Some(&record.old_placement)
            && !(allow_candidate && placement == Some(&record.new_placement))
        {
            return Err(invalid("Chat Authority placement changed during handoff"));
        }
        // The configuration itself must still be the one this handoff was
        // prepared against. Without version stamps, contents are the evidence.
        let configuration = header.extra.get("hook_configuration");
        let unchanged = match record.configuration.as_ref() {
            Some(expected) => configuration == Some(expected),
            None => configuration.is_some(),
        };
        let installed = allow_candidate
            && match record.candidate_configuration.as_ref() {
                Some(candidate) => configuration == Some(candidate),
                None => false,
            };
        if !unchanged && !installed {
            return Err(invalid("Chat configuration changed during handoff"));
        }
        Ok(())
    }

    /// Freeze the operation before network waits. A can save the intent without
    /// a candidate until C has resolved its own real execution directory.
    pub async fn prepare_chat_handoff(
        &self,
        record: &ChatHandoffRecord,
        candidate: Option<&Value>,
    ) -> Result<ChatHandoffRecord> {
        record.validate()?;
        if record.phase != ChatHandoffPhase::Prepared {
            return Err(invalid("New Chat handoff must be Prepared"));
        }
        let mut record = record.clone();
        let _chat = self.lock_chat_storage(&record.chat_id).await;
        let _journal = JOURNAL_COMMIT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Restating the same intent neither clears a candidate that was resolved
        // later nor replaces one that was already captured.
        let existing = self.read_handoff(&record.chat_id)?;
        if record.candidate_configuration.is_none() {
            record.candidate_configuration = existing
                .as_ref()
                .filter(|existing| existing.handoff_id == record.handoff_id)
                .and_then(|existing| existing.candidate_configuration.clone());
        }
        if let Some(candidate) = candidate {
            record.candidate_configuration = Some(candidate.clone());
        }
        let record = &record;
        if let Some(existing) = existing {
            if existing.handoff_id == record.handoff_id {
                // Resolving the candidate of an intent frozen before its directory
                // was known completes the same operation. Any other difference, or
                // replacing an already captured candidate, is a different operation.
                let facts_changed = existing.handoff_id != record.handoff_id
                    || existing.chat_id != record.chat_id
                    || existing.old_placement != record.old_placement
                    || existing.new_placement != record.new_placement
                    || existing.configuration != record.configuration
                    || existing.extra != record.extra
                    || existing.requirements != record.requirements
                    || existing
                        .candidate_configuration
                        .as_ref()
                        .zip(record.candidate_configuration.as_ref())
                        .is_some_and(|(existing, requested)| existing != requested);
                if facts_changed {
                    return Err(invalid("Chat handoff operation facts changed"));
                }
                if existing.candidate_configuration != record.candidate_configuration {
                    save_json(&self.chat_handoff_path(&record.chat_id), record)?;
                    return Ok(record.clone());
                }
                return Ok(existing);
            }
            if matches!(
                existing.phase,
                ChatHandoffPhase::Prepared | ChatHandoffPhase::Released
            ) {
                return Err(invalid("Another Chat handoff is still pending"));
            }
        }
        self.validate_handoff_head(record, false)?;
        save_json(&self.chat_handoff_path(&record.chat_id), record)?;
        Ok(record.clone())
    }

    pub async fn refresh_chat_handoff_requirements(
        &self,
        chat_id: &str,
        handoff_id: &str,
        requirements: &Value,
    ) -> Result<ChatHandoffRecord> {
        let _journal = JOURNAL_COMMIT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut record = self.require_handoff(chat_id, handoff_id)?;
        if record.phase != ChatHandoffPhase::Prepared {
            return Err(invalid("Only Prepared handoff requirements may change"));
        }
        record.requirements = requirements.clone();
        save_json(&self.chat_handoff_path(chat_id), &record)?;
        Ok(record)
    }

    /// Called inside the source's short permission commit barrier. No network,
    /// async wait or permission write occurs here.
    pub fn release_chat_handoff(
        &self,
        chat_id: &str,
        handoff_id: &str,
    ) -> Result<ChatHandoffRecord> {
        let _journal = JOURNAL_COMMIT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut record = self.require_handoff(chat_id, handoff_id)?;
        match record.phase {
            ChatHandoffPhase::Released | ChatHandoffPhase::Activated => return Ok(record),
            ChatHandoffPhase::Aborted => {
                return Err(invalid("Aborted Chat handoff cannot release"))
            }
            ChatHandoffPhase::Prepared => {}
        }
        self.validate_handoff_head(&record, false)?;
        if record.candidate_configuration.is_none() {
            return Err(invalid(
                "Chat handoff candidate configuration is unavailable",
            ));
        }
        record.phase = ChatHandoffPhase::Released;
        save_json(&self.chat_handoff_path(chat_id), &record)?;
        Ok(record)
    }

    pub async fn abort_chat_handoff(
        &self,
        chat_id: &str,
        handoff_id: &str,
    ) -> Result<ChatHandoffRecord> {
        let _journal = JOURNAL_COMMIT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut record = self.require_handoff(chat_id, handoff_id)?;
        match record.phase {
            ChatHandoffPhase::Aborted => return Ok(record),
            ChatHandoffPhase::Prepared => record.phase = ChatHandoffPhase::Aborted,
            _ => return Err(invalid("Released Chat handoff cannot abort")),
        }
        save_json(&self.chat_handoff_path(chat_id), &record)?;
        Ok(record)
    }

    /// Finish a durable Released intent under the same Chat/head locks used by
    /// local and replicated history. Every partial disk state is recoverable by
    /// this same ID; an unrelated epoch is never overwritten.
    pub async fn activate_chat_handoff(
        &self,
        chat_id: &str,
        handoff_id: &str,
    ) -> Result<ChatHandoffRecord> {
        let _chat = self.lock_chat_storage(chat_id).await;
        let _journal = JOURNAL_COMMIT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut record = self.require_handoff(chat_id, handoff_id)?;
        match record.phase {
            ChatHandoffPhase::Activated => return Ok(record),
            ChatHandoffPhase::Released => {}
            _ => return Err(invalid("Chat handoff has not been released")),
        }
        let observed: ChatSessionHeader = read_json(&self.chat_header_path(chat_id))?
            .ok_or_else(|| invalid("Chat handoff header is unavailable"))?;
        if observed.extra.get("authority_placement") == Some(&record.new_placement) {
            // The new writer may publish a later configuration before its
            // activation acknowledgement reaches A. Its exact placement plus a
            // complete configuration proves installation; only finish the local
            // journal, without restoring the older handoff candidate.
            let directory: crate::ChatExecutionDirectory = serde_json::from_value(
                observed
                    .extra
                    .get("hook_configuration")
                    .and_then(|configuration| {
                        configuration
                            .get("capability_configuration")?
                            .get(crate::EXECUTION_DIRECTORY_KEY)
                    })
                    .cloned()
                    .ok_or_else(|| invalid("Activated Chat execution directory is unavailable"))?,
            )
            .map_err(|error| {
                invalid(format!(
                    "Activated Chat execution directory is invalid: {error}"
                ))
            })?;
            if Some(directory.device_id.as_str()) != record.new_placement["device_id"].as_str() {
                return Err(invalid(
                    "Activated Chat execution directory does not belong to the new writer",
                ));
            }
            record.phase = ChatHandoffPhase::Activated;
            save_json(&self.chat_handoff_path(chat_id), &record)?;
            return Ok(record);
        }
        self.validate_handoff_head(&record, true)?;
        let candidate: Value = record
            .candidate_configuration
            .clone()
            .ok_or_else(|| invalid("Chat handoff candidate configuration is unavailable"))?;
        let mut header: ChatSessionHeader = read_json(&self.chat_header_path(chat_id))?
            .ok_or_else(|| invalid("Chat handoff header is unavailable"))?;
        let directory: crate::ChatExecutionDirectory = serde_json::from_value(
            candidate["capability_configuration"][crate::EXECUTION_DIRECTORY_KEY].clone(),
        )
        .map_err(|error| {
            invalid(format!(
                "Chat handoff execution directory is invalid: {error}"
            ))
        })?;
        if Some(directory.device_id.as_str()) != record.new_placement["device_id"].as_str() {
            return Err(invalid("Chat handoff directory belongs to another Device"));
        }
        if let Some(agent_id) = candidate.get("selected_agent_id").and_then(Value::as_str) {
            header.agent_id = agent_id.to_owned();
            header.metadata.agent_id = agent_id.to_owned();
        }
        // The configuration carries the execution position, so the header's own
        // directory record has to move with it in the same write.
        header.extra.insert(
            crate::EXECUTION_DIRECTORY_KEY.into(),
            serde_json::json!(directory),
        );
        header.extra.insert("hook_configuration".into(), candidate);
        header
            .extra
            .insert("authority_placement".into(), record.new_placement.clone());
        save_json(&self.chat_header_path(chat_id), &header)?;
        record.phase = ChatHandoffPhase::Activated;
        save_json(&self.chat_handoff_path(chat_id), &record)?;
        Ok(record)
    }
}
