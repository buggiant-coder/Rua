use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rua_core::Result;
use std::sync::Arc;
use tokio::sync::Mutex;

#[async_trait]
pub trait AndroidSchedulerBridge: Send + Sync {
    async fn schedule_wakeup(&self, job_id: &str, at: DateTime<Utc>) -> Result<()>;

    async fn cancel_wakeup(&self, job_id: &str) -> Result<()>;

    async fn cancel_all_wakeups(&self) -> Result<()>;

    fn is_available(&self) -> bool;
}

#[derive(Debug, Clone, PartialEq)]
pub enum MockAction {
    Schedule { job_id: String, at: DateTime<Utc> },
    Cancel { job_id: String },
    CancelAll,
}

pub struct MockAndroidScheduler {
    pub actions: Arc<Mutex<Vec<MockAction>>>,
    pub available: bool,
}

impl MockAndroidScheduler {
    pub fn new(available: bool) -> Self {
        Self {
            actions: Arc::new(Mutex::new(Vec::new())),
            available,
        }
    }

    pub async fn recorded_actions(&self) -> Vec<MockAction> {
        self.actions.lock().await.clone()
    }

    pub async fn clear(&self) {
        self.actions.lock().await.clear();
    }
}

impl Default for MockAndroidScheduler {
    fn default() -> Self {
        Self::new(false)
    }
}

#[async_trait]
impl AndroidSchedulerBridge for MockAndroidScheduler {
    async fn schedule_wakeup(&self, job_id: &str, at: DateTime<Utc>) -> Result<()> {
        self.actions.lock().await.push(MockAction::Schedule {
            job_id: job_id.to_string(),
            at,
        });
        Ok(())
    }

    async fn cancel_wakeup(&self, job_id: &str) -> Result<()> {
        self.actions.lock().await.push(MockAction::Cancel {
            job_id: job_id.to_string(),
        });
        Ok(())
    }

    async fn cancel_all_wakeups(&self) -> Result<()> {
        self.actions.lock().await.push(MockAction::CancelAll);
        Ok(())
    }

    fn is_available(&self) -> bool {
        self.available
    }
}
