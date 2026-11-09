use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::Response;
use futures::stream::Stream;
use rua_core::{runtime::RunControl, ChatAddress};
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch, Notify, RwLock};
use uuid::Uuid;

pub fn sse_response<S>(stream: S) -> Sse<axum::response::sse::KeepAliveStream<S>>
where
    S: Stream<Item = Result<Event, Infallible>> + Send + 'static,
{
    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keepalive"),
    )
}

pub fn sse_json_event(event_type: &str, data: &serde_json::Value) -> Result<Event, Infallible> {
    Ok(Event::default().event(event_type).data(data.to_string()))
}

pub fn sse_done_event() -> Result<Event, Infallible> {
    Ok(Event::default().event("done").data("[DONE]"))
}

pub fn sse_error_event(message: &str) -> Result<Event, Infallible> {
    let data = serde_json::json!({ "detail": message });
    Ok(Event::default().event("error").data(data.to_string()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Priority {
    Critical = 0,
    High = 10,
    Normal = 20,
    Low = 30,
}

impl Priority {
    pub const ALL: [Priority; 4] = [
        Priority::Critical,
        Priority::High,
        Priority::Normal,
        Priority::Low,
    ];
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StreamKey {
    pub channel_id: String,
    pub address: ChatAddress,
    pub priority: Priority,
}

impl StreamKey {
    pub fn new(channel_id: impl Into<String>, address: ChatAddress, priority: Priority) -> Self {
        Self {
            channel_id: channel_id.into(),
            address,
            priority,
        }
    }
}

pub struct ActiveStreamRegistry {
    streams: RwLock<HashMap<StreamKey, StreamEntry>>,
    key_available: RwLock<HashMap<StreamKey, Arc<Notify>>>,
}

struct StreamEntry {
    producer_id: Uuid,
    cancel_tx: watch::Sender<RunControl>,
    buffer: Vec<String>,
    subscriber_txs: Vec<mpsc::UnboundedSender<String>>,

    last_activity_at: std::time::Instant,

    cancelled: bool,

    drain_notify: Arc<Notify>,

    active_turn_id: Option<String>,

    producer_started: bool,
}

impl ActiveStreamRegistry {
    pub fn new() -> Self {
        Self {
            streams: RwLock::new(HashMap::new()),
            key_available: RwLock::new(HashMap::new()),
        }
    }

    pub async fn mark_completed(&self, key: &StreamKey, producer_id: Uuid) -> bool {
        let mut streams = self.streams.write().await;
        if let Some(entry) = streams.get(key) {
            if entry.producer_id == producer_id {
                let notify = entry.drain_notify.clone();
                streams.remove(key);

                notify.notify_waiters();
                return true;
            }
        }
        false
    }

    pub async fn register_active_keyed(
        &self,
        key: &StreamKey,
    ) -> (Uuid, watch::Receiver<RunControl>) {
        loop {
            let waiter = {
                let mut streams = self.streams.write().await;
                if let Some(entry) = streams.get(key) {
                    Some(entry.drain_notify.clone())
                } else {
                    let now = std::time::Instant::now();
                    let (cancel_tx, cancel_rx) = watch::channel(RunControl::Running);
                    let producer_id = Uuid::new_v4();
                    let entry = StreamEntry {
                        producer_id,
                        cancel_tx,
                        buffer: Vec::new(),
                        subscriber_txs: Vec::new(),
                        last_activity_at: now,
                        cancelled: false,
                        drain_notify: Arc::new(Notify::new()),
                        active_turn_id: None,
                        producer_started: true,
                    };
                    streams.insert(key.clone(), entry);
                    drop(streams);
                    self.notify_key_available(key).await;
                    return (producer_id, cancel_rx);
                }
            };

            if let Some(waiter) = waiter {
                let notified = waiter.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                let still_present = {
                    let streams = self.streams.read().await;
                    streams.contains_key(key)
                };
                if still_present {
                    notified.await;
                }
            }
        }
    }

    pub async fn push_event_keyed(&self, key: &StreamKey, event: String) {
        let now = std::time::Instant::now();
        let mut streams = self.streams.write().await;
        if let Some(entry) = streams.get_mut(key) {
            entry.buffer.push(event.clone());
            entry.last_activity_at = now;
            entry
                .subscriber_txs
                .retain(|tx| tx.send(event.clone()).is_ok());
        }
    }

    pub async fn subscribe_keyed(
        &self,
        key: &StreamKey,
    ) -> Option<(Vec<String>, mpsc::UnboundedReceiver<String>)> {
        let now = std::time::Instant::now();
        let mut streams = self.streams.write().await;
        if let Some(entry) = streams.get_mut(key) {
            if entry.cancelled {
                return None;
            }
            let buffer = entry.buffer.clone();
            let (tx, rx) = mpsc::unbounded_channel();
            entry.subscriber_txs.push(tx);
            entry.last_activity_at = now;
            Some((buffer, rx))
        } else {
            None
        }
    }

    pub async fn subscribe_keyed_or_wait(
        &self,
        key: &StreamKey,
    ) -> (Vec<String>, mpsc::UnboundedReceiver<String>) {
        loop {
            if let Some(subscription) = self.subscribe_keyed(key).await {
                return subscription;
            }
            self.wait_for_key_available(key).await;
        }
    }

    async fn wait_for_key_available(&self, key: &StreamKey) {
        let notify = {
            let mut waiters = self.key_available.write().await;
            Arc::clone(
                waiters
                    .entry(key.clone())
                    .or_insert_with(|| Arc::new(Notify::new())),
            )
        };
        let notified = notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.streams.read().await.contains_key(key) {
            return;
        }
        notified.await;
    }

    async fn notify_key_available(&self, key: &StreamKey) {
        if let Some(notify) = self.key_available.read().await.get(key).cloned() {
            notify.notify_waiters();
        }
    }

    pub async fn cancel_key(&self, key: &StreamKey) -> bool {
        let mut streams = self.streams.write().await;
        if let Some(entry) = streams.get_mut(key) {
            entry.cancelled = true;
            let _ = entry.cancel_tx.send(RunControl::Stop);

            entry.subscriber_txs.clear();
            true
        } else {
            false
        }
    }

    pub async fn cancel_all_priorities(&self, channel_id: &str, address: &ChatAddress) -> usize {
        let mut cancelled = 0;
        for priority in Priority::ALL {
            let key = StreamKey::new(channel_id, address.clone(), priority);
            if self.cancel_key(&key).await {
                cancelled += 1;
            }
        }
        cancelled
    }

    pub async fn cancel_address(&self, address: &ChatAddress) -> usize {
        let keys: Vec<_> = self
            .streams
            .read()
            .await
            .keys()
            .filter(|key| &key.address == address)
            .cloned()
            .collect();
        let mut cancelled = 0;
        for key in keys {
            cancelled += usize::from(self.cancel_key(&key).await);
        }
        cancelled
    }

    pub async fn interject_key(&self, key: &StreamKey) -> bool {
        let mut streams = self.streams.write().await;
        if let Some(entry) = streams.get_mut(key) {
            if entry.cancel_tx.borrow().is_stop() {
                return false;
            }
            let _ = entry.cancel_tx.send(RunControl::Interject);
            true
        } else {
            false
        }
    }

    pub async fn interject_key_at_boundary(&self, key: &StreamKey) -> bool {
        let mut streams = self.streams.write().await;
        if let Some(entry) = streams.get_mut(key) {
            if entry.cancel_tx.borrow().is_stop() {
                return false;
            }
            let _ = entry.cancel_tx.send(RunControl::InterjectAtBoundary);
            true
        } else {
            false
        }
    }

    pub async fn interject_after_tool_all_priorities(
        &self,
        channel_id: &str,
        address: &ChatAddress,
    ) {
        let mut streams = self.streams.write().await;
        for priority in Priority::ALL {
            if let Some(entry) =
                streams.get_mut(&StreamKey::new(channel_id, address.clone(), priority))
            {
                if !entry.cancel_tx.borrow().is_stop() {
                    let _ = entry.cancel_tx.send(RunControl::InterjectAfterTool);
                }
            }
        }
    }

    pub async fn continue_key_at_boundary(&self, key: &StreamKey) -> bool {
        let mut streams = self.streams.write().await;
        if let Some(entry) = streams.get_mut(key) {
            if entry.cancel_tx.borrow().is_stop() {
                return false;
            }
            let _ = entry.cancel_tx.send(RunControl::ContinueAtBoundary);
            true
        } else {
            false
        }
    }

    pub async fn interject_all_priorities(&self, channel_id: &str, address: &ChatAddress) -> usize {
        let mut interjected = 0;
        for priority in Priority::ALL {
            let key = StreamKey::new(channel_id, address.clone(), priority);
            if self.interject_key(&key).await {
                interjected += 1;
            }
        }
        interjected
    }

    pub async fn interject_all_priorities_at_boundary(
        &self,
        channel_id: &str,
        address: &ChatAddress,
    ) -> usize {
        let mut interjected = 0;
        for priority in Priority::ALL {
            let key = StreamKey::new(channel_id, address.clone(), priority);
            if self.interject_key_at_boundary(&key).await {
                interjected += 1;
            }
        }
        interjected
    }

    pub async fn continue_all_priorities_at_boundary(
        &self,
        channel_id: &str,
        address: &ChatAddress,
    ) -> usize {
        let mut continued = 0;
        for priority in Priority::ALL {
            let key = StreamKey::new(channel_id, address.clone(), priority);
            if self.continue_key_at_boundary(&key).await {
                continued += 1;
            }
        }
        continued
    }

    pub async fn clear_pending_control(&self, key: &StreamKey) {
        let mut streams = self.streams.write().await;
        if let Some(entry) = streams.get_mut(key) {
            let control = *entry.cancel_tx.borrow();
            if control.is_interject() || matches!(control, RunControl::ContinueAtBoundary) {
                let _ = entry.cancel_tx.send(RunControl::Running);
            }
        }
    }

    pub async fn set_active_turn_id(
        &self,
        channel_id: &str,
        address: &ChatAddress,
        priority: Priority,
        turn_id: impl Into<String>,
    ) -> bool {
        let key = StreamKey::new(channel_id, address.clone(), priority);
        let mut streams = self.streams.write().await;
        let Some(entry) = streams.get_mut(&key) else {
            return false;
        };
        entry.active_turn_id = Some(turn_id.into());
        true
    }

    pub async fn active_turn_id_for_chat(
        &self,
        channel_id: &str,
        address: &ChatAddress,
    ) -> Option<String> {
        let streams = self.streams.read().await;
        Priority::ALL.iter().find_map(|priority| {
            streams
                .get(&StreamKey::new(channel_id, address.clone(), *priority))
                .and_then(|entry| entry.active_turn_id.clone())
        })
    }

    pub async fn unstarted_active_turn_id_for_chat(
        &self,
        channel_id: &str,
        address: &ChatAddress,
    ) -> Option<String> {
        let streams = self.streams.read().await;
        Priority::ALL.iter().find_map(|priority| {
            streams
                .get(&StreamKey::new(channel_id, address.clone(), *priority))
                .filter(|entry| !entry.producer_started)
                .and_then(|entry| entry.active_turn_id.clone())
        })
    }

    pub async fn is_active_for_chat(&self, channel_id: &str, address: &ChatAddress) -> bool {
        let streams = self.streams.read().await;
        Priority::ALL
            .iter()
            .any(|p| streams.contains_key(&StreamKey::new(channel_id, address.clone(), *p)))
    }

    pub async fn active_count(&self) -> usize {
        self.streams.read().await.len()
    }

    pub async fn cleanup_stale(&self, max_age: Duration) -> usize {
        let now = std::time::Instant::now();
        let mut streams = self.streams.write().await;
        let mut removed = Vec::new();
        streams.retain(|_key, entry| {
            let keep = now.duration_since(entry.last_activity_at) < max_age;
            if !keep {
                removed.push(entry.drain_notify.clone());
            }
            keep
        });
        drop(streams);
        for notify in &removed {
            notify.notify_waiters();
        }
        removed.len()
    }
}

impl Default for ActiveStreamRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub struct CancelOnDropStream<S> {
    inner: S,
    cancel_tx: Option<watch::Sender<bool>>,
}

impl<S: Stream + Unpin> Stream for CancelOnDropStream<S> {
    type Item = S::Item;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        std::pin::Pin::new(&mut self.inner).poll_next(cx)
    }
}

impl<S> Drop for CancelOnDropStream<S> {
    fn drop(&mut self) {
        if let Some(tx) = self.cancel_tx.take() {
            let _ = tx.send(true);
        }
    }
}

pub fn with_disconnect_cancel<S>(stream: S, cancel_tx: watch::Sender<bool>) -> CancelOnDropStream<S>
where
    S: Stream + Unpin + Send + 'static,
{
    CancelOnDropStream {
        inner: stream,
        cancel_tx: Some(cancel_tx),
    }
}

pub async fn websocket_upgrade_handler() -> Response {
    crate::errors::api_not_implemented("websocket")
}
