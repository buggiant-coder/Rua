use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use futures::Stream;
use tokio::time::{Instant, Sleep};
use tracing::{debug, info, warn};

use agent_types::{AgentEvent, ChatRequest, ChatResponse, ContentPart, Role};
use llm_multiplex::rate_limiter::LlmRateLimiter;
use llm_multiplex::{ChatModel, EventStream, LlmError, ModelInfo};

use crate::capability_cache;
use crate::error::ErrorKind;

fn jitter_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64
}

pub fn add_jitter(delay: Duration) -> Duration {
    let jitter_frac = (jitter_nanos() % 250) as f64 / 1000.0;
    let jitter = Duration::from_millis((delay.as_millis() as f64 * jitter_frac) as u64);
    delay + jitter
}

#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_retries: u32,

    pub initial_delay: Duration,

    pub max_delay: Duration,

    pub backoff_factor: f64,

    /// Optional overall retry limit. Streaming enforces it across attempts and backoff.
    pub max_total_time: Option<Duration>,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
            backoff_factor: 2.0,
            max_total_time: None,
        }
    }
}

impl RetryPolicy {
    pub fn mobile() -> Self {
        Self {
            max_retries: 2,
            initial_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(10),
            backoff_factor: 2.0,
            max_total_time: Some(Duration::from_secs(20)),
        }
    }

    pub fn delay_for_attempt(&self, attempt: u32, retry_after: Option<Duration>) -> Duration {
        if let Some(ra) = retry_after {
            return add_jitter(ra.min(self.max_delay));
        }
        let delay =
            self.initial_delay.as_millis() as f64 * self.backoff_factor.powi(attempt as i32);
        let delay = Duration::from_millis(delay as u64);
        add_jitter(delay.min(self.max_delay))
    }
}

pub struct RetryChatModel {
    inner: Arc<dyn ChatModel>,
    policy: RetryPolicy,
    cache_key: std::sync::OnceLock<String>,
}

impl RetryChatModel {
    pub fn new(inner: Box<dyn ChatModel>, policy: RetryPolicy) -> Self {
        Self {
            inner: Arc::from(inner),
            policy,
            cache_key: std::sync::OnceLock::new(),
        }
    }

    pub fn with_defaults(inner: Box<dyn ChatModel>) -> Self {
        Self::new(inner, RetryPolicy::default())
    }

    fn set_cache_key_from_error(&self, err: &LlmError) {
        if self.cache_key.get().is_some() {
            return;
        }
        if let Some(provider) = extract_provider(err) {
            let model_id = &self.inner.model_info().id;
            let key = capability_cache::cache_key(provider, model_id);
            let _ = self.cache_key.set(key);
        }
    }

    fn apply_cached_capabilities(&self, request: &mut ChatRequest) {
        if let Some(key) = self.cache_key.get() {
            if capability_cache::needs_reasoning_content(key) {
                inject_reasoning_content(request);
            }
        }
    }
}

pub fn is_stream_retryable(err: &LlmError) -> bool {
    matches!(
        err,
        LlmError::RateLimit { .. }
            | LlmError::Server { .. }
            | LlmError::Connection { .. }
            | LlmError::Timeout { .. }
    )
}

fn extract_retry_after(err: &LlmError) -> Option<Duration> {
    match err {
        LlmError::RateLimit { retry_after, .. } => *retry_after,
        _ => None,
    }
}

fn retry_after_exceeds_max_pause(err: &LlmError) -> bool {
    extract_retry_after(err).is_some_and(|d| d.as_secs_f64() > LlmRateLimiter::MAX_PAUSE_SECONDS)
}

fn retry_budget_remaining(policy: &RetryPolicy, started_at: Instant) -> bool {
    match policy.max_total_time {
        Some(max_total) => started_at.elapsed() < max_total,
        None => true,
    }
}

