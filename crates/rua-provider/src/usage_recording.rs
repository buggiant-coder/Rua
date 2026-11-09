use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use futures::StreamExt;

use agent_types::{AgentEvent, ChatRequest, ChatResponse, TokenUsage};
use llm_multiplex::{ChatModel, EventStream, LlmError, ModelInfo};
use rua_contract::{UsageRecordMetadata, UsageRecorder};

pub struct UsageRecordingChatModel {
    inner: Box<dyn ChatModel>,
    recorder: Arc<dyn UsageRecorder>,
    provider_id: String,
    model_id: String,
}

impl UsageRecordingChatModel {
    pub fn new(
        inner: Box<dyn ChatModel>,
        recorder: Arc<dyn UsageRecorder>,
        provider_id: String,
        model_id: String,
    ) -> Self {
        Self {
            inner,
            recorder,
            provider_id,
            model_id,
        }
    }
}

fn metadata_string(
    metadata: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<String> {
    metadata
        .get(key)
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn usage_metadata_from_request(request: &ChatRequest) -> UsageRecordMetadata {
    let mut metadata = UsageRecordMetadata {
        usage_id: metadata_string(&request.metadata, "usage_id"),
        agent_id: metadata_string(&request.metadata, "agent_id"),
        chat_id: metadata_string(&request.metadata, "chat_id"),
        session_id: metadata_string(&request.metadata, "session_id"),
        turn_id: metadata_string(&request.metadata, "turn_id"),
        run_id: metadata_string(&request.metadata, "run_id"),
        device_id: metadata_string(&request.metadata, "device_id"),
        source: metadata_string(&request.metadata, "source")
            .or_else(|| Some("react_agent".to_string())),
        extra: serde_json::Map::new(),
    };
    metadata.extra.extend(request.metadata.clone());
    metadata
}

#[async_trait]
impl ChatModel for UsageRecordingChatModel {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let started_at = Utc::now();
        let metadata = usage_metadata_from_request(&request);
        let response = self.inner.chat(request).await?;
        let finished_at = Utc::now();

        if let Some(ref usage) = response.usage {
            self.recorder
                .record_usage(
                    &self.provider_id,
                    &self.model_id,
                    usage,
                    started_at,
                    finished_at,
                    metadata,
                )
                .await;
        }

        Ok(response)
    }

    fn chat_stream(&self, request: ChatRequest) -> EventStream {
        self.chat_stream_with_idle_timeout(request, Duration::ZERO)
    }

    fn chat_stream_with_idle_timeout(
        &self,
        request: ChatRequest,
        idle_timeout: Duration,
    ) -> EventStream {
        let metadata = usage_metadata_from_request(&request);
        let inner_stream = self
            .inner
            .chat_stream_with_idle_timeout(request, idle_timeout);
        let recorder = self.recorder.clone();
        let provider_id = self.provider_id.clone();
        let model_id = self.model_id.clone();

        Box::pin(async_stream::stream! {
            let started_at = Utc::now();
            let mut accumulated = TokenUsage::default();

            futures::pin_mut!(inner_stream);
            while let Some(item) = inner_stream.next().await {
                match &item {
                    Ok(AgentEvent::Usage { usage }) => {
                        accumulated.accumulate(usage);
                    }
                    Ok(AgentEvent::End { usage, .. }) => {
                        if let Some(u) = usage {
                            accumulated.accumulate(u);
                        }
                        let finished_at = Utc::now();
                        if !accumulated.is_empty() {
                            recorder
                                .record_usage(
                                    &provider_id,
                                    &model_id,
                                    &accumulated,
                                    started_at,
                                    finished_at,
                                    metadata.clone(),
                                )
                                .await;
                        }
                    }
                    _ => {}
                }
                yield item;
            }
        })
    }

    fn model_info(&self) -> &ModelInfo {
        self.inner.model_info()
    }
}
