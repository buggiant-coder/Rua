use std::sync::Arc;

use dashmap::DashMap;
use rua_config::types::cron::{CronJobSpec, CronJobState};
use tracing;

use crate::executor::{CronExecutor, CronJobStatus, JobExecutionResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunJobStatus {
    Spawned,

    NotFound,

    NoExecutor,
}

pub struct CronRunner {
    executor: Option<Arc<CronExecutor>>,
    jobs: Arc<DashMap<String, CronJobSpec>>,
    states: Arc<DashMap<String, CronJobState>>,
}

impl CronRunner {
    pub fn new(
        executor: Option<Arc<CronExecutor>>,
        jobs: Arc<DashMap<String, CronJobSpec>>,
        states: Arc<DashMap<String, CronJobState>>,
    ) -> Self {
        Self {
            executor,
            jobs,
            states,
        }
    }

    pub fn run_job(&self, job_id: &str) -> RunJobStatus {
        let executor = match &self.executor {
            Some(e) => Arc::clone(e),
            None => {
                tracing::error!(job_id = %job_id, "run_job called but no executor configured");
                return RunJobStatus::NoExecutor;
            }
        };

        let job = match self.jobs.get(job_id) {
            Some(entry) => entry.value().clone(),
            None => {
                tracing::warn!(job_id = %job_id, "run_job: job not found");
                return RunJobStatus::NotFound;
            }
        };

        let states = Arc::clone(&self.states);
        let job_id_owned = job_id.to_string();

        set_status(&states, &job_id_owned, CronJobStatus::Running);

        tokio::spawn(async move {
            let max_retries = job.max_retries.unwrap_or(0);
            let retry_delay = std::time::Duration::from_secs(job.retry_delay_secs.unwrap_or(0));

            let mut result = executor.execute(&job).await;
            let mut attempt = 1u32;

            while !result.success && attempt <= max_retries {
                tracing::warn!(
                    job_id = %job_id_owned,
                    attempt = attempt,
                    max_retries = max_retries,
                    error = ?result.error,
                    retry_delay_secs = retry_delay.as_secs(),
                    "Cron job failed, retrying"
                );
                tokio::time::sleep(retry_delay).await;
                attempt += 1;
                result = executor.execute(&job).await;
            }

            write_state(&states, &job_id_owned, &result);
            log_result(&job_id_owned, &result);
        });

        RunJobStatus::Spawned
    }

    pub fn get_state(&self, job_id: &str) -> Option<CronJobState> {
        self.states.get(job_id).map(|e| e.value().clone())
    }
}

fn set_status(states: &DashMap<String, CronJobState>, job_id: &str, status: CronJobStatus) {
    let mut state = states.entry(job_id.to_string()).or_default();
    state.last_status = Some(status.as_str().to_string());
}

pub fn classify_result(result: &JobExecutionResult) -> CronJobStatus {
    if result.success {
        return CronJobStatus::Success;
    }
    match result.error.as_deref() {
        Some(e) if e.contains("max concurrency") => CronJobStatus::Skipped,
        Some(e) if e.contains("timed out") => CronJobStatus::Cancelled,
        _ => CronJobStatus::Error,
    }
}

fn write_state(states: &DashMap<String, CronJobState>, job_id: &str, result: &JobExecutionResult) {
    let status = classify_result(result);
    let mut state = states.entry(job_id.to_string()).or_default();
    state.last_run_at = Some(chrono::Utc::now());
    state.last_status = Some(status.as_str().to_string());
    state.last_error = result.error.clone();
}

fn log_result(job_id: &str, result: &JobExecutionResult) {
    if result.success {
        tracing::info!(
            job_id = %job_id,
            duration_ms = result.duration_ms,
            "Cron run_job completed successfully"
        );
    } else {
        tracing::warn!(
            job_id = %job_id,
            error = ?result.error,
            duration_ms = result.duration_ms,
            "Cron run_job failed"
        );
    }
}
