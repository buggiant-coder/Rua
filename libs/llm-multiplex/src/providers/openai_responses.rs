use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use sha2::{Digest, Sha256};
use tracing::warn;

use agent_types::model_dialogue_item::{
    AssistantMessagePart, ASSISTANT_MESSAGE_PARTS_METADATA_KEY, ASSISTANT_PHASE_METADATA_KEY,
    PROVIDER_MESSAGE_ID_METADATA_KEY,
};
use agent_types::{
    AgentEvent, ChatRequest, ChatResponse, ContentPart, FinishReason, ModelDialogueItem, Role,
    TokenUsage, ToolCall,
};

use crate::error::{is_retryable_provider_error, LlmError};
use crate::key_selector::KeySelector;
use crate::providers::openai_compatible::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};
use crate::traits::{ChatModel, ConnectionStatus, EventStream, ModelInfo, Provider, ProviderInfo};
use crate::transport::{parse_sse_lines, HttpTransport, TransportConfig};

pub const RESPONSES_SESSION_METADATA_KEY: &str = "rua_responses_session";
const RESPONSES_SESSION_MAX_AGE_MS: u64 = 6 * 24 * 60 * 60 * 1_000;
const RESPONSES_SESSION_RETRY_DELAYS: [std::time::Duration; 4] = [
    std::time::Duration::from_millis(250),
    std::time::Duration::from_millis(500),
    std::time::Duration::from_secs(1),
    std::time::Duration::from_secs(2),
];

pub struct OpenAiResponsesProvider {
    config: OpenAiCompatibleConfig,
    transports: Vec<Arc<HttpTransport>>,
    key_selector: Arc<KeySelector>,
    info: ProviderInfo,
}

impl OpenAiResponsesProvider {
    pub fn new(config: OpenAiCompatibleConfig) -> Result<Self, LlmError> {
        let effective_keys: Vec<String> = if !config.api_keys.is_empty() {
            config.api_keys.clone()
        } else if let Some(ref key) = config.api_key {
            vec![key.clone()]
        } else {
            vec![]
        };

        let transports: Vec<Arc<HttpTransport>> = if effective_keys.is_empty() {
            let tc = TransportConfig::new(&config.base_url, &config.provider_id)
                .with_timeout(std::time::Duration::from_secs(config.timeout_secs))
                .with_headers_from_map(&config.headers);
            vec![Arc::new(HttpTransport::new(tc)?)]
        } else {
            effective_keys
                .iter()
                .map(|key| {
                    let tc = TransportConfig::new(&config.base_url, &config.provider_id)
                        .with_api_key(key.clone())
                        .with_timeout(std::time::Duration::from_secs(config.timeout_secs))
                        .with_headers_from_map(&config.headers);
                    HttpTransport::new(tc).map(Arc::new)
                })
                .collect::<Result<Vec<_>, _>>()?
        };

        let info = ProviderInfo {
            id: config.provider_id.clone(),
            name: config.provider_name.clone(),
            provider_type: Some("OpenAIResponseModel".into()),
            is_local: config.is_local,
            support_model_discovery: config.support_model_discovery,
            support_connection_check: config.support_connection_check,
        };

        Ok(Self {
            key_selector: Arc::new(KeySelector::new(transports.len())),
            config,
            transports,
            info,
        })
    }

    pub fn build_request_body(&self, request: &ChatRequest) -> serde_json::Value {
        self.prepare_request(request).0
    }

