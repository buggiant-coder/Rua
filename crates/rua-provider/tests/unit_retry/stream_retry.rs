use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_types::{AgentEvent, ChatRequest, ChatResponse, FinishReason};
use async_trait::async_trait;
use futures::StreamExt;
use llm_multiplex::{ChatModel, EventStream, LlmError, ModelInfo};
use rua_provider::{RetryChatModel, RetryPolicy};

struct MockStreamModel {
    call_count: Arc<AtomicU32>,
    fail_count: u32,
    error: LlmError,
    info: ModelInfo,
}

struct EventErrorModel {
    call_count: Arc<AtomicU32>,
    retryable: bool,
    content_before_error: bool,
    info: ModelInfo,
}

impl EventErrorModel {
    fn new(retryable: bool, content_before_error: bool) -> Self {
        Self {
            call_count: Arc::new(AtomicU32::new(0)),
            retryable,
            content_before_error,
            info: ModelInfo {
                id: "event-error-test".into(),
                name: "event-error-test".into(),
                context_window: None,
                max_output_tokens: None,
                supports_multimodal: None,
                supports_image: None,
                supports_video: None,
                probe_source: None,
                generate_kwargs: Default::default(),
            },
        }
    }
}

#[async_trait]
impl ChatModel for EventErrorModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        unreachable!("EventErrorModel is a stream-only test double")
    }

    fn chat_stream(&self, _request: ChatRequest) -> EventStream {
        let call = self.call_count.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            let mut events = Vec::new();
            if self.content_before_error {
                events.push(Ok(AgentEvent::TextDelta {
                    delta: "partial".into(),
                    metadata: serde_json::Value::Null,
                }));
            }
            events.push(Ok(AgentEvent::Error {
                code: Some("server_error".into()),
                message: "upstream unavailable".into(),
                retryable: self.retryable,
                metadata: serde_json::Value::Null,
            }));
            Box::pin(futures::stream::iter(events))
        } else {
            Box::pin(futures::stream::iter(vec![Ok(AgentEvent::End {
                finish_reason: FinishReason::Stop,
                usage: None,
                metadata: serde_json::Value::Null,
            })]))
        }
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

impl MockStreamModel {
    fn new(error: LlmError) -> Self {
        Self::with_fail_count(error, 1)
    }

    fn with_fail_count(error: LlmError, fail_count: u32) -> Self {
        Self {
            call_count: Arc::new(AtomicU32::new(0)),
            fail_count,
            error,
            info: ModelInfo {
                id: "test".into(),
                name: "test".into(),
                context_window: None,
                max_output_tokens: None,
                supports_multimodal: None,
                supports_image: None,
                supports_video: None,
                probe_source: None,
                generate_kwargs: Default::default(),
            },
        }
    }
}

