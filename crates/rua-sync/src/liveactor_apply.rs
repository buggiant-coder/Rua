use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures_lite::StreamExt;
use iroh_docs::engine::LiveEvent;
use iroh_docs::protocol::Docs;
use iroh_docs::{AuthorId, NamespaceId};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

const DEBOUNCE_DELAY: Duration = Duration::from_millis(250);

#[derive(Debug, Clone)]
pub enum ApplyMode {
    RepairScan { force: bool },

    Incremental(Vec<InboundEntryRef>, Vec<iroh_blobs::Hash>),

    Drain(Vec<iroh_blobs::Hash>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundEntryRef {
    pub author: AuthorId,
    pub key: Vec<u8>,
    pub content_hash: iroh_blobs::Hash,
}

/// Apply one bounded batch and report when durable inbox work is next due.
/// `None` means no pending or retryable work remains.
pub type ApplyFn =
    Arc<dyn Fn(ApplyMode) -> Pin<Box<dyn Future<Output = Option<Duration>> + Send>> + Send + Sync>;

pub struct LiveApplyHandle {
    task: JoinHandle<()>,
    cancel: CancellationToken,
}

impl LiveApplyHandle {
    pub fn from_task(task: JoinHandle<()>) -> Self {
        Self {
            task,
            cancel: CancellationToken::new(),
        }
    }

    pub fn abort(self) {
        self.cancel.cancel();
        self.task.abort();
    }

    pub async fn shutdown(self) {
        self.cancel.cancel();
        self.task.abort();
        let _ = self.task.await;
    }
}

pub fn subscribe_and_apply(
    docs: Docs,
    namespace_id: NamespaceId,
    apply_fn: ApplyFn,
) -> LiveApplyHandle {
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let task = tokio::spawn(async move {
        tokio::select! {
            biased;
            _ = task_cancel.cancelled() => {}
            _ = live_apply_loop(docs, namespace_id, apply_fn, task_cancel.clone()) => {}
        }
    });
    LiveApplyHandle { task, cancel }
}

async fn live_apply_loop(
    docs: Docs,
    namespace_id: NamespaceId,
    apply_fn: ApplyFn,
    cancel: CancellationToken,
) {
    let doc = match docs.open(namespace_id).await {
        Ok(Some(d)) => d,
        Ok(None) => {
            warn!("[liveactor] namespace not found, subscriber exiting");
            apply_fn(ApplyMode::RepairScan { force: true }).await;
            return;
        }
        Err(e) => {
            warn!(error = %e, "[liveactor] failed to open doc, subscriber exiting");
            apply_fn(ApplyMode::RepairScan { force: true }).await;
            return;
        }
    };

    let stream = match doc.subscribe().await {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "[liveactor] subscribe failed, exiting");
            apply_fn(ApplyMode::RepairScan { force: true }).await;
            return;
        }
    };

    info!("[liveactor] subscribed to iroh-docs entry changes");

    apply_inbound_event_stream(stream, apply_fn, cancel).await;
}

/// The production subscription scheduler, exposed for contract tests with a
/// controlled event source and the same durable inbox used by the server.
#[doc(hidden)]
pub async fn apply_inbound_event_stream(
    stream: impl futures_lite::Stream<Item = Result<LiveEvent, anyhow::Error>>,
    apply_fn: ApplyFn,
    cancel: CancellationToken,
) {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => {}
        _ = apply_stream(stream, apply_fn, cancel.clone()) => {}
    }
}

async fn apply_stream(
    stream: impl futures_lite::Stream<Item = Result<LiveEvent, anyhow::Error>>,
    apply_fn: ApplyFn,
    cancel: CancellationToken,
) {
    let mut next_drain = drain_deadline(apply_fn(ApplyMode::RepairScan { force: false }).await);
    let mut stream_ended = false;

    tokio::pin!(stream);

    loop {
        if cancel.is_cancelled() {
            return;
        }
        let mut pending = Vec::new();
        let mut content_ready = Vec::new();
        let (action, scheduled) = if stream_ended {
            if next_drain.is_none() {
                break;
            }
            wait_for_drain_deadline(next_drain).await;
            (Some(LiveEventAction::Drain), true)
        } else {
            tokio::select! {
                action = wait_for_inbound_event(&mut stream, &mut pending, &mut content_ready) => (action, false),
                _ = wait_for_drain_deadline(next_drain) => (Some(LiveEventAction::Drain), true),
            }
        };
        let Some(mut action) = action else {
            warn!("[liveactor] event stream ended; requesting Repair Scan");
            next_drain = drain_deadline(apply_fn(ApplyMode::RepairScan { force: true }).await);
            stream_ended = true;
            continue;
        };

        if !scheduled {
            debounce(
                &mut stream,
                &mut pending,
                &mut content_ready,
                &mut action,
                next_drain,
                &cancel,
            )
            .await;
        }
        if cancel.is_cancelled() {
            return;
        }

        if action == LiveEventAction::RepairScan && !content_ready.is_empty() {
            apply_fn(ApplyMode::Incremental(Vec::new(), content_ready.clone())).await;
        }
        let mode = if action == LiveEventAction::RepairScan {
            ApplyMode::RepairScan { force: true }
        } else if pending.is_empty() {
            match action {
                LiveEventAction::Entry => ApplyMode::Incremental(Vec::new(), content_ready),
                LiveEventAction::Drain => ApplyMode::Drain(content_ready),
                LiveEventAction::RepairScan => ApplyMode::RepairScan { force: true },
            }
        } else {
            ApplyMode::Incremental(pending, content_ready)
        };
        debug!("[liveactor] debounce window closed, running batch apply");
        next_drain = drain_deadline(apply_fn(mode).await);
        info!("[liveactor] batch apply completed");
        tokio::task::yield_now().await;
    }
}