    fn prepare_request(&self, request: &ChatRequest) -> (serde_json::Value, String) {
        if self.config.provider_id == "dashscope" {
            if let Some((session, prefix)) = self.matching_session(request) {
                if session.message_index + 1 < request.messages.len() {
                    let mut body = self.build_request_body_with_messages(
                        request,
                        &request.messages[session.message_index..],
                    );
                    // The matching prefix is already normalized and hashed. Extend
                    // it with the complete anchor + tail before removing the anchor
                    // from the incremental wire request. Never reuse a hash across calls.
                    let fingerprint = if let Some(mut prefix) = prefix {
                        prefix.extend(&body["input"]);
                        prefix.finish()
                    } else {
                        self.input_fingerprint(request, &request.messages)
                    };
                    Self::remove_anchor_items(&mut body);
                    body["store"] = serde_json::Value::Bool(true);
                    body["previous_response_id"] = serde_json::Value::String(session.response_id);
                    return (body, fingerprint);
                }
            }
        }
        let mut body = self.build_full_request_body(request);
        if self.config.provider_id == "dashscope" {
            body["store"] = serde_json::Value::Bool(true);
        }
        let fingerprint = if self.config.provider_id == "dashscope" {
            format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&body["input"]).unwrap_or_default())
            )
        } else {
            // Only DashScope persists a Responses session fingerprint.
            String::new()
        };
        (body, fingerprint)
    }

    fn remove_anchor_items(body: &mut serde_json::Value) {
        if let Some(input) = body
            .get_mut("input")
            .and_then(serde_json::Value::as_array_mut)
        {
            let anchor_items = input
                .iter()
                .take_while(|item| {
                    item.get("role").and_then(serde_json::Value::as_str) == Some("assistant")
                        || item.get("type").and_then(serde_json::Value::as_str)
                            == Some("function_call")
                })
                .count();
            input.drain(..anchor_items);
        }
    }

    fn build_full_request_body(&self, request: &ChatRequest) -> serde_json::Value {
        self.build_request_body_with_messages(request, &request.messages)
    }

    fn build_request_body_with_messages(
        &self,
        request: &ChatRequest,
        messages: &[ModelDialogueItem],
    ) -> serde_json::Value {
        // Protocol conversion borrows the selected history and does not construct HTTP clients.
        let mut chat_body = OpenAiCompatibleProvider::build_request_body_with_messages(
            &self.config,
            request,
            messages,
            None,
            true,
        );
        let mut body = serde_json::json!({
            "input": normalize_responses_input(
                chat_body
                    .as_object_mut()
                    .and_then(|body| body.remove("messages"))
                    .unwrap_or_else(|| serde_json::json!([])),
            ),
            "stream": request.stream,
        });

        if let Some(model) = chat_body.get("model").cloned() {
            body["model"] = model;
        }
        if let Some(temp) = chat_body.get("temperature").cloned() {
            body["temperature"] = temp;
        }
        if let Some(max_tokens) = chat_body
            .get("max_completion_tokens")
            .or_else(|| chat_body.get("max_tokens"))
            .cloned()
        {
            body["max_output_tokens"] = max_tokens;
        }
        if let Some(tools) = chat_body.get("tools").cloned() {
            body["tools"] = flatten_responses_tools(tools);
        }
        if let Some(tool_choice) = chat_body.get("tool_choice").cloned() {
            body["tool_choice"] = flatten_responses_tool_choice(tool_choice);
        }

        if let Some(obj) = body.as_object_mut() {
            let mut defaults = self.config.generate_kwargs.clone();
            normalize_responses_options(&mut defaults);
            super::reasoning::apply_defaults(obj, &defaults);
            let mut options = request.provider_options.clone();
            normalize_responses_options(&mut options);
            super::reasoning::merge_options(obj, &options);
            apply_responses_reasoning(obj, request);
        }
        body
    }

    fn response_session_metadata(
        &self,
        request: &ChatRequest,
        response_id: &str,
        request_input_sha256: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "provider_id": self.config.provider_id,
            "model": request.model,
            "response_id": response_id,
            "request_input_sha256": request_input_sha256,
            "created_at_unix_ms": unix_time_ms(),
        })
    }

    fn matching_session(
        &self,
        request: &ChatRequest,
    ) -> Option<(MatchingSession, Option<InputFingerprint>)> {
        let model = request.model.as_deref()?;
        let now = unix_time_ms();
        let mut candidates = request
            .messages
            .iter()
            .enumerate()
            .filter_map(|(message_index, message)| {
                if message.role != Role::Assistant {
                    return None;
                }
                let session = message.metadata.get(RESPONSES_SESSION_METADATA_KEY)?;
                if session.get("provider_id")?.as_str()? != self.config.provider_id
                    || session.get("model")?.as_str()? != model
                {
                    return None;
                }
                let created_at = session.get("created_at_unix_ms")?.as_u64()?;
                if now.saturating_sub(created_at) > RESPONSES_SESSION_MAX_AGE_MS {
                    return None;
                }
                Some((
                    MatchingSession {
                        message_index,
                        response_id: session.get("response_id")?.as_str()?.to_string(),
                    },
                    session.get("request_input_sha256")?.as_str()?,
                ))
            })
            .collect::<Vec<_>>();
        let (latest, expected_hash) = candidates.pop()?;
        // Normalization is independent across Assistant boundaries without Tool Results.
        // Hash each wire item once, rather than re-encoding the entire long prefix for
        // every older session. Unusual input overrides retain the exact general path.
        let independent_input = !model.to_ascii_lowercase().contains("omni")
            && ["messages", "input"].iter().all(|key| {
                !request.provider_options.contains_key(*key)
                    && !self.config.generate_kwargs.contains_key(*key)
            });
        let independent_boundary = |index: usize| {
            !request.messages[index]
                .content
                .iter()
                .any(|part| matches!(part, ContentPart::ToolResult { .. }))
        };
        let prefix = independent_input.then(|| {
            let input = normalize_responses_input(serde_json::Value::Array(
                OpenAiCompatibleProvider::convert_responses_messages(
                    &request.messages[..latest.message_index],
                ),
            ));
            let mut hash = InputFingerprint::new();
            hash.extend(&input);
            hash
        });
        let actual = prefix
            .as_ref()
            .map(|hash| hash.clone().finish())
            .unwrap_or_else(|| {
                self.input_fingerprint(request, &request.messages[..latest.message_index])
            });
        if actual == expected_hash {
            let prefix = prefix.filter(|_| independent_boundary(latest.message_index));
            return Some((latest, prefix));
        }
        let independent_prefixes = independent_input
            && candidates
                .iter()
                .all(|(candidate, _)| independent_boundary(candidate.message_index));
        if !independent_prefixes {
            return candidates
                .into_iter()
                .rev()
                .find_map(|(candidate, expected)| {
                    (self.input_fingerprint(request, &request.messages[..candidate.message_index])
                        == expected)
                        .then_some((candidate, None))
                });
        }
        let mut hash = InputFingerprint::new();
        let mut start = 0;
        let mut matching = None;
        for (candidate, expected) in candidates {
            let input = normalize_responses_input(serde_json::Value::Array(
                OpenAiCompatibleProvider::convert_responses_messages(
                    &request.messages[start..candidate.message_index],
                ),
            ));
            hash.extend(&input);
            start = candidate.message_index;
            if hash.clone().finish() == expected {
                matching = Some((candidate, Some(hash.clone())));
            }
        }
        matching
    }

    fn input_fingerprint(&self, request: &ChatRequest, messages: &[ModelDialogueItem]) -> String {
        let mut body = self.build_request_body_with_messages(request, messages);
        let input = body
            .as_object_mut()
            .and_then(|body| body.remove("input"))
            .unwrap_or(serde_json::Value::Null);
        let bytes = serde_json::to_vec(&input).unwrap_or_default();
        format!("{:x}", Sha256::digest(bytes))
    }
}

