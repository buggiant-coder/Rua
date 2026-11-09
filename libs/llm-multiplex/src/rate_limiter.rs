use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, Semaphore};
use tokio::time::Instant;
use tracing::{debug, warn};

use agent_types::{AgentEvent, ChatRequest, ChatResponse};

use crate::error::LlmError;
use crate::traits::{ChatModel, EventStream, ModelInfo};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimiterStats {
    pub current_in_flight: usize,
    pub requests_last_60s: usize,
    pub is_paused: bool,
    pub pause_remaining_s: Option<f64>,
    pub total_acquired: u64,
    pub total_paused: u64,
    pub total_qpm_waited: u64,
    pub total_rate_limited: u64,
}

#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    pub max_concurrent: usize,
    pub max_qpm: usize,
    pub pause_seconds: f64,
    pub jitter_range: f64,
    pub acquire_timeout: f64,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 10,
            max_qpm: 0,
            pause_seconds: 30.0,
            jitter_range: 5.0,
            acquire_timeout: 120.0,
        }
    }
}

pub struct LlmRateLimiter {
    semaphore: Arc<Semaphore>,
    max_qpm: usize,
    default_pause: f64,
    jitter_range: f64,
    acquire_timeout: Duration,
    request_times: Mutex<VecDeque<Instant>>,
    pause_until: Mutex<Instant>,
    in_flight: AtomicUsize,
    total_acquired: AtomicU64,
    total_paused: AtomicU64,
    total_qpm_waited: AtomicU64,
    total_rate_limited: AtomicU64,
}

struct LlmRateLimitPermit {
    limiter: Arc<LlmRateLimiter>,
}

impl Drop for LlmRateLimitPermit {
    fn drop(&mut self) {
        self.limiter.release();
    }
}

impl LlmRateLimiter {
    pub const MAX_PAUSE_SECONDS: f64 = 60.0;

    pub fn new(config: &RateLimitConfig) -> Self {
        let max_concurrent = config.max_concurrent.max(1);
        Self {
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            max_qpm: config.max_qpm,
            default_pause: config.pause_seconds.max(0.0),
            jitter_range: config.jitter_range.max(0.0),
            acquire_timeout: Duration::from_secs_f64(config.acquire_timeout.max(1.0)),
            request_times: Mutex::new(VecDeque::new()),
            pause_until: Mutex::new(Instant::now()),
            in_flight: AtomicUsize::new(0),
            total_acquired: AtomicU64::new(0),
            total_paused: AtomicU64::new(0),
            total_qpm_waited: AtomicU64::new(0),
            total_rate_limited: AtomicU64::new(0),
        }
    }

    pub async fn acquire(&self) -> Result<(), LlmError> {
        tokio::time::timeout(self.acquire_timeout, self.acquire_inner())
            .await
            .map_err(|_| LlmError::Config {
                message: format!(
                    "rate limiter: timed out waiting {:.0}s for an execution slot",
                    self.acquire_timeout.as_secs_f64()
                ),
            })
    }

    async fn acquire_permit(self: &Arc<Self>) -> Result<LlmRateLimitPermit, LlmError> {
        self.acquire().await?;
        Ok(LlmRateLimitPermit {
            limiter: Arc::clone(self),
        })
    }

    async fn acquire_inner(&self) {
        let mut paused = false;
        loop {
            let pause_until = *self.pause_until.lock().await;
            let now = Instant::now();
            if now >= pause_until {
                break;
            }
            if !paused {
                self.total_paused.fetch_add(1, Ordering::Relaxed);
                paused = true;
            }
            let remaining = pause_until - now;
            let jitter = Duration::from_secs_f64(self.jitter());
            let wait_time = remaining + jitter;
            debug!(
                remaining_ms = remaining.as_millis() as u64,
                jitter_ms = jitter.as_millis() as u64,
                "rate limiter: 429 cooldown"
            );
            tokio::time::sleep(wait_time).await;
        }

        let permit = self
            .semaphore
            .acquire()
            .await
            .expect("semaphore should not be closed");
        permit.forget();
        self.in_flight.fetch_add(1, Ordering::Relaxed);

        if self.max_qpm > 0 {
            self.acquire_qpm_slot().await;
        }

        self.total_acquired.fetch_add(1, Ordering::Relaxed);
    }

