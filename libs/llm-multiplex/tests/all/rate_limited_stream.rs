use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_types::{AgentEvent, ChatRequest, ChatResponse};
use async_trait::async_trait;
use futures::StreamExt;
use llm_multiplex::{
    ChatModel, EventStream, LlmError, LlmRateLimiter, ModelInfo, RateLimitChatModel,
    RateLimitConfig,
};

struct StreamModel {
    events: Mutex<Option<Vec<Result<AgentEvent, LlmError>>>>,
    info: ModelInfo,
}

impl StreamModel {
    fn new(events: Vec<Result<AgentEvent, LlmError>>) -> Self {
        Self {
            events: Mutex::new(Some(events)),
            info: ModelInfo {
                id: "test-model".into(),
                name: "Test Model".into(),
                context_window: None,
                max_output_tokens: None,
                supports_multimodal: None,
                supports_image: None,
                supports_video: None,
                probe_source: None,
                generate_kwargs: serde_json::Map::new(),
            },
        }
    }
}

#[async_trait]
impl ChatModel for StreamModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        Err(LlmError::Config {
            message: "stream-only test model".into(),
        })
    }

    fn chat_stream(&self, _request: ChatRequest) -> EventStream {
        let events = self
            .events
            .lock()
            .expect("stream model events mutex poisoned")
            .take()
            .expect("stream model should only be consumed once");
        Box::pin(futures::stream::iter(events))
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

struct PendingModel {
    info: ModelInfo,
}

#[async_trait]
impl ChatModel for PendingModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        std::future::pending().await
    }

    fn chat_stream(&self, _request: ChatRequest) -> EventStream {
        Box::pin(futures::stream::pending())
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

fn limiter(pause_seconds: f64) -> Arc<LlmRateLimiter> {
    Arc::new(LlmRateLimiter::new(&RateLimitConfig {
        max_concurrent: 1,
        max_qpm: 0,
        pause_seconds,
        jitter_range: 0.0,
        acquire_timeout: 5.0,
    }))
}

fn request() -> ChatRequest {
    ChatRequest::new(Vec::new())
}

#[tokio::test]
async fn chat_stream_releases_slot_on_first_chunk() {
    let limiter = limiter(0.0);
    let model = RateLimitChatModel::new(
        Box::new(StreamModel::new(vec![
            Ok(AgentEvent::Start {
                run_id: "test".into(),
                model: None,
                metadata: serde_json::Value::Null,
            }),
            Ok(AgentEvent::TextDelta {
                delta: "hi".into(),
                metadata: serde_json::Value::Null,
            }),
        ])),
        limiter.clone(),
    );
    let mut stream = model.chat_stream(request());

    assert_eq!(limiter.in_flight(), 0);
    assert!(stream.next().await.unwrap().is_ok());
    assert_eq!(limiter.in_flight(), 0);
    assert!(stream.next().await.unwrap().is_ok());
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn non_stream_error_releases_slot_without_starting_cooldown() {
    let limiter = limiter(10.0);
    let model = RateLimitChatModel::new(Box::new(StreamModel::new(Vec::new())), limiter.clone());

    let result = model.chat(request()).await;

    assert!(matches!(result, Err(LlmError::Config { .. })));
    assert_eq!(limiter.in_flight(), 0);
    let stats = limiter.stats().await;
    assert_eq!(stats.total_acquired, 1);
    assert_eq!(stats.total_rate_limited, 0);
    assert!(!stats.is_paused);
}

#[tokio::test]
async fn cancelling_non_stream_chat_future_releases_concurrency_slot() {
    let limiter = limiter(0.0);
    let model = Arc::new(RateLimitChatModel::new(
        Box::new(PendingModel {
            info: ModelInfo {
                id: "pending-model".into(),
                name: "Pending Model".into(),
                context_window: None,
                max_output_tokens: None,
                supports_multimodal: None,
                supports_image: None,
                supports_video: None,
                probe_source: None,
                generate_kwargs: serde_json::Map::new(),
            },
        }),
        Arc::clone(&limiter),
    ));
    let task = tokio::spawn({
        let model = Arc::clone(&model);
        async move { model.chat(request()).await }
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while limiter.in_flight() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("pending chat should acquire one slot");

    task.abort();
    let _ = task.await;
    assert_eq!(limiter.in_flight(), 0);
}

#[tokio::test]
async fn streaming_429_with_retry_after_pauses_limiter() {
    let limiter = limiter(30.0);
    let model = RateLimitChatModel::new(
        Box::new(StreamModel::new(vec![
            Ok(AgentEvent::Start {
                run_id: "test".into(),
                model: None,
                metadata: serde_json::Value::Null,
            }),
            Err(LlmError::RateLimit {
                provider: "test".into(),
                retry_after: Some(Duration::from_secs(3)),
                message: "rate limited".into(),
            }),
        ])),
        limiter.clone(),
    );
    let mut stream = model.chat_stream(request());

    assert!(stream.next().await.unwrap().is_ok());
    assert!(matches!(
        stream.next().await,
        Some(Err(LlmError::RateLimit { .. }))
    ));

    tokio::time::sleep(Duration::from_millis(50)).await;

    let stats = limiter.stats().await;
    assert!(stats.is_paused);
    let remaining = stats.pause_remaining_s.unwrap();
    assert!(
        remaining > 2.0 && remaining <= 3.0,
        "expected ~3s pause, got {remaining}"
    );
}

#[tokio::test]
async fn first_chunk_429_with_retry_after_releases_and_pauses_limiter() {
    let limiter = limiter(30.0);
    let model = RateLimitChatModel::new(
        Box::new(StreamModel::new(vec![Err(LlmError::RateLimit {
            provider: "test".into(),
            retry_after: Some(Duration::from_secs(3)),
            message: "rate limited".into(),
        })])),
        limiter.clone(),
    );
    let mut stream = model.chat_stream(request());

    assert!(matches!(
        stream.next().await,
        Some(Err(LlmError::RateLimit { .. }))
    ));
    assert_eq!(limiter.in_flight(), 0);

    tokio::time::sleep(Duration::from_millis(50)).await;

    let stats = limiter.stats().await;
    assert!(stats.is_paused);
    let remaining = stats.pause_remaining_s.unwrap();
    assert!(
        remaining > 2.0 && remaining <= 3.0,
        "expected ~3s pause, got {remaining}"
    );
}

#[tokio::test]
async fn streaming_429_without_retry_after_uses_default_pause() {
    let limiter = limiter(10.0);
    let model = RateLimitChatModel::new(
        Box::new(StreamModel::new(vec![
            Ok(AgentEvent::Start {
                run_id: "test".into(),
                model: None,
                metadata: serde_json::Value::Null,
            }),
            Err(LlmError::RateLimit {
                provider: "test".into(),
                retry_after: None,
                message: "rate limited".into(),
            }),
        ])),
        limiter.clone(),
    );
    let mut stream = model.chat_stream(request());

    assert!(stream.next().await.unwrap().is_ok());
    assert!(matches!(
        stream.next().await,
        Some(Err(LlmError::RateLimit { .. }))
    ));

    tokio::time::sleep(Duration::from_millis(50)).await;

    let stats = limiter.stats().await;
    assert!(stats.is_paused);
    let remaining = stats.pause_remaining_s.unwrap();
    assert!(
        remaining > 9.0 && remaining <= 10.0,
        "expected ~10s default pause, got {remaining}"
    );
}

#[tokio::test(start_paused = true)]
async fn model_idle_timeout_starts_after_rate_limit_queueing() {
    let limiter = limiter(0.0);
    limiter.acquire().await.unwrap();
    let model = RateLimitChatModel::new(
        Box::new(StreamModel::new(vec![Ok(AgentEvent::End {
            finish_reason: agent_types::FinishReason::Stop,
            usage: None,
            metadata: serde_json::Value::Null,
        })])),
        limiter.clone(),
    );
    let started = tokio::time::Instant::now();
    let events = model
        .chat_stream_with_idle_timeout(request(), Duration::from_secs(1))
        .collect::<Vec<_>>();
    let release = async {
        tokio::time::sleep(Duration::from_secs(2)).await;
        limiter.release();
    };
    let (events, ()) = tokio::join!(events, release);
    assert_eq!(started.elapsed(), Duration::from_secs(2));
    assert_eq!(events.len(), 1);
    assert!(
        matches!(events[0], Ok(AgentEvent::End { .. })),
        "{events:?}"
    );
    assert_eq!(limiter.in_flight(), 0);
}
