use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine;
use futures::StreamExt;
use tracing::{debug, warn};

use crate::key_selector::KeySelector;

use agent_types::{
    AgentEvent, ChatRequest, ChatResponse, ContentPart, FinishReason, ModelDialogueItem,
    ReasoningConfig, ReasoningMode, Role, StructuredOutput, TokenUsage, ToolCall, ToolChoice,
};

use crate::error::LlmError;
use crate::event_parser::parse_openai_chunk;
use crate::traits::{ChatModel, ConnectionStatus, EventStream, ModelInfo, Provider, ProviderInfo};
use crate::transport::{parse_sse_lines, HttpTransport, TransportConfig};

#[derive(Debug, Clone)]
pub struct OpenAiCompatibleConfig {
    pub provider_id: String,

    pub provider_name: String,

    pub base_url: String,

    pub api_key: Option<String>,

    pub api_keys: Vec<String>,

    pub headers: serde_json::Map<String, serde_json::Value>,

    pub models: Vec<ModelInfo>,

    pub timeout_secs: u64,

    pub generate_kwargs: serde_json::Map<String, serde_json::Value>,

    pub support_model_discovery: bool,

    pub support_connection_check: bool,

    pub is_local: bool,
}

pub struct OpenAiCompatibleProvider {
    config: OpenAiCompatibleConfig,
    transport: Arc<HttpTransport>,
    transports: Vec<Arc<HttpTransport>>,
    key_selector: Arc<KeySelector>,
    info: ProviderInfo,
}

impl OpenAiCompatibleProvider {
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

        let key_selector = Arc::new(KeySelector::new(transports.len()));
        let transport = transports[0].clone();

        let info = ProviderInfo {
            id: config.provider_id.clone(),
            name: config.provider_name.clone(),
            provider_type: Some("OpenAIChatModel".into()),
            is_local: config.is_local,
            support_model_discovery: config.support_model_discovery,
            support_connection_check: config.support_connection_check,
        };

