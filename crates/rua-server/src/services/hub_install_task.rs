use std::collections::HashMap;
use std::future::Future;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tokio_util::sync::CancellationToken;

pub const DEFAULT_INSTALL_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HubInstallStatusKind {
    Pending,
    Importing,
    Completed,
    Failed,
    Cancelled,
}

impl HubInstallStatusKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Importing => "importing",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone)]
pub struct HubInstallTask {
    pub task_id: String,
    pub agent_id: String,
    pub bundle_url: String,
    pub version: String,
    pub enable: bool,
    pub status: HubInstallStatusKind,
    pub error: Option<String>,
    pub result: Option<Value>,
    pub sync_published: bool,
    pub created_at: f64,
    pub updated_at: f64,
}

#[derive(Default)]
struct Inner {
    tasks: HashMap<String, HubInstallTask>,
    cancels: HashMap<String, CancellationToken>,
}

#[derive(Default)]
pub struct HubInstallRegistry {
    inner: Mutex<Inner>,
}

impl HubInstallRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create(
        &self,
        agent_id: String,
        bundle_url: String,
        version: String,
        enable: bool,
    ) -> (String, CancellationToken) {
        let task_id = uuid::Uuid::new_v4().to_string();
        let token = CancellationToken::new();
        let now = now_secs();
        let task = HubInstallTask {
            agent_id,
            task_id: task_id.clone(),
            bundle_url,
            version,
            enable,
            status: HubInstallStatusKind::Pending,
            error: None,
            result: None,
            sync_published: false,
            created_at: now,
            updated_at: now,
        };
        let mut guard = self.inner.lock().expect("hub install registry poisoned");
        guard.tasks.insert(task_id.clone(), task);
        guard.cancels.insert(task_id.clone(), token.clone());
        (task_id, token)
    }

    pub fn set_status(
        &self,
        task_id: &str,
        status: HubInstallStatusKind,
        error: Option<String>,
        result: Option<Value>,
    ) {
        let mut guard = self.inner.lock().expect("hub install registry poisoned");
        if let Some(task) = guard.tasks.get_mut(task_id) {
            task.status = status;
            task.updated_at = now_secs();
            if let Some(e) = error {
                task.error = Some(e);
            }
            if let Some(r) = result {
                task.result = Some(r);
            }
        }
        if status.is_terminal() {
            guard.cancels.remove(task_id);
        }
    }

    pub fn snapshot(&self, task_id: &str) -> Option<HubInstallTask> {
        let guard = self.inner.lock().expect("hub install registry poisoned");
        guard.tasks.get(task_id).cloned()
    }

    pub fn mark_sync_published(&self, agent_id: &str, task_id: &str) -> bool {
        let mut guard = self.inner.lock().expect("hub install registry poisoned");
        let Some(task) = guard.tasks.get_mut(task_id) else {
            return false;
        };
        if task.agent_id != agent_id || task.sync_published {
            return false;
        }
        task.sync_published = true;
        true
    }

    pub fn cancel(&self, agent_id: &str, task_id: &str) -> Option<HubInstallStatusKind> {
        let mut guard = self.inner.lock().expect("hub install registry poisoned");
        let task = guard.tasks.get_mut(task_id)?;
        if task.agent_id != agent_id {
            return None;
        }
        if task.status.is_terminal() {
            return Some(task.status);
        }
        task.status = HubInstallStatusKind::Cancelled;
        task.updated_at = now_secs();
        if let Some(token) = guard.cancels.remove(task_id) {
            token.cancel();
        }
        Some(HubInstallStatusKind::Cancelled)
    }
}

pub async fn run_install_task<F, Fut>(
    registry: std::sync::Arc<HubInstallRegistry>,
    task_id: String,
    cancel: CancellationToken,
    timeout: Duration,
    work: F,
) where
    F: FnOnce(CancellationToken) -> Fut,
    Fut: Future<Output = Result<Value, String>>,
{
    registry.set_status(&task_id, HubInstallStatusKind::Importing, None, None);

    let work_fut = work(cancel.clone());
    tokio::pin!(work_fut);

    tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            registry.set_status(&task_id, HubInstallStatusKind::Cancelled, None, None);
        }
        _ = tokio::time::sleep(timeout) => {
            cancel.cancel();
            registry.set_status(
                &task_id,
                HubInstallStatusKind::Failed,
                Some("install timed out".to_string()),
                None,
            );
        }
        res = &mut work_fut => {
            match res {
                Ok(result) => registry.set_status(
                    &task_id,
                    HubInstallStatusKind::Completed,
                    None,
                    Some(result),
                ),
                Err(err) => {
                    if cancel.is_cancelled() {
                        registry.set_status(&task_id, HubInstallStatusKind::Cancelled, None, None);
                    } else {
                        registry.set_status(
                            &task_id,
                            HubInstallStatusKind::Failed,
                            Some(err),
                            None,
                        );
                    }
                }
            }
        }
    }
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}