    async fn acquire_qpm_slot(&self) {
        let mut waited = false;
        loop {
            let wait_time = {
                let mut times = self.request_times.lock().await;
                let now = Instant::now();
                let cutoff = now - Duration::from_secs(60);

                while times.front().is_some_and(|&t| t < cutoff) {
                    times.pop_front();
                }

                if times.len() < self.max_qpm {
                    times.push_back(now);
                    return;
                }

                if !waited {
                    self.total_qpm_waited.fetch_add(1, Ordering::Relaxed);
                    waited = true;
                }

                let oldest = *times.front().unwrap();
                (oldest + Duration::from_secs(60)) - now + Duration::from_millis(50)
            };

            debug!(
                max_qpm = self.max_qpm,
                wait_ms = wait_time.as_millis() as u64,
                "QPM limit reached, waiting for slot"
            );
            tokio::time::sleep(wait_time).await;
        }
    }

    pub fn release(&self) {
        self.in_flight.fetch_sub(1, Ordering::Relaxed);
        self.semaphore.add_permits(1);
    }

    pub async fn report_rate_limit(&self, retry_after: Option<f64>) {
        self.total_rate_limited.fetch_add(1, Ordering::Relaxed);
        let raw_pause = retry_after.unwrap_or(self.default_pause);

        let pause = raw_pause.min(Self::MAX_PAUSE_SECONDS);
        if pause < raw_pause {
            debug!(
                raw_pause,
                capped = pause,
                "rate limiter: capping oversized retry_after"
            );
        }
        let new_until = Instant::now() + Duration::from_secs_f64(pause);
        let mut pause_until = self.pause_until.lock().await;
        if new_until > *pause_until {
            *pause_until = new_until;
            warn!(pause_seconds = pause, "rate limiter: global pause set");
        }
    }

    pub async fn on_success(&self, acquired_at: Instant) {
        let mut pause_until = self.pause_until.lock().await;
        if *pause_until <= acquired_at {
            *pause_until = acquired_at;
        }
    }

    pub fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::Relaxed)
    }

    pub async fn stats(&self) -> RateLimiterStats {
        let current_in_flight = self.in_flight.load(Ordering::Relaxed);

        let requests_last_60s = {
            let mut times = self.request_times.lock().await;
            let now = Instant::now();
            let cutoff = now - Duration::from_secs(60);
            while times.front().is_some_and(|&t| t < cutoff) {
                times.pop_front();
            }
            times.len()
        };

        let (is_paused, pause_remaining_s) = {
            let pause_until = *self.pause_until.lock().await;
            let now = Instant::now();
            if now < pause_until {
                let remaining = (pause_until - now).as_secs_f64();
                (true, Some(remaining))
            } else {
                (false, None)
            }
        };

        RateLimiterStats {
            current_in_flight,
            requests_last_60s,
            is_paused,
            pause_remaining_s,
            total_acquired: self.total_acquired.load(Ordering::Relaxed),
            total_paused: self.total_paused.load(Ordering::Relaxed),
            total_qpm_waited: self.total_qpm_waited.load(Ordering::Relaxed),
            total_rate_limited: self.total_rate_limited.load(Ordering::Relaxed),
        }
    }

    fn jitter(&self) -> f64 {
        if self.jitter_range <= 0.0 {
            return 0.0;
        }
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        (nanos as f64 / u32::MAX as f64) * self.jitter_range
    }
}

#[derive(Default)]
pub struct LlmRateLimiterRegistry {
    limiters: StdMutex<HashMap<String, Arc<LlmRateLimiter>>>,
}

impl LlmRateLimiterRegistry {
    pub fn new() -> Self {
        Self {
            limiters: StdMutex::new(HashMap::new()),
        }
    }

    pub fn get_or_create(&self, key: &str, config: &RateLimitConfig) -> Arc<LlmRateLimiter> {
        let mut map = self
            .limiters
            .lock()
            .expect("rate limiter registry poisoned");
        if let Some(existing) = map.get(key) {
            return Arc::clone(existing);
        }
        let limiter = Arc::new(LlmRateLimiter::new(config));
        map.insert(key.to_string(), Arc::clone(&limiter));
        limiter
    }
}

pub struct RateLimitChatModel {
    inner: Box<dyn ChatModel>,
    limiter: Arc<LlmRateLimiter>,
}

impl RateLimitChatModel {
    pub fn new(inner: Box<dyn ChatModel>, limiter: Arc<LlmRateLimiter>) -> Self {
        Self { inner, limiter }
    }
}