#[async_trait]
impl ChatModel for MockStreamModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        unreachable!("MockStreamModel is a stream-only test double; chat() must never be invoked")
    }

    fn chat_stream(&self, _request: ChatRequest) -> EventStream {
        let n = self.call_count.fetch_add(1, Ordering::SeqCst);
        if n < self.fail_count {
            let err = match &self.error {
                LlmError::RateLimit {
                    provider,
                    retry_after,
                    message,
                } => LlmError::RateLimit {
                    provider: provider.clone(),
                    retry_after: *retry_after,
                    message: message.clone(),
                },
                LlmError::Server {
                    provider,
                    status,
                    message,
                } => LlmError::Server {
                    provider: provider.clone(),
                    status: *status,
                    message: message.clone(),
                },
                _ => LlmError::Unknown {
                    provider: "test".into(),
                    message: "unexpected".into(),
                },
            };
            Box::pin(futures::stream::once(async move { Err(err) }))
        } else {
            Box::pin(futures::stream::iter(vec![Ok(AgentEvent::End {
                finish_reason: FinishReason::Stop,
                usage: None,
                metadata: serde_json::Value::Null,
            })]))
        }
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

fn make_request() -> ChatRequest {
    ChatRequest {
        messages: vec![],
        model: Some("test".into()),
        temperature: None,
        max_tokens: None,
        stream: true,
        tools: vec![],
        tool_choice: None,
        structured_output: None,
        reasoning: None,
        metadata: Default::default(),
        provider_options: Default::default(),
        extra: Default::default(),
    }
}

#[tokio::test]
async fn stream_retry_429_with_retry_after() {
    let model = MockStreamModel::new(LlmError::RateLimit {
        provider: "test".into(),
        retry_after: Some(Duration::from_millis(100)),
        message: "rate limited".into(),
    });
    let policy = RetryPolicy {
        max_delay: Duration::from_secs(30),
        ..RetryPolicy::default()
    };
    let retry = RetryChatModel::new(Box::new(model), policy);
    let start = std::time::Instant::now();
    let mut stream = retry.chat_stream(make_request());

    let mut events = vec![];
    while let Some(item) = stream.next().await {
        events.push(item);
    }

    assert!(start.elapsed() >= Duration::from_millis(100));
    assert_eq!(events.len(), 1);
    assert!(events[0].is_ok());
}

#[tokio::test]
async fn stream_retry_429_without_retry_after() {
    let model = MockStreamModel::new(LlmError::RateLimit {
        provider: "test".into(),
        retry_after: None,
        message: "rate limited".into(),
    });
    let policy = RetryPolicy {
        initial_delay: Duration::from_millis(50),
        ..RetryPolicy::default()
    };
    let retry = RetryChatModel::new(Box::new(model), policy);
    let start = std::time::Instant::now();
    let mut stream = retry.chat_stream(make_request());

    let mut events = vec![];
    while let Some(item) = stream.next().await {
        events.push(item);
    }

    assert!(start.elapsed() >= Duration::from_millis(50));
    assert_eq!(events.len(), 1);
    assert!(events[0].is_ok());
}

#[tokio::test]
async fn stream_retry_503_with_backoff() {
    let model = MockStreamModel::new(LlmError::Server {
        provider: "test".into(),
        status: Some(503),
        message: "unavailable".into(),
    });
    let policy = RetryPolicy {
        initial_delay: Duration::from_millis(50),
        ..RetryPolicy::default()
    };
    let retry = RetryChatModel::new(Box::new(model), policy);
    let start = std::time::Instant::now();
    let mut stream = retry.chat_stream(make_request());

    let mut events = vec![];
    while let Some(item) = stream.next().await {
        events.push(item);
    }

    assert!(start.elapsed() >= Duration::from_millis(50));
    assert_eq!(events.len(), 1);
    assert!(events[0].is_ok());
}

#[tokio::test]
async fn stream_retry_multi_attempt_success() {
    let model = MockStreamModel::with_fail_count(
        LlmError::Server {
            provider: "test".into(),
            status: Some(503),
            message: "unavailable".into(),
        },
        3,
    );
    let call_count = model.call_count.clone();
    let policy = RetryPolicy {
        max_retries: 3,
        initial_delay: Duration::from_millis(10),
        ..RetryPolicy::default()
    };
    let retry = RetryChatModel::new(Box::new(model), policy);
    let mut stream = retry.chat_stream(make_request());

    let mut events = vec![];
    while let Some(item) = stream.next().await {
        events.push(item);
    }

    assert_eq!(call_count.load(Ordering::SeqCst), 4);
    assert_eq!(events.len(), 1);
    assert!(events[0].is_ok());
}

#[tokio::test]
async fn stream_retry_exhausted() {
    let model = MockStreamModel::with_fail_count(
        LlmError::Server {
            provider: "test".into(),
            status: Some(503),
            message: "unavailable".into(),
        },
        100,
    );
    let call_count = model.call_count.clone();
    let policy = RetryPolicy {
        max_retries: 3,
        initial_delay: Duration::from_millis(10),
        ..RetryPolicy::default()
    };
    let retry = RetryChatModel::new(Box::new(model), policy);
    let mut stream = retry.chat_stream(make_request());

    let mut events = vec![];
    while let Some(item) = stream.next().await {
        events.push(item);
    }

    assert_eq!(call_count.load(Ordering::SeqCst), 4);
    assert_eq!(events.len(), 1);
    assert!(events[0].is_err());
}

#[tokio::test]
async fn stream_retry_multi_with_retry_after() {
    let model = MockStreamModel::with_fail_count(
        LlmError::RateLimit {
            provider: "test".into(),
            retry_after: Some(Duration::from_millis(50)),
            message: "rate limited".into(),
        },
        2,
    );
    let call_count = model.call_count.clone();
    let policy = RetryPolicy {
        max_retries: 3,
        initial_delay: Duration::from_millis(10),
        ..RetryPolicy::default()
    };
    let retry = RetryChatModel::new(Box::new(model), policy);
    let start = std::time::Instant::now();
    let mut stream = retry.chat_stream(make_request());

    let mut events = vec![];
    while let Some(item) = stream.next().await {
        events.push(item);
    }

    assert!(start.elapsed() >= Duration::from_millis(100));
    assert_eq!(call_count.load(Ordering::SeqCst), 3);
    assert_eq!(events.len(), 1);
    assert!(events[0].is_ok());
}

#[tokio::test]
async fn stream_retry_529_overload_triggers_retry() {
    let error = LlmError::from_http_status(
        "anthropic",
        529,
        r#"{"error":{"message":"overloaded"}}"#,
        Some("1"),
    );
    assert!(matches!(error, LlmError::RateLimit { .. }));

    let model = MockStreamModel::with_fail_count(
        LlmError::RateLimit {
            provider: "anthropic".into(),
            retry_after: Some(Duration::from_millis(50)),
            message: "overloaded".into(),
        },
        2,
    );
    let call_count = model.call_count.clone();
    let policy = RetryPolicy {
        max_retries: 3,
        initial_delay: Duration::from_millis(10),
        ..RetryPolicy::default()
    };
    let retry = RetryChatModel::new(Box::new(model), policy);
    let mut stream = retry.chat_stream(make_request());

    let mut events = vec![];
    while let Some(item) = stream.next().await {
        events.push(item);
    }

    assert!(
        call_count.load(Ordering::SeqCst) > 1,
        "retry must trigger for 529"
    );
    assert_eq!(call_count.load(Ordering::SeqCst), 3);
    assert_eq!(events.len(), 1);
    assert!(events[0].is_ok());
}

#[tokio::test]
async fn retryable_terminal_error_event_retries_before_content() {
    let model = EventErrorModel::new(true, false);
    let call_count = model.call_count.clone();
    let retry = RetryChatModel::new(
        Box::new(model),
        RetryPolicy {
            initial_delay: Duration::from_millis(1),
            ..RetryPolicy::default()
        },
    );

    let events = retry.chat_stream(make_request()).collect::<Vec<_>>().await;

    assert_eq!(call_count.load(Ordering::SeqCst), 2);
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0], Ok(AgentEvent::End { .. })));
}

