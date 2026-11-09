use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;

use agent_types::{
    AgentEvent, ChatRequest, ChatResponse, ContentPart, FinishReason, ModelDialogueItem,
};
use llm_multiplex::{ChatModel, EventStream, LlmError, ModelInfo};

use rua_provider::fallback::{FallbackChatModel, FallbackMode, FallbackPolicy, FallbackResolver};

fn make_model_info(id: &str) -> ModelInfo {
    ModelInfo {
        id: id.into(),
        name: id.into(),
        context_window: None,
        max_output_tokens: None,
        supports_multimodal: None,
        supports_image: None,
        supports_video: None,
        probe_source: None,
        generate_kwargs: Default::default(),
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

fn make_response(model_id: &str) -> ChatResponse {
    ChatResponse {
        message: ModelDialogueItem {
            id: None,
            role: agent_types::Role::Assistant,
            content: vec![ContentPart::Text { text: "ok".into() }],
            name: None,
            created_at: None,
            metadata: Default::default(),
        },
        usage: None,
        model: Some(model_id.into()),
        finish_reason: FinishReason::Stop,
        metadata: Default::default(),
        extra: Default::default(),
    }
}

fn server_error() -> LlmError {
    LlmError::Server {
        provider: "test".into(),
        status: Some(500),
        message: "internal error".into(),
    }
}

fn auth_error() -> LlmError {
    LlmError::Auth {
        provider: "test".into(),
        status: Some(401),
        message: "invalid key".into(),
    }
}

fn make_policy(providers: Vec<&str>) -> FallbackPolicy {
    FallbackPolicy {
        mode: FallbackMode::AlternateProvider,
        max_attempts: providers.len() as u32,
        fallback_providers: providers.into_iter().map(String::from).collect(),
        fallback_on_auth: false,
        fallback_on_content_filter: false,
    }
}

struct FailingModel {
    make_error: Box<dyn Fn() -> LlmError + Send + Sync>,
    info: ModelInfo,
}

impl FailingModel {
    fn new(make_error: impl Fn() -> LlmError + Send + Sync + 'static) -> Self {
        Self {
            make_error: Box::new(make_error),
            info: make_model_info("primary"),
        }
    }
}

#[async_trait]
impl ChatModel for FailingModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        Err((self.make_error)())
    }

    fn chat_stream(&self, _request: ChatRequest) -> EventStream {
        let err = (self.make_error)();
        Box::pin(futures::stream::once(async move { Err(err) }))
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

struct SuccessModel {
    info: ModelInfo,
    call_count: Arc<AtomicU32>,
}

impl SuccessModel {
    fn new(id: &str) -> Self {
        Self {
            info: make_model_info(id),
            call_count: Arc::new(AtomicU32::new(0)),
        }
    }
}

#[async_trait]
impl ChatModel for SuccessModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        Ok(make_response(&self.info.id))
    }

    fn chat_stream(&self, _request: ChatRequest) -> EventStream {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        let model = self.info.id.clone();
        Box::pin(futures::stream::iter(vec![Ok(AgentEvent::TextDelta {
            delta: format!("ok from {model}"),
            metadata: serde_json::Value::Null,
        })]))
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

struct PartialThenFailModel {
    make_error: Box<dyn Fn() -> LlmError + Send + Sync>,
    info: ModelInfo,
}

impl PartialThenFailModel {
    fn new(make_error: impl Fn() -> LlmError + Send + Sync + 'static) -> Self {
        Self {
            make_error: Box::new(make_error),
            info: make_model_info("partial"),
        }
    }
}

#[async_trait]
impl ChatModel for PartialThenFailModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        unreachable!()
    }

    fn chat_stream(&self, _request: ChatRequest) -> EventStream {
        let err = (self.make_error)();
        Box::pin(futures::stream::iter(vec![
            Ok(AgentEvent::TextDelta {
                delta: "partial content".into(),
                metadata: serde_json::Value::Null,
            }),
            Err(err),
        ]))
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

struct StaticResolver {
    model: std::sync::Mutex<Option<Box<dyn ChatModel>>>,
}

impl StaticResolver {
    fn new(model: Box<dyn ChatModel>) -> Self {
        Self {
            model: std::sync::Mutex::new(Some(model)),
        }
    }
}

impl FallbackResolver for StaticResolver {
    fn resolve(&self, _target: &str, _model_hint: &str) -> Result<Box<dyn ChatModel>, LlmError> {
        self.model
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| LlmError::Config {
                message: "resolver exhausted".into(),
            })
    }
}

struct CountingResolver {
    call_count: Arc<AtomicU32>,
}

impl FallbackResolver for CountingResolver {
    fn resolve(&self, _target: &str, _model_hint: &str) -> Result<Box<dyn ChatModel>, LlmError> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(SuccessModel::new("fallback")))
    }
}

#[tokio::test]
async fn chat_primary_fails_fallback_succeeds() {
    let primary = Box::new(FailingModel::new(server_error));
    let resolver = Arc::new(StaticResolver::new(Box::new(SuccessModel::new("fallback"))));
    let policy = make_policy(vec!["provider_b"]);

    let model = FallbackChatModel::new(primary, policy, resolver);
    let resp = model.chat(make_request()).await;
    assert!(resp.is_ok());
    assert_eq!(resp.unwrap().model.as_deref(), Some("fallback"));
}