        Ok(Self {
            config,
            transport,
            transports,
            key_selector,
            info,
        })
    }

    pub fn build_request_body(&self, request: &ChatRequest) -> serde_json::Value {
        Self::build_request_body_with_messages(
            &self.config,
            request,
            &request.messages,
            request.reasoning.as_ref(),
            false,
        )
    }

    pub(super) fn build_request_body_with_messages(
        config: &OpenAiCompatibleConfig,
        request: &ChatRequest,
        messages: &[ModelDialogueItem],
        reasoning: Option<&ReasoningConfig>,
        responses_format: bool,
    ) -> serde_json::Value {
        let mut messages = Self::convert_messages_with_phase(messages, responses_format);

        if let Some(ref model) = request.model {
            if model.to_lowercase().contains("omni") {
                apply_qwen_omni_audio_prefix(&mut messages);
            }
        }

        let mut body = serde_json::json!({
            "messages": messages,
            "stream": request.stream,
        });

        if let Some(ref model) = request.model {
            body["model"] = serde_json::json!(model);
        }

        if let Some(temp) = request.temperature {
            body["temperature"] = serde_json::json!(temp);
        }

        if let Some(max_tokens) = request.max_tokens {
            body["max_tokens"] = serde_json::json!(max_tokens);
        }

        if let Some(ref so) = request.structured_output {
            Self::apply_structured_output(&mut body, so);
        } else if !request.tools.is_empty() {
            let tools: Vec<serde_json::Value> = request
                .tools
                .iter()
                .map(|t| {
                    let mut params = t.parameters.clone();
                    crate::request::sanitize_boolean_schemas(&mut params);
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": params,
                        }
                    })
                })
                .collect();
            body["tools"] = serde_json::json!(tools);

            if let Some(ref tc) = request.tool_choice {
                body["tool_choice"] = match tc {
                    ToolChoice::Auto => serde_json::json!("auto"),
                    ToolChoice::None => serde_json::json!("none"),
                    ToolChoice::Required => serde_json::json!("required"),
                    ToolChoice::Function(name) => serde_json::json!({
                        "type": "function",
                        "function": {"name": name}
                    }),
                };
            }
        }

        if request.stream {
            body["stream_options"] = serde_json::json!({"include_usage": true});
        }

        if let Some(obj) = body.as_object_mut() {
            let limit_field = if obj
                .get("model")
                .and_then(serde_json::Value::as_str)
                .is_some_and(super::reasoning::uses_max_completion_tokens)
            {
                "max_completion_tokens"
            } else {
                "max_tokens"
            };
            let mut defaults = config.generate_kwargs.clone();
            super::reasoning::flatten_extra_body(&mut defaults);
            super::reasoning::normalize_output_limit(&mut defaults, limit_field);
            super::reasoning::normalize_output_limit(obj, limit_field);
            super::reasoning::apply_defaults(obj, &defaults);
            let mut options = request.provider_options.clone();
            super::reasoning::flatten_extra_body(&mut options);
            super::reasoning::normalize_output_limit(&mut options, limit_field);
            super::reasoning::merge_options(obj, &options);
            translate_openai_compatible_reasoning_options(obj, &config.provider_id, request);
            apply_openai_compatible_reasoning(obj, &config.provider_id, request, reasoning);
        }

        body
    }

    pub fn build_request_body_for_model(
        &self,
        request: &ChatRequest,
        model_id: &str,
    ) -> serde_json::Value {
        let mut request = request.clone();
        if request.model.is_none() {
            request.model = Some(model_id.into());
        }
        let mut config = self.config.clone();
        if let Some(model) = self.config.models.iter().find(|model| model.id == model_id) {
            super::reasoning::merge_options(&mut config.generate_kwargs, &model.generate_kwargs);
        }
        Self::build_request_body_with_messages(
            &config,
            &request,
            &request.messages,
            request.reasoning.as_ref(),
            false,
        )
    }

    fn apply_structured_output(body: &mut serde_json::Value, structured: &StructuredOutput) {
        if structured.fallback_to_tool_call {
            let tool = crate::request::create_format_tool(structured);
            body["tools"] = serde_json::json!([tool]);
            body["tool_choice"] = serde_json::json!({
                "type": "function",
                "function": {"name": structured.tool_name()}
            });
        } else {
            let mut schema = structured.schema.clone();
            crate::request::remove_title_field_public(&mut schema);
            let mut json_schema = serde_json::json!({
                "name": structured.tool_name(),
                "schema": schema,
            });
            if let Some(strict) = structured.strict {
                json_schema["strict"] = serde_json::json!(strict);
            }
            body["response_format"] = serde_json::json!({
                "type": "json_schema",
                "json_schema": json_schema,
            });
        }
    }

    pub fn convert_messages(&self, messages: &[ModelDialogueItem]) -> Vec<serde_json::Value> {
        Self::convert_request_messages(messages)
    }

    pub(super) fn convert_request_messages(
        messages: &[ModelDialogueItem],
    ) -> Vec<serde_json::Value> {
        Self::convert_messages_with_phase(messages, false)
    }

    pub(super) fn convert_responses_messages(
        messages: &[ModelDialogueItem],
    ) -> Vec<serde_json::Value> {
        Self::convert_messages_with_phase(messages, true)
    }

    fn convert_messages_with_phase(
        messages: &[ModelDialogueItem],
        responses_format: bool,
    ) -> Vec<serde_json::Value> {
        let normalized = crate::request::normalize_tool_history(messages);
        let messages = if responses_format {
            normalized
                .iter()
                .flat_map(ModelDialogueItem::split_assistant_messages)
                .collect::<Vec<_>>()
        } else {
            normalized
        };
        let mut instructions = ModelDialogueItem::text(Role::System, "");
        let mut has_instructions = false;
        // Preserve instruction priority when projecting developer messages to system.
        for role in [Role::System, Role::Developer] {
            for message in messages.iter().filter(|message| message.role == role) {
                has_instructions = true;
                for part in &message.content {
                    match (instructions.content.last_mut(), part) {
                        (Some(ContentPart::Text { text }), ContentPart::Text { text: next }) => {
                            if !next.is_empty() {
                                if !text.is_empty() {
                                    text.push_str("\n\n");
                                }
                                text.push_str(next);
                            }
                        }
                        _ => instructions.content.push(part.clone()),
                    }
                }
            }
        }
        let messages = has_instructions.then_some(&instructions).into_iter().chain(
            messages
                .iter()
                .filter(|message| !matches!(message.role, Role::System | Role::Developer)),
        );
        messages
            .flat_map(|msg| {
                let role = match msg.role {
                    Role::System => "system",
                    Role::User => "user",
                    Role::Assistant => "assistant",
                    Role::Tool => "tool",
                    Role::Developer => "system",
                    _ => "user",
                };







                let mut extra_msgs: Vec<serde_json::Value> = Vec::new();
                let mut m = serde_json::json!({ "role": role });
                if responses_format && msg.role == Role::Assistant {
                    if let Some(phase) = msg.metadata.get(agent_types::model_dialogue_item::ASSISTANT_PHASE_METADATA_KEY)
                        .filter(|phase| phase.is_string()) {
                        m["phase"] = phase.clone();
                    }
                }


                if msg.content.len() == 1 {
                    if let Some(text) = msg.content[0].as_text() {
                        m["content"] = serde_json::json!(text);
                        if msg.role == Role::Tool {
                            if let Some(ref name) = msg.name {
                                m["name"] = serde_json::json!(name);
                            }

                            if let Some(tc_id) =
                                msg.metadata.get("tool_call_id").and_then(|v| v.as_str())
                            {
                                m["tool_call_id"] = serde_json::json!(tc_id);
                            }
                        }
                        return vec![m];
                    }
                }


                let mut content_parts = Vec::new();
                let mut tool_calls = Vec::new();
                let mut reasoning_content = String::new();

                for part in &msg.content {
                    match part {
                        ContentPart::Text { text } => {
                            content_parts.push(serde_json::json!({"type": "text", "text": text}));
                        }
                        ContentPart::Thinking { text, .. }
                            if msg.role == Role::Assistant && !text.is_empty() =>
                        {
                            if !reasoning_content.is_empty() {
                                reasoning_content.push('\n');
                            }
                            reasoning_content.push_str(text);
                        }
                        ContentPart::Image {
                            url, media_type, ..
                        } => {
                            let final_url = if is_local_path(url) {
                                encode_local_image_as_data_uri(url, media_type.as_deref())
                            } else {
                                url.clone()
                            };
                            let img = serde_json::json!({
                                "type": "image_url",
                                "image_url": {"url": final_url}
                            });
                            content_parts.push(img);
                        }
                        ContentPart::Audio { url, media_type } => {
                            if msg.role == Role::Assistant {
                                continue;
                            }
                            if let Some((data, format)) =
                                extract_audio_data_and_format(url, media_type.as_deref())
                            {
                                content_parts.push(serde_json::json!({
                                    "type": "input_audio",
                                    "input_audio": {
                                        "data": data,
                                        "format": format,
                                    }
                                }));
                            }
                        }
                        ContentPart::Video { url, .. } => {
                            content_parts.push(serde_json::json!({
                                "type": "video_url",
                                "video_url": {"url": url}
                            }));
                        }
                        ContentPart::ToolUse { call } => {
                            if !call.has_valid_identity() {
                                continue;
                            }
                            let arguments_str = match call.raw_arguments.as_deref() {
                                Some(raw)
                                    if serde_json::from_str::<serde_json::Value>(raw).is_ok() =>
                                {
                                    raw.to_string()
                                }
                                _ => {
                                    if call.arguments.is_null() {
                                        "{}".to_string()
                                    } else {
                                        serde_json::to_string(&call.arguments)
                                            .unwrap_or_else(|_| "{}".to_string())
                                    }
                                }
                            };
                            tool_calls.push(serde_json::json!({
                                "id": call.provider_call_id.as_deref().unwrap_or(&call.id),
                                "type": "function",
                                "function": {
                                    "name": call.name,
                                    "arguments": arguments_str,
                                }
                            }));
                        }
                        ContentPart::File { url, name, .. } => {
                            content_parts.push(serde_json::json!({
                                "type": "text",
                                "text": format!(
                                    "用户上传文件 {}，已保存到本地路径: {}。你可以用 Read 工具读取文件内容。",
                                    name, url
                                )
                            }));
                        }
                        ContentPart::ToolResult { result } => {
                            if !result.has_valid_call_id() {
                                continue;
                            }









                            m["role"] = serde_json::json!("tool");
                            m["content"] = serde_json::json!(result.text_content());
                            m["tool_call_id"] = serde_json::json!(&result.call_id);
                            if let Some(ref name) = result.name {
                                m["name"] = serde_json::json!(name);
                            }



                            let mut promoted_parts: Vec<serde_json::Value> = Vec::new();
                            for inner in &result.content {
                                match inner {
                                    ContentPart::Image {
                                        url, media_type, ..
                                    } => {
                                        let final_url = if is_local_path(url) {
                                            encode_local_image_as_data_uri(
                                                url,
                                                media_type.as_deref(),
                                            )
                                        } else {
                                            url.clone()
                                        };
                                        promoted_parts.push(serde_json::json!({
                                            "type": "text",
                                            "text": format!(
                                                "\n- The image from tool `{}`:",
                                                result.name.as_deref().unwrap_or("")
                                            )
                                        }));
                                        promoted_parts.push(serde_json::json!({
                                            "type": "image_url",
                                            "image_url": {"url": final_url}
                                        }));
                                    }
                                    ContentPart::Video { url, .. } => {
                                        promoted_parts.push(serde_json::json!({
                                            "type": "text",
                                            "text": format!("[Video from tool: {}]", url)
                                        }));
                                    }
                                    _ => {}
                                }
                            }
                            if !promoted_parts.is_empty() {
                                extra_msgs.push(serde_json::json!({
                                    "role": "user",
                                    "content": promoted_parts,
                                }));
                            }

                            let mut out = vec![m];
                            out.extend(extra_msgs);
                            return out;
                        }
                        _ => {}
                    }
                }

                if !content_parts.is_empty() {
                    if content_parts.len() == 1 {
                        if let Some(text) = content_parts[0].get("text") {
                            m["content"] = text.clone();
                        } else {
                            m["content"] = serde_json::json!(content_parts);
                        }
                    } else {
                        m["content"] = serde_json::json!(content_parts);
                    }
                }

                if !tool_calls.is_empty() {
                    m["tool_calls"] = serde_json::json!(tool_calls);
                }

                if !reasoning_content.is_empty() {
                    m["reasoning_content"] = serde_json::json!(reasoning_content);
                }

                if !responses_format && msg.role == Role::Assistant
                    && m.get("content").is_none()
                    && m.get("tool_calls").is_none()
                    && m.get("reasoning_content").is_some()
                {
                    m["content"] = serde_json::json!("");
                }

                let mut out = vec![m];
                out.extend(extra_msgs);
                out
            })
            .collect()
    }
}

