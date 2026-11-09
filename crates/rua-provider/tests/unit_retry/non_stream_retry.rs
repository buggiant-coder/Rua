use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_types::{ChatRequest, ChatResponse, FinishReason, ModelDialogueItem, Role};
use async_trait::async_trait;
use llm_multiplex::{ChatModel, EventStream, LlmError, ModelInfo};
use rua_provider::{RetryChatModel, RetryPolicy};

struct MockChatModel {
    call_count: Arc<AtomicU32>,
    fail_count: u32,
    error: LlmError,
    info: ModelInfo,
}

impl MockChatModel {
    fn new(error: LlmError, fail_count: u32) -> Self {
        Self {
            call_count: Arc::new(AtomicU32::new(0)),
            fail_count,
            error,
            info: ModelInfo {
                id: "unit-retry".into(),
                name: "unit-retry".into(),
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
impl ChatModel for MockChatModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let n = self.call_count.fetch_add(1, Ordering::SeqCst);
        if n < self.fail_count {
            return Err(clone_error(&self.error));
        }
        Ok(ChatResponse {
            message: ModelDialogueItem::text(Role::Assistant, "ok"),
            usage: None,
            model: Some(self.info.id.clone()),
            finish_reason: FinishReason::Stop,
            metadata: Default::default(),
            extra: Default::default(),
        })
    }

    fn chat_stream(&self, _request: ChatRequest) -> EventStream {
        unreachable!("MockChatModel is a non-stream test double")
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

fn clone_error(error: &LlmError) -> LlmError {
    match error {
        LlmError::Auth {
            provider,
            status,
            message,
        } => LlmError::Auth {
            provider: provider.clone(),
            status: *status,
            message: message.clone(),
        },
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
        LlmError::Timeout { provider, timeout } => LlmError::Timeout {
            provider: provider.clone(),
            timeout: *timeout,
        },
        LlmError::RetryDeadlineExceeded { provider, timeout } => LlmError::RetryDeadlineExceeded {
            provider: provider.clone(),
            timeout: *timeout,
        },
        LlmError::Connection { provider, message } => LlmError::Connection {
            provider: provider.clone(),
            message: message.clone(),
        },
        LlmError::InvalidResponse { provider, message } => LlmError::InvalidResponse {
            provider: provider.clone(),
            message: message.clone(),
        },
        LlmError::ModelNotFound { provider, model } => LlmError::ModelNotFound {
            provider: provider.clone(),
            model: model.clone(),
        },
        LlmError::Cancelled => LlmError::Cancelled,
        LlmError::Config { message } => LlmError::Config {
            message: message.clone(),
        },
        LlmError::Unknown { provider, message } => LlmError::Unknown {
            provider: provider.clone(),
            message: message.clone(),
        },
    }
}

fn request() -> ChatRequest {
    ChatRequest {
        messages: vec![ModelDialogueItem::text(Role::User, "hello")],
        model: Some("unit-retry".into()),
        temperature: None,
        max_tokens: None,
        stream: false,
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
async fn non_stream_retries_5xx_then_succeeds() {
    let model = MockChatModel::new(
        LlmError::Server {
            provider: "test".into(),
            status: Some(503),
            message: "temporarily unavailable".into(),
        },
        1,
    );
    let call_count = model.call_count.clone();
    let retry = RetryChatModel::new(
        Box::new(model),
        RetryPolicy {
            max_retries: 3,
            initial_delay: Duration::from_millis(10),
            ..RetryPolicy::default()
        },
    );

    let response = retry.chat(request()).await.expect("retry should recover");

    assert_eq!(response.text_content(), "ok");
    assert_eq!(call_count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn non_stream_exhausts_retry_budget() {
    let model = MockChatModel::new(
        LlmError::Server {
            provider: "test".into(),
            status: Some(503),
            message: "still unavailable".into(),
        },
        100,
    );
    let call_count = model.call_count.clone();
    let retry = RetryChatModel::new(
        Box::new(model),
        RetryPolicy {
            max_retries: 2,
            initial_delay: Duration::from_millis(10),
            ..RetryPolicy::default()
        },
    );

    let result = retry.chat(request()).await;

    assert!(matches!(result, Err(LlmError::Server { .. })));
    assert_eq!(
        call_count.load(Ordering::SeqCst),
        3,
        "max_retries=2 means initial call plus two retries"
    );
}

#[tokio::test]
async fn non_stream_retries_rate_limit_with_bounded_retry_after_then_succeeds() {
    let model = MockChatModel::new(
        LlmError::RateLimit {
            provider: "test".into(),
            retry_after: Some(Duration::from_millis(1)),
            message: "slow down".into(),
        },
        1,
    );
    let call_count = model.call_count.clone();
    let retry = RetryChatModel::new(
        Box::new(model),
        RetryPolicy {
            max_retries: 3,
            initial_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(20),
            ..RetryPolicy::default()
        },
    );

    let response = retry.chat(request()).await.expect("retry should recover");

    assert_eq!(response.text_content(), "ok");
    assert_eq!(call_count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn non_stream_does_not_retry_oversized_retry_after() {
    let model = MockChatModel::new(
        LlmError::RateLimit {
            provider: "test".into(),
            retry_after: Some(Duration::from_secs(61)),
            message: "retry much later".into(),
        },
        100,
    );
    let call_count = model.call_count.clone();
    let retry = RetryChatModel::new(
        Box::new(model),
        RetryPolicy {
            max_retries: 3,
            initial_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(20),
            ..RetryPolicy::default()
        },
    );

    let result = retry.chat(request()).await;

    assert!(matches!(result, Err(LlmError::RateLimit { .. })));
    assert_eq!(
        call_count.load(Ordering::SeqCst),
        1,
        "oversized Retry-After should surface immediately instead of sleeping/retrying"
    );
}

#[tokio::test]
async fn non_retryable_auth_error_is_not_retried() {
    let model = MockChatModel::new(
        LlmError::Auth {
            provider: "test".into(),
            status: Some(401),
            message: "bad key".into(),
        },
        100,
    );
    let call_count = model.call_count.clone();
    let retry = RetryChatModel::new(
        Box::new(model),
        RetryPolicy {
            max_retries: 3,
            initial_delay: Duration::from_millis(10),
            ..RetryPolicy::default()
        },
    );

    let result = retry.chat(request()).await;

    assert!(matches!(result, Err(LlmError::Auth { .. })));
    assert_eq!(call_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn non_retryable_config_error_is_not_retried() {
    let model = MockChatModel::new(
        LlmError::Config {
            message: "missing provider config".into(),
        },
        100,
    );
    let call_count = model.call_count.clone();
    let retry = RetryChatModel::new(
        Box::new(model),
        RetryPolicy {
            max_retries: 3,
            initial_delay: Duration::from_millis(10),
            ..RetryPolicy::default()
        },
    );

    let result = retry.chat(request()).await;

    assert!(matches!(result, Err(LlmError::Config { .. })));
    assert_eq!(
        call_count.load(Ordering::SeqCst),
        1,
        "local configuration errors should surface immediately"
    );
}

#[tokio::test]
async fn non_retryable_unknown_400_error_is_not_retried() {
    let model = MockChatModel::new(
        LlmError::Unknown {
            provider: "test".into(),
            message: "HTTP 400: bad request".into(),
        },
        100,
    );
    let call_count = model.call_count.clone();
    let retry = RetryChatModel::new(
        Box::new(model),
        RetryPolicy {
            max_retries: 3,
            initial_delay: Duration::from_millis(10),
            ..RetryPolicy::default()
        },
    );

    let result = retry.chat(request()).await;

    assert!(matches!(result, Err(LlmError::Unknown { .. })));
    assert_eq!(
        call_count.load(Ordering::SeqCst),
        1,
        "ordinary HTTP 400 provider errors should not be retried as transient outages"
    );
}

#[test]
fn retry_policy_caps_backoff_and_retry_after_before_jitter() {
    let policy = RetryPolicy {
        max_retries: 3,
        initial_delay: Duration::from_millis(100),
        max_delay: Duration::from_millis(250),
        backoff_factor: 3.0,
        max_total_time: None,
    };

    let capped_backoff = policy.delay_for_attempt(4, None);
    assert!(capped_backoff >= Duration::from_millis(250));
    assert!(capped_backoff <= Duration::from_millis(312));

    let capped_retry_after = policy.delay_for_attempt(0, Some(Duration::from_secs(60)));
    assert!(capped_retry_after >= Duration::from_millis(250));
    assert!(capped_retry_after <= Duration::from_millis(312));
}
