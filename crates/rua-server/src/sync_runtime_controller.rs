use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rua_sync::liveactor_apply::LiveApplyHandle;
use rua_sync::transport::SyncCycleResult;
use rua_sync::trigger::{SyncScheduler, SyncTriggerHandle};
use serde::Serialize;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SyncRuntimeState {
    #[default]
    Unavailable,
    NotPaired,
    Active,
    WarmSuspended {
        pending_grace: bool,
    },
}

impl SyncRuntimeState {
    fn diagnostics_name(self) -> &'static str {
        match self {
            SyncRuntimeState::Unavailable => "unavailable",
            SyncRuntimeState::NotPaired => "not_paired",
            SyncRuntimeState::Active => "active",
            SyncRuntimeState::WarmSuspended { .. } => "warm_suspended",
        }
    }

    fn pending_grace(self) -> bool {
        matches!(
            self,
            SyncRuntimeState::WarmSuspended {
                pending_grace: true
            }
        )
    }

    fn warm_suspended(self) -> bool {
        matches!(self, SyncRuntimeState::WarmSuspended { .. })
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SyncRuntimeBackgroundGraceStatus {
    pub pending: bool,
    pub remaining_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SyncRuntimeEffectiveStatus {
    pub state: &'static str,
    pub pending_grace: bool,
    pub background_grace: SyncRuntimeBackgroundGraceStatus,
    pub warm_suspended: bool,
}

#[derive(Debug, Clone)]
pub struct SyncRuntimeError {
    reason: &'static str,
    detail: String,
}

impl SyncRuntimeError {
    pub fn unavailable(detail: impl Into<String>) -> Self {
        Self {
            reason: "runtime_unavailable",
            detail: detail.into(),
        }
    }

    pub fn not_paired(detail: impl Into<String>) -> Self {
        Self {
            reason: "not_paired",
            detail: detail.into(),
        }
    }

    pub fn runtime_suspended(detail: impl Into<String>) -> Self {
        Self {
            reason: "runtime_suspended",
            detail: detail.into(),
        }
    }

    pub fn cycle_failed(detail: impl Into<String>) -> Self {
        Self {
            reason: "cycle_failed",
            detail: detail.into(),
        }
    }

    pub fn reason(&self) -> &'static str {
        self.reason
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl std::fmt::Display for SyncRuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.reason, self.detail)
    }
}

impl std::error::Error for SyncRuntimeError {}

pub struct SyncRuntimeParts {
    scheduler: SyncScheduler,
    liveactor: Option<LiveApplyHandle>,
}

impl SyncRuntimeParts {
    pub fn new(scheduler: SyncScheduler, liveactor: Option<LiveApplyHandle>) -> Self {
        Self {
            scheduler,
            liveactor,
        }
    }
}

#[derive(Default)]
struct RuntimeInner {
    state: SyncRuntimeState,
    scheduler: Option<SyncScheduler>,
    liveactor: Option<LiveApplyHandle>,
    background_grace_cancel: Option<CancellationToken>,
    background_grace_deadline: Option<Instant>,
    background_grace_generation: u64,
    background_grace_resume_state: Option<SyncRuntimeState>,
    residency_generation: u64,
}

pub struct SyncRuntimeController {
    inner: Mutex<RuntimeInner>,
    transition: Mutex<()>,
    trigger: Arc<RwLock<Option<SyncTriggerHandle>>>,
}

impl Default for SyncRuntimeController {
    fn default() -> Self {
        Self::new(Arc::new(RwLock::new(None)))
    }
}

impl SyncRuntimeController {
    pub fn new(trigger: Arc<RwLock<Option<SyncTriggerHandle>>>) -> Self {
        Self {
            inner: Mutex::new(RuntimeInner::default()),
            transition: Mutex::new(()),
            trigger,
        }
    }

    pub async fn state(&self) -> SyncRuntimeState {
        self.inner.lock().await.state
    }

    async fn state_before_grace(&self) -> SyncRuntimeState {
        let inner = self.inner.lock().await;
        inner.background_grace_resume_state.unwrap_or(inner.state)
    }