fn http_status_from_message(message: &str) -> Option<u16> {
    message
        .strip_prefix("HTTP ")
        .and_then(|rest| rest.split(':').next())
        .and_then(|code| code.trim().parse::<u16>().ok())
}

fn is_missing_reasoning_content_error(err: &LlmError) -> bool {
    let message = match err {
        LlmError::Unknown { message, .. } => message.as_str(),
        LlmError::InvalidResponse { message, .. } => message.as_str(),
        _ => return false,
    };

    http_status_from_message(message) == Some(400) && message.contains("reasoning_content")
}

fn inject_reasoning_content(request: &mut ChatRequest) -> bool {
    let mut modified = false;
    for msg in &mut request.messages {
        if msg.role != Role::Assistant {
            continue;
        }
        let has_thinking = msg.content.iter().any(|p| p.is_thinking());
        if !has_thinking {
            msg.content.push(ContentPart::Thinking {
                text: " ".into(),
                signature: None,
            });
            modified = true;
        }
    }
    modified
}

fn extract_provider(err: &LlmError) -> Option<&str> {
    match err {
        LlmError::Auth { provider, .. }
        | LlmError::RateLimit { provider, .. }
        | LlmError::Server { provider, .. }
        | LlmError::Timeout { provider, .. }
        | LlmError::RetryDeadlineExceeded { provider, .. }
        | LlmError::Connection { provider, .. }
        | LlmError::InvalidResponse { provider, .. }
        | LlmError::ModelNotFound { provider, .. }
        | LlmError::Unknown { provider, .. } => Some(provider),
        LlmError::Cancelled | LlmError::Config { .. } => None,
    }
}

pub fn is_content_event(event: &AgentEvent) -> bool {
    matches!(
        event,
        AgentEvent::TextDelta { .. }
            | AgentEvent::ThinkingDelta { .. }
            | AgentEvent::ToolCallDelta { .. }
            | AgentEvent::ToolCall { .. }
            | AgentEvent::ToolResultDelta { .. }
    )
}

pub enum RetryStreamState {
    Streaming {
        stream: EventStream,
        inner: Arc<dyn ChatModel>,
        request: Box<ChatRequest>,
        policy: RetryPolicy,
        attempt: u32,
    },
    Sleeping {
        sleep: Pin<Box<dyn Future<Output = ()> + Send>>,
        last_error: Box<Result<AgentEvent, LlmError>>,
        inner: Arc<dyn ChatModel>,
        request: Box<ChatRequest>,
        policy: RetryPolicy,
        attempt: u32,
    },
    Done,
}

pub struct RetryStream {
    pub state: RetryStreamState,
    pub saw_content: bool,
    pub saw_start: bool,
    pub started_at: Instant,
    pub idle_timeout: Duration,
    pub deadline: Option<Pin<Box<Sleep>>>,
}

