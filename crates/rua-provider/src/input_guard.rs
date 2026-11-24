use agent_types::{AgentEvent, ChatRequest, ChatResponse};
use llm_multiplex::{ChatModel, EventStream, LlmError, ModelInfo};

/// Last line of defence before a request leaves the process.
///
/// Projection is supposed to keep the model input inside the provider window,
/// but every estimate it relies on can go wrong: a truncated projection, a
/// history replacement, or a summary over raw history. Those mistakes used to
/// surface as an opaque provider rejection after a long round trip. Measuring
/// the serialized request here turns them into an immediate, classifiable
/// context-overflow error instead.
pub struct InputLimitChatModel {
    inner: Box<dyn ChatModel>,
    max_input_tokens: u64,
}

impl InputLimitChatModel {
    pub fn new(inner: Box<dyn ChatModel>, max_input_tokens: u64) -> Self {
        Self {
            inner,
            max_input_tokens,
        }
    }

    /// Serialized size of everything the provider receives as model input.
    ///
    /// This deliberately over-counts bookkeeping metadata: the guard exists to
    /// stop requests that are already hopeless, so a conservative measure is
    /// the safe direction.
    fn request_input_tokens(request: &ChatRequest) -> u64 {
        let tokens = |bytes: usize| ((bytes as f64) / 4.0).ceil() as u64;
        let messages = serde_json::to_vec(&request.messages).map_or(0, |bytes| tokens(bytes.len()));
        let tools = serde_json::to_vec(&request.tools).map_or(0, |bytes| tokens(bytes.len()));
        let structured =
            serde_json::to_vec(&request.structured_output).map_or(0, |bytes| tokens(bytes.len()));
        messages.saturating_add(tools).saturating_add(structured)
    }

    fn overflow_error(&self, actual: u64) -> LlmError {
        LlmError::Unknown {
            provider: self.inner.model_info().id.clone(),
            // The wording matches the phrases context-overflow detection
            // recognises, so callers compact instead of failing the turn.
            message: format!(
                "input length should be at most {} but the request is {} tokens (context overflow)",
                self.max_input_tokens, actual
            ),
        }
    }

    /// Streaming callers decide between compaction and failure by inspecting
    /// the event, not the transport error, so the guard has to surface an
    /// `Error` event for the overflow to be recoverable.
    fn overflow_event(&self, actual: u64) -> AgentEvent {
        AgentEvent::Error {
            code: Some("context_length_exceeded".to_string()),
            message: format!(
                "input length should be at most {} but the request is {} tokens (context overflow)",
                self.max_input_tokens, actual
            ),
            // Retrying the identical request cannot help; the caller must
            // shrink the projection first.
            retryable: false,
            metadata: serde_json::Value::Null,
        }
    }

    fn check(&self, request: &ChatRequest) -> Result<(), LlmError> {
        if self.max_input_tokens == 0 {
            return Ok(());
        }
        let actual = Self::request_input_tokens(request);
        if actual > self.max_input_tokens {
            return Err(self.overflow_error(actual));
        }
        Ok(())
    }

    fn overflow_event_for(&self, request: &ChatRequest) -> Option<AgentEvent> {
        if self.max_input_tokens == 0 {
            return None;
        }
        let actual = Self::request_input_tokens(request);
        (actual > self.max_input_tokens).then(|| self.overflow_event(actual))
    }
}

#[async_trait::async_trait]
impl ChatModel for InputLimitChatModel {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        self.check(&request)?;
        self.inner.chat(request).await
    }

    fn chat_stream(&self, request: ChatRequest) -> EventStream {
        if let Some(event) = self.overflow_event_for(&request) {
            return Box::pin(futures::stream::once(async move { Ok(event) }));
        }
        self.inner.chat_stream(request)
    }

    fn chat_stream_with_idle_timeout(
        &self,
        request: ChatRequest,
        idle_timeout: std::time::Duration,
    ) -> EventStream {
        if let Some(event) = self.overflow_event_for(&request) {
            return Box::pin(futures::stream::once(async move { Ok(event) }));
        }
        self.inner
            .chat_stream_with_idle_timeout(request, idle_timeout)
    }

    fn model_info(&self) -> &ModelInfo {
        self.inner.model_info()
    }
}