struct MatchingSession {
    message_index: usize,
    response_id: String,
}

/// SHA of exactly the existing JSON input array, before its closing bracket.
#[derive(Clone)]
struct InputFingerprint {
    hash: Sha256,
    has_items: bool,
}

impl InputFingerprint {
    fn new() -> Self {
        let mut hash = Sha256::new();
        hash.update(b"[");
        Self {
            hash,
            has_items: false,
        }
    }

    fn extend(&mut self, input: &serde_json::Value) {
        for item in input.as_array().into_iter().flatten() {
            if self.has_items {
                self.hash.update(b",");
            }
            self.hash
                .update(serde_json::to_vec(item).unwrap_or_default());
            self.has_items = true;
        }
    }

    fn finish(mut self) -> String {
        self.hash.update(b"]");
        format!("{:x}", self.hash.finalize())
    }
}

fn unix_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn invalid_previous_response_id(error: &LlmError) -> bool {
    if matches!(
        error,
        LlmError::Unknown { message, .. } if message.starts_with("HTTP 400:")
    ) {
        return true;
    }
    let message = error.to_string().to_ascii_lowercase();
    message.contains("previous_response_id")
        && [
            "invalid",
            "expired",
            "not found",
            "does not exist",
            "不存在",
            "已过期",
            "无效",
        ]
        .iter()
        .any(|signal| message.contains(signal))
}

fn normalize_responses_options(options: &mut serde_json::Map<String, serde_json::Value>) {
    super::reasoning::flatten_extra_body(options);
    super::reasoning::normalize_output_limit(options, "max_output_tokens");
    if let Some(effort) = options.remove("reasoning_effort") {
        let reasoning = options
            .entry("reasoning")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(reasoning) = reasoning.as_object_mut() {
            reasoning.entry("effort").or_insert(effort);
        }
    }
}

fn apply_responses_reasoning(
    body: &mut serde_json::Map<String, serde_json::Value>,
    request: &ChatRequest,
) {
    let Some(reasoning) = request.reasoning.as_ref() else {
        if body
            .remove("disable_thinking")
            .and_then(|value| value.as_bool())
            == Some(true)
        {
            super::reasoning::clear_controls(body);
            body.insert(
                "reasoning".into(),
                serde_json::json!({
                    "effort": super::reasoning::off_effort(request, "low")
                }),
            );
            return;
        }
        if let Some(effort) = body.remove("reasoning_effort") {
            body.entry("reasoning")
                .or_insert(serde_json::json!({"effort": effort}));
        }
        return;
    };
    let mut value = body
        .get("reasoning")
        .and_then(|value| value.as_object())
        .cloned()
        .unwrap_or_default();
    value.remove("effort");
    super::reasoning::clear_controls(body);
    if !reasoning.enabled {
        value.remove("summary");
        value.insert(
            "effort".into(),
            serde_json::json!(super::reasoning::off_effort(request, "low")),
        );
    } else if let Some(effort) = reasoning.effort.as_deref() {
        value.insert("effort".into(), serde_json::json!(effort));
    }
    if reasoning.enabled {
        if let Some(summary) = reasoning.summary {
            value.insert("summary".into(), serde_json::json!(summary.as_str()));
        }
    }
    if value.is_empty() {
        body.remove("reasoning");
    } else {
        body.insert("reasoning".into(), serde_json::Value::Object(value));
    }
}

fn normalize_responses_input(messages: serde_json::Value) -> serde_json::Value {
    let serde_json::Value::Array(messages) = messages else {
        return messages;
    };
    let mut input = Vec::new();

    for mut message in messages {
        let role = message
            .get("role")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if role == "tool" {
            let Some(call_id) = message
                .get("tool_call_id")
                .and_then(serde_json::Value::as_str)
            else {
                continue;
            };
            let output = match message.get("content") {
                Some(serde_json::Value::String(output)) => output.clone(),
                Some(output) => output.to_string(),
                None => String::new(),
            };
            input.push(serde_json::json!({
                "type": "function_call_output",
                "call_id": call_id,
                "output": output,
            }));
            continue;
        }

        let assistant_role =
            message.get("role").and_then(serde_json::Value::as_str) == Some("assistant");
        if let Some(serde_json::Value::Array(parts)) = message.get_mut("content") {
            for part in parts {
                let serde_json::Value::Object(part) = part else {
                    continue;
                };
                match part
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                {
                    "text" => {
                        part.insert(
                            "type".into(),
                            serde_json::json!(if assistant_role {
                                "output_text"
                            } else {
                                "input_text"
                            }),
                        );
                    }
                    "image_url" => {
                        let image_url = part
                            .remove("image_url")
                            .and_then(|value| {
                                value.as_str().map(str::to_string).or_else(|| {
                                    value
                                        .get("url")
                                        .and_then(serde_json::Value::as_str)
                                        .map(str::to_string)
                                })
                            })
                            .unwrap_or_default();
                        part.clear();
                        part.insert("type".into(), serde_json::json!("input_image"));
                        part.insert("image_url".into(), serde_json::json!(image_url));
                    }
                    _ => {}
                }
            }
        }

        let tool_calls = message
            .as_object_mut()
            .and_then(|message| message.remove("tool_calls"))
            .and_then(|tool_calls| tool_calls.as_array().cloned())
            .unwrap_or_default();
        if let Some(message) = message.as_object_mut() {
            message.remove("reasoning_content");
        }

        if message.get("content").is_some() {
            input.push(message);
        }

        for tool_call in tool_calls {
            let Some(call_id) = tool_call.get("id").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let Some(function) = tool_call.get("function") else {
                continue;
            };
            let Some(name) = function.get("name").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let arguments = function
                .get("arguments")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("{}");
            input.push(serde_json::json!({
                "type": "function_call",
                "call_id": call_id,
                "name": name,
                "arguments": arguments,
            }));
        }
    }

    serde_json::Value::Array(input)
}