fn translate_openai_compatible_reasoning_options(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    provider_id: &str,
    request: &ChatRequest,
) {
    let enabled = obj
        .remove("thinking_enabled")
        .and_then(|value| value.as_bool());
    let legacy_enabled = obj
        .remove("thinking_enable")
        .and_then(|value| value.as_bool());
    if let Some(enabled) = enabled.or(legacy_enabled) {
        obj.entry("enable_thinking")
            .or_insert(serde_json::Value::Bool(enabled));
    }
    if obj
        .remove("disable_thinking")
        .and_then(|value| value.as_bool())
        == Some(true)
        && request.reasoning.is_none()
    {
        let mode = if obj.contains_key("enable_thinking") || obj.contains_key("thinking_budget") {
            ReasoningMode::Budget
        } else {
            ReasoningMode::Effort
        };
        apply_openai_compatible_reasoning(
            obj,
            provider_id,
            request,
            Some(&ReasoningConfig {
                mode,
                enabled: false,
                effort: None,
                budget_tokens: None,
                summary: None,
            }),
        );
    }
}

fn apply_openai_compatible_reasoning(
    body: &mut serde_json::Map<String, serde_json::Value>,
    provider_id: &str,
    request: &ChatRequest,
    reasoning: Option<&ReasoningConfig>,
) {
    let Some(reasoning) = reasoning else {
        return;
    };
    let declared = request
        .metadata
        .get("rua_reasoning_format")
        .and_then(serde_json::Value::as_str);
    let format = declared
        .filter(|format| *format != "protocol")
        .unwrap_or_else(|| match provider_id {
            "openrouter" => "openrouter",
            "dashscope"
            | "aliyun-codingplan"
            | "aliyun-codingplan-intl"
            | "aliyun-tokenplan"
            | "aliyun-tokenplan-intl" => {
                if reasoning.mode == ReasoningMode::Budget {
                    "enable_thinking"
                } else {
                    "thinking"
                }
            }
            "kimi-cn"
            | "kimi-intl"
            | "kimi-codingplan"
            | "zhipu-cn"
            | "zhipu-cn-codingplan"
            | "zhipu-intl"
            | "zhipu-intl-codingplan"
            | "deepseek"
            | "mimo"
            | "mimo-tokenplan"
            | "volcengine-cn"
            | "volcengine-cn-codingplan"
            | "volcengine-cn-agentplan" => "thinking",
            _ if reasoning.mode == ReasoningMode::Budget => "enable_thinking",
            _ => "effort",
        });
    let format = if format == "dashscope" {
        if reasoning.mode == ReasoningMode::Budget {
            "enable_thinking"
        } else {
            "thinking"
        }
    } else {
        format
    };
    let router_options = body
        .get("reasoning")
        .and_then(|value| value.as_object())
        .cloned();
    super::reasoning::clear_controls(body);
    match format {
        "openrouter" => {
            let mut value = router_options.unwrap_or_default();
            for key in ["effort", "enabled", "max_tokens"] {
                value.remove(key);
            }
            if !reasoning.enabled {
                value.insert("enabled".into(), serde_json::json!(false));
            } else if reasoning.mode == ReasoningMode::Budget {
                if let Some(budget) = reasoning.budget_tokens {
                    value.insert("max_tokens".into(), serde_json::json!(budget));
                } else {
                    value.insert("enabled".into(), serde_json::json!(true));
                }
            } else if let Some(effort) = reasoning.effort.as_deref() {
                value.insert("effort".into(), serde_json::json!(effort));
            } else {
                value.insert("enabled".into(), serde_json::json!(true));
            }
            body.insert("reasoning".into(), serde_json::Value::Object(value));
        }
        "thinking" => {
            body.insert(
                "thinking".into(),
                serde_json::json!({
                    "type": if reasoning.enabled { "enabled" } else { "disabled" }
                }),
            );
            if reasoning.enabled {
                if let Some(effort) = reasoning.effort.as_deref() {
                    body.insert("reasoning_effort".into(), serde_json::json!(effort));
                }
            }
        }
        "enable_thinking" => {
            body.insert(
                "enable_thinking".into(),
                serde_json::json!(reasoning.enabled),
            );
            if reasoning.enabled {
                if let Some(budget) = reasoning.budget_tokens {
                    body.insert("thinking_budget".into(), serde_json::json!(budget));
                }
            }
        }
        _ => {
            if !reasoning.enabled {
                body.insert(
                    "reasoning_effort".into(),
                    serde_json::json!(super::reasoning::off_effort(request, "low")),
                );
            } else if let Some(effort) = reasoning.effort.as_deref() {
                body.insert("reasoning_effort".into(), serde_json::json!(effort));
            }
        }
    }
}

