use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::StreamExt;
use tokio::sync::Mutex;

use agent_types::{AgentEvent, ChatRequest, ChatResponse, FinishReason, TokenUsage};
use llm_multiplex::{ChatModel, EventStream, LlmError, ModelInfo};
use rua_contract::{UsageRecordMetadata, UsageRecorder};
use rua_provider::usage_recording::UsageRecordingChatModel;

#[derive(Clone)]
struct RecordedEntry {
    provider_id: String,
    model_id: String,
    usage: TokenUsage,
    metadata: UsageRecordMetadata,
}

struct SpyRecorder {
    entries: Mutex<Vec<RecordedEntry>>,
}

impl SpyRecorder {
    fn new() -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
        }
    }

    async fn entries(&self) -> Vec<RecordedEntry> {
        self.entries.lock().await.clone()
    }
}

#[async_trait]
impl UsageRecorder for SpyRecorder {
    async fn record_usage(
        &self,
        provider_id: &str,
        model_id: &str,
        usage: &TokenUsage,
        _started_at: DateTime<Utc>,
        _finished_at: DateTime<Utc>,
        metadata: UsageRecordMetadata,
    ) {
        self.entries.lock().await.push(RecordedEntry {
            provider_id: provider_id.to_string(),
            model_id: model_id.to_string(),
            usage: usage.clone(),
            metadata,
        });
    }
}

fn make_model_info() -> ModelInfo {
    ModelInfo {
        id: "test-model".into(),
        name: "Test".into(),
        context_window: None,
        max_output_tokens: None,
        supports_multimodal: None,
        supports_image: None,
        supports_video: None,
        probe_source: None,
        generate_kwargs: Default::default(),
    }
}

struct StreamingModel {
    info: ModelInfo,
    make_events: Box<dyn Fn() -> Vec<Result<AgentEvent, LlmError>> + Send + Sync>,
}

#[async_trait]
impl ChatModel for StreamingModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        unimplemented!()
    }

    fn chat_stream(&self, _request: ChatRequest) -> EventStream {
        Box::pin(futures::stream::iter((self.make_events)()))
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

fn make_request() -> ChatRequest {
    ChatRequest {
        messages: vec![],
        model: Some("test-model".into()),
        temperature: None,
        max_tokens: None,
        stream: true,
        tools: vec![],
        tool_choice: None,
        structured_output: None,
        reasoning: None,
        metadata: serde_json::Map::from_iter([
            ("agent_id".to_string(), serde_json::json!("agent-a")),
            ("chat_id".to_string(), serde_json::json!("chat-a")),
        ]),
        provider_options: Default::default(),
        extra: Default::default(),
    }
}

#[tokio::test]
async fn stream_accumulates_usage_and_records() {
    let recorder = Arc::new(SpyRecorder::new());

    let inner = StreamingModel {
        info: make_model_info(),
        make_events: Box::new(|| {
            vec![
                Ok(AgentEvent::Start {
                    run_id: "r1".into(),
                    model: Some("test-model".into()),
                    metadata: serde_json::Value::Null,
                }),
                Ok(AgentEvent::TextDelta {
                    delta: "hello ".into(),
                    metadata: serde_json::Value::Null,
                }),
                Ok(AgentEvent::Usage {
                    usage: TokenUsage::new(50, 10),
                }),
                Ok(AgentEvent::TextDelta {
                    delta: "world".into(),
                    metadata: serde_json::Value::Null,
                }),
                Ok(AgentEvent::End {
                    finish_reason: FinishReason::Stop,
                    usage: Some(TokenUsage::new(50, 20)),
                    metadata: serde_json::Value::Null,
                }),
            ]
        }),
    };

    let model = UsageRecordingChatModel::new(
        Box::new(inner),
        recorder.clone(),
        "anthropic".into(),
        "claude-3".into(),
    );

    let mut stream = model.chat_stream(make_request());
    let mut collected = vec![];
    while let Some(item) = stream.next().await {
        collected.push(item);
    }

    assert_eq!(collected.len(), 5);

    let entries = recorder.entries().await;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].provider_id, "anthropic");
    assert_eq!(entries[0].model_id, "claude-3");

    assert_eq!(entries[0].usage.prompt_tokens, 100);
    assert_eq!(entries[0].usage.completion_tokens, 30);
    assert_eq!(entries[0].metadata.agent_id.as_deref(), Some("agent-a"));
    assert_eq!(entries[0].metadata.chat_id.as_deref(), Some("chat-a"));
}

