use chrono::{DateTime, Duration, Utc};
use dashmap::DashMap;
use rua_config::types::cron::{CronJobSpec, CronJobState, ScheduleSpec};
use rua_contract::cron::{CronJob, SystemScheduler};
use rua_core::RuaError;
use std::sync::Arc;
use tracing;

use crate::executor::{CronExecutor, JobExecutionResult};
use crate::normalize::normalize_cron_expr;
use crate::repo::JsonCronJobRepository;

#[derive(Debug, Clone, Default)]
pub struct JobStateEntry {
    pub state: CronJobState,
}

pub struct CronManager {
    repo: Arc<JsonCronJobRepository>,

    executor: Option<Arc<CronExecutor>>,

    scheduler: Option<Arc<dyn SystemScheduler>>,

    states: DashMap<String, JobStateEntry>,

    running: std::sync::atomic::AtomicBool,
}

impl CronManager {
    pub fn new(repo: Arc<JsonCronJobRepository>) -> Self {
        Self {
            repo,
            executor: None,
            scheduler: None,
            states: DashMap::new(),
            running: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub fn set_executor(&mut self, executor: Arc<CronExecutor>) {
        self.executor = Some(executor);
    }

    pub fn set_scheduler(&mut self, scheduler: Arc<dyn SystemScheduler>) {
        self.scheduler = Some(scheduler);
    }

    pub async fn start(&self) -> Result<(), RuaError> {
        tracing::info!("Starting cron manager");
        self.running
            .store(true, std::sync::atomic::Ordering::SeqCst);

        let jobs = self.repo.load_jobs().map_err(|e| RuaError::Cron {
            job_id: String::new(),
            message: format!("Failed to load jobs: {e}"),
        })?;

        tracing::info!("Loaded {} cron jobs", jobs.len());

        for job in &jobs {
            if !self.states.contains_key(&job.id) {
                let mut entry = JobStateEntry::default();
                if job.enabled {
                    if let Some(next) = compute_next_run_for_schedule(&job.schedule) {
                        entry.state.next_run_at = Some(next);
                    }
                }
                self.states.insert(job.id.clone(), entry);
            }
        }

        Ok(())
    }

    pub async fn stop(&self) {
        tracing::info!("Stopping cron manager");
        self.running
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn is_running(&self) -> bool {
        self.running.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn list_jobs(&self) -> Result<Vec<CronJobSpec>, RuaError> {
        self.repo.load_jobs().map_err(|e| RuaError::Cron {
            job_id: String::new(),
            message: format!("Failed to load jobs: {e}"),
        })
    }

    pub fn get_job(&self, job_id: &str) -> Result<Option<CronJobSpec>, RuaError> {
        self.repo.get_job(job_id).map_err(|e| RuaError::Cron {
            job_id: job_id.to_string(),
            message: format!("Failed to get job: {e}"),
        })
    }

    pub fn create_or_replace(&self, mut job: CronJobSpec) -> Result<CronJobSpec, RuaError> {
        if job.id.is_empty() {
            job.id = uuid::Uuid::new_v4().to_string();
        }

        if job.schedule.r#type.is_empty() {
            job.schedule.r#type = "cron".to_string();
        }

        if job.schedule.r#type == "cron" {
            let cron_str = if !job.schedule.cron.is_empty() {
                &job.schedule.cron
            } else if let Some(ref expr) = job.cron_expr {
                expr
            } else {
                return Err(RuaError::Validation {
                    message: "No cron expression provided".to_string(),
                });
            };

            let normalized = normalize_cron_expr(cron_str).map_err(|e| RuaError::Validation {
                message: format!("Invalid cron expression: {e}"),
            })?;

            validate_cron_expr(&normalized)?;

            job.schedule.cron = normalized;
            job.schedule.run_at = None;
            job.schedule.repeat_every_days = None;
            job.schedule.repeat_end_type = None;
            job.schedule.repeat_until = None;
            job.schedule.repeat_count = None;
        } else if job.schedule.r#type == "once" {
            if job.schedule.run_at.is_none() {
                return Err(RuaError::Validation {
                    message: "schedule.type is once but run_at is missing".to_string(),
                });
            }
            job.schedule.cron.clear();
            normalize_once_repeat(&mut job.schedule)?;
        } else {
            return Err(RuaError::Validation {
                message: format!("unsupported schedule.type: {}", job.schedule.r#type),
            });
        }

        let mut entry = JobStateEntry::default();
        if job.enabled {
            if let Some(next) = compute_next_run_for_schedule(&job.schedule) {
                entry.state.next_run_at = Some(next);
            }
        }
        self.states.insert(job.id.clone(), entry);

        self.repo
            .upsert_job(job.clone())
            .map_err(|e| RuaError::Cron {
                job_id: job.id.clone(),
                message: format!("Failed to save job: {e}"),
            })?;

        tracing::info!(job_id = %job.id, name = %job.name, "Created/updated cron job");
        Ok(job)
    }

    pub fn delete_job(&self, job_id: &str) -> Result<Option<CronJobSpec>, RuaError> {
        self.states.remove(job_id);

        self.repo.delete_job(job_id).map_err(|e| RuaError::Cron {
            job_id: job_id.to_string(),
            message: format!("Failed to delete job: {e}"),
        })
    }

    pub async fn run_job(&self, job_id: &str) -> Result<JobExecutionResult, RuaError> {
        let job = self.get_job(job_id)?.ok_or_else(|| RuaError::NotFound {
            resource: "cron_job".to_string(),
            id: job_id.to_string(),
        })?;

        let executor = self.executor.as_ref().ok_or_else(|| RuaError::Cron {
            job_id: job_id.to_string(),
            message: "No executor configured".to_string(),
        })?;

        let result = executor.execute(&job).await;

        self.update_state_after_execution(job_id, &result);

        Ok(result)
    }

    pub fn get_state(&self, job_id: &str) -> Option<CronJobState> {
        self.states.get(job_id).map(|e| e.state.clone())
    }

    fn update_state_after_execution(&self, job_id: &str, result: &JobExecutionResult) {
        if let Some(mut entry) = self.states.get_mut(job_id) {
            entry.state.last_run_at = Some(Utc::now());
            entry.state.last_status = Some(if result.success {
                "success".to_string()
            } else {
                "error".to_string()
            });
            entry.state.last_error = result.error.clone();

            if let Ok(Some(job)) = self.get_job(job_id) {
                if job.enabled && !job.paused {
                    if let Some(next) = compute_next_run_for_schedule(&job.schedule) {
                        entry.state.next_run_at = Some(next);
                    }
                }
            }
        }
    }

    pub fn job_count(&self) -> usize {
        self.states.len()
    }
}

pub fn spec_to_cron_job(spec: &CronJobSpec) -> CronJob {
    CronJob {
        id: spec.id.clone(),
        cron_expr: spec.schedule.cron.clone(),
        agent_id: spec.agent_id.clone().unwrap_or_default(),
        prompt: spec
            .prompt
            .clone()
            .or(spec.text.clone())
            .unwrap_or_default(),
        channel: spec.dispatch.as_ref().map(|d| d.channel.clone()),
        enabled: spec.enabled,
        next_run_at: None,
        last_run_at: None,
        missed_policy: spec
            .missed_policy
            .clone()
            .unwrap_or_else(|| "run_once".to_string()),
        extra: Default::default(),
    }
}

pub fn validate_cron_expr(expr: &str) -> Result<(), RuaError> {
    let with_seconds = format!("0 {expr}");
    cron::Schedule::from_str(&with_seconds).map_err(|e| RuaError::Validation {
        message: format!("Invalid cron expression '{expr}': {e}"),
    })?;
    Ok(())
}

pub fn compute_next_run(cron_expr: &str, timezone: &str) -> Option<DateTime<Utc>> {
    use std::str::FromStr;

    let tz: chrono_tz::Tz = timezone.parse().unwrap_or(chrono_tz::UTC);

    let with_seconds = format!("0 {cron_expr}");
    let schedule = cron::Schedule::from_str(&with_seconds).ok()?;

    let now_in_tz = Utc::now().with_timezone(&tz);
    let next_in_tz = schedule.after(&now_in_tz).next()?;

    Some(next_in_tz.with_timezone(&Utc))
}

pub fn compute_next_run_for_schedule(schedule: &ScheduleSpec) -> Option<DateTime<Utc>> {
    match schedule.r#type.as_str() {
        "" | "cron" => compute_next_run(&schedule.cron, &schedule.timezone),
        "once" => compute_next_once_run(schedule, Utc::now()),
        _ => None,
    }
}

fn compute_next_once_run(schedule: &ScheduleSpec, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let run_at = schedule.run_at?;
    let Some(every_days) = schedule.repeat_every_days else {
        return (run_at > now).then_some(run_at);
    };
    if every_days == 0 {
        return (run_at > now).then_some(run_at);
    }

    let interval = Duration::days(i64::from(every_days));
    let mut next = run_at;
    if next <= now {
        let elapsed = now.signed_duration_since(run_at);
        let interval_secs = interval.num_seconds();
        if interval_secs <= 0 {
            return None;
        }
        let elapsed_secs = elapsed.num_seconds().max(0);
        let intervals_elapsed = elapsed_secs / interval_secs + 1;
        next = run_at + interval * (intervals_elapsed as i32);
    }

    let end_at = match schedule.repeat_end_type.as_deref() {
        Some("until") => schedule.repeat_until,
        Some("count") => schedule
            .repeat_count
            .map(|count| run_at + interval * (count.saturating_sub(1) as i32)),
        _ => None,
    };
    if end_at.is_some_and(|end| next > end) {
        return None;
    }
    Some(next)
}

fn normalize_once_repeat(schedule: &mut ScheduleSpec) -> Result<(), RuaError> {
    if schedule.repeat_every_days.is_none() {
        schedule.repeat_end_type = None;
        schedule.repeat_until = None;
        schedule.repeat_count = None;
        return Ok(());
    }

    if schedule.repeat_end_type.is_none() {
        schedule.repeat_end_type = Some("never".to_string());
    }

    match schedule.repeat_end_type.as_deref() {
        Some("never") => {
            schedule.repeat_until = None;
            schedule.repeat_count = None;
        }
        Some("until") => {
            let repeat_until = schedule.repeat_until.ok_or_else(|| RuaError::Validation {
                message: "repeat_end_type is until but repeat_until is missing".to_string(),
            })?;
            let run_at = schedule.run_at.expect("checked by caller");
            if repeat_until <= run_at {
                return Err(RuaError::Validation {
                    message: "repeat_until must be later than run_at".to_string(),
                });
            }
            schedule.repeat_count = None;
        }
        Some("count") => {
            if schedule.repeat_count.is_none() {
                return Err(RuaError::Validation {
                    message: "repeat_end_type is count but repeat_count is missing".to_string(),
                });
            }
            schedule.repeat_until = None;
        }
        Some(other) => {
            return Err(RuaError::Validation {
                message: format!("unsupported repeat_end_type: {other}"),
            });
        }
        None => {}
    }
    Ok(())
}

use std::str::FromStr;
