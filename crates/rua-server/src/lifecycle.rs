use async_trait::async_trait;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub struct StartupContext {
    pub profile: String,
}

#[derive(Debug, Clone)]
pub struct WarmupContext {
    pub profile: String,
}

#[derive(Debug, Clone)]
pub struct ShutdownContext {
    pub graceful: bool,
}

#[async_trait]
pub trait ManagedService: Send + Sync {
    fn name(&self) -> &'static str;

    async fn start(&self, ctx: &StartupContext) -> anyhow::Result<()>;

    async fn warmup(&self, ctx: &WarmupContext) -> anyhow::Result<()>;

    async fn shutdown(&self, ctx: &ShutdownContext) -> anyhow::Result<()>;
}

pub struct LifecycleManager {
    services: RwLock<Vec<Arc<dyn ManagedService>>>,
    background_tasks: RwLock<Vec<BackgroundTask>>,
}

impl LifecycleManager {
    pub fn new() -> Self {
        Self {
            services: RwLock::new(Vec::new()),
            background_tasks: RwLock::new(Vec::new()),
        }
    }

    pub async fn register(&self, service: Arc<dyn ManagedService>) {
        self.services.write().await.push(service);
    }

    pub async fn register_background_task(&self, task: BackgroundTask) {
        self.background_tasks.write().await.push(task);
    }

    pub async fn start_all(&self, ctx: &StartupContext) -> anyhow::Result<()> {
        let services = self.services.read().await;
        for service in services.iter() {
            let start = Instant::now();
            let name = service.name();
            tracing::info!("[startup-prof] starting {name}");

            if let Err(e) = service.start(ctx).await {
                tracing::error!(
                    service = name,
                    error = %e,
                    "[startup-prof] {name} failed to start"
                );
                return Err(e);
            }

            let elapsed = start.elapsed();
            tracing::info!("[startup-prof] {name}_started_ms={}", elapsed.as_millis());
        }
        Ok(())
    }

    pub async fn warmup_all(&self, ctx: &WarmupContext) {
        let services = self.services.read().await;
        for service in services.iter() {
            let start = Instant::now();
            let name = service.name();

            if let Err(e) = service.warmup(ctx).await {
                tracing::warn!(
                    service = name,
                    error = %e,
                    "[startup-prof] {name} warmup failed (non-fatal)"
                );
            } else {
                let elapsed = start.elapsed();
                tracing::info!("[startup-prof] {name}_warmed_up_ms={}", elapsed.as_millis());
            }
        }

        let tasks: Vec<BackgroundTask> = std::mem::take(&mut *self.background_tasks.write().await);
        if !tasks.is_empty() {
            start_background_tasks(tasks);
        }
    }

    pub async fn shutdown_all(&self, ctx: &ShutdownContext) {
        let services = self.services.read().await;
        for service in services.iter().rev() {
            let name = service.name();
            tracing::info!("shutting down {name}");

            if let Err(e) = service.shutdown(ctx).await {
                tracing::error!(
                    service = name,
                    error = %e,
                    "{name} shutdown error (continuing)"
                );
            }
        }
    }
}

impl Default for LifecycleManager {
    fn default() -> Self {
        Self::new()
    }
}

pub struct StartupTimer {
    start: Instant,
    phase: String,
}

impl StartupTimer {
    pub fn new(phase: &str) -> Self {
        tracing::info!("[startup-prof] begin {phase}");
        Self {
            start: Instant::now(),
            phase: phase.to_string(),
        }
    }

    pub fn finish(self) -> std::time::Duration {
        let elapsed = self.start.elapsed();
        tracing::info!("[startup-prof] {}_ms={}", self.phase, elapsed.as_millis());
        elapsed
    }
}

pub struct BackgroundTask {
    pub name: &'static str,
    pub interval: Duration,
    pub task_fn: Box<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>,
}

pub fn start_background_tasks(tasks: Vec<BackgroundTask>) {
    for task in tasks {
        let name = task.name;
        let interval = task.interval;
        let task_fn = task.task_fn;

        tracing::info!(
            task = name,
            interval_secs = interval.as_secs(),
            "session cleanup scheduled: interval={}h",
            interval.as_secs() / 3600
        );

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);

            ticker.tick().await;

            loop {
                ticker.tick().await;
                tracing::debug!(task = name, "Running background task");
                (task_fn)().await;
            }
        });
    }
}
