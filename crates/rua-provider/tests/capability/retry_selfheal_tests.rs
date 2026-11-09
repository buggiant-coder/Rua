use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;

use agent_types::{AgentEvent, ChatRequest, ChatResponse, ContentPart, ModelDialogueItem, Role};
use llm_multiplex::{ChatModel, EventStream, LlmError, ModelInfo};
use rua_provider::capability_cache;
use rua_provider::{RetryChatModel, RetryPolicy};

fn make_model_info(model_id: &str) -> ModelInfo {
    ModelInfo {
        id: model_id.into(),
        name: model_id.into(),
        context_window: None,
        max_output_tokens: None,
        supports_multimodal: None,
        supports_image: None,
        supports_video: None,
        probe_source: None,
        generate_kwargs: Default::default(),
    }
}

fn make_request_with_assistant_history(model_id: &str) -> ChatRequest {
    ChatRequest {
        messages: vec![
            ModelDialogueItem::text(Role::User, "hello"),
            ModelDialogueItem::text(Role::Assistant, "hi there"),
            ModelDialogueItem::text(Role::User, "how are you?"),
        ],
        model: Some(model_id.into()),
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

struct ReasoningContentErrorModel {
    call_count: Arc<AtomicU32>,
    fail_first: bool,
    info: ModelInfo,
}

impl ReasoningContentErrorModel {
    fn new(fail_first: bool, model_id: &str) -> Self {
        Self {
            call_count: Arc::new(AtomicU32::new(0)),
            fail_first,
            info: make_model_info(model_id),
        }
    }
}

#[async_trait]
impl ChatModel for ReasoningContentErrorModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let n = self.call_count.fetch_add(1, Ordering::SeqCst);
        if self.fail_first && n == 0 {
            return Err(LlmError::Unknown {
                provider: "deepseek".into(),
                message: "HTTP 400: Each assistant message must include reasoning_content when using thinking mode".into(),
            });
        }
        Ok(ChatResponse {
            message: ModelDialogueItem::text(Role::Assistant, "ok"),
            usage: None,
            model: Some(self.info.id.clone()),
            finish_reason: agent_types::FinishReason::Stop,
            metadata: Default::default(),
            extra: Default::default(),
        })
    }

    fn chat_stream(&self, _request: ChatRequest) -> EventStream {
        Box::pin(futures::stream::once(async {
            Ok(AgentEvent::TextDelta {
                delta: "ok".into(),
                metadata: serde_json::Value::Null,
            })
        }))
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

#[tokio::test]
async fn selfheal_reasoning_content_400_injects_and_retries() {
    let model_id = "selfheal-inject-test";
    let model = ReasoningContentErrorModel::new(true, model_id);
    let call_count = model.call_count.clone();
    let policy = RetryPolicy {
        max_retries: 3,
        initial_delay: Duration::from_millis(10),
        ..RetryPolicy::default()
    };
    let retry = RetryChatModel::new(Box::new(model), policy);
    let request = make_request_with_assistant_history(model_id);

    let result = retry.chat(request).await;
    assert!(result.is_ok(), "should succeed after self-heal retry");
    assert_eq!(
        call_count.load(Ordering::SeqCst),
        2,
        "first call fails, second succeeds"
    );

    let key = capability_cache::cache_key("deepseek", model_id);
    assert!(
        capability_cache::needs_reasoning_content(&key),
        "cache should learn needs_reasoning_content"
    );
}

#[tokio::test]
async fn selfheal_uses_cache_on_subsequent_calls() {
    let model_id = "selfheal-cache-test";
    let key = capability_cache::cache_key("deepseek", model_id);
    capability_cache::learn_needs_reasoning_content(&key);

    let model = ReasoningContentErrorModel::new(false, model_id);
    let call_count = model.call_count.clone();
    let policy = RetryPolicy::default();
    let retry = RetryChatModel::new(Box::new(model), policy);

    let request = make_request_with_assistant_history(model_id);
    let result = retry.chat(request).await;
    assert!(result.is_ok());
    assert_eq!(call_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cached_selfheal_preserves_existing_thinking_content() {
    let model_id = "selfheal-existing-thinking-test";
    let key = capability_cache::cache_key("deepseek", model_id);
    capability_cache::learn_needs_reasoning_content(&key);

    struct CaptureRequestModel {
        captured: Arc<Mutex<Option<ChatRequest>>>,
        info: ModelInfo,
    }

    #[async_trait]
    impl ChatModel for CaptureRequestModel {
        async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
            *self.captured.lock().expect("captured mutex") = Some(request);
            Ok(ChatResponse {
                message: ModelDialogueItem::text(Role::Assistant, "ok"),
                usage: None,
                model: Some(self.info.id.clone()),
                finish_reason: agent_types::FinishReason::Stop,
                metadata: Default::default(),
                extra: Default::default(),
            })
        }

        fn chat_stream(&self, _request: ChatRequest) -> EventStream {
            unreachable!("CaptureRequestModel is a non-stream test double")
        }

        fn model_info(&self) -> &ModelInfo {
            &self.info
        }
    }

    let captured = Arc::new(Mutex::new(None));
    let model = CaptureRequestModel {
        captured: captured.clone(),
        info: make_model_info(model_id),
    };
    let retry = RetryChatModel::new(Box::new(model), RetryPolicy::default());
    let mut assistant = ModelDialogueItem::text(Role::Assistant, "hi there");
    assistant.content.push(ContentPart::Thinking {
        text: "already present".into(),
        signature: None,
    });
    let request = ChatRequest {
        messages: vec![ModelDialogueItem::text(Role::User, "hello"), assistant],
        model: Some(model_id.into()),
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
    };

    retry.chat(request).await.expect("cached self-heal call");

    let captured = captured
        .lock()
        .expect("captured mutex")
        .clone()
        .expect("request should be captured");
    let thinking: Vec<_> = captured.messages[1]
        .content
        .iter()
        .filter_map(|part| match part {
            ContentPart::Thinking { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(thinking, vec!["already present"]);
}

#[tokio::test]
async fn non_reasoning_400_not_intercepted() {
    let model_id = "non-reasoning-test";

    struct OtherErrorModel {
        info: ModelInfo,
    }

    #[async_trait]
    impl ChatModel for OtherErrorModel {
        async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
            Err(LlmError::Unknown {
                provider: "openai".into(),
                message: "HTTP 400: Invalid request: max_tokens exceeded".into(),
            })
        }

        fn chat_stream(&self, _request: ChatRequest) -> EventStream {
            Box::pin(futures::stream::once(async {
                Err(LlmError::Unknown {
                    provider: "openai".into(),
                    message: "error".into(),
                })
            }))
        }

        fn model_info(&self) -> &ModelInfo {
            &self.info
        }
    }

    let model = OtherErrorModel {
        info: make_model_info(model_id),
    };
    let policy = RetryPolicy {
        max_retries: 0,
        ..RetryPolicy::default()
    };
    let retry = RetryChatModel::new(Box::new(model), policy);
    let request = make_request_with_assistant_history(model_id);

    let result = retry.chat(request).await;
    assert!(
        result.is_err(),
        "non-reasoning 400 should not be self-healed"
    );
}

#[tokio::test]
async fn selfheal_learns_then_cache_populated() {
    let model_id = "selfheal-learn-test";

    let model1 = ReasoningContentErrorModel::new(true, model_id);
    let policy = RetryPolicy {
        max_retries: 3,
        initial_delay: Duration::from_millis(10),
        ..RetryPolicy::default()
    };
    let retry1 = RetryChatModel::new(Box::new(model1), policy.clone());
    let result = retry1
        .chat(make_request_with_assistant_history(model_id))
        .await;
    assert!(result.is_ok());

    let key = capability_cache::cache_key("deepseek", model_id);
    assert!(capability_cache::needs_reasoning_content(&key));
}

#[tokio::test]
async fn non_400_reasoning_content_not_intercepted() {
    let model_id = "non-400-reasoning-test";

    struct NonHttp400ReasoningModel {
        call_count: Arc<AtomicU32>,
        info: ModelInfo,
    }

    #[async_trait]
    impl ChatModel for NonHttp400ReasoningModel {
        async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);

            Err(LlmError::Unknown {
                provider: "deepseek".into(),
                message: "stream error: reasoning_content channel closed".into(),
            })
        }

        fn chat_stream(&self, _request: ChatRequest) -> EventStream {
            Box::pin(futures::stream::once(async {
                Err(LlmError::Unknown {
                    provider: "deepseek".into(),
                    message: "reasoning_content".into(),
                })
            }))
        }

        fn model_info(&self) -> &ModelInfo {
            &self.info
        }
    }

    let call_count = Arc::new(AtomicU32::new(0));
    let model = NonHttp400ReasoningModel {
        call_count: call_count.clone(),
        info: make_model_info(model_id),
    };
    let policy = RetryPolicy {
        max_retries: 0,
        ..RetryPolicy::default()
    };
    let retry = RetryChatModel::new(Box::new(model), policy);

    let result = retry
        .chat(make_request_with_assistant_history(model_id))
        .await;
    assert!(
        result.is_err(),
        "non-400 reasoning_content error must surface"
    );
    assert_eq!(
        call_count.load(Ordering::SeqCst),
        1,
        "must not inject + retry when the error is not an HTTP 400"
    );

    let key = capability_cache::cache_key("deepseek", model_id);
    assert!(
        !capability_cache::needs_reasoning_content(&key),
        "must not learn reasoning_content for a non-400 error"
    );
}

struct AlwaysRateLimitedModel {
    call_count: Arc<AtomicU32>,
    retry_after: Duration,
    info: ModelInfo,
}

#[async_trait]
impl ChatModel for AlwaysRateLimitedModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        Err(LlmError::RateLimit {
            provider: "anthropic".into(),
            retry_after: Some(self.retry_after),
            message: "rate limited".into(),
        })
    }

    fn chat_stream(&self, _request: ChatRequest) -> EventStream {
        let n = self.call_count.fetch_add(1, Ordering::SeqCst);
        let retry_after = self.retry_after;
        let _ = n;
        Box::pin(futures::stream::once(async move {
            Err(LlmError::RateLimit {
                provider: "anthropic".into(),
                retry_after: Some(retry_after),
                message: "rate limited".into(),
            })
        }))
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

#[tokio::test]
async fn non_stream_does_not_retry_when_retry_after_exceeds_cap() {
    let call_count = Arc::new(AtomicU32::new(0));
    let model = AlwaysRateLimitedModel {
        call_count: call_count.clone(),

        retry_after: Duration::from_secs(51496),
        info: make_model_info("cap-nonstream"),
    };
    let policy = RetryPolicy {
        max_retries: 3,
        initial_delay: Duration::from_millis(10),
        ..RetryPolicy::default()
    };
    let retry = RetryChatModel::new(Box::new(model), policy);

    let result = retry
        .chat(make_request_with_assistant_history("cap-nonstream"))
        .await;
    assert!(
        result.is_err(),
        "oversized retry_after should surface an error"
    );
    assert_eq!(
        call_count.load(Ordering::SeqCst),
        1,
        "Retry-After beyond the cap must not be retried"
    );
}

#[tokio::test]
async fn stream_does_not_retry_when_retry_after_exceeds_cap() {
    let call_count = Arc::new(AtomicU32::new(0));
    let model = AlwaysRateLimitedModel {
        call_count: call_count.clone(),
        retry_after: Duration::from_secs(51496),
        info: make_model_info("cap-stream"),
    };
    let policy = RetryPolicy {
        max_retries: 3,
        initial_delay: Duration::from_millis(10),
        ..RetryPolicy::default()
    };
    let retry = RetryChatModel::new(Box::new(model), policy);

    let mut stream = retry.chat_stream(make_request_with_assistant_history("cap-stream"));
    let mut errs = 0;
    while let Some(item) = stream.next().await {
        if item.is_err() {
            errs += 1;
        }
    }
    assert_eq!(errs, 1, "stream surfaces the rate-limit error");
    assert_eq!(
        call_count.load(Ordering::SeqCst),
        1,
        "stream Retry-After beyond the cap must not be retried"
    );
}

struct ContentThenErrorModel {
    call_count: Arc<AtomicU32>,
    fail_count: u32,
    info: ModelInfo,
}

#[async_trait]
impl ChatModel for ContentThenErrorModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        unreachable!("stream-only test double")
    }

    fn chat_stream(&self, _request: ChatRequest) -> EventStream {
        let n = self.call_count.fetch_add(1, Ordering::SeqCst);
        if n < self.fail_count {
            Box::pin(futures::stream::iter(vec![
                Ok(AgentEvent::TextDelta {
                    delta: "partial".into(),
                    metadata: serde_json::Value::Null,
                }),
                Err(LlmError::Server {
                    provider: "test".into(),
                    status: Some(503),
                    message: "mid-stream failure".into(),
                }),
            ]))
        } else {
            Box::pin(futures::stream::iter(vec![Ok(AgentEvent::TextDelta {
                delta: "ok".into(),
                metadata: serde_json::Value::Null,
            })]))
        }
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

#[tokio::test]
async fn stream_does_not_retry_after_content_emitted() {
    let call_count = Arc::new(AtomicU32::new(0));
    let model = ContentThenErrorModel {
        call_count: call_count.clone(),
        fail_count: 1,
        info: make_model_info("a2-content"),
    };
    let policy = RetryPolicy {
        max_retries: 3,
        initial_delay: Duration::from_millis(10),
        ..RetryPolicy::default()
    };
    let retry = RetryChatModel::new(Box::new(model), policy);

    let mut stream = retry.chat_stream(make_request_with_assistant_history("a2-content"));
    let mut oks = 0;
    let mut errs = 0;
    while let Some(item) = stream.next().await {
        match item {
            Ok(_) => oks += 1,
            Err(_) => errs += 1,
        }
    }

    assert_eq!(
        call_count.load(Ordering::SeqCst),
        1,
        "retrying after visible content would merge two attempts"
    );
    assert_eq!(errs, 1, "the mid-stream failure must be surfaced");
    assert_eq!(oks, 1, "the first attempt's content remains visible");
}

struct EofThenSuccessModel {
    call_count: Arc<AtomicU32>,
    info: ModelInfo,
}

#[async_trait]
impl ChatModel for EofThenSuccessModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        unreachable!("stream-only test double")
    }

    fn chat_stream(&self, _request: ChatRequest) -> EventStream {
        let n = self.call_count.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            Box::pin(futures::stream::empty())
        } else {
            Box::pin(futures::stream::iter(vec![Ok(AgentEvent::End {
                finish_reason: agent_types::FinishReason::Stop,
                usage: None,
                metadata: serde_json::Value::Null,
            })]))
        }
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

#[tokio::test]
async fn stream_retries_eof_before_any_content_or_terminal_event() {
    let call_count = Arc::new(AtomicU32::new(0));
    let retry = RetryChatModel::new(
        Box::new(EofThenSuccessModel {
            call_count: Arc::clone(&call_count),
            info: make_model_info("eof-retry"),
        }),
        RetryPolicy {
            max_retries: 1,
            initial_delay: Duration::from_millis(1),
            ..RetryPolicy::default()
        },
    );

    let events = retry
        .chat_stream(make_request_with_assistant_history("eof-retry"))
        .collect::<Vec<_>>()
        .await;

    assert_eq!(call_count.load(Ordering::SeqCst), 2);
    assert!(events
        .iter()
        .any(|event| matches!(event, Ok(AgentEvent::End { .. }))));
    assert!(events.iter().all(Result::is_ok));
}

struct ContentThenEofModel {
    call_count: Arc<AtomicU32>,
    info: ModelInfo,
}

#[async_trait]
impl ChatModel for ContentThenEofModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        unreachable!("stream-only test double")
    }

    fn chat_stream(&self, _request: ChatRequest) -> EventStream {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        Box::pin(futures::stream::iter(vec![Ok(AgentEvent::TextDelta {
            delta: "partial".into(),
            metadata: serde_json::Value::Null,
        })]))
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

#[tokio::test]
async fn stream_does_not_retry_eof_after_content() {
    let call_count = Arc::new(AtomicU32::new(0));
    let retry = RetryChatModel::new(
        Box::new(ContentThenEofModel {
            call_count: Arc::clone(&call_count),
            info: make_model_info("content-eof"),
        }),
        RetryPolicy {
            max_retries: 3,
            initial_delay: Duration::from_millis(1),
            ..RetryPolicy::default()
        },
    );

    let events = retry
        .chat_stream(make_request_with_assistant_history("content-eof"))
        .collect::<Vec<_>>()
        .await;

    assert_eq!(call_count.load(Ordering::SeqCst), 1);
    assert_eq!(events.iter().filter(|event| event.is_err()).count(), 1);
}
