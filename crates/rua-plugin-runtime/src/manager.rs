use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::task::JoinHandle;

use crate::client::{PluginClient, PluginClientConfig, PluginClientError, PluginHealth};
use crate::registry::{ActivePluginRegistry, ActiveRegistryError, PluginInfo, PluginStatus};

const CRASH_WINDOW: Duration = Duration::from_secs(300);
const BACKOFF_DELAY: Duration = Duration::from_secs(60);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum LifecycleError {
    #[error("plugin client error: {0}")]
    Client(#[from] PluginClientError),
    #[error("registry error: {0}")]
    Registry(#[from] ActiveRegistryError),
    #[error("plugin '{id}' is already running")]
    AlreadyRunning { id: String },
    #[error("plugin '{id}' not found")]
    NotFound { id: String },
    #[error("plugin '{id}' auto-disabled after {crash_count} crashes")]
    AutoDisabled { id: String, crash_count: u32 },
}

pub struct PluginLifecycleManager {
    active: Arc<ActivePluginRegistry>,
    crash_watchers: tokio::sync::Mutex<Vec<JoinHandle<()>>>,
}

impl PluginLifecycleManager {
    pub fn new(active: Arc<ActivePluginRegistry>) -> Self {
        Self {
            active,
            crash_watchers: tokio::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn active_registry(&self) -> &Arc<ActivePluginRegistry> {
        &self.active
    }

    pub async fn start(&self, config: PluginClientConfig) -> Result<(), LifecycleError> {
        let plugin_id = config.plugin_id.clone();

        if self.active.contains(&plugin_id).await {
            return Err(LifecycleError::AlreadyRunning { id: plugin_id });
        }

        self.active
            .set_status(&plugin_id, PluginStatus::Starting)
            .await;

        let client = PluginClient::spawn(config).await?;
        self.active.insert(plugin_id.clone(), client).await?;
        self.spawn_crash_watcher(plugin_id).await;

        Ok(())
    }

    pub async fn stop(&self, plugin_id: &str) -> Result<(), LifecycleError> {
        self.active
            .set_status(plugin_id, PluginStatus::Stopping)
            .await;

        let shutdown_result = self
            .active
            .with_client_mut(plugin_id, |client| {
                let transport_running = client.is_running();
                (transport_running, ())
            })
            .await;

        match shutdown_result {
            Ok((true, ())) => {
                let grace_shutdown = async {
                    self.active
                        .with_client_mut(plugin_id, |client| {
                            let _ = client.health();
                        })
                        .await
                        .ok();
                };

                if tokio::time::timeout(SHUTDOWN_GRACE, async {
                    grace_shutdown.await;
                })
                .await
                .is_err()
                {
                    tracing::warn!(
                        plugin_id = %plugin_id,
                        "graceful shutdown timed out after {}s, force removing",
                        SHUTDOWN_GRACE.as_secs()
                    );
                }
            }
            Ok((false, ())) => {
                tracing::debug!(plugin_id = %plugin_id, "transport already stopped");
            }
            Err(_) => {
                return Err(LifecycleError::NotFound {
                    id: plugin_id.to_string(),
                });
            }
        }

        let mut managed =
            self.active
                .remove(plugin_id)
                .await
                .map_err(|_| LifecycleError::NotFound {
                    id: plugin_id.to_string(),
                })?;
        managed.client.shutdown().await.ok();

        Ok(())
    }

    pub async fn restart(&self, config: PluginClientConfig) -> Result<(), LifecycleError> {
        let plugin_id = config.plugin_id.clone();

        if self.active.contains(&plugin_id).await {
            self.stop(&plugin_id).await.ok();
        }

        self.start(config).await
    }

    pub async fn list(&self) -> Vec<PluginInfo> {
        self.active.list().await
    }

    pub async fn status(&self, plugin_id: &str) -> Option<PluginStatus> {
        self.active.get_status(plugin_id).await
    }

    pub async fn disable(&self, plugin_id: &str) -> Result<(), LifecycleError> {
        if self.active.contains(plugin_id).await {
            self.stop(plugin_id).await.ok();
        }

        let _ = self.active.subscribe();
        Ok(())
    }

    async fn spawn_crash_watcher(&self, plugin_id: String) {
        let active = Arc::clone(&self.active);
        let handle = tokio::spawn(async move {
            crash_watch_loop(active, plugin_id).await;
        });
        self.crash_watchers.lock().await.push(handle);
    }
}

impl Drop for PluginLifecycleManager {
    fn drop(&mut self) {}
}

async fn crash_watch_loop(active: Arc<ActivePluginRegistry>, plugin_id: String) {
    loop {
        let health_rx = match active.health_watch(&plugin_id).await {
            Some(rx) => rx,
            None => return,
        };

        if wait_for_unhealthy(health_rx).await.is_none() {
            return;
        }

        let crash_count = match active.record_crash(&plugin_id).await {
            Some(c) => c,
            None => return,
        };

        let (_, last_crash) = active.get_crash_info(&plugin_id).await.unwrap_or((0, None));

        match apply_restart_policy(crash_count, last_crash) {
            RestartDecision::Immediate => {
                tracing::info!(
                    plugin_id = %plugin_id,
                    crash_count = crash_count,
                    "crash #1: restarting immediately"
                );

                active.set_status(&plugin_id, PluginStatus::Starting).await;
            }
            RestartDecision::Backoff(delay) => {
                tracing::info!(
                    plugin_id = %plugin_id,
                    crash_count = crash_count,
                    delay_secs = delay.as_secs(),
                    "crash #{}: restarting after backoff",
                    crash_count
                );
                tokio::time::sleep(delay).await;
                active.set_status(&plugin_id, PluginStatus::Starting).await;
            }
            RestartDecision::Disable => {
                tracing::warn!(
                    plugin_id = %plugin_id,
                    crash_count = crash_count,
                    "crash #{}: auto-disabling plugin",
                    crash_count
                );
                active.set_status(&plugin_id, PluginStatus::Disabled).await;
                return;
            }
        }
    }
}

async fn wait_for_unhealthy(mut rx: tokio::sync::watch::Receiver<PluginHealth>) -> Option<String> {
    loop {
        if rx.changed().await.is_err() {
            return None;
        }
        let health = rx.borrow_and_update().clone();
        match health {
            PluginHealth::Unhealthy { reason } => return Some(reason),
            PluginHealth::Stopped => return None,
            PluginHealth::Healthy => continue,
        }
    }
}

enum RestartDecision {
    Immediate,
    Backoff(Duration),
    Disable,
}

fn apply_restart_policy(crash_count: u32, last_crash: Option<Instant>) -> RestartDecision {
    if crash_count <= 1 {
        return RestartDecision::Immediate;
    }

    let within_window = last_crash
        .map(|t| t.elapsed() < CRASH_WINDOW)
        .unwrap_or(false);

    if crash_count == 2 && within_window {
        return RestartDecision::Backoff(BACKOFF_DELAY);
    }

    if crash_count >= 3 {
        return RestartDecision::Disable;
    }

    RestartDecision::Immediate
}