    pub async fn effective_status(&self) -> SyncRuntimeEffectiveStatus {
        let inner = self.inner.lock().await;
        let state = inner.state;
        let now = Instant::now();
        let remaining_ms = inner
            .background_grace_deadline
            .and_then(|deadline| deadline.checked_duration_since(now))
            .map(duration_millis_u64);
        SyncRuntimeEffectiveStatus {
            state: state.diagnostics_name(),
            pending_grace: state.pending_grace(),
            background_grace: SyncRuntimeBackgroundGraceStatus {
                pending: inner.background_grace_cancel.is_some() && state.pending_grace(),
                remaining_ms,
            },
            warm_suspended: state.warm_suspended(),
        }
    }

    pub async fn trigger_handle(&self) -> Option<SyncTriggerHandle> {
        self.trigger.read().await.clone()
    }

    pub async fn mark_not_paired(&self) {
        self.shutdown_to_state(SyncRuntimeState::NotPaired).await;
    }

    pub async fn shutdown_unavailable(&self) {
        self.shutdown_to_state(SyncRuntimeState::Unavailable).await;
    }

    pub async fn ensure_active<F, Fut>(&self, start: F) -> Result<(), SyncRuntimeError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<SyncRuntimeParts, SyncRuntimeError>>,
    {
        let _transition = self.transition.lock().await;
        self.ensure_active_inner(start).await
    }

    async fn ensure_active_inner<F, Fut>(&self, start: F) -> Result<(), SyncRuntimeError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<SyncRuntimeParts, SyncRuntimeError>>,
    {
        match self.state_before_grace().await {
            SyncRuntimeState::Active
            | SyncRuntimeState::WarmSuspended {
                pending_grace: true,
            } => {
                return Ok(());
            }
            SyncRuntimeState::Unavailable
            | SyncRuntimeState::NotPaired
            | SyncRuntimeState::WarmSuspended {
                pending_grace: false,
            } => {}
        }

        let parts = match start().await {
            Ok(parts) => parts,
            Err(error) => {
                let state = if error.reason() == "not_paired" {
                    SyncRuntimeState::NotPaired
                } else {
                    SyncRuntimeState::Unavailable
                };
                let mut inner = self.inner.lock().await;
                inner.background_grace_generation =
                    inner.background_grace_generation.wrapping_add(1);
                if let Some(cancel) = inner.background_grace_cancel.take() {
                    cancel.cancel();
                }
                inner.background_grace_deadline = None;
                inner.background_grace_resume_state = None;
                inner.state = state;
                return Err(error);
            }
        };
        self.install_active(parts).await;
        Ok(())
    }

    pub async fn warm_suspend(&self) -> Result<(), SyncRuntimeError> {
        self.warm_suspend_with(|| async {}).await
    }

    pub async fn warm_suspend_with<S, SuspendFut>(&self, suspend: S) -> Result<(), SyncRuntimeError>
    where
        S: FnOnce() -> SuspendFut,
        SuspendFut: Future<Output = ()>,
    {
        let _transition = self.transition.lock().await;
        self.warm_suspend_inner(suspend).await
    }

    async fn warm_suspend_inner<S, SuspendFut>(&self, suspend: S) -> Result<(), SyncRuntimeError>
    where
        S: FnOnce() -> SuspendFut,
        SuspendFut: Future<Output = ()>,
    {
        let (scheduler, liveactor, cancel) = {
            let mut inner = self.inner.lock().await;
            inner.background_grace_generation = inner.background_grace_generation.wrapping_add(1);
            let cancel = inner.background_grace_cancel.take();
            inner.background_grace_deadline = None;
            let previous = inner.background_grace_resume_state.take();
            if previous == Some(SyncRuntimeState::NotPaired) {
                inner.state = SyncRuntimeState::NotPaired;
            }
            match inner.state {
                SyncRuntimeState::Active
                | SyncRuntimeState::WarmSuspended {
                    pending_grace: true,
                } => {
                    inner.state = SyncRuntimeState::WarmSuspended {
                        pending_grace: false,
                    };
                    (inner.scheduler.take(), inner.liveactor.take(), cancel)
                }
                SyncRuntimeState::WarmSuspended {
                    pending_grace: false,
                }
                | SyncRuntimeState::Unavailable
                | SyncRuntimeState::NotPaired => (None, None, cancel),
            }
        };
        if let Some(cancel) = cancel {
            cancel.cancel();
        }
        *self.trigger.write().await = None;
        shutdown_parts(scheduler, liveactor).await;
        suspend().await;
        Ok(())
    }

