use std::path::{Path, PathBuf};

use chrono::Utc;
use rua_config::types::cron::CronJobSpec;
use serde::{Deserialize, Serialize};

const HISTORY_FILE: &str = "cron_dispatch_history.json";
const HISTORY_CAPACITY: usize = 500;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

#[derive(Debug, Clone)]
pub struct CronDispatchHistoryStore {
    path: PathBuf,
}

impl CronDispatchHistoryStore {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            path: data_dir.into().join(HISTORY_FILE),
        }
    }

    pub fn append(&self, entry: CronDispatchHistoryEntry) {
        let mut entries = self.load_all();
        if entries.iter().any(|existing| existing.id == entry.id) {
            return;
        }
        entries.push(entry);
        prune_history(&mut entries);
        let value = serde_json::to_value(&entries).unwrap_or_else(|_| serde_json::json!([]));
        let _ = rua_config::atomic_write_json(&self.path, &value);
    }

    pub fn load_all(&self) -> Vec<CronDispatchHistoryEntry> {
        let Ok(content) = std::fs::read_to_string(&self.path) else {
            return Vec::new();
        };
        let mut entries =
            serde_json::from_str::<Vec<CronDispatchHistoryEntry>>(&content).unwrap_or_default();
        prune_history(&mut entries);
        entries
    }

    pub fn list_for_job(&self, job_id: &str) -> Vec<CronDispatchHistoryEntry> {
        self.load_all()
            .into_iter()
            .filter(|entry| entry.job_id == job_id)
            .collect()
    }
}

pub fn record_cron_dispatch_result(
    data_dir: &Path,
    spec: &CronJobSpec,
    success: bool,
    error: Option<String>,
    duration_ms: u64,
) {
    let entry = build_history_entry(spec, success, error, duration_ms);
    CronDispatchHistoryStore::new(data_dir).append(entry);
}

pub fn build_history_entry(
    spec: &CronJobSpec,
    success: bool,
    error: Option<String>,
    duration_ms: u64,
) -> CronDispatchHistoryEntry {
    let dispatch = spec.dispatch.as_ref();
    let target = dispatch.and_then(|dispatch| dispatch.target.as_ref());
    let completed_at = Utc::now().to_rfc3339();
    let status = classify_history_status(success, error.as_deref());
    CronDispatchHistoryEntry {
        id: format!("{}:{}:{}", spec.id, completed_at, status),
        job_id: spec.id.clone(),
        agent_id: spec.agent_id.as_deref().unwrap_or("default").to_string(),
        job_name: spec.name.clone(),
        task_type: spec.task_type.clone(),
        channel: dispatch
            .map(|dispatch| dispatch.channel.as_str())
            .filter(|channel| !channel.is_empty())
            .unwrap_or("console")
            .to_string(),
        user_id: target
            .map(|target| target.user_id.as_str())
            .filter(|user_id| !user_id.is_empty())
            .unwrap_or("default")
            .to_string(),
        session_id: target
            .map(|target| target.session_id.as_str())
            .filter(|session_id| !session_id.is_empty())
            .unwrap_or("default")
            .to_string(),
        mode: dispatch
            .map(|dispatch| dispatch.mode.as_str())
            .filter(|mode| !mode.is_empty())
            .unwrap_or("stream")
            .to_string(),
        status: status.to_string(),
        success,
        error,
        duration_ms,
        completed_at,
    }
}

fn classify_history_status(success: bool, error: Option<&str>) -> &'static str {
    if success {
        return "success";
    }
    match error {
        Some(value) if value.contains("max concurrency") => "skipped",
        Some(value) if value.contains("missed scheduled run") => "skipped",
        Some(value) if value.contains("timed out") => "cancelled",
        _ => "error",
    }
}

fn prune_history(entries: &mut Vec<CronDispatchHistoryEntry>) {
    if entries.len() > HISTORY_CAPACITY {
        let remove_count = entries.len() - HISTORY_CAPACITY;
        entries.drain(0..remove_count);
    }
}
