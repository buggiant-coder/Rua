//! Local, non-exported intent for a cooperative Chat writer handoff. Released is
//! the commit intent: recovery may finish it, but must never restore the old writer.

use std::sync::Mutex;

use super::{
    chat_write_lock_for_path, write_bytes_atomic_sync, ChatConfigurationHead, ChatSessionHeader,
    WorkspaceSessionStore,
};
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
    pub expected_configuration_epoch: String,
    pub candidate_configuration_epoch: String,
    pub requirements: Value,
    pub phase: ChatHandoffPhase,
    #[serde(default, flatten)]
    pub extra: serde_json::Map<String, Value>,
}

impl ChatHandoffRecord {
    fn same_operation(&self, other: &Self) -> bool {
        self.handoff_id == other.handoff_id
            && self.chat_id == other.chat_id
            && self.old_placement == other.old_placement
            && self.new_placement == other.new_placement
            && self.expected_configuration_epoch == other.expected_configuration_epoch
            && self.candidate_configuration_epoch == other.candidate_configuration_epoch
            && self.extra == other.extra
    }

    fn validate(&self) -> Result<()> {
        if self.handoff_id.trim().is_empty()
            || self.chat_id.trim().is_empty()
            || self.expected_configuration_epoch == self.candidate_configuration_epoch
        {
            return Err(invalid(
                "Invalid Chat handoff identity/configuration epochs",
            ));
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
        let head: Option<ChatConfigurationHead> =
            read_json(&self.configuration_head_path(&record.chat_id))?;
        let epoch = head.as_ref().map(|head| head.epoch.as_str()).or_else(|| {
            header
                .extra
                .get("hook_configuration")?
                .get("epoch")?
                .as_str()
        });
        if epoch != Some(record.expected_configuration_epoch.as_str())
            && !(allow_candidate && epoch == Some(record.candidate_configuration_epoch.as_str()))
        {
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
        self.configuration_snapshot_path(&record.chat_id, &record.expected_configuration_epoch)?;
        self.configuration_snapshot_path(&record.chat_id, &record.candidate_configuration_epoch)?;
        if record.phase != ChatHandoffPhase::Prepared {
            return Err(invalid("New Chat handoff must be Prepared"));
        }
        let _chat = self.lock_chat_storage(&record.chat_id).await;
        let head_lock = chat_write_lock_for_path(self.configuration_head_path(&record.chat_id));
        let _head = head_lock.lock().await;
        if let Some(candidate) = candidate {
            if candidate["epoch"].as_str() != Some(record.candidate_configuration_epoch.as_str()) {
                return Err(invalid("Chat handoff candidate epoch does not match"));
            }
            self.save_configuration_snapshot(&record.chat_id, candidate)
                .await?;
        }
        let _journal = JOURNAL_COMMIT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(existing) = self.read_handoff(&record.chat_id)? {
            if existing.handoff_id == record.handoff_id {
                if !existing.same_operation(record) || existing.requirements != record.requirements
                {
                    return Err(invalid("Chat handoff operation facts changed"));
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
        if read_json::<Value>(
            &self.configuration_snapshot_path(chat_id, &record.candidate_configuration_epoch)?,
        )?
        .is_none()
        {
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
        let head_lock = chat_write_lock_for_path(self.configuration_head_path(chat_id));
        let _head = head_lock.lock().await;
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
            // complete authoritative head proves installation; only finish the
            // local journal, without restoring the older handoff candidate.
            let head: ChatConfigurationHead = read_json(&self.configuration_head_path(chat_id))?
                .ok_or_else(|| invalid("Activated Chat configuration head is unavailable"))?;
            let configuration: Value = read_json(
                &self.configuration_snapshot_path(chat_id, &head.epoch)?,
            )?
            .ok_or_else(|| invalid("Activated Chat configuration snapshot is unavailable"))?;
            if configuration["epoch"].as_str() != Some(head.epoch.as_str()) {
                return Err(invalid(
                    "Activated Chat configuration epoch does not match its head",
                ));
            }
            head.validate_configuration(&configuration)?;
            let directory: crate::ChatExecutionDirectory = serde_json::from_value(
                configuration["capability_configuration"][crate::EXECUTION_DIRECTORY_KEY].clone(),
            )
            .map_err(|error| {
                invalid(format!(
                    "Activated Chat execution directory is invalid: {error}"
                ))
            })?;
            if head.execution_directory.as_ref() != Some(&directory)
                || Some(directory.device_id.as_str()) != record.new_placement["device_id"].as_str()
            {
                return Err(invalid(
                    "Activated Chat execution directory does not match its head",
                ));
            }
            record.phase = ChatHandoffPhase::Activated;
            save_json(&self.chat_handoff_path(chat_id), &record)?;
            return Ok(record);
        }
        self.validate_handoff_head(&record, true)?;
        let candidate: Value = read_json(
            &self.configuration_snapshot_path(chat_id, &record.candidate_configuration_epoch)?,
        )?
        .ok_or_else(|| invalid("Chat handoff candidate configuration is unavailable"))?;
        let mut header: ChatSessionHeader = read_json(&self.chat_header_path(chat_id))?
            .ok_or_else(|| invalid("Chat handoff header is unavailable"))?;
        let mut head: ChatConfigurationHead =
            read_json(&self.configuration_head_path(chat_id))?.unwrap_or(
                ChatConfigurationHead::from_header(&record.expected_configuration_epoch, &header)?,
            );
        head.validate_configuration(&candidate)?;
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
        head.epoch.clone_from(&record.candidate_configuration_epoch);
        head.execution_directory = Some(directory);
        save_json(&self.configuration_head_path(chat_id), &head)?;
        head.apply_selection(&mut header);
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