    pub async fn notify_background(
        self: &Arc<Self>,
        grace: Duration,
    ) -> Result<(), SyncRuntimeError> {
        self.notify_background_with_suspend(grace, || async {})
            .await
    }

    pub async fn notify_background_with_suspend<S, SuspendFut>(
        self: &Arc<Self>,
        grace: Duration,
        suspend: S,
    ) -> Result<(), SyncRuntimeError>
    where
        S: FnOnce() -> SuspendFut + Send + 'static,
        SuspendFut: Future<Output = ()> + Send + 'static,
    {
        if grace.is_zero() {
            return self.warm_suspend_with(suspend).await;
        }

        let _transition = self.transition.lock().await;
        let (token, generation, should_spawn) = {
            let mut inner = self.inner.lock().await;
            match inner.state {
                SyncRuntimeState::Active
                | SyncRuntimeState::Unavailable
                | SyncRuntimeState::NotPaired => {
                    if let Some(cancel) = inner.background_grace_cancel.take() {
                        cancel.cancel();
                    }
                    inner.background_grace_generation =
                        inner.background_grace_generation.wrapping_add(1);
                    let generation = inner.background_grace_generation;
                    let token = CancellationToken::new();
                    inner.background_grace_cancel = Some(token.clone());
                    inner.background_grace_deadline = Some(Instant::now() + grace);
                    inner.background_grace_resume_state = Some(inner.state);
                    inner.state = SyncRuntimeState::WarmSuspended {
                        pending_grace: true,
                    };
                    (token, generation, true)
                }
                SyncRuntimeState::WarmSuspended {
                    pending_grace: true,
                } => {
                    if let Some(cancel) = inner.background_grace_cancel.take() {
                        cancel.cancel();
                    }
                    inner.background_grace_generation =
                        inner.background_grace_generation.wrapping_add(1);
                    let generation = inner.background_grace_generation;
                    let token = CancellationToken::new();
                    inner.background_grace_cancel = Some(token.clone());
                    inner.background_grace_deadline = Some(Instant::now() + grace);
                    (token, generation, true)
                }
                SyncRuntimeState::WarmSuspended {
                    pending_grace: false,
                } => (CancellationToken::new(), 0, false),
            }
        };

        if should_spawn {
            let controller = Arc::clone(self);
            tokio::spawn(async move {
                tokio::select! {
                    _ = token.cancelled() => {}
                    _ = tokio::time::sleep(grace) => {
                        if !token.is_cancelled() {
                            controller
                                .complete_background_grace(generation, suspend)
                                .await;
                        }
                    }
                }
            });
        }

        Ok(())
    }

    async fn complete_background_grace<S, SuspendFut>(&self, generation: u64, suspend: S)
    where
        S: FnOnce() -> SuspendFut,
        SuspendFut: Future<Output = ()>,
    {
        let _transition = self.transition.lock().await;
        let (scheduler, liveactor) = {
            let mut inner = self.inner.lock().await;
            if inner.background_grace_generation != generation {
                return;
            }
            match inner.state {
                SyncRuntimeState::WarmSuspended {
                    pending_grace: true,
                } => {
                    inner.background_grace_cancel = None;
                    inner.background_grace_deadline = None;
                    inner.state = match inner.background_grace_resume_state.take() {
                        Some(SyncRuntimeState::NotPaired) => SyncRuntimeState::NotPaired,
                        _ => SyncRuntimeState::WarmSuspended {
                            pending_grace: false,
                        },
                    };
                    (inner.scheduler.take(), inner.liveactor.take())
                }
                SyncRuntimeState::Active
                | SyncRuntimeState::WarmSuspended {
                    pending_grace: false,
                }
                | SyncRuntimeState::Unavailable
                | SyncRuntimeState::NotPaired => return,
            }
        };
        *self.trigger.write().await = None;
        shutdown_parts(scheduler, liveactor).await;
        suspend().await;
    }

