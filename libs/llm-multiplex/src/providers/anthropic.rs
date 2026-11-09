use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine;
use futures::StreamExt;
use tracing::warn;

use agent_types::{
    AgentEvent, ChatRequest, ChatResponse, ContentPart, FinishReason, ModelDialogueItem,
    ReasoningConfig, ReasoningMode, Role, TokenUsage, ToolCall, ToolChoice,
};

use crate::error::LlmError;
use crate::event_parser::{extract_reasoning_tokens, parse_anthropic_event};
use crate::traits::{ChatModel, ConnectionStatus, EventStream, ModelInfo, Provider, ProviderInfo};
use crate::transport::{parse_anthropic_sse, HttpTransport, TransportConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AuthMode {
    #[default]
    ApiKey,

    AuthToken,
}

#[derive(Debug, Clone)]
pub struct AnthropicConfig {
    pub provider_id: String,

    pub provider_name: String,

    pub base_url: String,

    pub api_key: Option<String>,

    pub headers: serde_json::Map<String, serde_json::Value>,

    pub models: Vec<ModelInfo>,

    pub timeout_secs: u64,

    pub generate_kwargs: serde_json::Map<String, serde_json::Value>,

    pub support_connection_check: bool,

    pub api_version: String,

    pub auth_mode: AuthMode,
}

impl Default for AnthropicConfig {
    fn default() -> Self {
        Self {
            provider_id: "anthropic".into(),
            provider_name: "Anthropic".into(),
            base_url: "https://api.anthropic.com".into(),
            api_key: None,
            headers: serde_json::Map::new(),
            models: vec![],
            timeout_secs: 120,
            generate_kwargs: serde_json::Map::new(),
            support_connection_check: true,
            api_version: "2023-06-01".into(),
            auth_mode: AuthMode::default(),
        }
    }
}

impl AnthropicConfig {
    pub fn build_transport_config(&self) -> TransportConfig {
        let mut tc = TransportConfig::new(&self.base_url, &self.provider_id);
        if let Some(ref key) = self.api_key {
            match self.auth_mode {
                AuthMode::ApiKey => {
                    tc = tc.with_header("x-api-key", key);
                }
                AuthMode::AuthToken => {
                    tc = tc.with_header("Authorization", &format!("Bearer {key}"));
                }
            }
        }
        tc = tc
            .with_timeout(std::time::Duration::from_secs(self.timeout_secs))
            .with_header("anthropic-version", &self.api_version)
            .with_headers_from_map(&self.headers);
        tc
    }
}

pub struct AnthropicProvider {
    config: AnthropicConfig,
    transport: Arc<HttpTransport>,
    info: ProviderInfo,
}

impl AnthropicProvider {
    pub fn new(config: AnthropicConfig) -> Result<Self, LlmError> {
        let transport_config = config.build_transport_config();

        let transport = Arc::new(HttpTransport::new(transport_config)?);

        let info = ProviderInfo {
            id: config.provider_id.clone(),
            name: config.provider_name.clone(),
            provider_type: Some("AnthropicChatModel".into()),
            is_local: false,
            support_model_discovery: true,
            support_connection_check: config.support_connection_check,
        };

        Ok(Self {
            config,
            transport,
            info,
        })
    }

    pub fn build_request_body(&self, request: &ChatRequest) -> serde_json::Value {
        let mut body = serde_json::json!({
            "stream": request.stream,
        });

        if let Some(ref model) = request.model {
            body["model"] = serde_json::json!(model);
        }

        let normalized_messages = crate::request::normalize_tool_history(&request.messages);

        let system_msgs: Vec<_> = request
            .messages
            .iter()
            .filter(|message| message.role == Role::System)
            .collect();

        if !system_msgs.is_empty() {
            let system_text: Vec<String> = system_msgs.iter().map(|m| m.text_content()).collect();
            body["system"] = serde_json::json!(system_text.join("\n\n"));
        }

        let messages: Vec<serde_json::Value> = normalized_messages
            .iter()
            .filter(|msg| msg.role != Role::System)
            .map(|msg| self.convert_message(msg))
            .collect();
        body["messages"] = serde_json::json!(messages);

        if let Some(temp) = request.temperature {
            body["temperature"] = serde_json::json!(temp);
        }

        if let Some(ref so) = request.structured_output {
            let format_tool = crate::request::create_anthropic_format_tool(so);
            body["tools"] = serde_json::json!([format_tool]);
            body["tool_choice"] = serde_json::json!({
                "type": "tool",
                "name": so.tool_name(),
            });
        } else if !request.tools.is_empty() {
            let tools: Vec<serde_json::Value> = request
                .tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.parameters,
                    })
                })
                .collect();
            body["tools"] = serde_json::json!(tools);

            if let Some(ref tc) = request.tool_choice {
                body["tool_choice"] = match tc {
                    ToolChoice::Auto => serde_json::json!({"type": "auto"}),
                    ToolChoice::None => serde_json::json!({"type": "none"}),
                    ToolChoice::Required => serde_json::json!({"type": "any"}),
                    ToolChoice::Function(name) => serde_json::json!({"type": "tool", "name": name}),
                };
            }
        }

        if let Some(obj) = body.as_object_mut() {
            super::reasoning::apply_defaults(obj, &self.config.generate_kwargs);
            if let Some(max_tokens) = request.max_tokens {
                obj.insert("max_tokens".into(), serde_json::json!(max_tokens));
            }
            super::reasoning::merge_options(obj, &request.provider_options);
            super::reasoning::flatten_extra_body(obj);
            obj.entry("max_tokens")
                .or_insert_with(|| serde_json::json!(2048));
            if request.reasoning.is_some() {
                super::reasoning::clear_controls(obj);
            } else {
                translate_anthropic_thinking_options(obj);
            }
            apply_anthropic_reasoning(obj, request.reasoning.as_ref());
        }

        body
    }

    pub fn convert_message(&self, msg: &ModelDialogueItem) -> serde_json::Value {
        let projected = crate::request::dedupe_tool_uses_in_message(msg);
        let msg = &projected;
        let role = match msg.role {
            Role::User | Role::System | Role::Developer => "user",
            Role::Assistant => "assistant",
            Role::Tool => "user",
            _ => "user",
        };

        let mut content = Vec::new();

        for part in &msg.content {
            match part {
                ContentPart::Text { text } => {
                    content.push(serde_json::json!({"type": "text", "text": text}));
                }
                ContentPart::Thinking {
                    text,
                    signature: Some(sig),
                } if !sig.is_empty() => {
                    content.push(serde_json::json!({
                        "type": "thinking",
                        "thinking": text,
                        "signature": sig,
                    }));
                }
                ContentPart::Thinking { .. } => {}
                ContentPart::Image {
                    url, media_type, ..
                } => {
                    if url.starts_with("http://") || url.starts_with("https://") {
                        content.push(serde_json::json!({
                            "type": "image",
                            "source": {
                                "type": "url",
                                "url": url,
                            }
                        }));
                    } else if url.starts_with("data:") {
                        let (extracted_mime, data) = extract_data_uri_parts(url);
                        let mime = media_type.as_deref().unwrap_or(&extracted_mime);
                        content.push(serde_json::json!({
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": mime,
                                "data": data,
                            }
                        }));
                    } else {
                        let path = url.strip_prefix("file://").unwrap_or(url);
                        match std::fs::read(path) {
                            Ok(bytes) => {
                                let mime = media_type
                                    .as_deref()
                                    .unwrap_or_else(|| mime_from_extension(path));
                                let encoded =
                                    base64::engine::general_purpose::STANDARD.encode(&bytes);
                                content.push(serde_json::json!({
                                    "type": "image",
                                    "source": {
                                        "type": "base64",
                                        "media_type": mime,
                                        "data": encoded,
                                    }
                                }));
                            }
                            Err(e) => {
                                warn!("Failed to read local image {}: {}", path, e);
                                content.push(serde_json::json!({
                                    "type": "text",
                                    "text": format!("[Failed to read image: {}]", url),
                                }));
                            }
                        }
                    }
                }
                ContentPart::ToolUse { call } => {
                    if !call.has_valid_identity() {
                        continue;
                    }
                    content.push(serde_json::json!({
                        "type": "tool_use",
                        "id": call.provider_call_id.as_deref().unwrap_or(&call.id),
                        "name": call.name,
                        "input": call.effective_arguments(),
                    }));
                }
                ContentPart::File { url, name, .. } => {
                    content.push(serde_json::json!({
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
                    let mut tool_content = Vec::new();
                    for part in &result.content {
                        match part {
                            ContentPart::Text { text } => {
                                tool_content
                                    .push(serde_json::json!({"type": "text", "text": text}));
                            }
                            ContentPart::Image {
                                url, media_type, ..
                            } => {
                                if url.starts_with("http://") || url.starts_with("https://") {
                                    tool_content.push(serde_json::json!({
                                        "type": "image",
                                        "source": {
                                            "type": "url",
                                            "url": url,
                                        }
                                    }));
                                } else if url.starts_with("data:") {
                                    let (extracted_mime, data) = extract_data_uri_parts(url);
                                    let mime = media_type.as_deref().unwrap_or(&extracted_mime);
                                    tool_content.push(serde_json::json!({
                                        "type": "image",
                                        "source": {
                                            "type": "base64",
                                            "media_type": mime,
                                            "data": data,
                                        }
                                    }));
                                } else {
                                    let path = url.strip_prefix("file://").unwrap_or(url);
                                    match std::fs::read(path) {
                                        Ok(bytes) => {
                                            let mime = media_type
                                                .as_deref()
                                                .unwrap_or_else(|| mime_from_extension(path));
                                            let encoded = base64::engine::general_purpose::STANDARD
                                                .encode(&bytes);
                                            tool_content.push(serde_json::json!({
                                                "type": "image",
                                                "source": {
                                                    "type": "base64",
                                                    "media_type": mime,
                                                    "data": encoded,
                                                }
                                            }));
                                        }
                                        Err(e) => {
                                            warn!("Failed to read local image {}: {}", path, e);
                                            tool_content.push(serde_json::json!({
                                                "type": "text",
                                                "text": format!("[Failed to read image: {}]", url),
                                            }));
                                        }
                                    }
                                }
                            }
                            ContentPart::File { url, name, .. } => {
                                tool_content.push(serde_json::json!({
                                    "type": "text",
                                    "text": format!(
                                        "用户上传文件 {}，已保存到本地路径: {}。你可以用 Read 工具读取文件内容。",
                                        name, url
                                    )
                                }));
                            }
                            ContentPart::Video { url, .. } => {
                                tool_content.push(serde_json::json!({
                                    "type": "text",
                                    "text": format!("[Video: {}]", url),
                                }));
                            }
                            _ => {}
                        }
                    }
                    content.push(serde_json::json!({
                        "type": "tool_result",
                        "tool_use_id": result.call_id,
                        "content": tool_content,
                        "is_error": result.is_error,
                    }));
                }
                _ => {}
            }
        }

        serde_json::json!({
            "role": role,
            "content": content,
        })
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
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
        super::reasoning::merge_options(&mut config.generate_kwargs, &info.generate_kwargs);
        Ok(Box::new(AnthropicChatModel {
            model_info: info,
            transport: self.transport.clone(),
            config,
        }))
    }

    async fn fetch_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        let mut models = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut cursors = std::collections::HashSet::new();
        let mut path = "/v1/models?limit=100".to_string();
        loop {
            let response = self.transport.get(&path).await?;
            let value: serde_json::Value =
                serde_json::from_str(&response).map_err(|error| LlmError::InvalidResponse {
                    provider: self.config.provider_id.clone(),
                    message: error.to_string(),
                })?;
            let rows = value
                .get("data")
                .and_then(|value| value.as_array())
                .ok_or_else(|| LlmError::InvalidResponse {
                    provider: self.config.provider_id.clone(),
                    message: "missing model list".into(),
                })?;
            for row in rows {
                let Some(id) = row
                    .get("id")
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    .filter(|id| !id.is_empty() && seen.insert(id.to_string()))
                else {
                    continue;
                };
                models.push(ModelInfo {
                    id: id.into(),
                    name: row
                        .get("display_name")
                        .and_then(|value| value.as_str())
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .unwrap_or(id)
                        .into(),
                    context_window: row
                        .get("context_window")
                        .and_then(|value| value.as_u64())
                        .filter(|value| *value >= 1000),
                    max_output_tokens: row
                        .get("max_output_tokens")
                        .and_then(|value| value.as_u64())
                        .filter(|value| *value > 0),
                    supports_multimodal: None,
                    supports_image: None,
                    supports_video: None,
                    probe_source: Some("api".into()),
                    generate_kwargs: serde_json::Map::new(),
                });
            }
            if value.get("has_more").and_then(|value| value.as_bool()) != Some(true) {
                break;
            }
            let cursor = value
                .get("last_id")
                .and_then(|value| value.as_str())
                .filter(|cursor| !cursor.is_empty())
                .filter(|cursor| cursors.insert(cursor.to_string()))
                .ok_or_else(|| LlmError::InvalidResponse {
                    provider: self.config.provider_id.clone(),
                    message: "invalid model pagination cursor".into(),
                })?;
            let mut url =
                reqwest::Url::parse("https://pagination.invalid/v1/models").expect("constant URL");
            url.query_pairs_mut()
                .append_pair("limit", "100")
                .append_pair("after_id", cursor);
            path = format!("{}?{}", url.path(), url.query().unwrap_or_default());
        }
        Ok(models)
    }

    async fn check_connection(&self) -> Result<ConnectionStatus, LlmError> {
        if !self.config.support_connection_check {
            return Ok(ConnectionStatus::Connected);
        }
        let body = serde_json::json!({
            "model": self.config.models.first().map(|m| m.id.as_str()).unwrap_or("claude-sonnet-4-20250514"),
            "max_tokens": 1,
            "messages": [{"role": "user", "content": "ping"}],
        });

        match self.transport.post("/v1/messages", &body).await {
            Ok(_) => Ok(ConnectionStatus::Connected),
            Err(LlmError::Auth { .. }) => {
                Ok(ConnectionStatus::Failed("authentication failed".into()))
            }
            Err(e) => Ok(ConnectionStatus::Failed(e.to_string())),
        }
    }
}

