use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;

pub type QueueKey = (String, String, u32);

#[async_trait]
pub trait QueueConsumer: Send + Sync + 'static {
    async fn consume(
        &self,
        rx: mpsc::Receiver<serde_json::Value>,
        channel_id: String,
        session_id: String,
        priority: u32,
    );
}

const DEFAULT_QUEUE_CAPACITY: usize = 1000;
const DEFAULT_IDLE_EVICTION_SECS: u64 = 600;
const DEFAULT_CLEANUP_INTERVAL_SECS: u64 = 60;

#[allow(dead_code)]
struct QueueEntry {
    tx: mpsc::Sender<serde_json::Value>,
    consumer_task: JoinHandle<()>,
    queue_capacity: usize,
    created_at: u64,
    last_activity: u64,
    processed_count: u64,
}

impl QueueEntry {
    fn is_queue_empty(&self) -> bool {
        self.tx.capacity() == self.queue_capacity
    }
}

fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub struct UnifiedQueueManager {
    consumer: Arc<dyn QueueConsumer>,
    queues: Arc<Mutex<HashMap<QueueKey, QueueEntry>>>,
    queue_capacity: usize,

    pub idle_eviction_secs: u64,
    cleanup_interval_secs: u64,
    cleanup_handle: std::sync::Mutex<Option<JoinHandle<()>>>,
    running: Arc<AtomicBool>,
}

impl UnifiedQueueManager {
    pub fn new(consumer: Arc<dyn QueueConsumer>) -> Self {
        Self {
            consumer,
            queues: Arc::new(Mutex::new(HashMap::new())),
            queue_capacity: DEFAULT_QUEUE_CAPACITY,
            idle_eviction_secs: DEFAULT_IDLE_EVICTION_SECS,
            cleanup_interval_secs: DEFAULT_CLEANUP_INTERVAL_SECS,
            cleanup_handle: std::sync::Mutex::new(None),
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn with_queue_capacity(mut self, capacity: usize) -> Self {
        self.queue_capacity = capacity;
        self
    }

    pub fn with_idle_eviction_secs(mut self, secs: u64) -> Self {
        self.idle_eviction_secs = secs;
        self
    }

    pub fn with_cleanup_interval_secs(mut self, secs: u64) -> Self {
        self.cleanup_interval_secs = secs;
        self
    }

    pub async fn enqueue(
        &self,
        channel_id: String,
        session_id: String,
        priority: u32,
        payload: serde_json::Value,
    ) -> rua_core::Result<()> {
        let key = (channel_id.clone(), session_id.clone(), priority);

        let tx = {
            let mut queues = self.queues.lock().await;

            if let Some(entry) = queues.get_mut(&key) {
                entry.last_activity = now_epoch_secs();
                entry.tx.clone()
            } else {
                let (tx, rx) = mpsc::channel(self.queue_capacity);
                let consumer = Arc::clone(&self.consumer);
                let ch = channel_id.clone();
                let sess = session_id.clone();

                let consumer_task = tokio::spawn(async move {
                    consumer.consume(rx, ch, sess, priority).await;
                });

                let entry = QueueEntry {
                    tx: tx.clone(),
                    consumer_task,
                    queue_capacity: self.queue_capacity,
                    created_at: now_epoch_secs(),
                    last_activity: now_epoch_secs(),
                    processed_count: 0,
                };
                queues.insert(key, entry);

                tracing::info!(
                    %channel_id,
                    %session_id,
                    priority,
                    "created queue and consumer"
                );

                tx
            }
        };

        tokio::time::timeout(Duration::from_secs(30), tx.send(payload))
            .await
            .map_err(|_| rua_core::RuaError::Timeout {
                operation: format!(
                    "queue send: channel={channel_id} session={session_id} priority={priority}"
                ),
            })?
            .map_err(|_| rua_core::RuaError::Runtime {
                message: format!(
                    "consumer closed: channel={channel_id} session={session_id} priority={priority}"
                ),
            })?;

        tracing::debug!(
            %channel_id,
            %session_id,
            priority,
            "enqueued message"
        );

        Ok(())
    }

    pub fn start_cleanup_loop(&self) {
        let mut handle = self.cleanup_handle.lock().unwrap();
        if handle.is_some() {
            tracing::warn!("cleanup loop already running");
            return;
        }

        self.running.store(true, Ordering::SeqCst);

        let queues = Arc::clone(&self.queues);
        let running = Arc::clone(&self.running);
        let idle_timeout = self.idle_eviction_secs;
        let interval = self.cleanup_interval_secs;

        *handle = Some(tokio::spawn(async move {
            run_cleanup(queues, running, idle_timeout, interval).await;
        }));

        tracing::info!("cleanup loop started");
    }

    pub async fn stop_all(&self) {
        self.running.store(false, Ordering::SeqCst);

        let cleanup_task = self.cleanup_handle.lock().unwrap().take();
        if let Some(task) = cleanup_task {
            task.abort();
            let _ = task.await;
        }

        let tasks: Vec<JoinHandle<()>> = {
            let mut queues = self.queues.lock().await;
            queues
                .drain()
                .map(|(key, entry)| {
                    tracing::info!(
                        channel_id = %key.0,
                        session_id = %key.1,
                        priority = key.2,
                        "stopping consumer"
                    );
                    entry.consumer_task
                })
                .collect()
        };

        for task in &tasks {
            task.abort();
        }
        for task in tasks {
            let _ = task.await;
        }

        tracing::info!("stopped all queues");
    }

    pub async fn increment_processed(
        &self,
        channel_id: &str,
        session_id: &str,
        priority: u32,
        count: u64,
    ) {
        let key = (channel_id.to_string(), session_id.to_string(), priority);
        let mut queues = self.queues.lock().await;
        if let Some(entry) = queues.get_mut(&key) {
            entry.processed_count += count;
            entry.last_activity = now_epoch_secs();
        }
    }

    pub async fn queue_count(&self) -> usize {
        self.queues.lock().await.len()
    }
}

async fn run_cleanup(
    queues: Arc<Mutex<HashMap<QueueKey, QueueEntry>>>,
    running: Arc<AtomicBool>,
    idle_timeout: u64,
    interval: u64,
) {
    tracing::info!("cleanup loop running");

    while running.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_secs(interval)).await;

        if !running.load(Ordering::SeqCst) {
            break;
        }

        let now = now_epoch_secs();
        let mut to_abort = Vec::new();

        {
            let mut queues = queues.lock().await;
            let idle_keys: Vec<QueueKey> = queues
                .iter()
                .filter(|(_, entry)| {
                    entry.is_queue_empty() && now.saturating_sub(entry.last_activity) > idle_timeout
                })
                .map(|(key, _)| key.clone())
                .collect();

            for key in idle_keys {
                if let Some(entry) = queues.remove(&key) {
                    tracing::info!(
                        channel_id = %key.0,
                        session_id = %key.1,
                        priority = key.2,
                        processed = entry.processed_count,
                        "cleaned up idle queue"
                    );
                    to_abort.push(entry.consumer_task);
                }
            }
        }

        for task in to_abort {
            task.abort();
            let _ = task.await;
        }
    }

    tracing::info!("cleanup loop stopped");
}
