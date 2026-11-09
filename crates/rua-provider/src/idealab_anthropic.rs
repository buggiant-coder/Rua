use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use tracing::warn;

use agent_types::{
    AgentEvent, ChatRequest, ChatResponse, ContentPart, FinishReason, ModelDialogueItem, Role,
    TokenUsage, ToolCall,
};
use llm_multiplex::error::LlmError;
use llm_multiplex::event_parser::parse_anthropic_event;
use llm_multiplex::providers::anthropic::{AnthropicConfig, AnthropicProvider};
use llm_multiplex::traits::{
    ChatModel, ConnectionStatus, EventStream, ModelInfo, Provider, ProviderInfo,
};
use llm_multiplex::transport::{parse_anthropic_sse, HttpTransport, TransportConfig};

pub struct IdealabAnthropicProvider {
    transport: Arc<HttpTransport>,
    config: AnthropicConfig,
    info: ProviderInfo,

    anthropic: Arc<AnthropicProvider>,
}

impl IdealabAnthropicProvider {
    pub fn new(config: AnthropicConfig) -> Result<Self, LlmError> {
        let mut tc = TransportConfig::new(&config.base_url, &config.provider_id);
        if let Some(ref key) = config.api_key {
            tc = tc.with_header("Authorization", &format!("Bearer {key}"));
        }
        tc = tc
            .with_timeout(std::time::Duration::from_secs(config.timeout_secs))
            .with_header("anthropic-version", &config.api_version)
            .with_headers_from_map(&config.headers);
        let transport = Arc::new(HttpTransport::new(tc)?);

        let info = ProviderInfo {
            id: config.provider_id.clone(),
            name: config.provider_name.clone(),
            provider_type: Some("IdealabAnthropic".into()),
            is_local: false,
            support_model_discovery: false,
            support_connection_check: config.support_connection_check,
        };

        let anthropic = Arc::new(AnthropicProvider::new(config.clone())?);

        Ok(Self {
            transport,
            config,
            info,
            anthropic,
        })
    }

    pub fn inner_ref_count(&self) -> usize {
        Arc::strong_count(&self.anthropic)
    }
}

#[async_trait]
impl Provider for IdealabAnthropicProvider {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    fn supports_model(&self, model: &str) -> bool {
        self.config.models.iter().any(|m| m.id == model)
    }

    fn create_chat_model(&self, model: &str) -> Result<Box<dyn ChatModel>, LlmError> {
        let model_info = self.config.models.iter().find(|m| m.id == model).cloned();
        let info = model_info.unwrap_or(ModelInfo {
            id: model.to_string(),
            name: model.to_string(),
            context_window: Some(200_000),
            max_output_tokens: Some(4096),
            supports_multimodal: Some(true),
            supports_image: Some(true),
            supports_video: Some(false),
            probe_source: None,
            generate_kwargs: serde_json::Map::new(),
        });

        let mut config = self.config.clone();
        crate::manager::deep_merge_maps(&mut config.generate_kwargs, &info.generate_kwargs);
        let inner = if info.generate_kwargs.is_empty() {
            self.anthropic.clone()
        } else {
            Arc::new(AnthropicProvider::new(config.clone())?)
        };
        Ok(Box::new(IdealabAnthropicChatModel {
            model_info: info,
            transport: self.transport.clone(),
            config,
            inner,
        }))
    }

    async fn fetch_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        Ok(self.config.models.clone())
    }

    async fn check_connection(&self) -> Result<ConnectionStatus, LlmError> {
        let model_id = self
            .config
            .models
            .first()
            .map(|m| m.id.as_str())
            .unwrap_or("claude-opus-4-7");
        let body = serde_json::json!({
            "model": model_id,
            "max_tokens": 1,
            "messages": [{"role": "user", "content": "ping"}],
        });
        match self.transport.post("/v1/messages", &body).await {
            Ok(_) => Ok(ConnectionStatus::Connected),
            Err(e) => Ok(ConnectionStatus::Failed(format!("Idealab Anthropic: {e}"))),
        }
    }
}

struct IdealabAnthropicChatModel {
    model_info: ModelInfo,
    transport: Arc<HttpTransport>,
    config: AnthropicConfig,
    inner: Arc<AnthropicProvider>,
}

#[async_trait]
impl ChatModel for IdealabAnthropicChatModel {
    async fn chat(&self, mut request: ChatRequest) -> Result<ChatResponse, LlmError> {
        request.stream = false;
        if request.model.is_none() {
            request.model = Some(self.model_info.id.clone());
        }

        let body = self.inner.build_request_body(&request);

        let response_text = self.transport.post("/v1/messages", &body).await?;
        let value: serde_json::Value =
            serde_json::from_str(&response_text).map_err(|e| LlmError::InvalidResponse {
                provider: self.config.provider_id.clone(),
                message: format!("failed to parse response: {e}"),
            })?;

        parse_anthropic_response(&value)
    }

