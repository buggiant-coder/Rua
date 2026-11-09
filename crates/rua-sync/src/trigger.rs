use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::transport::SyncCycleResult;

const WRITE_QUIET_PERIOD: Duration = Duration::from_millis(500);
const WRITE_MAX_DELAY: Duration = Duration::from_secs(1);
const PERIODIC_INTERVAL: Duration = Duration::from_secs(600);
const APP_RESUME_MIN_INTERVAL: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerReason {
    WriteCoalesce,
    Periodic,
    AppResume,
    Manual,
}

pub type CycleResult = Result<SyncCycleResult, String>;

#[derive(Clone)]
pub struct SyncTriggerHandle {
    tx: mpsc::Sender<TriggerEvent>,
}

enum TriggerEvent {
    WriteOp { at: tokio::time::Instant },
    AppResume,
    Manual { reply: oneshot::Sender<CycleResult> },
}

impl SyncTriggerHandle {
    pub fn notify_write(&self) {
        let _ = self.tx.try_send(TriggerEvent::WriteOp {
            at: tokio::time::Instant::now(),
        });
    }

    pub fn notify_resume(&self) {
        let _ = self.tx.try_send(TriggerEvent::AppResume);
    }

    pub async fn trigger_manual(&self) -> CycleResult {
        let (reply_tx, reply_rx) = oneshot::channel();
        if self
            .tx
            .try_send(TriggerEvent::Manual { reply: reply_tx })
            .is_err()
        {
            return Err("trigger channel full or closed".into());
        }
        reply_rx
            .await
            .unwrap_or_else(|_| Err("scheduler dropped reply".into()))
    }
}

pub type SyncCycleFn =
    Arc<dyn Fn(TriggerReason) -> Pin<Box<dyn Future<Output = CycleResult> + Send>> + Send + Sync>;

pub struct TriggerConfig {
    pub write_quiet_period: Duration,
    pub write_max_delay: Duration,
    pub periodic_interval: Duration,
    pub app_resume_min_interval: Duration,
}

impl Default for TriggerConfig {
    fn default() -> Self {
        Self {
            write_quiet_period: WRITE_QUIET_PERIOD,
            write_max_delay: WRITE_MAX_DELAY,
            periodic_interval: PERIODIC_INTERVAL,
            app_resume_min_interval: APP_RESUME_MIN_INTERVAL,
        }
    }
}

pub struct SyncScheduler {
    handle: SyncTriggerHandle,
    task: JoinHandle<()>,
}

impl SyncScheduler {
    pub fn start(sync_fn: SyncCycleFn, config: TriggerConfig) -> Self {
        let (tx, rx) = mpsc::channel(64);
        let handle = SyncTriggerHandle { tx };
        let task = tokio::spawn(scheduler_loop(rx, sync_fn, config));
        Self { handle, task }
    }

    pub fn handle(&self) -> SyncTriggerHandle {
        self.handle.clone()
    }

    pub async fn shutdown(self) {
        self.task.abort();
        let _ = self.task.await;
    }
}

async fn scheduler_loop(
    mut rx: mpsc::Receiver<TriggerEvent>,
    sync_fn: SyncCycleFn,
    config: TriggerConfig,
) {
    let mut periodic = tokio::time::interval(config.periodic_interval);
    periodic.tick().await;
    let mut last_app_resume_at = None;
    let mut pending = PendingTrigger::default();
    let mut channel_open = true;

    loop {
        let Some(request) = wait_for_trigger(
            &mut rx,
            &mut periodic,
            &config,
            &mut last_app_resume_at,
            &mut pending,
            &mut channel_open,
        )
        .await
        else {
            debug!("trigger channel closed, scheduler exiting");
            break;
        };

        let result = run_cycle_and_collect(
            &sync_fn,
            request.reason,
            &mut rx,
            &mut periodic,
            &config,
            &mut last_app_resume_at,
            &mut pending,
            &mut channel_open,
        )
        .await;
        for tx in request.replies {
            let _ = tx.send(result.clone());
        }
    }
}

type ReplyTx = oneshot::Sender<CycleResult>;

struct CycleRequest {
    reason: TriggerReason,
    replies: Vec<ReplyTx>,
}

#[derive(Debug, Clone, Copy)]
struct PendingWrites {
    first_at: tokio::time::Instant,
    last_at: tokio::time::Instant,
}

#[derive(Default)]
struct PendingTrigger {
    writes: Option<PendingWrites>,
    immediate_reason: Option<TriggerReason>,
    replies: Vec<ReplyTx>,
}

impl PendingTrigger {
    fn record_write(&mut self, at: tokio::time::Instant) {
        match &mut self.writes {
            Some(writes) => {
                writes.first_at = writes.first_at.min(at);
                writes.last_at = writes.last_at.max(at);
            }
            None => {
                self.writes = Some(PendingWrites {
                    first_at: at,
                    last_at: at,
                });
            }
        }
    }