    pub async fn resume_foreground<F, Fut>(&self, start: F) -> Result<(), SyncRuntimeError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<SyncRuntimeParts, SyncRuntimeError>>,
    {
        self.resume_resident(start).await
    }

    pub async fn retain_for_foreground_operation<F, Fut>(
        &self,
        start: F,
    ) -> Result<(), SyncRuntimeError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<SyncRuntimeParts, SyncRuntimeError>>,
    {
        self.resume_resident(start).await
    }

    async fn resume_resident<F, Fut>(&self, start: F) -> Result<(), SyncRuntimeError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<SyncRuntimeParts, SyncRuntimeError>>,
    {
        let _transition = self.transition.lock().await;
        let (state, cancel) = {
            let mut inner = self.inner.lock().await;
            inner.residency_generation = inner.residency_generation.wrapping_add(1);
            let cancel = inner.background_grace_cancel.take();
            if matches!(
                inner.state,
                SyncRuntimeState::WarmSuspended {
                    pending_grace: true
                }
            ) {
                inner.background_grace_generation =
                    inner.background_grace_generation.wrapping_add(1);
                inner.background_grace_deadline = None;
                inner.state = inner
                    .background_grace_resume_state
                    .take()
                    .unwrap_or(SyncRuntimeState::Active);
            }
            (inner.state, cancel)
        };
        if let Some(cancel) = cancel {
            cancel.cancel();
        }

        match state {
            SyncRuntimeState::Active => Ok(()),
            SyncRuntimeState::WarmSuspended { .. } => self.ensure_active_inner(start).await,
            SyncRuntimeState::NotPaired => {
                Err(SyncRuntimeError::not_paired("sync runtime is not paired"))
            }
            SyncRuntimeState::Unavailable => self.ensure_active_inner(start).await,
        }
    }

    pub async fn notify_foreground<F, Fut>(&self, start: F) -> Result<(), SyncRuntimeError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<SyncRuntimeParts, SyncRuntimeError>>,
    {
        self.resume_foreground(start).await
    }

    pub async fn trigger_app_resume(&self) -> Result<(), SyncRuntimeError> {
        let handle = self
            .trigger_handle()
            .await
            .ok_or_else(|| SyncRuntimeError::unavailable("sync trigger is not initialized"))?;
        handle.notify_resume();
        Ok(())
    }

    pub async fn trigger_manual_sync<F, Fut>(
        &self,
        start: F,
    ) -> Result<SyncCycleResult, SyncRuntimeError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<SyncRuntimeParts, SyncRuntimeError>>,
    {
        match self.state_before_grace().await {
            SyncRuntimeState::Active => {}
            SyncRuntimeState::WarmSuspended {
                pending_grace: true,
            } => {}
            SyncRuntimeState::WarmSuspended {
                pending_grace: false,
            } => self.ensure_active(start).await?,
            SyncRuntimeState::NotPaired => {
                return Err(SyncRuntimeError::not_paired("sync runtime is not paired"));
            }
            SyncRuntimeState::Unavailable => {
                return Err(SyncRuntimeError::unavailable("sync runtime is unavailable"));
            }
        }

        let handle = self
            .trigger_handle()
            .await
            .ok_or_else(|| SyncRuntimeError::unavailable("sync trigger is not initialized"))?;
        handle
            .trigger_manual()
            .await
            .map_err(SyncRuntimeError::cycle_failed)
    }

    pub async fn trigger_scheduled_background_sync<F, Fut>(
        &self,
        start: F,
        keep_runtime_active: bool,
    ) -> Result<SyncCycleResult, SyncRuntimeError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<SyncRuntimeParts, SyncRuntimeError>>,
    {
        self.trigger_scheduled_background_sync_with_suspend(start, keep_runtime_active, || async {})
            .await
    }

