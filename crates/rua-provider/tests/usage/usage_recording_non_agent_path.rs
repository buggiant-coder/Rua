use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::Mutex;

use agent_types::{ChatRequest, ChatResponse, TokenUsage};
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

struct FakeChatModel {
    info: ModelInfo,
    response_usage: Option<TokenUsage>,
}

#[async_trait]
impl ChatModel for FakeChatModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        Ok(ChatResponse {
            message: agent_types::ModelDialogueItem::text(agent_types::Role::Assistant, "hello"),
            usage: self.response_usage.clone(),
            model: Some("test-model".into()),
            finish_reason: agent_types::FinishReason::Stop,
            metadata: Default::default(),
            extra: Default::default(),
        })
    }

    fn chat_stream(&self, _request: ChatRequest) -> EventStream {
        Box::pin(futures::stream::empty())
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
        stream: false,
        tools: vec![],
        tool_choice: None,
        structured_output: None,
        reasoning: None,
        metadata: serde_json::Map::from_iter([
            ("agent_id".to_string(), serde_json::json!("agent-a")),
            ("chat_id".to_string(), serde_json::json!("chat-a")),
            ("turn_id".to_string(), serde_json::json!("turn-a")),
            (
                "usage_id".to_string(),
                serde_json::json!("usage-history-replacement"),
            ),
            (
                "source".to_string(),
                serde_json::json!("history_replacement"),
            ),
            (
                "source_block_id".to_string(),
                serde_json::json!("hr-block-a"),
            ),
        ]),
        provider_options: Default::default(),
        extra: Default::default(),
    }
}

#[tokio::test]
async fn chat_records_usage_to_recorder() {
    let recorder = Arc::new(SpyRecorder::new());
    let inner = FakeChatModel {
        info: make_model_info(),
        response_usage: Some(TokenUsage::new(100, 50)),
    };

    let model = UsageRecordingChatModel::new(
        Box::new(inner),
        recorder.clone(),
        "dashscope".into(),
        "qwen3-max".into(),
    );

    let resp = model.chat(make_request()).await.unwrap();
    assert_eq!(resp.usage.as_ref().unwrap().prompt_tokens, 100);

    let entries = recorder.entries().await;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].provider_id, "dashscope");
    assert_eq!(entries[0].model_id, "qwen3-max");
    assert_eq!(entries[0].usage.prompt_tokens, 100);
    assert_eq!(entries[0].usage.completion_tokens, 50);
    assert_eq!(entries[0].metadata.agent_id.as_deref(), Some("agent-a"));
    assert_eq!(entries[0].metadata.chat_id.as_deref(), Some("chat-a"));
    assert_eq!(entries[0].metadata.turn_id.as_deref(), Some("turn-a"));
    assert_eq!(
        entries[0].metadata.usage_id.as_deref(),
        Some("usage-history-replacement")
    );
    assert_eq!(
        entries[0].metadata.source.as_deref(),
        Some("history_replacement")
    );
    assert_eq!(
        entries[0].metadata.extra["source_block_id"],
        serde_json::json!("hr-block-a")
    );
}

#[tokio::test]
async fn chat_no_usage_skips_recording() {
    let recorder = Arc::new(SpyRecorder::new());
    let inner = FakeChatModel {
        info: make_model_info(),
        response_usage: None,
    };

    let model = UsageRecordingChatModel::new(
        Box::new(inner),
        recorder.clone(),
        "openai".into(),
        "gpt-4".into(),
    );

    let resp = model.chat(make_request()).await.unwrap();
    assert!(resp.usage.is_none());

    let entries = recorder.entries().await;
    assert!(entries.is_empty());
}