fn drain_deadline(delay: Option<Duration>) -> Option<tokio::time::Instant> {
    // Yield between bounded drains, including immediate pending work. This also
    // gives incoming events and cancellation a chance to run before continuing.
    delay.map(|delay| {
        tokio::time::Instant::now()
            + if delay.is_zero() {
                DEBOUNCE_DELAY
            } else {
                delay
            }
    })
}

async fn wait_for_drain_deadline(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveEventAction {
    Entry,
    Drain,
    RepairScan,
}

impl LiveEventAction {
    fn priority(self) -> u8 {
        match self {
            Self::Entry => 0,
            Self::Drain => 1,
            Self::RepairScan => 2,
        }
    }
}

pub fn merge_live_event_action(current: LiveEventAction, next: LiveEventAction) -> LiveEventAction {
    if next.priority() > current.priority() {
        next
    } else {
        current
    }
}

fn extract_entry_ref(ev: &LiveEvent) -> Option<InboundEntryRef> {
    match ev {
        LiveEvent::InsertRemote { entry, .. } => Some(InboundEntryRef {
            author: entry.author(),
            key: entry.key().to_vec(),
            content_hash: entry.content_hash(),
        }),
        _ => None,
    }
}

fn push_content_ready(ready: &mut Vec<iroh_blobs::Hash>, event: &LiveEvent) {
    if let LiveEvent::ContentReady { hash } = event {
        if !ready.contains(hash) {
            ready.push(*hash);
        }
    }
}

pub fn classify_live_event(ev: &LiveEvent) -> Option<LiveEventAction> {
    match ev {
        LiveEvent::InsertRemote { .. } => Some(LiveEventAction::Entry),
        LiveEvent::SyncFinished(_) => Some(LiveEventAction::Drain),
        LiveEvent::ContentReady { .. } | LiveEvent::PendingContentReady => {
            Some(LiveEventAction::Drain)
        }
        _ => None,
    }
}

#[doc(hidden)]
pub async fn wait_for_inbound_event(
    stream: &mut (impl futures_lite::Stream<Item = Result<LiveEvent, anyhow::Error>> + Unpin),
    pending: &mut Vec<InboundEntryRef>,
    content_ready: &mut Vec<iroh_blobs::Hash>,
) -> Option<LiveEventAction> {
    loop {
        match stream.next().await {
            None => return None,
            Some(Ok(ref ev)) => {
                let Some(action) = classify_live_event(ev) else {
                    continue;
                };
                if let Some(entry) = extract_entry_ref(ev) {
                    push_unique_entry_ref(pending, entry);
                }
                push_content_ready(content_ready, ev);
                return Some(action);
            }
            Some(Err(e)) => {
                warn!(error = %e, "[liveactor] event stream error");
                return Some(LiveEventAction::RepairScan);
            }
        }
    }
}

async fn debounce(
    stream: &mut (impl futures_lite::Stream<Item = Result<LiveEvent, anyhow::Error>> + Unpin),
    pending: &mut Vec<InboundEntryRef>,
    content_ready: &mut Vec<iroh_blobs::Hash>,
    action: &mut LiveEventAction,
    next_drain: Option<tokio::time::Instant>,
    cancel: &CancellationToken,
) {
    let sleep = tokio::time::sleep(DEBOUNCE_DELAY);
    tokio::pin!(sleep);

    loop {
        let now = tokio::time::Instant::now();
        if cancel.is_cancelled()
            || now >= sleep.deadline()
            || next_drain.is_some_and(|deadline| now >= deadline)
        {
            return;
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return,
            // A stream of new content must not postpone already-due work.
            _ = wait_for_drain_deadline(next_drain) => return,
            _ = &mut sleep => return,
            event = stream.next() => {
                match event {
                    None => return,
                    Some(Ok(ref ev)) => {
                        let Some(next_action) = classify_live_event(ev) else {
                            continue;
                        };
                        *action = merge_live_event_action(*action, next_action);
                        if let Some(entry) = extract_entry_ref(ev) {
                            push_unique_entry_ref(pending, entry);
                        }
                        push_content_ready(content_ready, ev);
                        // Keep the original coalescing deadline. A continuous
                        // stream must not indefinitely delay its first apply.
                    }
                    Some(Err(error)) => {
                        warn!(%error, "[liveactor] event stream error during debounce");
                        *action = LiveEventAction::RepairScan;
                        return;
                    }
                }
            }
        }
    }
}

pub fn push_unique_entry_ref(pending: &mut Vec<InboundEntryRef>, entry: InboundEntryRef) {
    if pending.iter().any(|existing| existing == &entry) {
        return;
    }
    pending.push(entry);
}