fn translate_anthropic_thinking_options(obj: &mut serde_json::Map<String, serde_json::Value>) {
    let disable_thinking = obj
        .remove("disable_thinking")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let thinking_enable = obj
        .remove("thinking_enable")
        .or_else(|| obj.remove("enable_thinking"))
        .or_else(|| obj.remove("thinking_enabled"))
        .and_then(|v| v.as_bool());
    let thinking_budget = obj.remove("thinking_budget").and_then(|v| v.as_u64());

    if disable_thinking || thinking_enable == Some(false) {
        if let Some(output_config) = obj
            .get_mut("output_config")
            .and_then(serde_json::Value::as_object_mut)
        {
            output_config.remove("effort");
        }
        obj.insert(
            "thinking".into(),
            serde_json::json!({
                "type": "disabled"
            }),
        );
        return;
    }

    if obj.contains_key("thinking") {
        return;
    }

    if thinking_enable == Some(true) || thinking_budget.is_some() {
        let mut max_tokens = obj
            .get("max_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(2048);
        let budget = thinking_budget.unwrap_or(max_tokens / 2).max(1024);
        if budget >= max_tokens {
            max_tokens = budget + 1024;
            obj.insert("max_tokens".into(), serde_json::json!(max_tokens));
        }
        obj.insert(
            "thinking".into(),
            serde_json::json!({
                "type": "enabled",
                "budget_tokens": budget
            }),
        );
    }
}

fn apply_anthropic_reasoning(
    body: &mut serde_json::Map<String, serde_json::Value>,
    reasoning: Option<&ReasoningConfig>,
) {
    let Some(reasoning) = reasoning else {
        return;
    };
    if let Some(output_config) = body
        .get_mut("output_config")
        .and_then(|value| value.as_object_mut())
    {
        output_config.remove("effort");
        if output_config.is_empty() {
            body.remove("output_config");
        }
    }
    if reasoning.mode == ReasoningMode::Effort {
        body.insert(
            "thinking".into(),
            serde_json::json!({
                "type": if reasoning.enabled { "adaptive" } else { "disabled" }
            }),
        );
        let mut output_config = body
            .remove("output_config")
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();
        output_config.remove("effort");
        if let Some(effort) = reasoning.effort.as_ref().filter(|_| reasoning.enabled) {
            output_config.insert("effort".into(), serde_json::json!(effort));
        }
        if !output_config.is_empty() {
            body.insert(
                "output_config".into(),
                serde_json::Value::Object(output_config),
            );
        }
        return;
    }
    if !reasoning.enabled {
        body.insert("thinking".into(), serde_json::json!({ "type": "disabled" }));
        return;
    }
    let mut max_tokens = body
        .get("max_tokens")
        .and_then(|value| value.as_u64())
        .unwrap_or(2048);
    let budget = reasoning
        .budget_tokens
        .and_then(|value| u64::try_from(value).ok())
        .unwrap_or(max_tokens / 2)
        .max(1024);
    if budget >= max_tokens {
        max_tokens = budget + 1024;
        body.insert("max_tokens".into(), serde_json::json!(max_tokens));
    }
    body.insert(
        "thinking".into(),
        serde_json::json!({
            "type": "enabled",
            "budget_tokens": budget
        }),
    );
}

struct AnthropicChatModel {
    model_info: ModelInfo,
    transport: Arc<HttpTransport>,
    config: AnthropicConfig,
}

#[async_trait]
impl ChatModel for AnthropicChatModel {
    async fn chat(&self, mut request: ChatRequest) -> Result<ChatResponse, LlmError> {
        request.stream = false;
        if request.model.is_none() {
            request.model = Some(self.model_info.id.clone());
        }

        let provider = AnthropicProvider {
            config: self.config.clone(),
            transport: self.transport.clone(),
            info: ProviderInfo {
                id: self.config.provider_id.clone(),
                name: self.config.provider_name.clone(),
                provider_type: Some("AnthropicChatModel".into()),
                is_local: false,
                support_model_discovery: false,
                support_connection_check: false,
            },
        };

        let body = provider.build_request_body(&request);
        crate::debug_capture::record_provider_request(
            &request.metadata,
            &self.config.provider_id,
            "/v1/messages",
            &body,
        );
        let response_text = self.transport.post("/v1/messages", &body).await?;

        let value: serde_json::Value =
            serde_json::from_str(&response_text).map_err(|e| LlmError::InvalidResponse {
                provider: self.config.provider_id.clone(),
                message: format!("failed to parse response: {e}"),
            })?;

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
                reasoning_tokens: extract_reasoning_tokens(u),
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

    fn chat_stream(&self, mut request: ChatRequest) -> EventStream {
        request.stream = true;
        if request.model.is_none() {
            request.model = Some(self.model_info.id.clone());
        }

        let provider = AnthropicProvider {
            config: self.config.clone(),
            transport: self.transport.clone(),
            info: ProviderInfo {
                id: self.config.provider_id.clone(),
                name: self.config.provider_name.clone(),
                provider_type: Some("AnthropicChatModel".into()),
                is_local: false,
                support_model_discovery: false,
                support_connection_check: false,
            },
        };

        let body = provider.build_request_body(&request);
        crate::debug_capture::record_provider_request(
            &request.metadata,
            &self.config.provider_id,
            "/v1/messages",
            &body,
        );
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
                                    warn!(provider = %provider_id, error = %e, "failed to parse Anthropic SSE event");
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

fn extract_data_uri_parts(url: &str) -> (String, String) {
    if let Some(rest) = url.strip_prefix("data:") {
        if let Some(semicolon) = rest.find(';') {
            let mime = rest[..semicolon].to_string();
            if let Some(comma) = rest.find(',') {
                let data = rest[comma + 1..].to_string();
                return (mime, data);
            }
        }
    }
    ("image/png".to_string(), url.to_string())
}