#[async_trait]
impl ChatModel for RateLimitChatModel {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let mut trace = crate::debug_capture::CaptureTrace::new(&request.metadata, "rate-limit");
        let acquired_at = Instant::now();
        trace.checkpoint("acquire_started");
        let permit = self.limiter.acquire_permit().await;
        if permit.is_err() {
            trace.finish("error");
        }
        let _permit = permit?;
        trace.mark("acquired");
        trace.finish("completed");
        let result = self.inner.chat(request).await;
        match &result {
            Ok(_) => self.limiter.on_success(acquired_at).await,
            Err(LlmError::RateLimit { retry_after, .. }) => {
                self.limiter
                    .report_rate_limit(retry_after.map(|d| d.as_secs_f64()))
                    .await;
            }
            Err(_) => {}
        }
        result
    }

    fn chat_stream(&self, request: ChatRequest) -> EventStream {
        self.chat_stream_with_idle_timeout(request, Duration::ZERO)
    }

    fn chat_stream_with_idle_timeout(
        &self,
        request: ChatRequest,
        idle_timeout: Duration,
    ) -> EventStream {
        let trace = crate::debug_capture::CaptureTrace::new(&request.metadata, "rate-limit");
        let limiter = self.limiter.clone();
        let inner_stream = self
            .inner
            .chat_stream_with_idle_timeout(request, idle_timeout);
        Box::pin(RateLimitedStream::new(limiter, inner_stream, trace))
    }

    fn model_info(&self) -> &ModelInfo {
        self.inner.model_info()
    }
}

enum RlStreamState {
    Init,
    Acquiring(Pin<Box<dyn Future<Output = Result<(), LlmError>> + Send>>),

    WaitingFirstChunk,

    Streaming,
    Done,
}

struct RateLimitedStream {
    limiter: Arc<LlmRateLimiter>,
    inner: EventStream,
    state: RlStreamState,
    trace: crate::debug_capture::CaptureTrace,
}

impl RateLimitedStream {
    fn new(
        limiter: Arc<LlmRateLimiter>,
        inner: EventStream,
        trace: crate::debug_capture::CaptureTrace,
    ) -> Self {
        Self {
            limiter,
            inner,
            state: RlStreamState::Init,
            trace,
        }
    }
}

impl Stream for RateLimitedStream {
    type Item = Result<AgentEvent, LlmError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        loop {
            match this.state {
                RlStreamState::Init => {
                    this.trace.mark("acquire_started");
                    let limiter = this.limiter.clone();
                    let fut = Box::pin(async move { limiter.acquire().await });
                    this.state = RlStreamState::Acquiring(fut);
                }
                RlStreamState::Acquiring(ref mut fut) => match fut.as_mut().poll(cx) {
                    Poll::Ready(Ok(())) => {
                        this.trace.mark("acquired");
                        this.trace.finish("completed");
                        this.state = RlStreamState::WaitingFirstChunk;
                    }
                    Poll::Ready(Err(e)) => {
                        this.trace.finish("error");
                        this.state = RlStreamState::Done;
                        return Poll::Ready(Some(Err(e)));
                    }
                    Poll::Pending => {
                        this.trace.checkpoint("acquire_pending");
                        return Poll::Pending;
                    }
                },
                RlStreamState::WaitingFirstChunk => {
                    let poll = this.inner.as_mut().poll_next(cx);
                    match poll {
                        Poll::Ready(Some(Ok(_))) => {
                            this.limiter.release();
                            this.state = RlStreamState::Streaming;
                            return poll;
                        }
                        Poll::Ready(Some(Err(ref e))) => {
                            if let LlmError::RateLimit { retry_after, .. } = e {
                                let retry_after_secs = retry_after.map(|d| d.as_secs_f64());
                                let limiter = this.limiter.clone();
                                tokio::spawn(async move {
                                    limiter.report_rate_limit(retry_after_secs).await;
                                });
                            }
                            this.limiter.release();
                            this.state = RlStreamState::Done;
                            return poll;
                        }
                        Poll::Ready(None) => {
                            this.limiter.release();
                            this.state = RlStreamState::Done;
                            return Poll::Ready(None);
                        }
                        Poll::Pending => return Poll::Pending,
                    }
                }
                RlStreamState::Streaming => {
                    let poll = this.inner.as_mut().poll_next(cx);
                    if let Poll::Ready(Some(Err(LlmError::RateLimit { retry_after, .. }))) = &poll {
                        let retry_after_secs = retry_after.map(|d| d.as_secs_f64());
                        let limiter = this.limiter.clone();
                        tokio::spawn(async move {
                            limiter.report_rate_limit(retry_after_secs).await;
                        });
                    }
                    if matches!(&poll, Poll::Ready(None)) {
                        this.state = RlStreamState::Done;
                    }
                    return poll;
                }
                RlStreamState::Done => return Poll::Ready(None),
            }
        }
    }
}

impl Drop for RateLimitedStream {
    fn drop(&mut self) {
        if matches!(self.state, RlStreamState::WaitingFirstChunk) {
            self.limiter.release();
        }
    }
}
