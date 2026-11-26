//! Chat-owned routing references. Execution and full results remain on the Task Device.
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rua_task::{TaskId, TaskOwner, TaskRecord};
use serde::{Deserialize, Serialize};

// Registration and target evidence allocation share one short, synchronous transaction.
static REGISTRATION: Mutex<()> = Mutex::new(());
pub const MAX_TASK_REF_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatTaskExecution {
    pub device_id: String,
    pub public_cwd: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatTaskOrigin {
    pub turn_id: Option<String>,
    pub tool_call_id: Option<String>,
    pub agent_id: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatTaskEvidence {
    pub revision: u64,
    /// List projection with only the current Attempt. Online Get supplies full history.
    pub record: TaskRecord,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatTaskRef {
    pub task_id: TaskId,
    pub owner: TaskOwner,
    pub execution: ChatTaskExecution,
    pub origin: ChatTaskOrigin,
    /// None means acceptance is unconfirmed, not a fabricated Task state.
    pub last_target_evidence: Option<ChatTaskEvidence>,
    /// A confirmed outcome of the unique Create/Adopt invocation, not a Task state.
    /// Actual target acceptance evidence always takes precedence over this fact.
    #[serde(default)]
    pub not_accepted: bool,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl ChatTaskRef {
    pub fn validate_id(id: &str) -> io::Result<()> {
        if id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(invalid("Invalid Task reference ID"));
        }
        Ok(())
    }

    pub fn validate(&self) -> io::Result<()> {
        Self::validate_id(&self.task_id)?;
        Self::validate_id(self.owner.chat_id())?;
        if !identifier(&self.execution.device_id)
            || self.execution.device_id == "unknown"
            || !rua_core::execution_directory::is_device_absolute_path(&self.execution.public_cwd)
            || self.execution.public_cwd.chars().any(char::is_control)
            || !identifier(&self.origin.agent_id)
            || [&self.origin.turn_id, &self.origin.tool_call_id]
                .into_iter()
                .flatten()
                .any(|value| !identifier(value))
        {
            return Err(invalid("Incomplete Task reference identity"));
        }
        if let Some(evidence) = &self.last_target_evidence {
            let record = &evidence.record;
            let attempt = record
                .attempts
                .first()
                .ok_or_else(|| invalid("Task evidence has no current Attempt"))?;
            if evidence.revision == 0
                || record.task_id != self.task_id
                || record.spec.owner != self.owner
                || record.spec.origin.turn_id != self.origin.turn_id
                || record.spec.origin.tool_call_id != self.origin.tool_call_id
                || record.attempt_no == 0
                || record.attempts.len() != 1
                || attempt.attempt_no != record.attempt_no
                || (attempt.state != record.state
                    && !(record.state == rua_task::TaskState::WaitingApproval
                        && attempt.state == rua_task::TaskState::Running))
                || !identifier(&attempt.authorization_agent_id)
                || attempt.device.device_id.as_deref() != Some(self.execution.device_id.as_str())
                || attempt.device.public_cwd.as_deref() != Some(self.execution.public_cwd.as_str())
            {
                return Err(invalid(
                    "Task evidence differs from its fixed reference identity",
                ));
            }
        }
        Ok(())
    }

    pub fn load(chat_dir: &Path, chat_id: &str, task_id: &str) -> io::Result<Option<Self>> {
        Self::validate_id(chat_id)?;
        Self::validate_id(task_id)?;
        let directory = chat_dir.join("task_refs");
        if !existing_directory(chat_dir)? || !existing_directory(&directory)? {
            return Ok(None);
        }
        let path = directory.join(format!("{task_id}.json"));
        match fs::symlink_metadata(&path) {
            Ok(metadata)
                if metadata.is_symlink()
                    || !metadata.is_file()
                    || metadata.len() > MAX_TASK_REF_BYTES as u64 =>
            {
                return Err(invalid("Invalid Task reference file"));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
            _ => {}
        }
        let reference: Self = serde_json::from_slice(&fs::read(path)?).map_err(io::Error::other)?;
        reference.validate()?;
        if reference.task_id != task_id || reference.owner.chat_id() != chat_id {
            return Err(invalid("Task reference belongs to a different record"));
        }
        Ok(Some(reference))
    }

    pub fn list(chat_dir: &Path, chat_id: &str) -> io::Result<Vec<Self>> {
        Self::validate_id(chat_id)?;
        let directory = chat_dir.join("task_refs");
        if !existing_directory(chat_dir)? || !existing_directory(&directory)? {
            return Ok(Vec::new());
        }
        let mut references = Vec::new();
        for entry in fs::read_dir(directory)? {
            let path = entry?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let task_id = path
                .file_stem()
                .and_then(|value| value.to_str())
                .ok_or_else(|| invalid("Invalid Task reference filename"))?;
            if let Some(reference) = Self::load(chat_dir, chat_id, task_id)? {
                references.push(reference);
            }
        }
        references.sort_by(|left, right| left.task_id.cmp(&right.task_id));
        Ok(references)
    }

    /// Merge target revisions, independently of filesystem timestamps or relay HLCs.
    pub fn merged(&self, existing: &Self) -> io::Result<Self> {
        self.validate()?;
        existing.validate()?;
        if self.task_id != existing.task_id
            || self.owner != existing.owner
            || self.execution != existing.execution
            || self.origin != existing.origin
        {
            return Err(invalid("Task reference identity is immutable"));
        }
        let mut retained = existing.clone();
        retained.not_accepted |= self.not_accepted;
        match (&self.last_target_evidence, &existing.last_target_evidence) {
            (None, _) => return Ok(retained),
            (Some(incoming), Some(previous)) => {
                if incoming.record.attempt_no < previous.record.attempt_no
                    || incoming.revision < previous.revision
                {
                    return Ok(retained);
                }
                if incoming.revision == previous.revision {
                    if incoming != previous {
                        return Err(invalid(
                            "Conflicting Task evidence at the same target revision",
                        ));
                    }
                    return Ok(retained);
                }
                if incoming.record.spec != previous.record.spec {
                    return Err(invalid("Task specification is immutable"));
                }
                if incoming.record.attempt_no == previous.record.attempt_no {
                    let incoming_attempt = &incoming.record.attempts[0];
                    let previous_attempt = &previous.record.attempts[0];
                    if incoming_attempt.authorization_agent_id
                        != previous_attempt.authorization_agent_id
                        || incoming_attempt.authorization_origin
                            != previous_attempt.authorization_origin
                        || incoming_attempt.device != previous_attempt.device
                    {
                        return Err(invalid(
                            "Task Attempt authorization and execution source are immutable",
                        ));
                    }
                }
                if incoming.record.attempt_no == previous.record.attempt_no
                    && previous.record.state.is_terminal()
                    && incoming.record.state != previous.record.state
                {
                    return Err(invalid("A terminal Task Attempt cannot change state"));
                }
                // Evidence revisions record persistence order, not observation
                // order: a capture reads TaskRuntime first and allocates its
                // revision only afterwards, so two captures of one Attempt can
                // persist out of order. Keep the newer observation instead of
                // letting a stale non-terminal snapshot rewind the projection.
                if incoming.record.attempt_no == previous.record.attempt_no
                    && incoming.record.updated_at < previous.record.updated_at
                {
                    return Ok(retained);
                }
            }
            _ => {}
        }
        let mut result = self.clone();
        result.not_accepted |= existing.not_accepted;
        result.extra = existing.extra.clone();
        result.extra.extend(self.extra.clone());
        if let (Some(incoming), Some(previous)) = (
            &mut result.last_target_evidence,
            &existing.last_target_evidence,
        ) {
            let additions = incoming.extra.clone();
            incoming.extra = previous.extra.clone();
            incoming.extra.extend(additions);
        }
        Ok(result)
    }

    /// Register an unconfirmed route or merge actual target evidence. Returns whether it changed.
    pub fn register(&self, chat_dir: &Path, chat_id: &str) -> io::Result<bool> {
        self.validate()?;
        if self.owner.chat_id() != chat_id {
            return Err(invalid("Chat does not own this Task reference"));
        }
        let _guard = REGISTRATION
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let existing = Self::load(chat_dir, chat_id, &self.task_id)?;
        let merged = match &existing {
            Some(existing) => self.merged(existing)?,
            None => self.clone(),
        };
        if existing.as_ref() == Some(&merged) {
            return Ok(false);
        }
        merged.write(chat_dir)?;
        Ok(true)
    }

    /// Record only a confirmed non-acceptance: a verified target response or a
    /// source failure before dispatch. Timeouts, disconnections and Get(NotFound)
    /// cannot establish this fact. This never changes an accepted Task's state.
    pub fn mark_not_accepted(chat_dir: &Path, chat_id: &str, task_id: &str) -> io::Result<Self> {
        let _guard = REGISTRATION
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut reference = Self::load(chat_dir, chat_id, task_id)?
            .ok_or_else(|| invalid("Task reference must exist before resolving acceptance"))?;
        if reference.last_target_evidence.is_none() && !reference.not_accepted {
            reference.not_accepted = true;
            reference.write(chat_dir)?;
        }
        Ok(reference)
    }

    /// Called on the execution Device after reading its TaskRuntime. Allocates a durable
    /// revision only when the current-Attempt list projection changes. Callers serialize
    /// target Task reads with this capture; source devices only register returned evidence.
    pub fn capture_target_evidence(
        chat_dir: &Path,
        chat_id: &str,
        task_id: &str,
        local_device_id: &str,
        mut record: TaskRecord,
    ) -> io::Result<Self> {
        let _guard = REGISTRATION
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut reference = Self::load(chat_dir, chat_id, task_id)?
            .ok_or_else(|| invalid("Task reference must be registered before target capture"))?;
        if reference.execution.device_id != local_device_id {
            return Err(invalid(
                "Only the execution Device may capture Task evidence",
            ));
        }
        record
            .attempts
            .retain(|attempt| attempt.attempt_no == record.attempt_no);
        if reference
            .last_target_evidence
            .as_ref()
            .is_some_and(|previous| previous.record == record)
        {
            return Ok(reference);
        }
        let revision = reference
            .last_target_evidence
            .as_ref()
            .map_or(Some(1), |evidence| evidence.revision.checked_add(1))
            .ok_or_else(|| invalid("Task evidence revision exhausted"))?;
        let previous = reference.clone();
        reference.last_target_evidence = Some(ChatTaskEvidence {
            revision,
            record,
            extra: previous
                .last_target_evidence
                .as_ref()
                .map(|evidence| evidence.extra.clone())
                .unwrap_or_default(),
        });
        let merged = reference.merged(&previous)?;
        if merged != previous {
            merged.write(chat_dir)?;
        }
        Ok(merged)
    }

    fn write(&self, chat_dir: &Path) -> io::Result<()> {
        let bytes = serde_json::to_vec(self)?;
        if bytes.len() > MAX_TASK_REF_BYTES {
            return Err(invalid("Task reference summary exceeds its bounded size"));
        }
        fs::create_dir_all(chat_dir)?;
        existing_directory(chat_dir)?;
        let directory: PathBuf = chat_dir.join("task_refs");
        if !existing_directory(&directory)? {
            fs::create_dir(&directory)?;
        }
        rua_config::atomic_write(&directory.join(format!("{}.json", self.task_id)), &bytes)
    }
}

fn existing_directory(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.is_symlink() => Ok(true),
        Ok(_) => Err(invalid("Task reference directory is not a real directory")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn identifier(value: &str) -> bool {
    !value.is_empty() && value.trim() == value && !value.chars().any(char::is_control)
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