#[tokio::test]
async fn stream_end_only_usage_records() {
    let recorder = Arc::new(SpyRecorder::new());

    let inner = StreamingModel {
        info: make_model_info(),
        make_events: Box::new(|| {
            vec![
                Ok(AgentEvent::Start {
                    run_id: "r2".into(),
                    model: None,
                    metadata: serde_json::Value::Null,
                }),
                Ok(AgentEvent::TextDelta {
                    delta: "hi".into(),
                    metadata: serde_json::Value::Null,
                }),
                Ok(AgentEvent::End {
                    finish_reason: FinishReason::Stop,
                    usage: Some(TokenUsage::new(80, 40)),
                    metadata: serde_json::Value::Null,
                }),
            ]
        }),
    };

    let model = UsageRecordingChatModel::new(
        Box::new(inner),
        recorder.clone(),
        "openai".into(),
        "gpt-4o".into(),
    );

    let mut stream = model.chat_stream(make_request());
    while stream.next().await.is_some() {}

    let entries = recorder.entries().await;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].usage.prompt_tokens, 80);
    assert_eq!(entries[0].usage.completion_tokens, 40);
}

#[tokio::test]
async fn stream_error_no_recording() {
    let recorder = Arc::new(SpyRecorder::new());

    let inner = StreamingModel {
        info: make_model_info(),
        make_events: Box::new(|| {
            vec![
                Ok(AgentEvent::Start {
                    run_id: "r3".into(),
                    model: None,
                    metadata: serde_json::Value::Null,
                }),
                Ok(AgentEvent::Usage {
                    usage: TokenUsage::new(10, 5),
                }),
                Err(LlmError::Server {
                    provider: "test".into(),
                    status: Some(500),
                    message: "boom".into(),
                }),
            ]
        }),
    };

    let model =
        UsageRecordingChatModel::new(Box::new(inner), recorder.clone(), "test".into(), "m".into());

    let mut stream = model.chat_stream(make_request());
    while stream.next().await.is_some() {}

    let entries = recorder.entries().await;
    assert!(entries.is_empty());
}

#[tokio::test(start_paused = true)]
async fn factory_usage_recording_preserves_per_attempt_timeout_across_backoff() {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    let recorder = Arc::new(SpyRecorder::new());
    let calls = Arc::new(AtomicU32::new(0));
    let calls_in_model = calls.clone();
    let inner = StreamingModel {
        info: make_model_info(),
        make_events: Box::new(move || {
            if calls_in_model.fetch_add(1, Ordering::SeqCst) == 0 {
                vec![Err(LlmError::Server {
                    provider: "test".into(),
                    status: Some(502),
                    message: "upstream unavailable".into(),
                })]
            } else {
                vec![Ok(AgentEvent::End {
                    finish_reason: FinishReason::Stop,
                    usage: Some(TokenUsage::new(80, 40)),
                    metadata: serde_json::Value::Null,
                })]
            }
        }),
    };
    let model = rua_provider::ModelFactory::new()
        .with_usage_recorder(recorder.clone())
        .with_retry_policy(rua_provider::RetryPolicy {
            max_retries: 1,
            initial_delay: Duration::from_secs(2),
            ..Default::default()
        })
        .build(Box::new(inner), "test");
    let started = tokio::time::Instant::now();
    let events = model
        .chat_stream_with_idle_timeout(make_request(), Duration::from_secs(1))
        .collect::<Vec<_>>()
        .await;
    assert!(events.iter().all(Result::is_ok), "{events:?}");
    assert!(events
        .iter()
        .any(|event| matches!(event, Ok(AgentEvent::End { .. }))));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(started.elapsed() >= Duration::from_secs(2));
    let entries = recorder.entries().await;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].usage.prompt_tokens, 80);
    assert_eq!(entries[0].usage.completion_tokens, 40);
}
