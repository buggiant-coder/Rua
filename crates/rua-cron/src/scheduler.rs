use async_trait::async_trait;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use rua_contract::cron::{CronJob, SystemScheduler};
use rua_core::Result;
use std::sync::Arc;
use tokio::sync::Notify;
use tracing;

struct TimerHandle {
    cancel: Arc<Notify>,

    scheduled_at: DateTime<Utc>,
}

pub type TimerCallback = Arc<dyn Fn(String) + Send + Sync>;

pub struct TokioScheduler {
    timers: DashMap<String, TimerHandle>,

    on_fire: TimerCallback,
}

impl TokioScheduler {
    pub fn new(on_fire: TimerCallback) -> Self {
        Self {
            timers: DashMap::new(),
            on_fire,
        }
    }

    pub fn active_count(&self) -> usize {
        self.timers.len()
    }

    pub fn has_timer(&self, job_id: &str) -> bool {
        self.timers.contains_key(job_id)
    }

    pub fn scheduled_at(&self, job_id: &str) -> Option<DateTime<Utc>> {
        self.timers.get(job_id).map(|h| h.scheduled_at)
    }

    pub fn cancel_all(&self) {
        for entry in self.timers.iter() {
            entry.value().cancel.notify_one();
        }
        self.timers.clear();
    }
}

#[async_trait]
impl SystemScheduler for TokioScheduler {
    async fn register_next_run(&self, job: &CronJob, next_run_at: DateTime<Utc>) -> Result<()> {
        let job_id = job.id.clone();

        if let Some((_, old_handle)) = self.timers.remove(&job_id) {
            old_handle.cancel.notify_one();
        }

        let now = Utc::now();
        let duration = next_run_at.signed_duration_since(now);

        if duration.num_milliseconds() <= 0 {
            tracing::warn!(
                job_id = %job_id,
                "next_run_at is in the past ({:?}), firing immediately",
                next_run_at
            );

            let callback = Arc::clone(&self.on_fire);
            let jid = job_id.clone();
            tokio::spawn(async move {
                (callback)(jid);
            });
            return Ok(());
        }

        let std_duration = std::time::Duration::from_millis(duration.num_milliseconds() as u64);

        let cancel = Arc::new(Notify::new());
        let cancel_clone = Arc::clone(&cancel);
        let callback = Arc::clone(&self.on_fire);
        let jid = job_id.clone();

        let handle = TimerHandle {
            cancel,
            scheduled_at: next_run_at,
        };
        self.timers.insert(job_id.clone(), handle);

        tokio::spawn(async move {
            tokio::select! {
                _ = tokio::time::sleep(std_duration) => {
                    tracing::info!(job_id = %jid, "Timer fired for cron job");
                    (callback)(jid);
                }
                _ = cancel_clone.notified() => {
                    tracing::debug!(job_id = %jid, "Timer cancelled for cron job");
                }
            }
        });

        Ok(())
    }

    async fn cancel(&self, job_id: &str) -> Result<()> {
        if let Some((_, handle)) = self.timers.remove(job_id) {
            handle.cancel.notify_one();
            tracing::debug!(job_id = %job_id, "Cancelled cron timer");
        }
        Ok(())
    }
}