    fn chat_stream(&self, mut request: ChatRequest) -> EventStream {
        request.stream = true;
        if request.model.is_none() {
            request.model = Some(self.model_info.id.clone());
        }

        let body = self.inner.build_request_body(&request);

        let transport = self.transport.clone();
        let provider_id = self.config.provider_id.clone();
        let run_id = request
            .metadata
            .get("request_id")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let model = request.model.clone();

        let stream = async_stream::stream! {
            yield Ok(AgentEvent::Start {
                run_id: run_id.clone(),
                model: model.clone(),
                metadata: serde_json::Value::Null,
            });

            let byte_stream = match transport.post_stream("/v1/messages", &body).await {
                Ok(s) => s,
                Err(e) => {
                    yield Err(e);
                    return;
                }
            };

            let mut buffer = String::new();
            let mut tool_identities: std::collections::HashMap<usize, String> =
                std::collections::HashMap::new();
            let mut final_finish_reason: Option<FinishReason> = None;
            let mut completed = false;

            tokio::pin!(byte_stream);

            'stream: while let Some(result) = byte_stream.next().await {
                match result {
                    Ok(bytes) => {
                        buffer.push_str(&String::from_utf8_lossy(&bytes));
                        let (sse_events, remaining) = parse_anthropic_sse(&buffer);
                        buffer = remaining;

                        for (event_type, data) in sse_events {
                            if event_type == "content_block_start" {
                                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&data) {
                                    if let Some(cb) = val.get("content_block") {
                                        let block_type = cb.get("type").and_then(|t| t.as_str()).unwrap_or("");
                                        if block_type == "tool_use" {
                                            let index = val.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                                            let id = cb.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
                                            let name = cb.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                                            tool_identities.insert(index, id.clone());
                                            yield Ok(AgentEvent::ToolCallDelta {
                                                id,
                                                index,
                                                name: Some(name),
                                                arguments_delta: String::new(),
                                                metadata: serde_json::Value::Null,
                                            });
                                        }
                                    }
                                }
                            }

                            match parse_anthropic_event(&event_type, &data) {
                                Ok((events, finish)) => {
                                    if let Some(fr) = finish {
                                        if event_type != "message_stop"
                                            || final_finish_reason.is_none()
                                        {
                                            final_finish_reason = Some(fr);
                                        }
                                    }
                                    let terminal_error = events
                                        .iter()
                                        .any(|event| matches!(event, AgentEvent::Error { .. }));
                                    for mut event in events {
                                        match &mut event {
                                            AgentEvent::ToolCallDelta {
                                                id, index, ..
                                            } => {
                                                if let Some(provider_id) = tool_identities.get(index) {
                                                    *id = provider_id.clone();
                                                }
                                                yield Ok(event);
                                            }
                                            _ => yield Ok(event),
                                        }
                                    }
                                    if terminal_error {
                                        return;
                                    }
                                }
                                Err(e) => {
                                    warn!(provider = %provider_id, error = %e, "failed to parse Idealab Anthropic SSE");
                                }
                            }

                            if event_type == "content_block_stop" {
                                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&data) {
                                    let index = val.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                                    tool_identities.remove(&index);
                                }
                            }
                            if event_type == "message_stop" {
                                completed = true;
                                break 'stream;
                            }
                        }
                    }
                    Err(e) => {
                        yield Err(e);
                        return;
                    }
                }
            }

            if !completed {
                yield Err(LlmError::Connection {
                    provider: provider_id,
                    message: "Anthropic stream closed before message_stop".into(),
                });
                return;
            }

            let finish = final_finish_reason.unwrap_or(FinishReason::Stop);
            yield Ok(AgentEvent::End {
                finish_reason: finish,
                usage: None,
                metadata: serde_json::Value::Null,
            });
        };

        Box::pin(stream)
    }

    fn model_info(&self) -> &ModelInfo {
        &self.model_info
    }
}

fn parse_anthropic_response(value: &serde_json::Value) -> Result<ChatResponse, LlmError> {
    let mut content = Vec::new();

    if let Some(blocks) = value.get("content").and_then(|c| c.as_array()) {
        for block in blocks {
            let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match block_type {
                "text" => {
                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                        content.push(ContentPart::Text {
                            text: text.to_string(),
                        });
                    }
                }
                "thinking" => {
                    if let Some(text) = block.get("thinking").and_then(|t| t.as_str()) {
                        content.push(ContentPart::Thinking {
                            text: text.to_string(),
                            signature: block
                                .get("signature")
                                .and_then(|s| s.as_str())
                                .map(String::from),
                        });
                    }
                }
                "tool_use" => {
                    let id = block
                        .get("id")
                        .and_then(|i| i.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = block
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string();
                    let input = block
                        .get("input")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    content.push(ContentPart::ToolUse {
                        call: ToolCall {
                            id: id.clone(),
                            name,
                            arguments: input,
                            raw_arguments: None,
                            provider_call_id: Some(id),
                            metadata: serde_json::Map::new(),
                        },
                    });
                }
                _ => {}
            }
        }
    }

    let finish_reason = value
        .get("stop_reason")
        .and_then(|r| r.as_str())
        .map(FinishReason::from_provider)
        .unwrap_or(FinishReason::Stop);

    let usage = value.get("usage").and_then(|u| {
        Some(TokenUsage {
            prompt_tokens: u.get("input_tokens")?.as_u64()?,
            completion_tokens: u.get("output_tokens")?.as_u64()?,
            reasoning_tokens: None,
            cached_tokens: u.get("cache_read_input_tokens").and_then(|c| c.as_u64()),
            cache_write_tokens: u
                .get("cache_creation_input_tokens")
                .and_then(|c| c.as_u64()),
            total_tokens: None,
        })
    });

    let model = value
        .get("model")
        .and_then(|m| m.as_str())
        .map(String::from);

    let message = ModelDialogueItem {
        id: None,
        role: Role::Assistant,
        content,
        name: None,
        created_at: None,
        metadata: serde_json::Map::new(),
    };

    Ok(ChatResponse {
        message,
        usage,
        model,
        finish_reason,
        metadata: serde_json::Map::new(),
        extra: Default::default(),
    })
}