impl Stream for RetryStream {
    type Item = Result<AgentEvent, LlmError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            if let Some(deadline) = &mut this.deadline {
                if deadline.as_mut().poll(cx).is_ready() {
                    let state = std::mem::replace(&mut this.state, RetryStreamState::Done);
                    return Poll::Ready(match state {
                        RetryStreamState::Sleeping { last_error, .. } => Some(*last_error),
                        RetryStreamState::Streaming { inner, policy, .. } => {
                            Some(Err(LlmError::RetryDeadlineExceeded {
                                provider: inner.model_info().id.clone(),
                                timeout: policy.max_total_time.expect("retry deadline duration"),
                            }))
                        }
                        RetryStreamState::Done => None,
                    });
                }
            }
            match &mut this.state {
                RetryStreamState::Streaming { stream, .. } => match stream.as_mut().poll_next(cx) {
                    Poll::Ready(Some(Ok(event))) => {
                        if matches!(
                            &event,
                            AgentEvent::Error {
                                retryable: true,
                                ..
                            }
                        ) && !this.saw_content
                        {
                            let old = std::mem::replace(&mut this.state, RetryStreamState::Done);
                            if let RetryStreamState::Streaming {
                                inner,
                                request,
                                policy,
                                attempt,
                                ..
                            } = old
                            {
                                if attempt < policy.max_retries
                                    && retry_budget_remaining(&policy, this.started_at)
                                {
                                    let delay = policy.delay_for_attempt(attempt, None);
                                    warn!(
                                        attempt,
                                        max_retries = policy.max_retries,
                                        delay_ms = delay.as_millis(),
                                        "stream reported a retryable terminal error, retrying model call"
                                    );
                                    this.state = RetryStreamState::Sleeping {
                                        sleep: Box::pin(tokio::time::sleep(delay)),
                                        last_error: Box::new(Ok(event)),
                                        inner,
                                        request,
                                        policy,
                                        attempt,
                                    };
                                    this.saw_content = false;
                                    continue;
                                }
                            }
                        }
                        if event.is_start() {
                            if this.saw_start {
                                continue;
                            }
                            this.saw_start = true;
                        }
                        if event.is_content() {
                            this.saw_content = true;
                        }
                        if event.is_terminal() {
                            this.state = RetryStreamState::Done;
                        }
                        return Poll::Ready(Some(Ok(event)));
                    }
                    Poll::Ready(Some(Err(e))) => {
                        let old = std::mem::replace(&mut this.state, RetryStreamState::Done);
                        if let RetryStreamState::Streaming {
                            inner,
                            request,
                            policy,
                            attempt,
                            ..
                        } = old
                        {
                            if is_stream_retryable(&e)
                                && !this.saw_content
                                && !retry_after_exceeds_max_pause(&e)
                                && attempt < policy.max_retries
                                && retry_budget_remaining(&policy, this.started_at)
                            {
                                let retry_after = extract_retry_after(&e);
                                let delay = policy.delay_for_attempt(attempt, retry_after);
                                warn!(
                                    attempt,
                                    max_retries = policy.max_retries,
                                    delay_ms = delay.as_millis(),
                                    error = %e,
                                    "stream failed, retrying from scratch"
                                );
                                this.state = RetryStreamState::Sleeping {
                                    sleep: Box::pin(tokio::time::sleep(delay)),
                                    last_error: Box::new(Err(e)),
                                    inner,
                                    request,
                                    policy,
                                    attempt,
                                };
                                this.saw_content = false;
                                continue;
                            }
                        }
                        return Poll::Ready(Some(Err(e)));
                    }
                    Poll::Ready(None) => {
                        let old = std::mem::replace(&mut this.state, RetryStreamState::Done);
                        if let RetryStreamState::Streaming {
                            inner,
                            request,
                            policy,
                            attempt,
                            ..
                        } = old
                        {
                            let error = LlmError::Connection {
                                provider: inner.model_info().id.clone(),
                                message: "stream closed before terminal End/Error event"
                                    .to_string(),
                            };
                            if !this.saw_content
                                && attempt < policy.max_retries
                                && retry_budget_remaining(&policy, this.started_at)
                            {
                                let delay = policy.delay_for_attempt(attempt, None);
                                warn!(
                                    attempt,
                                    max_retries = policy.max_retries,
                                    delay_ms = delay.as_millis(),
                                    error = %error,
                                    "stream closed before terminal event, retrying from scratch"
                                );
                                this.state = RetryStreamState::Sleeping {
                                    sleep: Box::pin(tokio::time::sleep(delay)),
                                    last_error: Box::new(Err(error)),
                                    inner,
                                    request,
                                    policy,
                                    attempt,
                                };
                                this.saw_content = false;
                                continue;
                            }
                            return Poll::Ready(Some(Err(error)));
                        }
                        return Poll::Ready(None);
                    }
                    Poll::Pending => return Poll::Pending,
                },
                RetryStreamState::Sleeping { sleep, .. } => match sleep.as_mut().poll(cx) {
                    Poll::Ready(()) => {
                        let old = std::mem::replace(&mut this.state, RetryStreamState::Done);
                        if let RetryStreamState::Sleeping {
                            inner,
                            request,
                            policy,
                            attempt,
                            ..
                        } = old
                        {
                            let retry_stream = inner.chat_stream_with_idle_timeout(
                                (*request).clone(),
                                this.idle_timeout,
                            );
                            this.state = RetryStreamState::Streaming {
                                stream: retry_stream,
                                inner,
                                request,
                                policy,
                                attempt: attempt + 1,
                            };
                            this.saw_content = false;
                            continue;
                        }
                    }
                    Poll::Pending => return Poll::Pending,
                },
                RetryStreamState::Done => return Poll::Ready(None),
            }
        }
    }
}

