use crate::serde_helpers::deserialize_null_default;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobsFile {
    #[serde(
        default = "default_version",
        deserialize_with = "deserialize_null_default"
    )]
    pub version: u32,
    #[serde(default)]
    pub jobs: BTreeMap<String, CronJobSpec>,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn default_version() -> u32 {
    1
}

impl Default for JobsFile {
    fn default() -> Self {
        Self {
            version: 1,
            jobs: BTreeMap::new(),
            extra: serde_json::Map::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJobSpec {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub name: String,
    #[serde(
        default = "default_true",
        deserialize_with = "deserialize_null_default"
    )]
    pub enabled: bool,

    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub paused: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub schedule: ScheduleSpec,
    #[serde(
        default = "default_task_type",
        deserialize_with = "deserialize_null_default"
    )]
    pub task_type: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub request: Option<serde_json::Value>,
    #[serde(default)]
    pub dispatch: Option<DispatchSpec>,
    #[serde(default)]
    pub save_result_to_inbox: Option<bool>,
    #[serde(default)]
    pub runtime: Option<JobRuntimeSpec>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub meta: serde_json::Map<String, serde_json::Value>,

    #[serde(default)]
    pub cron_expr: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub missed_policy: Option<String>,
    #[serde(default)]
    pub max_retries: Option<u32>,
    #[serde(default)]
    pub retry_delay_secs: Option<u64>,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn default_true() -> bool {
    true
}

fn default_task_type() -> String {
    "agent".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ScheduleSpec {
    #[serde(
        default = "default_schedule_type",
        deserialize_with = "deserialize_null_default"
    )]
    pub r#type: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub cron: String,
    #[serde(default)]
    pub run_at: Option<DateTime<Utc>>,
    #[serde(
        default = "default_timezone",
        deserialize_with = "deserialize_null_default"
    )]
    pub timezone: String,
    #[serde(default)]
    pub repeat_every_days: Option<u32>,
    #[serde(default)]
    pub repeat_end_type: Option<String>,
    #[serde(default)]
    pub repeat_until: Option<DateTime<Utc>>,
    #[serde(default)]
    pub repeat_count: Option<u32>,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn default_schedule_type() -> String {
    "cron".into()
}

fn default_timezone() -> String {
    iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".into())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchSpec {
    #[serde(
        default = "default_dispatch_type",
        deserialize_with = "deserialize_null_default"
    )]
    pub r#type: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub channel: String,
    #[serde(default)]
    pub target: Option<DispatchTarget>,
    #[serde(
        default = "default_dispatch_mode",
        deserialize_with = "deserialize_null_default"
    )]
    pub mode: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub meta: serde_json::Map<String, serde_json::Value>,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn default_dispatch_type() -> String {
    "channel".into()
}

fn default_dispatch_mode() -> String {
    "stream".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchTarget {
    pub user_id: String,
    pub session_id: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRuntimeSpec {
    #[serde(default = "default_one", deserialize_with = "deserialize_null_default")]
    pub max_concurrency: u32,
    #[serde(
        default = "default_timeout",
        deserialize_with = "deserialize_null_default"
    )]
    pub timeout_seconds: u32,
    #[serde(
        default = "default_misfire_grace",
        deserialize_with = "deserialize_null_default"
    )]
    pub misfire_grace_seconds: u32,
    #[serde(
        default = "default_true",
        deserialize_with = "deserialize_null_default"
    )]
    pub share_session: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub tool_safety: bool,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn default_one() -> u32 {
    1
}
fn default_timeout() -> u32 {
    120
}
fn default_misfire_grace() -> u32 {
    600
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CronJobState {
    #[serde(default)]
    pub next_run_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_run_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_status: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}
