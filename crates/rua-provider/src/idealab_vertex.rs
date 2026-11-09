use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use tracing::warn;

use agent_types::{AgentEvent, ChatRequest, ChatResponse, FinishReason};
use llm_multiplex::error::LlmError;
use llm_multiplex::event_parser::parse_gemini_chunk;
use llm_multiplex::providers::gemini::{
    merge_function_response_contents, GeminiConfig, GeminiProvider,
};
use llm_multiplex::traits::{
    ChatModel, ConnectionStatus, EventStream, ModelInfo, Provider, ProviderInfo,
};
use llm_multiplex::transport::{parse_sse_lines, HttpTransport, TransportConfig};

use crate::idealab::{is_valid_base64, strip_anyof_null};

pub struct IdealabVertexProvider {
    gemini: GeminiProvider,
    transport: Arc<HttpTransport>,
    config: GeminiConfig,
    info: ProviderInfo,
}

impl IdealabVertexProvider {
    pub fn new(config: GeminiConfig) -> Result<Self, LlmError> {
        let is_custom = !config.base_url.is_empty() && !config.base_url.contains("googleapis.com");
        let mut tc = TransportConfig::new(&config.base_url, &config.provider_id);
        if let Some(ref key) = config.api_key {
            if is_custom {
                tc = tc.with_api_key(key.clone());
            }
        }
        tc = tc
            .with_timeout(std::time::Duration::from_secs(config.timeout_secs))
            .with_headers_from_map(&config.headers);
        let transport = Arc::new(HttpTransport::new(tc)?);

        let gemini = GeminiProvider::new(config.clone())?;

        let info = ProviderInfo {
            id: config.provider_id.clone(),
            name: config.provider_name.clone(),
            provider_type: Some("IdealabVertex".into()),
            is_local: false,
            support_model_discovery: false,
            support_connection_check: true,
        };

        Ok(Self {
            gemini,
            transport,
            config,
            info,
        })
    }
}