#[async_trait]
impl ChatModel for RetryChatModel {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let mut request = request;
        self.apply_cached_capabilities(&mut request);

        let start = std::time::Instant::now();
        let mut last_error = None;

        for attempt in 0..=self.policy.max_retries {
            if attempt > 0 {
                if let Some(max_total) = self.policy.max_total_time {
                    if start.elapsed() > max_total {
                        debug!(
                            attempt,
                            elapsed_ms = start.elapsed().as_millis(),
                            "retry budget exhausted"
                        );
                        break;
                    }
                }

                let retry_after = last_error.as_ref().and_then(|e: &LlmError| {
                    if let LlmError::RateLimit { retry_after, .. } = e {
                        *retry_after
                    } else {
                        None
                    }
                });

                let delay = self.policy.delay_for_attempt(attempt - 1, retry_after);
                debug!(
                    attempt,
                    delay_ms = delay.as_millis(),
                    "retrying non-stream request"
                );
                tokio::time::sleep(delay).await;
            }

            match self.inner.chat(request.clone()).await {
                Ok(response) => return Ok(response),
                Err(e) => {
                    self.set_cache_key_from_error(&e);

                    if is_missing_reasoning_content_error(&e)
                        && inject_reasoning_content(&mut request)
                    {
                        if let Some(key) = self.cache_key.get() {
                            capability_cache::learn_needs_reasoning_content(key);
                        }
                        info!("injected empty reasoning_content, retrying");
                        match self.inner.chat(request.clone()).await {
                            Ok(response) => return Ok(response),
                            Err(retry_err) => return Err(retry_err),
                        }
                    }

                    let kind = ErrorKind::classify(&e);
                    if !kind.is_retryable() {
                        return Err(e);
                    }

                    if retry_after_exceeds_max_pause(&e) {
                        warn!(error = %e, "retry-after exceeds cap, not retrying");
                        return Err(e);
                    }
                    warn!(
                        attempt,
                        error = %e,
                        error_kind = %kind,
                        "request failed, will retry"
                    );
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| LlmError::Unknown {
            provider: "retry".into(),
            message: "retry exhausted without error".into(),
        }))
    }

    fn chat_stream(&self, request: ChatRequest) -> EventStream {
        self.chat_stream_with_idle_timeout(request, Duration::ZERO)
    }

    fn chat_stream_with_idle_timeout(
        &self,
        request: ChatRequest,
        idle_timeout: Duration,
    ) -> EventStream {
        let mut request = request;
        self.apply_cached_capabilities(&mut request);

        let stream = self
            .inner
            .chat_stream_with_idle_timeout(request.clone(), idle_timeout);
        Box::pin(RetryStream {
            state: RetryStreamState::Streaming {
                stream,
                inner: self.inner.clone(),
                request: Box::new(request),
                policy: self.policy.clone(),
                attempt: 0,
            },
            saw_content: false,
            saw_start: false,
            started_at: Instant::now(),
            idle_timeout,
            deadline: self
                .policy
                .max_total_time
                .map(|limit| Box::pin(tokio::time::sleep(limit))),
        })
    }

    fn model_info(&self) -> &ModelInfo {
        self.inner.model_info()
    }
}
