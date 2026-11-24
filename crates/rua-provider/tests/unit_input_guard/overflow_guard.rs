use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use agent_types::{AgentEvent, ChatRequest, ChatResponse, FinishReason, ModelDialogueItem, Role};
use async_trait::async_trait;
use llm_multiplex::{ChatModel, EventStream, LlmError, ModelInfo};
use rua_provider::InputLimitChatModel;

/// Records whether the wrapped model was reached at all.
struct CountingModel {
    calls: Arc<AtomicU32>,
    info: ModelInfo,
}

impl CountingModel {
    fn boxed(calls: Arc<AtomicU32>) -> Box<dyn ChatModel> {
        Box::new(Self {
            calls,
            info: ModelInfo {
                id: "guard-test-model".into(),
                name: "guard-test-model".into(),
                context_window: Some(1_000),
                max_output_tokens: None,
                supports_multimodal: None,
                supports_image: None,
                supports_video: None,
                probe_source: None,
                generate_kwargs: Default::default(),
            },
        })
    }
}

#[async_trait]
impl ChatModel for CountingModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
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
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(futures::stream::once(async {
            Ok(AgentEvent::End {
                finish_reason: FinishReason::Stop,
                usage: None,
                metadata: serde_json::Value::Null,
            })
        }))
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

fn big_request() -> ChatRequest {
    // Roughly 4 characters per token, so this clears a 1_000 token window.
    ChatRequest::new(vec![ModelDialogueItem::text(
        Role::User,
        "x".repeat(40_000),
    )])
}

fn small_request() -> ChatRequest {
    ChatRequest::new(vec![ModelDialogueItem::text(Role::User, "hi")])
}

#[tokio::test]
async fn oversized_streamed_request_is_rejected_before_the_provider() {
    let calls = Arc::new(AtomicU32::new(0));
    let model = InputLimitChatModel::new(CountingModel::boxed(Arc::clone(&calls)), 1_000);
    let mut stream = model.chat_stream(big_request());
    let event = futures::StreamExt::next(&mut stream)
        .await
        .expect("guard must emit an event")
        .expect("guard reports overflow as an event, not a transport error");
    let AgentEvent::Error { message, .. } = event else {
        panic!("expected an error event, got something else");
    };
    assert!(
        rua_provider::error::is_context_overflow(&message, None),
        "overflow must be recognisable so the caller compacts: {message}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "the provider must never be reached"
    );
}

#[tokio::test]
async fn oversized_non_streamed_request_is_rejected_before_the_provider() {
    let calls = Arc::new(AtomicU32::new(0));
    let model = InputLimitChatModel::new(CountingModel::boxed(Arc::clone(&calls)), 1_000);
    let error = model
        .chat(big_request())
        .await
        .expect_err("oversized request must be rejected");
    assert!(
        rua_provider::error::is_context_overflow(&error.to_string(), None),
        "overflow must be recognisable so the caller compacts: {error}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "the provider must never be reached"
    );
}

#[tokio::test]
async fn request_inside_the_limit_still_reaches_the_provider() {
    let calls = Arc::new(AtomicU32::new(0));
    let model = InputLimitChatModel::new(CountingModel::boxed(Arc::clone(&calls)), 1_000);
    model.chat(small_request()).await.expect("request fits");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn zero_limit_disables_the_guard() {
    let calls = Arc::new(AtomicU32::new(0));
    let model = InputLimitChatModel::new(CountingModel::boxed(Arc::clone(&calls)), 0);
    model
        .chat(big_request())
        .await
        .expect("an unset limit must not reject anything");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn idle_timeout_stream_path_is_guarded_too() {
    let calls = Arc::new(AtomicU32::new(0));
    let model = InputLimitChatModel::new(CountingModel::boxed(Arc::clone(&calls)), 1_000);
    // The agent runtime calls this path rather than `chat_stream`, so the guard
    // has to enforce the limit here as well.
    let mut stream =
        model.chat_stream_with_idle_timeout(big_request(), std::time::Duration::from_secs(30));
    let event = futures::StreamExt::next(&mut stream)
        .await
        .expect("guard must emit an event")
        .expect("guard reports overflow as an event");
    assert!(matches!(event, AgentEvent::Error { .. }));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn factory_applies_the_configured_input_ceiling() {
    use rua_provider::ModelFactory;

    let calls = Arc::new(AtomicU32::new(0));
    let model = ModelFactory::new()
        .with_max_input_tokens(1_000)
        .build(CountingModel::boxed(Arc::clone(&calls)), "guard-test");
    let error = model
        .chat(big_request())
        .await
        .expect_err("factory-built model must enforce the ceiling");
    assert!(rua_provider::error::is_context_overflow(
        &error.to_string(),
        None
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn factory_without_a_configured_ceiling_does_not_guess_from_context_window() {
    use rua_provider::ModelFactory;

    // The model advertises a 1_000 token window, but that is provider metadata
    // rather than an operator decision. A factory with no configured ceiling
    // must not invent one and start rejecting requests.
    let calls = Arc::new(AtomicU32::new(0));
    let model = ModelFactory::new().build(CountingModel::boxed(Arc::clone(&calls)), "guard-test");
    model
        .chat(big_request())
        .await
        .expect("no ceiling configured means no rejection");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