#[async_trait]
impl Provider for OpenAiCompatibleProvider {
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
        Ok(Box::new(OpenAiCompatibleChatModel {
            model_info: info,
            transport: self.transport.clone(),
            transports: self.transports.clone(),
            key_selector: self.key_selector.clone(),
            config,
        }))
    }

    async fn fetch_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        if !self.config.support_model_discovery {
            return Ok(self.config.models.clone());
        }

        let body = self.transport.get("/models").await?;
        let value: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| LlmError::InvalidResponse {
                provider: self.config.provider_id.clone(),
                message: format!("failed to parse models response: {e}"),
            })?;

        let models = value
            .get("data")
            .and_then(|d| d.as_array())
            .map(|arr| {
                let mut seen = HashSet::new();
                arr.iter()
                    .filter_map(|m| {
                        let id = m.get("id")?.as_str()?.trim().to_string();
                        if id.is_empty() || !seen.insert(id.clone()) {
                            return None;
                        }
                        let name = m
                            .get("name")
                            .and_then(|n| n.as_str())
                            .map(str::trim)
                            .filter(|n| !n.is_empty())
                            .unwrap_or(&id)
                            .to_string();
                        Some(ModelInfo {
                            id: id.clone(),
                            name,
                            context_window: [
                                "context_window",
                                "context_length",
                                "max_model_len",
                                "max_context_length",
                            ]
                            .iter()
                            .find_map(|key| {
                                m.get(key)
                                    .and_then(|value| value.as_u64())
                                    .filter(|value| *value >= 1000)
                            }),
                            max_output_tokens: m
                                .get("max_output_tokens")
                                .and_then(|value| value.as_u64())
                                .filter(|value| *value > 0),
                            supports_multimodal: None,
                            supports_image: None,
                            supports_video: None,
                            probe_source: Some("api".into()),
                            generate_kwargs: serde_json::Map::new(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(models)
    }

    async fn check_connection(&self) -> Result<ConnectionStatus, LlmError> {
        if !self.config.support_connection_check {
            return Ok(ConnectionStatus::Connected);
        }

        match self.transport.get("/models").await {
            Ok(_) => Ok(ConnectionStatus::Connected),
            Err(e) => Ok(ConnectionStatus::Failed(e.to_string())),
        }
    }
}

struct OpenAiCompatibleChatModel {
    model_info: ModelInfo,
    transport: Arc<HttpTransport>,
    transports: Vec<Arc<HttpTransport>>,
    key_selector: Arc<KeySelector>,
    config: OpenAiCompatibleConfig,
}

#[async_trait]
impl ChatModel for OpenAiCompatibleChatModel {
    async fn chat(&self, mut request: ChatRequest) -> Result<ChatResponse, LlmError> {
        request.stream = false;
        if request.model.is_none() {
            request.model = Some(self.model_info.id.clone());
        }

        let provider = OpenAiCompatibleProvider {
            config: self.config.clone(),
            transport: self.transport.clone(),
            transports: self.transports.clone(),
            key_selector: self.key_selector.clone(),
            info: ProviderInfo {
                id: self.config.provider_id.clone(),
                name: self.config.provider_name.clone(),
                provider_type: Some("OpenAIChatModel".into()),
                is_local: false,
                support_model_discovery: false,
                support_connection_check: false,
            },
        };

        let body = provider.build_request_body(&request);
        crate::debug_capture::record_provider_request(
            &request.metadata,
            &self.config.provider_id,
            "/chat/completions",
            &body,
        );
        let key_idx = self.key_selector.acquire();
        let transport = &self.transports[key_idx];
        let result = transport.post("/chat/completions", &body).await;
        self.key_selector.release(key_idx);
        let response_text = result?;

        let value: serde_json::Value =
            serde_json::from_str(&response_text).map_err(|e| LlmError::InvalidResponse {
                provider: self.config.provider_id.clone(),
                message: format!("failed to parse response: {e}"),
            })?;

        let choice = value
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
            .ok_or_else(|| LlmError::InvalidResponse {
                provider: self.config.provider_id.clone(),
                message: "no choices in response".into(),
            })?;

        let message_val = choice
            .get("message")
            .ok_or_else(|| LlmError::InvalidResponse {
                provider: self.config.provider_id.clone(),
                message: "no message in choice".into(),
            })?;

        let content_text = message_val
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();

        let mut content = vec![ContentPart::Text { text: content_text }];

        if let Some(tcs) = message_val.get("tool_calls").and_then(|t| t.as_array()) {
            for tc in tcs {
                let id = tc
                    .get("id")
                    .and_then(|i| i.as_str())
                    .unwrap_or("")
                    .to_string();
                let func = tc.get("function").unwrap_or(&serde_json::Value::Null);
                let name = func
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string();
                let args_str = func
                    .get("arguments")
                    .map(|a| {
                        if let Some(s) = a.as_str() {
                            s.to_string()
                        } else if a.is_object() || a.is_array() {
                            a.to_string()
                        } else {
                            "{}".to_string()
                        }
                    })
                    .unwrap_or_else(|| "{}".to_string());
                let arguments = serde_json::from_str(&args_str).unwrap_or_else(|_| {
                    rua_json_repair::repair_json(&args_str)
                        .map(|outcome| outcome.value)
                        .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()))
                });

                content.push(ContentPart::ToolUse {
                    call: ToolCall {
                        id: id.clone(),
                        name,
                        arguments,
                        raw_arguments: Some(args_str.to_string()),
                        provider_call_id: Some(id),
                        metadata: serde_json::Map::new(),
                    },
                });
            }
        }

        let finish_reason = choice
            .get("finish_reason")
            .and_then(|f| f.as_str())
            .map(FinishReason::from_provider)
            .unwrap_or(FinishReason::Stop);

        let usage = value.get("usage").and_then(|u| {
            Some(TokenUsage {
                prompt_tokens: u.get("prompt_tokens")?.as_u64()?,
                completion_tokens: u.get("completion_tokens")?.as_u64()?,
                reasoning_tokens: u
                    .get("completion_tokens_details")
                    .and_then(|d| d.get("reasoning_tokens"))
                    .and_then(|r| r.as_u64()),
                cached_tokens: u
                    .get("prompt_tokens_details")
                    .and_then(|d| d.get("cached_tokens"))
                    .and_then(|c| c.as_u64()),
                cache_write_tokens: None,
                total_tokens: u.get("total_tokens").and_then(|t| t.as_u64()),
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

    fn chat_stream(&self, mut request: ChatRequest) -> EventStream {
        let mut trace = crate::debug_capture::CaptureTrace::new(&request.metadata, "provider");
        request.stream = true;
        if request.model.is_none() {
            request.model = Some(self.model_info.id.clone());
        }

        let provider = OpenAiCompatibleProvider {
            config: self.config.clone(),
            transport: self.transport.clone(),
            transports: self.transports.clone(),
            key_selector: self.key_selector.clone(),
            info: ProviderInfo {
                id: self.config.provider_id.clone(),
                name: self.config.provider_name.clone(),
                provider_type: Some("OpenAIChatModel".into()),
                is_local: false,
                support_model_discovery: false,
                support_connection_check: false,
            },
        };

        let body = provider.build_request_body(&request);
        trace.mark("body_built");
        trace.record_request(&self.config.provider_id, "/chat/completions", &body);
        let key_idx = self.key_selector.acquire();
        let transport = self.transports[key_idx].clone();
        let key_selector = self.key_selector.clone();
        let provider_id = self.config.provider_id.clone();
        let run_id = request
            .metadata
            .get("request_id")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let model = request.model.clone();

        let stream = async_stream::stream! {
            trace.mark("stream_polled");
            let yielded_at = std::time::Instant::now();
            yield Ok(AgentEvent::Start {
                run_id: run_id.clone(),
                model: model.clone(),
                metadata: serde_json::Value::Null,
            });
            trace.add_duration("consumer_pause_ms", "max_consumer_pause_ms", yielded_at.elapsed());

            let byte_stream = match transport.post_stream_traced("/chat/completions", &body, &mut trace).await {
                Ok(s) => s,
                Err(e) => {
                    key_selector.release(key_idx);
                    trace.error(&e, "http_request");
                    yield Err(e);
                    return;
                }
            };
            key_selector.release(key_idx);

            let mut buffer = String::new();
            let mut pending_utf8 = Vec::new();
            let mut tool_call_index_map: std::collections::HashMap<usize, String> =
                std::collections::HashMap::new();
            let mut final_finish_reason: Option<FinishReason> = None;
            let mut completed = false;

            tokio::pin!(byte_stream);

            let mut chunk_idx: u64 = 0;
            'stream: loop {
                let read_started = std::time::Instant::now();
                let result = byte_stream.next().await;
                trace.add_duration("stream_wait_ms", "max_stream_wait_ms", read_started.elapsed());
                let Some(result) = result else {
                    trace.mark("http_eof");
                    break;
                };
                match result {
                    Ok(bytes) => {
                        trace.received_bytes(bytes.len());
                        chunk_idx += 1;
                        debug!(
                            provider = %provider_id,
                            chunk = chunk_idx,
                            bytes = bytes.len(),
                            "openai_compatible: TCP chunk arrived"
                        );
                        append_utf8_chunk(&mut buffer, &mut pending_utf8, &bytes);
                        let (lines, remaining) = parse_sse_lines(&buffer);
                        buffer = remaining;

                        for line in lines {
                            trace.checkpoint("first_sse_data");
                            if line == "[DONE]" {



                                completed = true;
                                break 'stream;
                            }

                            debug!(
                                provider = %provider_id,
                                sse_line_len = line.len(),
                                "openai_compatible: SSE data line parsed"
                            );

                            if line.contains("tool_calls") {
                                debug!(
                                    provider = %provider_id,
                                    sse_line = %line,
                                    "openai_compatible: raw SSE line with tool_calls"
                                );
                            }

                            match parse_openai_chunk(&line, &mut tool_call_index_map) {
                                Ok((events, finish)) => {
                                    if let Some(fr) = finish {
                                        final_finish_reason = Some(fr);
                                    }
                                    let terminal_error = events
                                        .iter()
                                        .any(|event| matches!(event, AgentEvent::Error { .. }));
                                    for event in events {
                                        trace.event(&event);
                                        let yielded_at = std::time::Instant::now();
                                        yield Ok(event);
                                        trace.add_duration("consumer_pause_ms", "max_consumer_pause_ms", yielded_at.elapsed());
                                    }
                                    if terminal_error {
                                        return;
                                    }
                                }
                                Err(e) => {
                                    warn!(provider = %provider_id, error = %e, "failed to parse SSE chunk");
                                }
                            }

                            if let Ok(chunk_val) = serde_json::from_str::<serde_json::Value>(&line) {
                                trace.response_metadata(&chunk_val);
                                if let Some(choices) = chunk_val.get("choices").and_then(|c| c.as_array()) {
                                    for choice in choices {
                                        if let Some(audio) = choice.pointer("/delta/audio") {
                                            if let Some(transcript) = audio.get("transcript").and_then(|t| t.as_str()) {
                                                if !transcript.is_empty() {
                                                    let event = AgentEvent::TextDelta {
                                                        delta: transcript.to_string(),
                                                        metadata: serde_json::Value::Null,
                                                    };
                                                    trace.event(&event);
                                                    let yielded_at = std::time::Instant::now();
                                                    yield Ok(event);
                                                    trace.add_duration("consumer_pause_ms", "max_consumer_pause_ms", yielded_at.elapsed());
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        trace.error(&e, "stream_read");
                        yield Err(e);
                        return;
                    }
                }
            }

            if !completed && final_finish_reason.is_none() {
                trace.set("error_kind", "incomplete_stream");
                trace.finish("error");
                yield Err(LlmError::Connection {
                    provider: provider_id,
                    message: "Chat Completions stream closed before [DONE]/finish_reason".into(),
                });
                return;
            }

            let finish = final_finish_reason.unwrap_or(FinishReason::Stop);
            trace.mark("provider_finished");
            trace.finish("completed");
            yield Ok(AgentEvent::End {
                finish_reason: finish,
                usage: None,
                metadata: serde_json::Value::Null,
            });
        };

        crate::stream_sanitizer::SanitizedStream::wrap(Box::pin(stream))
    }

    fn model_info(&self) -> &ModelInfo {
        &self.model_info
    }
}

fn append_utf8_chunk(output: &mut String, pending: &mut Vec<u8>, chunk: &[u8]) {
    pending.extend_from_slice(chunk);

    match std::str::from_utf8(pending) {
        Ok(text) => {
            output.push_str(text);
            pending.clear();
        }
        Err(error) if error.error_len().is_none() => {
            let valid_up_to = error.valid_up_to();
            if valid_up_to > 0 {
                let valid = std::str::from_utf8(&pending[..valid_up_to])
                    .expect("UTF-8 validator reported a valid prefix")
                    .to_string();
                output.push_str(&valid);
                pending.drain(..valid_up_to);
            }
        }
        Err(_) => {
            output.push_str(&String::from_utf8_lossy(pending));
            pending.clear();
        }
    }
}

fn extract_audio_data_and_format(url: &str, media_type: Option<&str>) -> Option<(String, String)> {
    if let Some(rest) = url.strip_prefix("data:") {
        if let Some(semicolon) = rest.find(';') {
            let mime = &rest[..semicolon];
            if let Some(comma) = rest.find(',') {
                let data = &rest[comma + 1..];
                let format = mime.rsplit('/').next().unwrap_or("wav");
                return Some((data.to_string(), format.to_string()));
            }
        }
    }

    let format = media_type
        .and_then(|mt| mt.rsplit('/').next())
        .unwrap_or_else(|| {
            url.rsplit('.')
                .next()
                .filter(|ext| *ext == "wav" || *ext == "mp3")
                .unwrap_or("wav")
        });
    Some((url.to_string(), format.to_string()))
}

fn is_local_path(url: &str) -> bool {
    !url.starts_with("http://") && !url.starts_with("https://") && !url.starts_with("data:")
}

fn mime_from_extension(path: &str) -> &'static str {
    match std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
    {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        Some("svg") => "image/svg+xml",
        _ => "image/png",
    }
}

fn encode_local_image_as_data_uri(url: &str, media_type: Option<&str>) -> String {
    let path = url.strip_prefix("file://").unwrap_or(url);
    match std::fs::read(path) {
        Ok(bytes) => {
            let mime = media_type.unwrap_or_else(|| mime_from_extension(path));
            let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
            format!("data:{mime};base64,{encoded}")
        }
        Err(e) => {
            warn!("Failed to read local image {}: {}", path, e);
            url.to_string()
        }
    }
}

fn apply_qwen_omni_audio_prefix(messages: &mut [serde_json::Value]) {
    for msg in messages.iter_mut() {
        let content = match msg.get_mut("content").and_then(|c| c.as_array_mut()) {
            Some(arr) => arr,
            None => continue,
        };
        for block in content.iter_mut() {
            if let Some(input_audio) = block.get_mut("input_audio") {
                if let Some(data) = input_audio.get_mut("data").and_then(|d| d.as_str()) {
                    if !data.starts_with("http") && !data.starts_with("data:") {
                        let prefixed = format!("data:;base64,{data}");
                        input_audio["data"] = serde_json::json!(prefixed);
                    }
                }
            }
        }
    }
}