    fn request_immediate(&mut self, reason: TriggerReason) {
        let replace = self
            .immediate_reason
            .map(|current| trigger_priority(reason) > trigger_priority(current))
            .unwrap_or(true);
        if replace {
            self.immediate_reason = Some(reason);
        }
    }

    fn write_deadline(&self, config: &TriggerConfig) -> Option<tokio::time::Instant> {
        self.writes.map(|writes| {
            (writes.last_at + config.write_quiet_period)
                .min(writes.first_at + config.write_max_delay)
        })
    }

    fn take_due(
        &mut self,
        now: tokio::time::Instant,
        config: &TriggerConfig,
    ) -> Option<CycleRequest> {
        if let Some(reason) = self.immediate_reason.take() {
            self.writes = None;
            return Some(CycleRequest {
                reason,
                replies: std::mem::take(&mut self.replies),
            });
        }
        if self
            .write_deadline(config)
            .is_some_and(|deadline| deadline <= now)
        {
            self.writes = None;
            return Some(CycleRequest {
                reason: TriggerReason::WriteCoalesce,
                replies: Vec::new(),
            });
        }
        None
    }

    fn is_empty(&self) -> bool {
        self.writes.is_none() && self.immediate_reason.is_none() && self.replies.is_empty()
    }
}

async fn wait_for_trigger(
    rx: &mut mpsc::Receiver<TriggerEvent>,
    periodic: &mut tokio::time::Interval,
    config: &TriggerConfig,
    last_app_resume_at: &mut Option<tokio::time::Instant>,
    pending: &mut PendingTrigger,
    channel_open: &mut bool,
) -> Option<CycleRequest> {
    loop {
        let now = tokio::time::Instant::now();
        if let Some(request) = pending.take_due(now, config) {
            return Some(request);
        }
        if !*channel_open && pending.is_empty() {
            return None;
        }

        let deadline = pending
            .write_deadline(config)
            .unwrap_or(now + Duration::from_secs(86400 * 365));
        let write_timer = tokio::time::sleep_until(deadline);
        tokio::pin!(write_timer);

        tokio::select! {
            biased;

            event = rx.recv(), if *channel_open => {
                match event {
                    None => *channel_open = false,
                    Some(event) => record_event(event, pending, config, last_app_resume_at),
                }
            }

            _ = &mut write_timer, if pending.writes.is_some() => {}

            _ = periodic.tick(), if *channel_open => {
                pending.request_immediate(TriggerReason::Periodic);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_cycle_and_collect(
    sync_fn: &SyncCycleFn,
    reason: TriggerReason,
    rx: &mut mpsc::Receiver<TriggerEvent>,
    periodic: &mut tokio::time::Interval,
    config: &TriggerConfig,
    last_app_resume_at: &mut Option<tokio::time::Instant>,
    pending: &mut PendingTrigger,
    channel_open: &mut bool,
) -> CycleResult {
    info!(?reason, "starting sync cycle");
    let cycle = sync_fn(reason);
    tokio::pin!(cycle);

    let result = loop {
        tokio::select! {
            biased;

            result = &mut cycle => break result,

            event = rx.recv(), if *channel_open => {
                match event {
                    None => *channel_open = false,
                    Some(event) => record_event(event, pending, config, last_app_resume_at),
                }
            }

            _ = periodic.tick(), if *channel_open => {
                pending.request_immediate(TriggerReason::Periodic);
            }
        }
    };
    log_cycle_result(reason, &result);
    result
}

fn record_event(
    event: TriggerEvent,
    pending: &mut PendingTrigger,
    config: &TriggerConfig,
    last_app_resume_at: &mut Option<tokio::time::Instant>,
) {
    match event {
        TriggerEvent::WriteOp { at } => pending.record_write(at),
        TriggerEvent::AppResume => {
            if accept_app_resume(last_app_resume_at, config.app_resume_min_interval) {
                pending.request_immediate(TriggerReason::AppResume);
            } else {
                debug!("dropping duplicate app-resume sync trigger");
            }
        }
        TriggerEvent::Manual { reply } => {
            pending.request_immediate(TriggerReason::Manual);
            pending.replies.push(reply);
        }
    }
}

fn trigger_priority(reason: TriggerReason) -> u8 {
    match reason {
        TriggerReason::Manual => 3,
        TriggerReason::AppResume => 2,
        TriggerReason::Periodic => 1,
        TriggerReason::WriteCoalesce => 0,
    }
}

fn accept_app_resume(
    last_app_resume_at: &mut Option<tokio::time::Instant>,
    min_interval: Duration,
) -> bool {
    let now = tokio::time::Instant::now();
    if let Some(last) = last_app_resume_at {
        if now.duration_since(*last) < min_interval {
            return false;
        }
    }
    *last_app_resume_at = Some(now);
    true
}

fn log_cycle_result(reason: TriggerReason, result: &CycleResult) {
    match result {
        Ok(r) => debug!(
            ?reason,
            events_received = r.events_received,
            events_sent = r.events_sent,
            "sync cycle completed"
        ),
        Err(e) => warn!(?reason, error = %e, "sync cycle failed"),
    }
}
