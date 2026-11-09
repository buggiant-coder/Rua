use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use rua_core::Result;

pub const ACTIVE_WORK_PLAN_FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActiveWorkPlanStepStatus {
    Pending,
    InProgress,
    Completed,
}

impl ActiveWorkPlanStepStatus {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActiveWorkPlanStep {
    pub step: String,
    pub status: ActiveWorkPlanStepStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActiveWorkPlanContent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
    pub plan: Vec<ActiveWorkPlanStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActiveWorkPlanSnapshot {
    pub format_version: u32,
    pub chat_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_injected_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_injected_checkpoint_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
    pub plan: Vec<ActiveWorkPlanStep>,
    pub updated_at: String,
    pub checksum: String,
}

pub(crate) fn active_work_plan_checksum(snapshot: &ActiveWorkPlanSnapshot) -> Result<String> {
    let mut payload = snapshot.clone();
    payload.checksum.clear();
    let bytes = serde_json::to_vec(&payload).map_err(|error| rua_core::RuaError::Io {
        message: format!("failed to serialize active work plan checksum payload: {error}"),
    })?;
    let digest = Sha256::digest(bytes);
    Ok(format!("sha256:{digest:x}"))
}