#[tokio::test]
async fn chat_auth_error_no_fallback() {
    let primary = Box::new(FailingModel::new(auth_error));
    let counter = Arc::new(AtomicU32::new(0));
    let resolver = Arc::new(CountingResolver {
        call_count: counter.clone(),
    });
    let policy = make_policy(vec!["provider_b"]);

    let model = FallbackChatModel::new(primary, policy, resolver);
    let resp = model.chat(make_request()).await;
    assert!(resp.is_err());
    assert_eq!(
        counter.load(Ordering::SeqCst),
        0,
        "resolver must not be called for auth errors"
    );
}

#[tokio::test]
async fn chat_cancelled_no_fallback() {
    let primary = Box::new(FailingModel::new(|| LlmError::Cancelled));
    let counter = Arc::new(AtomicU32::new(0));
    let resolver = Arc::new(CountingResolver {
        call_count: counter.clone(),
    });
    let policy = make_policy(vec!["provider_b"]);

    let model = FallbackChatModel::new(primary, policy, resolver);
    let resp = model.chat(make_request()).await;
    assert!(resp.is_err());
    assert_eq!(counter.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn chat_disabled_mode_no_fallback() {
    let primary = Box::new(FailingModel::new(server_error));
    let counter = Arc::new(AtomicU32::new(0));
    let resolver = Arc::new(CountingResolver {
        call_count: counter.clone(),
    });
    let policy = FallbackPolicy::default();

    let model = FallbackChatModel::new(primary, policy, resolver);
    let resp = model.chat(make_request()).await;
    assert!(resp.is_err());
    assert_eq!(counter.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn stream_primary_fails_fallback_succeeds() {
    let primary = Box::new(FailingModel::new(server_error));
    let resolver = Arc::new(StaticResolver::new(Box::new(SuccessModel::new("fallback"))));
    let policy = make_policy(vec!["provider_b"]);

    let model = FallbackChatModel::new(primary, policy, resolver);
    let mut stream = model.chat_stream(make_request());

    let mut events = vec![];
    while let Some(item) = stream.next().await {
        events.push(item);
    }

    assert_eq!(events.len(), 1);
    assert!(events[0].is_ok());
    if let Ok(AgentEvent::TextDelta { delta, .. }) = &events[0] {
        assert!(delta.contains("fallback"));
    } else {
        panic!("expected TextDelta from fallback model");
    }
}

#[tokio::test]
async fn stream_content_emitted_no_fallback() {
    let primary = Box::new(PartialThenFailModel::new(server_error));
    let counter = Arc::new(AtomicU32::new(0));
    let resolver = Arc::new(CountingResolver {
        call_count: counter.clone(),
    });
    let policy = make_policy(vec!["provider_b"]);

    let model = FallbackChatModel::new(primary, policy, resolver);
    let mut stream = model.chat_stream(make_request());

    let mut events = vec![];
    while let Some(item) = stream.next().await {
        events.push(item);
    }

    assert_eq!(events.len(), 2);
    assert!(events[0].is_ok());
    assert!(events[1].is_err());
    assert_eq!(
        counter.load(Ordering::SeqCst),
        0,
        "resolver must not be called after content was emitted"
    );
}

#[tokio::test]
async fn stream_auth_error_no_fallback() {
    let primary = Box::new(FailingModel::new(auth_error));
    let counter = Arc::new(AtomicU32::new(0));
    let resolver = Arc::new(CountingResolver {
        call_count: counter.clone(),
    });
    let policy = make_policy(vec!["provider_b"]);

    let model = FallbackChatModel::new(primary, policy, resolver);
    let mut stream = model.chat_stream(make_request());

    let mut events = vec![];
    while let Some(item) = stream.next().await {
        events.push(item);
    }

    assert_eq!(events.len(), 1);
    assert!(events[0].is_err());
    assert_eq!(counter.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn fallback_waits_for_primary_retries_before_switching_models() {
    use std::time::Duration;

    let primary = rua_provider::RetryChatModel::new(
        Box::new(FailingModel::new(server_error)),
        rua_provider::RetryPolicy {
            max_retries: 1,
            initial_delay: Duration::from_secs(2),
            ..Default::default()
        },
    );
    let fallback = SuccessModel::new("fallback");
    let fallback_calls = fallback.call_count.clone();
    let model = FallbackChatModel::new(
        Box::new(primary),
        make_policy(vec!["provider_b"]),
        Arc::new(StaticResolver::new(Box::new(fallback))),
    );
    let started = tokio::time::Instant::now();
    let events = model
        .chat_stream_with_idle_timeout(make_request(), Duration::from_secs(1))
        .collect::<Vec<_>>()
        .await;
    assert!(events.iter().all(Result::is_ok), "{events:?}");
    assert!(events.iter().any(
        |event| matches!(event, Ok(AgentEvent::TextDelta { delta, .. })
        if delta == "ok from fallback")
    ));
    assert_eq!(fallback_calls.load(Ordering::SeqCst), 1);
    assert!(started.elapsed() >= Duration::from_secs(2));
}