#[async_trait]
impl Provider for IdealabVertexProvider {
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
            context_window: Some(1_048_576),
            max_output_tokens: None,
            supports_multimodal: Some(true),
            supports_image: Some(true),
            supports_video: Some(true),
            probe_source: None,
            generate_kwargs: serde_json::Map::new(),
        });

        let mut config = self.config.clone();
        crate::manager::deep_merge_maps(&mut config.generate_kwargs, &info.generate_kwargs);
        let gemini = GeminiProvider::new(config.clone())?;
        Ok(Box::new(IdealabVertexChatModel {
            model_info: info,
            transport: self.transport.clone(),
            config,
            gemini,
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
            .unwrap_or("gemini-3.1-pro-preview");
        let endpoint = self.gemini.get_endpoint(model_id, false);
        let body = serde_json::json!({
            "contents": [{"role": "user", "parts": [{"text": "ping"}]}],
            "generationConfig": {"maxOutputTokens": 1}
        });
        match self.transport.post(&endpoint, &body).await {
            Ok(_) => Ok(ConnectionStatus::Connected),
            Err(e) => Ok(ConnectionStatus::Failed(format!("Idealab Vertex: {e}"))),
        }
    }
}

fn patch_strip_function_response_id(body: &mut serde_json::Value) {
    let Some(contents) = body.get_mut("contents").and_then(|c| c.as_array_mut()) else {
        return;
    };
    for msg in contents {
        let Some(parts) = msg.get_mut("parts").and_then(|p| p.as_array_mut()) else {
            continue;
        };
        for part in parts {
            if let Some(fr) = part.get_mut("functionResponse") {
                if let Some(obj) = fr.as_object_mut() {
                    obj.remove("id");
                }
            }
        }
    }
}

fn patch_fix_thought_signature(body: &mut serde_json::Value) {
    let Some(contents) = body.get_mut("contents").and_then(|c| c.as_array_mut()) else {
        return;
    };
    for msg in contents {
        let Some(parts) = msg.get_mut("parts").and_then(|p| p.as_array_mut()) else {
            continue;
        };
        for part in parts.iter_mut() {
            let Some(obj) = part.as_object_mut() else {
                continue;
            };
            if !obj.contains_key("thoughtSignature") {
                continue;
            }
            let ts = obj.get("thoughtSignature").cloned();
            match ts {
                Some(serde_json::Value::String(s)) => {
                    if !is_valid_base64(&s) {
                        obj.remove("thoughtSignature");
                    }
                }
                Some(serde_json::Value::Null) => {
                    obj.remove("thoughtSignature");
                }
                _ => {
                    obj.remove("thoughtSignature");
                }
            }
        }
    }
}

fn patch_clean_function_call_parts(body: &mut serde_json::Value) {
    let Some(contents) = body.get_mut("contents").and_then(|c| c.as_array_mut()) else {
        return;
    };
    for msg in contents {
        let Some(parts) = msg.get_mut("parts").and_then(|p| p.as_array_mut()) else {
            continue;
        };
        for part in parts.iter_mut() {
            let Some(obj) = part.as_object_mut() else {
                continue;
            };
            if let Some(fc) = obj.get_mut("functionCall") {
                if let Some(fc_obj) = fc.as_object_mut() {
                    fc_obj.remove("id");
                }
            }
            if obj.get("thoughtSignature") == Some(&serde_json::Value::Null) {
                obj.remove("thoughtSignature");
            }
        }
    }
}

fn patch_tool_schemas(body: &mut serde_json::Value) {
    let Some(tools) = body.get_mut("tools").and_then(|t| t.as_array_mut()) else {
        return;
    };
    for tool_group in tools {
        let Some(decls) = tool_group
            .get_mut("functionDeclarations")
            .and_then(|d| d.as_array_mut())
        else {
            continue;
        };
        for decl in decls {
            if let Some(params) = decl.get_mut("parameters") {
                strip_anyof_null(params);
            }
        }
    }
}

fn patch_filter_null_config(body: &mut serde_json::Value) {
    if let Some(gen_config) = body.get_mut("generationConfig") {
        if let Some(obj) = gen_config.as_object_mut() {
            obj.retain(|_, v| !v.is_null());
        }
    }
}

pub fn apply_idealab_vertex_patches(body: &mut serde_json::Value) {
    patch_strip_function_response_id(body);
    patch_fix_thought_signature(body);
    merge_function_response_contents(body);

    patch_clean_function_call_parts(body);
    patch_tool_schemas(body);
    patch_filter_null_config(body);
}

struct IdealabVertexChatModel {
    model_info: ModelInfo,
    transport: Arc<HttpTransport>,
    config: GeminiConfig,
    gemini: GeminiProvider,
}

#[async_trait]
impl ChatModel for IdealabVertexChatModel {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let model = request.model.as_deref().unwrap_or(&self.model_info.id);
        let endpoint = self.gemini.get_endpoint(model, false);
        let mut body = self.gemini.build_request_body(&request, model);
        apply_idealab_vertex_patches(&mut body);

        let response_text = self.transport.post(&endpoint, &body).await?;
        let value: serde_json::Value =
            serde_json::from_str(&response_text).map_err(|e| LlmError::InvalidResponse {
                provider: self.config.provider_id.clone(),
                message: format!("failed to parse response: {e}"),
            })?;

        parse_gemini_response(&value, model)
    }

    fn chat_stream(&self, request: ChatRequest) -> EventStream {
        let model = request
            .model
            .clone()
            .unwrap_or_else(|| self.model_info.id.clone());
        let endpoint = self.gemini.get_endpoint(&model, true);
        let mut body = self.gemini.build_request_body(&request, &model);
        apply_idealab_vertex_patches(&mut body);

        let transport = self.transport.clone();
        let provider_id = self.config.provider_id.clone();
        let run_id = uuid::Uuid::new_v4().to_string();

        let stream = async_stream::stream! {
            yield Ok(AgentEvent::Start {
                run_id: run_id.clone(),
                model: Some(model.clone()),
                metadata: serde_json::Value::Null,
            });

            let byte_stream = match transport.post_stream(&endpoint, &body).await {
                Ok(s) => s,
                Err(e) => {
                    yield Err(e);
                    return;
                }
            };

            let mut buffer = String::new();
            let mut final_finish_reason: Option<FinishReason> = None;
            let mut emitted_content = false;
            let mut completed = false;

            tokio::pin!(byte_stream);

            'stream: while let Some(result) = byte_stream.next().await {
                match result {
                    Ok(bytes) => {
                        buffer.push_str(&String::from_utf8_lossy(&bytes));
                        let (lines, remaining) = parse_sse_lines(&buffer);
                        buffer = remaining;

                        for line in lines {
                            if line == "[DONE]" {
                                completed = true;
                                break 'stream;
                            }
                            match parse_gemini_chunk(&line) {
                                Ok((events, finish)) => {
                                    let stream_finished = finish.is_some();
                                    if let Some(fr) = finish {
                                        final_finish_reason = Some(fr);
                                    }
                                    let terminal_error = events
                                        .iter()
                                        .any(|event| matches!(event, AgentEvent::Error { .. }));
                                    for event in events {
                                        emitted_content |= event.is_content();
                                        yield Ok(event);
                                    }
                                    if terminal_error {
                                        return;
                                    }
                                    if stream_finished {
                                        completed = true;
                                        break 'stream;
                                    }
                                }
                                Err(e) => {
                                    let msg = e.to_string();
                                    if msg.contains("[DONE]")
                                        || msg.contains("data:")
                                        || msg.contains("Failed to parse")
                                    {
                                        continue;
                                    }
                                    warn!(
                                        provider = %provider_id,
                                        error = %e,
                                        "failed to parse Idealab Vertex chunk"
                                    );
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
                    message: "Gemini stream closed before finishReason/[DONE]".into(),
                });
                return;
            }

            if !emitted_content {
                yield Ok(AgentEvent::Error {
                    code: Some("empty_response".into()),
                    message: format!(
                        "Idealab Gemini model '{model}' returned no text or tool output; image generation output is not supported by the current chat adapter"
                    ),
                    retryable: false,
                    metadata: serde_json::Value::Null,
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

fn parse_gemini_response(value: &serde_json::Value, model: &str) -> Result<ChatResponse, LlmError> {
    use agent_types::{ContentPart, ModelDialogueItem, Role, TokenUsage, ToolCall};

    let mut content = Vec::new();

    if let Some(candidates) = value.get("candidates").and_then(|c| c.as_array()) {
        if let Some(candidate) = candidates.first() {
            if let Some(parts) = candidate
                .get("content")
                .and_then(|c| c.get("parts"))
                .and_then(|p| p.as_array())
            {
                for part in parts {
                    if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                        let is_thought = part
                            .get("thought")
                            .and_then(|t| t.as_bool())
                            .unwrap_or(false);
                        if is_thought {
                            content.push(ContentPart::Thinking {
                                text: text.to_string(),
                                signature: None,
                            });
                        } else {
                            content.push(ContentPart::Text {
                                text: text.to_string(),
                            });
                        }
                    }
                    if let Some(fc) = part.get("functionCall") {
                        let name = fc
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("")
                            .to_string();
                        let args = fc.get("args").cloned().unwrap_or(serde_json::Value::Null);
                        let provider_call_id =
                            fc.get("id").and_then(|id| id.as_str()).map(str::to_string);
                        let call_id = provider_call_id
                            .clone()
                            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                        let mut metadata = serde_json::Map::new();
                        if let Some(signature) = part
                            .get("thoughtSignature")
                            .and_then(|signature| signature.as_str())
                        {
                            metadata.insert(
                                "extra_content".into(),
                                serde_json::json!({"thought_signature": signature}),
                            );
                        }
                        content.push(ContentPart::ToolUse {
                            call: ToolCall {
                                id: call_id,
                                name,
                                arguments: args,
                                raw_arguments: None,
                                provider_call_id,
                                metadata,
                            },
                        });
                    }
                }
            }
        }
    }

    if content.is_empty() {
        content.push(ContentPart::Text {
            text: String::new(),
        });
    }

    let finish_reason = value
        .get("candidates")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .and_then(|c| c.get("finishReason"))
        .and_then(|r| r.as_str())
        .map(FinishReason::from_provider)
        .unwrap_or(FinishReason::Stop);

    let usage = value.get("usageMetadata").map(|u| TokenUsage {
        prompt_tokens: u
            .get("promptTokenCount")
            .and_then(|t| t.as_u64())
            .unwrap_or(0),
        completion_tokens: u
            .get("candidatesTokenCount")
            .and_then(|t| t.as_u64())
            .unwrap_or(0),
        reasoning_tokens: None,
        cached_tokens: u.get("cachedContentTokenCount").and_then(|t| t.as_u64()),
        cache_write_tokens: None,
        total_tokens: u.get("totalTokenCount").and_then(|t| t.as_u64()),
    });

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
        model: Some(model.to_string()),
        finish_reason,
        metadata: serde_json::Map::new(),
        extra: Default::default(),
    })
}
