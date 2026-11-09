use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use rua_core::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    pub id: String,
    pub cron_expr: String,
    pub agent_id: String,
    pub prompt: String,
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub next_run_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_run_at: Option<DateTime<Utc>>,

    #[serde(default = "default_missed_policy")]
    pub missed_policy: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn default_missed_policy() -> String {
    "run_once".into()
}

#[async_trait]
pub trait SystemScheduler: Send + Sync {
    async fn register_next_run(&self, job: &CronJob, next_run_at: DateTime<Utc>) -> Result<()>;

    async fn cancel(&self, job_id: &str) -> Result<()>;
}
