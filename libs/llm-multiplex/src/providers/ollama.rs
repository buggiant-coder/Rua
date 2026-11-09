use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use tracing::warn;

use base64::Engine;

use agent_types::{
    AgentEvent, ChatRequest, ChatResponse, ContentPart, FinishReason, ModelDialogueItem, Role,
    TokenUsage, ToolCall,
};

use crate::error::LlmError;
use crate::event_parser::parse_ollama_chunk;
use crate::traits::{ChatModel, ConnectionStatus, EventStream, ModelInfo, Provider, ProviderInfo};
use crate::transport::{HttpTransport, TransportConfig};

pub const OLLAMA_DEFAULT_URL: &str = "http://localhost:11434";

#[derive(Debug, Clone)]
pub struct OllamaConfig {
    pub provider_id: String,
    pub provider_name: String,
    pub base_url: String,
    pub models: Vec<ModelInfo>,
    pub timeout_secs: u64,
    pub generate_kwargs: serde_json::Map<String, serde_json::Value>,
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self {
            provider_id: "ollama".into(),
            provider_name: "Ollama".into(),
            base_url: OLLAMA_DEFAULT_URL.into(),
            models: vec![],
            timeout_secs: 120,
            generate_kwargs: serde_json::Map::new(),
        }
    }
}

pub struct OllamaProvider {
    config: OllamaConfig,
    transport: Arc<HttpTransport>,
    info: ProviderInfo,
}

impl OllamaProvider {
    pub fn new(mut config: OllamaConfig) -> Result<Self, LlmError> {
        config.base_url = normalize_ollama_base_url(&config.base_url);
        let transport_config = TransportConfig::new(&config.base_url, &config.provider_id)
            .with_timeout(std::time::Duration::from_secs(config.timeout_secs));

        let transport = Arc::new(HttpTransport::new(transport_config)?);

        let info = ProviderInfo {
            id: config.provider_id.clone(),
            name: config.provider_name.clone(),
            provider_type: Some("OllamaChatModel".into()),
            is_local: true,
            support_model_discovery: true,
            support_connection_check: true,
        };

        Ok(Self {
            config,
            transport,
            info,
        })
    }

    pub fn build_request_body(&self, request: &ChatRequest, model: &str) -> serde_json::Value {
        let messages: Vec<serde_json::Value> =
            crate::request::normalize_tool_history(&request.messages)
                .iter()
                .map(|msg| {
                    let role = match msg.role {
                        Role::System => "system",
                        Role::User => "user",
                        Role::Assistant => "assistant",
                        Role::Tool => "tool",
                        _ => "user",
                    };

                    let images = extract_ollama_images(&msg.content);

                    let mut message = serde_json::json!({
                        "role": role,
                        "content": content_text_for_ollama(&msg.content),
                    });
                    let thinking = msg.thinking_content();
                    if msg.role == Role::Assistant && !thinking.is_empty() {
                        message["thinking"] = serde_json::json!(thinking);
                    }
                    if !images.is_empty() {
                        message["images"] = serde_json::json!(images);
                    }
                    message
                })
                .collect();

        let keep_alive = request
            .provider_options
            .get("keep_alive")
            .or_else(|| self.config.generate_kwargs.get("keep_alive"))
            .and_then(|v| v.as_str())
            .unwrap_or("5m");

        let mut body = serde_json::json!({
            "model": model,
            "messages": messages,
            "stream": request.stream,
            "keep_alive": keep_alive,
        });

        let think = request
            .provider_options
            .get("think")
            .or_else(|| request.provider_options.get("enable_thinking"))
            .or_else(|| self.config.generate_kwargs.get("think"))
            .or_else(|| self.config.generate_kwargs.get("enable_thinking"));
        if let Some(think_val) = think {
            body["think"] = think_val.clone();
        }
        if let Some(reasoning) = request.reasoning.as_ref() {
            body["think"] = if !reasoning.enabled {
                serde_json::Value::Bool(false)
            } else if let Some(effort) = reasoning.effort.as_deref() {
                serde_json::Value::String(effort.to_string())
            } else {
                serde_json::Value::Bool(true)
            };
        }

        if !request.tools.is_empty() {
            let tools: Vec<serde_json::Value> = request
                .tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        }
                    })
                })
                .collect();
            body["tools"] = serde_json::json!(tools);

            if request.tool_choice.is_some() {
                warn!("Ollama does not support tool_choice yet, ignored");
            }
        }

        let mut options = serde_json::Map::new();
        if let Some(temp) = request.temperature {
            options.insert("temperature".into(), serde_json::json!(temp));
        }
        if let Some(max_tokens) = request.max_tokens {
            options.insert("num_predict".into(), serde_json::json!(max_tokens));
        }

        for (key, value) in &self.config.generate_kwargs {
            if !matches!(key.as_str(), "keep_alive" | "think" | "enable_thinking") {
                options.entry(key.clone()).or_insert(value.clone());
            }
        }

        for (key, value) in &request.provider_options {
            if !matches!(key.as_str(), "keep_alive" | "think" | "enable_thinking") {
                options.insert(key.clone(), value.clone());
            }
        }
        if !options.is_empty() {
            body["options"] = serde_json::Value::Object(options);
        }

        body
    }
}