#[tokio::test]
async fn non_retryable_terminal_error_event_is_not_retried() {
    let model = EventErrorModel::new(false, false);
    let call_count = model.call_count.clone();
    let retry = RetryChatModel::new(Box::new(model), RetryPolicy::default());

    let events = retry.chat_stream(make_request()).collect::<Vec<_>>().await;

    assert_eq!(call_count.load(Ordering::SeqCst), 1);
    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0],
        Ok(AgentEvent::Error {
            retryable: false,
            ..
        })
    ));
}

#[tokio::test]
async fn retryable_terminal_error_event_is_returned_when_retry_budget_is_exhausted() {
    let model = EventErrorModel::new(true, false);
    let call_count = model.call_count.clone();
    let retry = RetryChatModel::new(
        Box::new(model),
        RetryPolicy {
            max_retries: 0,
            ..RetryPolicy::default()
        },
    );

    let events = retry.chat_stream(make_request()).collect::<Vec<_>>().await;

    assert_eq!(call_count.load(Ordering::SeqCst), 1);
    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0],
        Ok(AgentEvent::Error {
            retryable: true,
            ..
        })
    ));
}

#[tokio::test]
async fn retryable_terminal_error_event_is_not_retried_after_content() {
    let model = EventErrorModel::new(true, true);
    let call_count = model.call_count.clone();
    let retry = RetryChatModel::new(Box::new(model), RetryPolicy::default());

    let events = retry.chat_stream(make_request()).collect::<Vec<_>>().await;

    assert_eq!(call_count.load(Ordering::SeqCst), 1);
    assert_eq!(events.len(), 2);
    assert!(matches!(events[0], Ok(AgentEvent::TextDelta { .. })));
    assert!(matches!(
        events[1],
        Ok(AgentEvent::Error {
            retryable: true,
            ..
        })
    ));
}

#[tokio::test(start_paused = true)]
async fn retry_deadline_bounds_backoff_and_preserves_last_http_error() {
    let model = MockStreamModel::new(LlmError::Server {
        provider: "test".into(),
        status: Some(502),
        message: "upstream unavailable".into(),
    });
    let calls = model.call_count.clone();
    let retry = RetryChatModel::new(
        Box::new(model),
        RetryPolicy {
            max_retries: 3,
            initial_delay: Duration::from_secs(10),
            max_total_time: Some(Duration::from_secs(1)),
            ..Default::default()
        },
    );
    let started = tokio::time::Instant::now();
    let events = retry
        .chat_stream_with_idle_timeout(make_request(), Duration::from_millis(100))
        .collect::<Vec<_>>()
        .await;
    assert_eq!(started.elapsed(), Duration::from_secs(1));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(events.len(), 1);
    assert!(
        matches!(&events[0], Err(LlmError::Server { status: Some(502), message, .. })
        if message == "upstream unavailable")
    );
}

#[tokio::test(start_paused = true)]
async fn retry_deadline_preserves_terminal_error_during_backoff() {
    let model = EventErrorModel::new(true, false);
    let calls = model.call_count.clone();
    let retry = RetryChatModel::new(
        Box::new(model),
        RetryPolicy {
            max_retries: 3,
            initial_delay: Duration::from_secs(10),
            max_total_time: Some(Duration::from_secs(1)),
            ..Default::default()
        },
    );
    let started = tokio::time::Instant::now();
    let events = retry
        .chat_stream_with_idle_timeout(make_request(), Duration::from_millis(100))
        .collect::<Vec<_>>()
        .await;
    assert_eq!(started.elapsed(), Duration::from_secs(1));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(events.len(), 1);
    assert!(
        matches!(&events[0], Ok(AgentEvent::Error { code, message, .. })
        if code.as_deref() == Some("server_error") && message == "upstream unavailable")
    );
}