fn flatten_responses_tools(tools: serde_json::Value) -> serde_json::Value {
    let serde_json::Value::Array(tools) = tools else {
        return tools;
    };
    serde_json::Value::Array(
        tools
            .into_iter()
            .map(|tool| {
                let serde_json::Value::Object(mut tool) = tool else {
                    return tool;
                };
                let Some(serde_json::Value::Object(mut function)) = tool.remove("function") else {
                    return serde_json::Value::Object(tool);
                };
                function.insert(
                    "type".into(),
                    tool.remove("type")
                        .unwrap_or_else(|| serde_json::json!("function")),
                );
                serde_json::Value::Object(function)
            })
            .collect(),
    )
}

fn flatten_responses_tool_choice(tool_choice: serde_json::Value) -> serde_json::Value {
    let serde_json::Value::Object(mut choice) = tool_choice else {
        return tool_choice;
    };
    let Some(serde_json::Value::Object(mut function)) = choice.remove("function") else {
        return serde_json::Value::Object(choice);
    };
    function.insert(
        "type".into(),
        choice
            .remove("type")
            .unwrap_or_else(|| serde_json::json!("function")),
    );
    serde_json::Value::Object(function)
}

#[async_trait]
impl Provider for OpenAiResponsesProvider {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    fn supports_model(&self, model: &str) -> bool {
        self.config.models.iter().any(|m| m.id == model)
    }

    fn create_chat_model(&self, model: &str) -> Result<Box<dyn ChatModel>, LlmError> {
        let model_info = self
            .config
            .models
            .iter()
            .find(|m| m.id == model)
            .cloned()
            .unwrap_or(ModelInfo {
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
        super::reasoning::merge_options(&mut config.generate_kwargs, &model_info.generate_kwargs);
        Ok(Box::new(OpenAiResponsesChatModel {
            model_info,
            config,
            transports: self.transports.clone(),
            key_selector: self.key_selector.clone(),
        }))
    }

    async fn fetch_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        if !self.config.support_model_discovery {
            return Ok(self.config.models.clone());
        }
        let compat = OpenAiCompatibleProvider::new(self.config.clone())?;
        compat.fetch_models().await
    }

    async fn check_connection(&self) -> Result<ConnectionStatus, LlmError> {
        let model = self
            .config
            .models
            .first()
            .map(|m| m.id.clone())
            .unwrap_or_else(|| "gpt-4o-mini".to_string());
        let request = ChatRequest::new(vec![agent_types::ModelDialogueItem {
            id: None,
            role: Role::User,
            content: vec![ContentPart::Text {
                text: "ping".into(),
            }],
            name: None,
            created_at: None,
            metadata: serde_json::Map::new(),
        }])
        .with_model(model)
        .with_max_tokens(16);
        let chat_model =
            self.create_chat_model(request.model.as_deref().unwrap_or("gpt-4o-mini"))?;
        match chat_model.chat(request).await {
            Ok(_) => Ok(ConnectionStatus::Connected),
            Err(e) => Ok(ConnectionStatus::Failed(e.to_string())),
        }
    }
}

struct OpenAiResponsesChatModel {
    model_info: ModelInfo,
    config: OpenAiCompatibleConfig,
    transports: Vec<Arc<HttpTransport>>,
    key_selector: Arc<KeySelector>,
}

impl OpenAiResponsesChatModel {
    fn provider(&self) -> OpenAiResponsesProvider {
        OpenAiResponsesProvider {
            config: self.config.clone(),
            transports: self.transports.clone(),
            key_selector: self.key_selector.clone(),
            info: ProviderInfo {
                id: self.config.provider_id.clone(),
                name: self.config.provider_name.clone(),
                provider_type: Some("OpenAIResponseModel".into()),
                is_local: self.config.is_local,
                support_model_discovery: self.config.support_model_discovery,
                support_connection_check: self.config.support_connection_check,
            },
        }
    }
}

#[async_trait]
impl ChatModel for OpenAiResponsesChatModel {
    async fn chat(&self, mut request: ChatRequest) -> Result<ChatResponse, LlmError> {
        request.stream = false;
        if request.model.is_none() {
            request.model = Some(self.model_info.id.clone());
        }

        let provider = self.provider();
        let (body, request_input_sha256) = provider.prepare_request(&request);
        let has_previous_response_id = body.get("previous_response_id").is_some();
        crate::debug_capture::record_provider_request(
            &request.metadata,
            &self.config.provider_id,
            "/responses",
            &body,
        );
        let key_idx = self.key_selector.acquire();
        let transport = &self.transports[key_idx];
        let mut result = transport.post("/responses", &body).await;
        if has_previous_response_id {
            for delay in RESPONSES_SESSION_RETRY_DELAYS {
                let Err(error) = &result else {
                    break;
                };
                if !invalid_previous_response_id(error) {
                    break;
                }
                tokio::time::sleep(delay).await;
                result = transport.post("/responses", &body).await;
            }
        }
        self.key_selector.release(key_idx);
        let response_body = match result {
            Err(error) if has_previous_response_id && invalid_previous_response_id(&error) => {
                let mut fallback_body = provider.build_full_request_body(&request);
                fallback_body["store"] = serde_json::Value::Bool(true);
                crate::debug_capture::record_provider_request(
                    &request.metadata,
                    &self.config.provider_id,
                    "/responses",
                    &fallback_body,
                );
                let key_idx = self.key_selector.acquire();
                let result = self.transports[key_idx]
                    .post("/responses", &fallback_body)
                    .await;
                self.key_selector.release(key_idx);
                result?
            }
            result => result?,
        };
        let value: serde_json::Value =
            serde_json::from_str(&response_body).map_err(|e| LlmError::InvalidResponse {
                provider: self.config.provider_id.clone(),
                message: format!("failed to parse Responses API response: {e}"),
            })?;
        crate::debug_capture::record_provider_response(
            &request.metadata,
            &self.config.provider_id,
            "/responses",
            &value,
        );
        let response_id = value
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let mut response = parse_response_value(value)?;
        if self.config.provider_id == "dashscope" {
            if let Some(response_id) = response_id {
                response.metadata.insert(
                    RESPONSES_SESSION_METADATA_KEY.to_string(),
                    provider.response_session_metadata(
                        &request,
                        &response_id,
                        &request_input_sha256,
                    ),
                );
            }
        }
        Ok(response)
    }

