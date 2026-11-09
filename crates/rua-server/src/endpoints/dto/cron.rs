use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CronScheduleSpec {
    #[serde(default, rename = "type")]
    pub schedule_type: String,
    #[serde(default)]
    pub cron: String,
    #[serde(default)]
    pub run_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub timezone: String,
    #[serde(default)]
    pub repeat_every_days: Option<u32>,
    #[serde(default)]
    pub repeat_end_type: Option<String>,
    #[serde(default)]
    pub repeat_until: Option<DateTime<Utc>>,
    #[serde(default)]
    pub repeat_count: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CronDispatchTarget {
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub session_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CronDispatchSpec {
    #[serde(default, rename = "type")]
    pub dispatch_type: String,
    #[serde(default)]
    pub channel: String,
    #[serde(default)]
    pub target: CronDispatchTarget,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub meta: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CronJobRuntimeSpec {
    #[serde(default)]
    pub max_concurrency: Option<u32>,
    #[serde(default)]
    pub timeout_seconds: Option<u32>,
    #[serde(default)]
    pub misfire_grace_seconds: Option<u32>,
    #[serde(default)]
    pub share_session: Option<bool>,
    #[serde(default)]
    pub tool_safety: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub enabled: bool,

    #[serde(default)]
    pub schedule: CronScheduleSpec,

    #[serde(default)]
    pub task_type: String,

    #[serde(default)]
    pub text: Option<String>,

    #[serde(default)]
    pub request: Option<serde_json::Value>,
    #[serde(default)]
    pub save_result_to_inbox: Option<bool>,

    #[serde(default)]
    pub dispatch: CronDispatchSpec,

    #[serde(default)]
    pub runtime: CronJobRuntimeSpec,

    #[serde(default)]
    pub meta: serde_json::Map<String, serde_json::Value>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJobCreate {
    pub name: String,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub schedule: CronScheduleSpec,
    #[serde(default)]
    pub task_type: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub request: Option<serde_json::Value>,
    #[serde(default)]
    pub save_result_to_inbox: Option<bool>,
    #[serde(default)]
    pub dispatch: CronDispatchSpec,
    #[serde(default)]
    pub runtime: Option<CronJobRuntimeSpec>,
    #[serde(default)]
    pub meta: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJobUpdate {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub schedule: Option<CronScheduleSpec>,
    #[serde(default)]
    pub task_type: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub request: Option<serde_json::Value>,
    #[serde(default)]
    pub save_result_to_inbox: Option<bool>,
    #[serde(default)]
    pub dispatch: Option<CronDispatchSpec>,
    #[serde(default)]
    pub runtime: Option<CronJobRuntimeSpec>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub meta: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CronJobState {
    #[serde(default)]
    pub next_run_at: Option<String>,
    #[serde(default)]
    pub last_run_at: Option<String>,
    #[serde(default)]
    pub last_status: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<CronDispatchHistoryEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CronDispatchHistoryEntry {
    pub id: String,
    pub job_id: String,
    pub agent_id: String,
    pub job_name: String,
    pub task_type: String,
    pub channel: String,
    pub user_id: String,
    pub session_id: String,
    pub mode: String,
    pub status: String,
    pub success: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub duration_ms: u64,
    pub completed_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJobRunOverride {
    #[serde(default)]
    pub message: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CronDispatchTargetItem {
    pub channel: String,
    pub user_id: String,
    pub session_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CronDispatchTargetsResponse {
    #[serde(default)]
    pub channels: Vec<String>,
    #[serde(default)]
    pub items: Vec<CronDispatchTargetItem>,
}