fn normalize_ollama_base_url(base_url: &str) -> String {
    let mut normalized = base_url.trim().trim_end_matches('/').to_string();
    if normalized.ends_with("/v1") {
        normalized.truncate(normalized.len() - "/v1".len());
    }
    normalized
}

#[async_trait]
impl Provider for OllamaProvider {
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
            context_window: None,
            max_output_tokens: None,
            supports_multimodal: None,
            supports_image: None,
            supports_video: None,
            probe_source: None,
            generate_kwargs: serde_json::Map::new(),
        });

        let mut config = self.config.clone();
        super::reasoning::merge_options(&mut config.generate_kwargs, &info.generate_kwargs);
        Ok(Box::new(OllamaChatModel {
            model_info: info,
            transport: self.transport.clone(),
            config,
        }))
    }

    async fn fetch_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        let body = self.transport.get("/api/tags").await?;
        let value: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| LlmError::InvalidResponse {
                provider: self.config.provider_id.clone(),
                message: format!("failed to parse models response: {e}"),
            })?;

        let models = value
            .get("models")
            .and_then(|m| m.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| {
                        let name = m.get("name")?.as_str()?.to_string();
                        Some(ModelInfo {
                            id: name.clone(),
                            name: name.clone(),
                            context_window: None,
                            max_output_tokens: None,
                            supports_multimodal: None,
                            supports_image: None,
                            supports_video: None,
                            probe_source: Some("ollama-api".into()),
                            generate_kwargs: serde_json::Map::new(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(models)
    }

    async fn check_connection(&self) -> Result<ConnectionStatus, LlmError> {
        match self.transport.get("/api/tags").await {
            Ok(_) => Ok(ConnectionStatus::Connected),
            Err(e) => Ok(ConnectionStatus::Failed(e.to_string())),
        }
    }
}

struct OllamaChatModel {
    model_info: ModelInfo,
    transport: Arc<HttpTransport>,
    config: OllamaConfig,
}

#[async_trait]
impl ChatModel for OllamaChatModel {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let model = request.model.as_deref().unwrap_or(&self.model_info.id);
        let provider = OllamaProvider {
            config: self.config.clone(),
            transport: self.transport.clone(),
            info: ProviderInfo {
                id: self.config.provider_id.clone(),
                name: self.config.provider_name.clone(),
                provider_type: Some("OllamaChatModel".into()),
                is_local: true,
                support_model_discovery: false,
                support_connection_check: false,
            },
        };

        let body = provider.build_request_body(&request, model);
        crate::debug_capture::record_provider_request(
            &request.metadata,
            &self.config.provider_id,
            "/api/chat",
            &body,
        );
        let response_text = self.transport.post("/api/chat", &body).await?;

        let value: serde_json::Value =
            serde_json::from_str(&response_text).map_err(|e| LlmError::InvalidResponse {
                provider: self.config.provider_id.clone(),
                message: format!("failed to parse response: {e}"),
            })?;

        let msg = value.get("message");

        let mut content_parts = Vec::new();

        if let Some(thinking) = msg.and_then(|m| m.get("thinking")).and_then(|t| t.as_str()) {
            if !thinking.is_empty() {
                content_parts.push(ContentPart::Thinking {
                    text: thinking.to_string(),
                    signature: None,
                });
            }
        }

        let content_text = msg
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or("");
        if !content_text.is_empty() {
            content_parts.push(ContentPart::Text {
                text: content_text.to_string(),
            });
        }

        if let Some(tool_calls) = msg
            .and_then(|m| m.get("tool_calls"))
            .and_then(|t| t.as_array())
        {
            for (idx, tc) in tool_calls.iter().enumerate() {
                if let Some(function) = tc.get("function") {
                    let name = function
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string();
                    let arguments = function
                        .get("arguments")
                        .cloned()
                        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                    let raw_arguments = Some(arguments.to_string());
                    let tool_id = format!("{idx}_{name}");

                    content_parts.push(ContentPart::ToolUse {
                        call: ToolCall {
                            id: tool_id,
                            name,
                            arguments,
                            raw_arguments,
                            provider_call_id: None,
                            metadata: serde_json::Map::new(),
                        },
                    });
                }
            }
        }

        if content_parts.is_empty() {
            content_parts.push(ContentPart::Text {
                text: String::new(),
            });
        }

        let usage = {
            let prompt = value
                .get("prompt_eval_count")
                .and_then(|t| t.as_u64())
                .unwrap_or(0);
            let completion = value
                .get("eval_count")
                .and_then(|t| t.as_u64())
                .unwrap_or(0);
            if prompt > 0 || completion > 0 {
                Some(TokenUsage::new(prompt, completion))
            } else {
                None
            }
        };

        let message = ModelDialogueItem {
            id: None,
            role: Role::Assistant,
            content: content_parts,
            name: None,
            created_at: None,
            metadata: serde_json::Map::new(),
        };

        Ok(ChatResponse {
            message,
            usage,
            model: Some(model.to_string()),
            finish_reason: FinishReason::Stop,
            metadata: serde_json::Map::new(),
            extra: Default::default(),
        })
    }

    fn chat_stream(&self, request: ChatRequest) -> EventStream {
        let model = request
            .model
            .clone()
            .unwrap_or_else(|| self.model_info.id.clone());
        let provider = OllamaProvider {
            config: self.config.clone(),
            transport: self.transport.clone(),
            info: ProviderInfo {
                id: self.config.provider_id.clone(),
                name: self.config.provider_name.clone(),
                provider_type: Some("OllamaChatModel".into()),
                is_local: true,
                support_model_discovery: false,
                support_connection_check: false,
            },
        };

        let mut req = request;
        req.stream = true;
        let body = provider.build_request_body(&req, &model);
        crate::debug_capture::record_provider_request(
            &req.metadata,
            &self.config.provider_id,
            "/api/chat",
            &body,
        );
        let transport = self.transport.clone();
        let provider_id = self.config.provider_id.clone();
        let run_id = uuid::Uuid::new_v4().to_string();

        let stream = async_stream::stream! {
            yield Ok(AgentEvent::Start {
                run_id: run_id.clone(),
                model: Some(model.clone()),
                metadata: serde_json::Value::Null,
            });

            let byte_stream = match transport.post_stream("/api/chat", &body).await {
                Ok(s) => s,
                Err(e) => {
                    yield Err(e);
                    return;
                }
            };

            let mut buffer = String::new();
            let mut final_finish_reason: Option<FinishReason> = None;
            let mut completed = false;

            tokio::pin!(byte_stream);

            'stream: while let Some(result) = byte_stream.next().await {
                match result {
                    Ok(bytes) => {
                        buffer.push_str(&String::from_utf8_lossy(&bytes));


                        while let Some(newline_pos) = buffer.find('\n') {
                            let line = buffer[..newline_pos].trim().to_string();
                            buffer = buffer[newline_pos + 1..].to_string();

                            if line.is_empty() {
                                continue;
                            }

                            match parse_ollama_chunk(&line) {
                                Ok((events, finish)) => {
                                    if let Some(fr) = finish {
                                        final_finish_reason = Some(fr);
                                        completed = true;
                                    }
                                    let terminal_error = events
                                        .iter()
                                        .any(|event| matches!(event, AgentEvent::Error { .. }));
                                    for event in events {
                                        yield Ok(event);
                                    }
                                    if terminal_error {
                                        return;
                                    }
                                    if completed {
                                        break 'stream;
                                    }
                                }
                                Err(e) => {
                                    warn!(provider = %provider_id, error = %e, "failed to parse Ollama chunk");
                                }
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
                    message: "Ollama stream closed before done=true".into(),
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

fn extract_ollama_images(content: &[ContentPart]) -> Vec<String> {
    let mut images = Vec::new();
    for part in content {
        if let ContentPart::Image { url, .. } = part {
            if url.starts_with("data:") {
                if let Some(comma) = url.find(',') {
                    images.push(url[comma + 1..].to_string());
                }
            } else if url.starts_with("http://") || url.starts_with("https://") {
                warn!(
                    "Ollama does not support image URLs directly, skipping: {}",
                    url
                );
            } else {
                let path = url.strip_prefix("file://").unwrap_or(url);
                match std::fs::read(path) {
                    Ok(bytes) => {
                        images.push(base64::engine::general_purpose::STANDARD.encode(&bytes));
                    }
                    Err(e) => {
                        warn!("Failed to read image file {}: {}", path, e);
                    }
                }
            }
        }
    }
    images
}

fn content_text_for_ollama(content: &[ContentPart]) -> String {
    let mut text = String::new();
    for part in content {
        match part {
            ContentPart::Text { text: part_text } => text.push_str(part_text),
            ContentPart::ToolResult { result } => text.push_str(&result.text_content()),
            _ => {}
        }
    }
    text
}