    fn chat_stream(&self, mut request: ChatRequest) -> EventStream {
        let mut trace = crate::debug_capture::CaptureTrace::new(&request.metadata, "provider");
        request.stream = true;
        if request.model.is_none() {
            request.model = Some(self.model_info.id.clone());
        }

        let provider = self.provider();
        let (body, request_input_sha256) = provider.prepare_request(&request);
        trace.mark("body_built");
        let has_previous_response_id = body.get("previous_response_id").is_some();
        trace.set("responses_session_reused", has_previous_response_id);
        trace.set(
            "responses_input_items",
            body.get("input")
                .and_then(serde_json::Value::as_array)
                .map_or(0, Vec::len),
        );
        trace.set("responses_full_fallback_built", false);
        trace.record_request(&self.config.provider_id, "/responses", &body);
        let key_idx = self.key_selector.acquire();
        let transport = self.transports[key_idx].clone();
        let key_selector = self.key_selector.clone();
        let provider_id = self.config.provider_id.clone();
        let model = request.model.clone();
        let run_id = uuid::Uuid::new_v4().to_string();
        let capture_metadata =
            crate::debug_capture::CaptureTarget::from_metadata(&request.metadata)
                .map(|_| request.metadata.clone());

        Box::pin(async_stream::stream! {
            trace.mark("stream_polled");
            yield Ok(AgentEvent::Start {
                run_id: run_id.clone(),
                model: model.clone(),
                metadata: serde_json::Value::Null,
            });
            let mut session_retry_count = 0_u64;
            let byte_stream = loop {
                match transport
                    .post_stream_traced("/responses", &body, &mut trace)
                    .await
                {
                    Ok(stream) => break stream,
                    Err(error)
                        if has_previous_response_id
                            && invalid_previous_response_id(&error)
                            && (session_retry_count as usize)
                                < RESPONSES_SESSION_RETRY_DELAYS.len() =>
                    {
                        let delay =
                            RESPONSES_SESSION_RETRY_DELAYS[session_retry_count as usize];
                        session_retry_count += 1;
                        trace.set("previous_response_id_retry_count", session_retry_count);
                        trace.set("previous_response_id_retry_error", error.to_string());
                        tokio::time::sleep(delay).await;
                    }
                    Err(error)
                        if has_previous_response_id
                            && invalid_previous_response_id(&error) =>
                    {
                        trace.set("previous_response_id_fallback", true);
                        trace.mark("fallback_body_build_started");
                        let mut fallback_body = provider.build_full_request_body(&request);
                        fallback_body["store"] = serde_json::Value::Bool(true);
                        trace.mark("fallback_body_built");
                        trace.set("responses_full_fallback_built", true);
                        trace.record_request(&provider_id, "/responses", &fallback_body);
                        match transport
                            .post_stream_traced("/responses", &fallback_body, &mut trace)
                            .await
                        {
                            Ok(stream) => break stream,
                            Err(e) => {
                                key_selector.release(key_idx);
                                trace.error(&e, "http_request");
                                yield Err(e);
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        key_selector.release(key_idx);
                        trace.error(&e, "http_request");
                        yield Err(e);
                        return;
                    }
                }
            };
            key_selector.release(key_idx);
            // The full logical request is needed only until the fallback decision.
            drop(request);
            drop(provider);
            tokio::pin!(byte_stream);
            let mut buffer = String::new();
            let mut usage = None;
            let mut saw_tool_call = false;
            let mut parse_state = ResponseStreamParseState::default();
            let mut completed = false;
            let mut response_id = None;
            let mut captured_events = capture_metadata.as_ref().map(|_| Vec::new());
            'stream: while let Some(chunk) = byte_stream.next().await {
                match chunk {
                    Ok(bytes) => {
                        trace.received_bytes(bytes.len());
                        trace.checkpoint("first_response_byte");
                        buffer.push_str(&String::from_utf8_lossy(&bytes));
                        let (lines, remaining) = parse_sse_lines(&buffer);
                        buffer = remaining;
                        for line in lines {
                            trace.checkpoint("first_sse_data");
                            if line == "[DONE]" {
                                completed = true;
                                break 'stream;
                            }
                            match serde_json::from_str::<serde_json::Value>(&line) {
                                Ok(value) => {
                                    if let Some(events) = captured_events.as_mut() {
                                        events.push(value.clone());
                                    }
                                    let stream_completed =
                                        value.get("type").and_then(|v| v.as_str())
                                            == Some("response.completed");
                                    if let Some(id) = value
                                        .pointer("/response/id")
                                        .and_then(serde_json::Value::as_str)
                                    {
                                        response_id = Some(id.to_string());
                                    }
                                    if let Some(response) = value.get("response") {
                                        trace.response_metadata(response);
                                    }
                                    let parsed = parse_response_stream_event_with_state(&value, &mut parse_state);
                                    if let Some(parsed_usage) = parsed.usage {
                                        trace.checkpoint("first_usage");
                                        usage = Some(parsed_usage);
                                    }
                                    let is_terminal_error = parsed.terminal_error;
                                    saw_tool_call |= parsed.events.iter().any(|event| {
                                        matches!(
                                            event,
                                            AgentEvent::ToolCallDelta { .. }
                                                | AgentEvent::ToolCall { .. }
                                        )
                                    });
                                    for event in parsed.events {
                                        match &event {
                                            AgentEvent::TextDelta { .. } => {
                                                trace.checkpoint("first_text_delta");
                                            }
                                            AgentEvent::ThinkingDelta { .. } => {
                                                trace.checkpoint("first_thinking_delta");
                                            }
                                            AgentEvent::ToolCallDelta { .. }
                                            | AgentEvent::ToolCall { .. } => {
                                                trace.checkpoint("first_tool_call_delta");
                                            }
                                            _ => {}
                                        }
                                        yield Ok(event);
                                    }
                                    if is_terminal_error {
                                        if let Some(metadata) = capture_metadata.as_ref() {
                                            crate::debug_capture::record_provider_response(
                                                metadata,
                                                &provider_id,
                                                "/responses",
                                                &serde_json::json!({
                                                    "stream": true,
                                                    "completed": false,
                                                    "events": captured_events,
                                                }),
                                            );
                                        }
                                        return;
                                    }
                                    if stream_completed {
                                        completed = true;
                                        break 'stream;
                                    }
                                }
                                Err(e) => warn!(provider = %provider_id, error = %e, "failed to parse Responses API SSE chunk"),
                            }
                        }
                    }
                    Err(e) => {
                        if let Some(metadata) = capture_metadata.as_ref() {
                            crate::debug_capture::record_provider_response(
                                metadata,
                                &provider_id,
                                "/responses",
                                &serde_json::json!({
                                    "stream": true,
                                    "completed": false,
                                    "events": captured_events,
                                    "transport_error": e.to_string(),
                                }),
                            );
                        }
                        trace.error(&e, "stream_read");
                        yield Err(e);
                        return;
                    }
                }
            }
            if let Some(metadata) = capture_metadata.as_ref() {
                crate::debug_capture::record_provider_response(
                    metadata,
                    &provider_id,
                    "/responses",
                    &serde_json::json!({
                        "stream": true,
                        "completed": completed,
                        "events": captured_events,
                    }),
                );
            }
            if !completed {
                let error = LlmError::Connection {
                    provider: provider_id,
                    message: "Responses stream closed before response.completed/[DONE]".into(),
                };
                trace.error(&error, "stream_incomplete");
                yield Err(error);
                return;
            }
            trace.mark("provider_finished");
            trace.finish("completed");
            yield Ok(AgentEvent::End {
                finish_reason: if saw_tool_call {
                    FinishReason::ToolUse
                } else {
                    FinishReason::Stop
                },
                usage,
                metadata: (provider_id == "dashscope")
                    .then_some(response_id)
                    .flatten()
                    .map_or(serde_json::Value::Null, |response_id| {
                        serde_json::json!({
                            RESPONSES_SESSION_METADATA_KEY: {
                                "provider_id": provider_id,
                                "model": model,
                                "response_id": response_id,
                                "request_input_sha256": request_input_sha256,
                                "created_at_unix_ms": unix_time_ms(),
                            }
                        })
                    })
            });
        })
    }

    fn model_info(&self) -> &ModelInfo {
        &self.model_info
    }
}

pub fn parse_response_value(value: serde_json::Value) -> Result<ChatResponse, LlmError> {
    let mut content = Vec::new();
    let mut message_parts = Vec::new();
    if let Some(output) = value.get("output").and_then(|v| v.as_array()) {
        for (index, item) in output.iter().enumerate() {
            match item.get("type").and_then(|v| v.as_str()) {
                Some("message") => {
                    message_parts.push(AssistantMessagePart {
                        text_index: message_parts.len(),
                        metadata: response_message_metadata(index, item),
                    });
                    content.push(ContentPart::Text {
                        text: response_message_text(item),
                    });
                }
                Some("function_call") => {
                    let id = item
                        .get("call_id")
                        .or_else(|| item.get("id"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = item
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let raw_arguments = item
                        .get("arguments")
                        .and_then(|v| v.as_str())
                        .unwrap_or("{}")
                        .to_string();
                    let arguments = serde_json::from_str(&raw_arguments)
                        .unwrap_or_else(|_| serde_json::Value::Object(Default::default()));
                    content.push(ContentPart::ToolUse {
                        call: ToolCall {
                            id: id.clone(),
                            name,
                            arguments,
                            raw_arguments: Some(raw_arguments),
                            provider_call_id: Some(id),
                            metadata: serde_json::Map::new(),
                        },
                    });
                }
                Some("reasoning") => {
                    if let Some(summary) = item.get("summary").and_then(|v| v.as_array()) {
                        for part in summary {
                            if part.get("type").and_then(|v| v.as_str()) == Some("summary_text") {
                                if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                                    content.push(ContentPart::Thinking {
                                        text: text.to_string(),
                                        signature: None,
                                    });
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    if message_parts.is_empty() {
        if let Some(text) = value.get("output_text").and_then(|v| v.as_str()) {
            content.push(ContentPart::Text { text: text.into() });
        }
    }
    if content.is_empty() {
        content.push(ContentPart::Text {
            text: String::new(),
        });
    }
    let mut metadata = serde_json::Map::new();
    if !message_parts.is_empty() {
        metadata.insert(
            ASSISTANT_MESSAGE_PARTS_METADATA_KEY.into(),
            serde_json::to_value(message_parts).expect("assistant message metadata"),
        );
    }
    Ok(ChatResponse {
        message: agent_types::ModelDialogueItem {
            id: None,
            role: Role::Assistant,
            content,
            name: None,
            created_at: None,
            metadata,
        },
        usage: parse_usage(&value),
        model: value
            .get("model")
            .and_then(|v| v.as_str())
            .map(String::from),
        finish_reason: FinishReason::Stop,
        metadata: serde_json::Map::new(),
        extra: Default::default(),
    })
}

fn parse_usage(value: &serde_json::Value) -> Option<TokenUsage> {
    let usage = value.get("usage")?;
    Some(TokenUsage {
        prompt_tokens: usage
            .get("input_tokens")
            .or_else(|| usage.get("prompt_tokens"))?
            .as_u64()?,
        completion_tokens: usage
            .get("output_tokens")
            .or_else(|| usage.get("completion_tokens"))?
            .as_u64()?,
        reasoning_tokens: usage
            .get("output_tokens_details")
            .and_then(|d| d.get("reasoning_tokens"))
            .and_then(|v| v.as_u64()),
        cached_tokens: usage
            .get("input_tokens_details")
            .and_then(|d| d.get("cached_tokens"))
            .and_then(|v| v.as_u64()),
        cache_write_tokens: None,
        total_tokens: usage.get("total_tokens").and_then(|v| v.as_u64()),
    })
}

#[derive(Debug, Default)]
pub struct ParsedResponseStreamEvent {
    pub events: Vec<AgentEvent>,
    pub usage: Option<TokenUsage>,
    pub terminal_error: bool,
}

#[derive(Debug, Default)]
pub struct ResponseStreamParseState {
    tool_argument_lengths: HashMap<String, usize>,
    tool_call_ids_by_item_id: HashMap<String, String>,
    message_metadata: HashMap<usize, serde_json::Map<String, serde_json::Value>>,
    message_text_lengths: HashMap<usize, usize>,
}

pub fn parse_response_stream_event(value: &serde_json::Value) -> ParsedResponseStreamEvent {
    let mut state = ResponseStreamParseState::default();
    parse_response_stream_event_with_state(value, &mut state)
}

pub fn parse_response_stream_event_with_state(
    value: &serde_json::Value,
    state: &mut ResponseStreamParseState,
) -> ParsedResponseStreamEvent {
    let mut parsed = ParsedResponseStreamEvent::default();
    match value.get("type").and_then(|v| v.as_str()) {
        Some("response.output_text.delta") => {
            if let Some(delta) = value.get("delta").and_then(|v| v.as_str()) {
                let index = value
                    .get("output_index")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize;
                let metadata = state
                    .message_metadata
                    .entry(index)
                    .or_insert_with(|| {
                        response_message_metadata(
                            index,
                            &serde_json::json!({"id": value.get("item_id")}),
                        )
                    })
                    .clone();
                *state.message_text_lengths.entry(index).or_default() += delta.len();
                parsed.events.push(AgentEvent::TextDelta {
                    delta: delta.to_string(),
                    metadata: serde_json::Value::Object(metadata),
                });
            }
        }
        Some("response.reasoning_summary_text.delta" | "response.reasoning_text.delta") => {
            if let Some(delta) = value.get("delta").and_then(|v| v.as_str()) {
                parsed.events.push(AgentEvent::ThinkingDelta {
                    delta: delta.to_string(),
                    signature: None,
                    metadata: serde_json::Value::Null,
                });
            }
        }
        Some("response.output_item.added" | "response.output_item.done")
            if value.pointer("/item/type").and_then(|v| v.as_str()) == Some("message") =>
        {
            let index = value
                .get("output_index")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            push_response_message(index, &value["item"], state, &mut parsed);
        }
        Some("response.output_item.added")
            if value.pointer("/item/type").and_then(|v| v.as_str()) == Some("function_call") =>
        {
            let item_id = value
                .pointer("/item/id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let call_id = value
                .pointer("/item/call_id")
                .or_else(|| value.pointer("/item/id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if !item_id.is_empty() && !call_id.is_empty() {
                state
                    .tool_call_ids_by_item_id
                    .insert(item_id.to_string(), call_id.clone());
            }
            let name = value
                .pointer("/item/name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            parsed.events.push(AgentEvent::ToolCallDelta {
                id: call_id,
                index: value
                    .get("output_index")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize,
                name: Some(name),
                arguments_delta: String::new(),
                metadata: serde_json::Value::Null,
            });
        }
        Some("response.function_call_arguments.delta") => {
            let item_id = value.get("item_id").and_then(|v| v.as_str()).unwrap_or("");
            let id = state
                .tool_call_ids_by_item_id
                .get(item_id)
                .cloned()
                .unwrap_or_else(|| item_id.to_string());
            let delta = value
                .get("delta")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let seen = state.tool_argument_lengths.entry(id.clone()).or_default();
            *seen += delta.len();
            parsed.events.push(AgentEvent::ToolCallDelta {
                id,
                index: value
                    .get("output_index")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize,
                name: None,
                arguments_delta: delta,
                metadata: serde_json::Value::Null,
            });
        }
        Some("response.function_call_arguments.done") => {
            let item_id = value.get("item_id").and_then(|v| v.as_str()).unwrap_or("");
            let id = state
                .tool_call_ids_by_item_id
                .get(item_id)
                .cloned()
                .unwrap_or_else(|| item_id.to_string());
            let arguments = value
                .get("arguments")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            push_completed_arguments_delta(value, state, &mut parsed, id, None, arguments);
        }
        Some("response.output_item.done")
            if value.pointer("/item/type").and_then(|v| v.as_str()) == Some("function_call") =>
        {
            let item_id = value
                .pointer("/item/id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let call_id = value
                .pointer("/item/call_id")
                .or_else(|| value.pointer("/item/id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if !item_id.is_empty() && !call_id.is_empty() {
                state
                    .tool_call_ids_by_item_id
                    .insert(item_id.to_string(), call_id.clone());
            }
            let name = value
                .pointer("/item/name")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let arguments = value
                .pointer("/item/arguments")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            push_completed_arguments_delta(value, state, &mut parsed, call_id, name, arguments);
        }
        Some("response.completed") => {
            parsed.usage = value.get("response").and_then(parse_usage);
            if let Some(output) = value.pointer("/response/output").and_then(|v| v.as_array()) {
                for (index, item) in output.iter().enumerate() {
                    if item["type"] == "message" {
                        push_response_message(index, item, state, &mut parsed);
                    }
                }
            }
        }
        Some("response.failed") => {
            let code = value
                .pointer("/response/error/code")
                .or_else(|| value.pointer("/response/error/type"))
                .and_then(|v| v.as_str());
            let message = value
                .pointer("/response/error/message")
                .and_then(|v| v.as_str())
                .unwrap_or("Responses API request failed");
            parsed.events.push(AgentEvent::Error {
                code: code.map(str::to_string),
                message: message.to_string(),
                retryable: is_retryable_provider_error(code, message),
                metadata: serde_json::Value::Null,
            });
            parsed.terminal_error = true;
        }
        Some("response.incomplete") => {
            let reason = value
                .pointer("/response/incomplete_details/reason")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            parsed.events.push(AgentEvent::Error {
                code: Some("response_incomplete".to_string()),
                message: format!("Responses API response incomplete: {reason}"),
                retryable: false,
                metadata: serde_json::Value::Null,
            });
            parsed.terminal_error = true;
        }
        Some("error") => {
            let code = value
                .pointer("/error/code")
                .or_else(|| value.pointer("/error/type"))
                .and_then(|v| v.as_str());
            let message = value
                .pointer("/error/message")
                .or_else(|| value.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or("Responses API request failed");
            parsed.events.push(AgentEvent::Error {
                code: code.map(str::to_string),
                message: message.to_string(),
                retryable: is_retryable_provider_error(code, message),
                metadata: serde_json::Value::Null,
            });
            parsed.terminal_error = true;
        }
        _ => {}
    }
    parsed
}

fn response_message_metadata(
    index: usize,
    item: &serde_json::Value,
) -> serde_json::Map<String, serde_json::Value> {
    let mut metadata = serde_json::Map::new();
    metadata.insert(
        PROVIDER_MESSAGE_ID_METADATA_KEY.into(),
        item.get("id")
            .filter(|id| id.as_str().is_some_and(|id| !id.is_empty()))
            .cloned()
            .unwrap_or_else(|| serde_json::json!(format!("output-{index}"))),
    );
    if let Some(phase) = item.get("phase").filter(|phase| phase.is_string()) {
        metadata.insert(ASSISTANT_PHASE_METADATA_KEY.into(), phase.clone());
    }
    metadata
}

fn response_message_text(item: &serde_json::Value) -> String {
    item.get("content")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|part| part.get("text").and_then(|v| v.as_str()))
        .collect()
}

fn push_response_message(
    index: usize,
    item: &serde_json::Value,
    state: &mut ResponseStreamParseState,
    parsed: &mut ParsedResponseStreamEvent,
) {
    let incoming = response_message_metadata(index, item);
    let metadata = state.message_metadata.entry(index).or_default();
    let changed = incoming
        .iter()
        .any(|(key, value)| metadata.get(key) != Some(value));
    metadata.extend(incoming);
    let text = response_message_text(item);
    let seen = state.message_text_lengths.entry(index).or_default();
    let delta = text.get(*seen..).unwrap_or_default();
    *seen = (*seen).max(text.len());
    // A terminal item can supply phase after the last text delta. Carry that
    // update without emitting the already-streamed text a second time.
    if !delta.is_empty() || (changed && *seen > 0) {
        parsed.events.push(AgentEvent::TextDelta {
            delta: delta.to_string(),
            metadata: serde_json::Value::Object(metadata.clone()),
        });
    }
}

fn push_completed_arguments_delta(
    value: &serde_json::Value,
    state: &mut ResponseStreamParseState,
    parsed: &mut ParsedResponseStreamEvent,
    id: String,
    name: Option<String>,
    arguments: &str,
) {
    let seen = state.tool_argument_lengths.entry(id.clone()).or_default();
    let arguments_delta = if *seen == 0 {
        arguments.to_string()
    } else if *seen < arguments.len() && arguments.is_char_boundary(*seen) {
        arguments[*seen..].to_string()
    } else {
        String::new()
    };
    *seen = arguments.len();
    if name.is_none() && arguments_delta.is_empty() {
        return;
    }
    parsed.events.push(AgentEvent::ToolCallDelta {
        id,
        index: value
            .get("output_index")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize,
        name,
        arguments_delta,
        metadata: serde_json::Value::Null,
    });
}