    pub async fn trigger_scheduled_background_sync_with_suspend<F, Fut, S, SuspendFut>(
        &self,
        start: F,
        keep_runtime_active: bool,
        suspend: S,
    ) -> Result<SyncCycleResult, SyncRuntimeError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<SyncRuntimeParts, SyncRuntimeError>>,
        S: FnOnce() -> SuspendFut,
        SuspendFut: Future<Output = ()>,
    {
        let residency_generation = self.inner.lock().await.residency_generation;
        match self.state_before_grace().await {
            SyncRuntimeState::Active
            | SyncRuntimeState::WarmSuspended {
                pending_grace: true,
            } => {}
            SyncRuntimeState::WarmSuspended {
                pending_grace: false,
            }
            | SyncRuntimeState::Unavailable => self.ensure_active(start).await?,
            SyncRuntimeState::NotPaired => {
                return Err(SyncRuntimeError::not_paired("sync runtime is not paired"));
            }
        }

        let handle = self
            .trigger_handle()
            .await
            .ok_or_else(|| SyncRuntimeError::unavailable("sync trigger is not initialized"))?;
        let result = handle
            .trigger_manual()
            .await
            .map_err(SyncRuntimeError::cycle_failed);
        if !keep_runtime_active {
            if let Err(e) = self
                .warm_suspend_if_residency_unchanged(residency_generation, suspend)
                .await
            {
                tracing::warn!(
                    reason = e.reason(),
                    detail = e.detail(),
                    "[sync] scheduled background attempt failed to warm suspend runtime"
                );
            }
        }
        result
    }

    async fn warm_suspend_if_residency_unchanged<S, SuspendFut>(
        &self,
        residency_generation: u64,
        suspend: S,
    ) -> Result<(), SyncRuntimeError>
    where
        S: FnOnce() -> SuspendFut,
        SuspendFut: Future<Output = ()>,
    {
        let _transition = self.transition.lock().await;
        if self.inner.lock().await.residency_generation != residency_generation {
            return Ok(());
        }
        self.warm_suspend_inner(suspend).await
    }

    async fn install_active(&self, parts: SyncRuntimeParts) {
        let handle = parts.scheduler.handle();
        let (stale, cancel) = {
            let mut inner = self.inner.lock().await;
            inner.background_grace_generation = inner.background_grace_generation.wrapping_add(1);
            let cancel = inner.background_grace_cancel.take();
            inner.background_grace_deadline = None;
            inner.background_grace_resume_state = None;
            let stale = (inner.scheduler.take(), inner.liveactor.take());
            inner.scheduler = Some(parts.scheduler);
            inner.liveactor = parts.liveactor;
            inner.state = SyncRuntimeState::Active;
            (stale, cancel)
        };
        if let Some(cancel) = cancel {
            cancel.cancel();
        }
        *self.trigger.write().await = Some(handle);
        shutdown_parts(stale.0, stale.1).await;
    }

    async fn shutdown_to_state(&self, state: SyncRuntimeState) {
        let _transition = self.transition.lock().await;
        let (scheduler, liveactor, cancel) = {
            let mut inner = self.inner.lock().await;
            inner.background_grace_generation = inner.background_grace_generation.wrapping_add(1);
            let cancel = inner.background_grace_cancel.take();
            inner.background_grace_deadline = None;
            inner.background_grace_resume_state = None;
            inner.state = state;
            (inner.scheduler.take(), inner.liveactor.take(), cancel)
        };
        if let Some(cancel) = cancel {
            cancel.cancel();
        }
        *self.trigger.write().await = None;
        shutdown_parts(scheduler, liveactor).await;
    }
}

fn duration_millis_u64(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

async fn shutdown_parts(scheduler: Option<SyncScheduler>, liveactor: Option<LiveApplyHandle>) {
    if let Some(scheduler) = scheduler {
        scheduler.shutdown().await;
    }
    if let Some(liveactor) = liveactor {
        liveactor.shutdown().await;
    }
}
