use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine;
use futures::StreamExt;
use tracing::warn;

use agent_types::{
    AgentEvent, ChatRequest, ChatResponse, ContentPart, FinishReason, ModelDialogueItem,
    ReasoningMode, Role, TokenUsage, ToolCall, ToolChoice,
};

use crate::error::LlmError;
use crate::event_parser::parse_gemini_chunk;
use crate::traits::{ChatModel, ConnectionStatus, EventStream, ModelInfo, Provider, ProviderInfo};
use crate::transport::{HttpTransport, TransportConfig};

pub use super::gemini_schema::{
    lower_tool_parameters as lower_gemini_tool_parameters,
    validate_tool_parameters as validate_gemini_tool_parameters,
};

fn gemini_thought_signature(call: &ToolCall) -> Option<&str> {
    call.metadata
        .get("extra_content")
        .and_then(|value| value.get("thought_signature"))
        .and_then(|value| value.as_str())
        .or_else(|| {
            let legacy_id = call.id.as_str();
            (call.provider_call_id.is_none()
                && legacy_id.len() >= 32
                && legacy_id.len().is_multiple_of(4)
                && base64::engine::general_purpose::STANDARD
                    .decode(legacy_id)
                    .is_ok())
            .then_some(legacy_id)
        })
}

pub fn flatten_json_schema(schema: &serde_json::Value) -> serde_json::Value {
    let mut schema = schema.clone();
    let defs = schema
        .as_object_mut()
        .and_then(|m| m.remove("$defs"))
        .unwrap_or(serde_json::Value::Null);

    fn resolve_ref(
        obj: &serde_json::Value,
        defs: &serde_json::Value,
        visited: &mut std::collections::HashSet<String>,
    ) -> serde_json::Value {
        match obj {
            serde_json::Value::Object(map) => {
                if let Some(ref_val) = map.get("$ref").and_then(|r| r.as_str()) {
                    if let Some(def_name) = ref_val.strip_prefix("#/$defs/") {
                        if visited.contains(def_name) {
                            return serde_json::json!({
                                "type": "object",
                                "description": format!("(circular: {def_name})")
                            });
                        }
                        visited.insert(def_name.to_string());
                        if let Some(def) = defs.get(def_name) {
                            let mut resolved = resolve_ref(def, defs, &mut visited.clone());
                            for (key, value) in map {
                                if key != "$ref" {
                                    if let Some(obj) = resolved.as_object_mut() {
                                        obj.insert(
                                            key.clone(),
                                            resolve_ref(value, defs, &mut visited.clone()),
                                        );
                                    }
                                }
                            }
                            return resolved;
                        }
                    }
                    return obj.clone();
                }
                let mut result = serde_json::Map::new();
                for (key, value) in map {
                    result.insert(key.clone(), resolve_ref(value, defs, &mut visited.clone()));
                }
                serde_json::Value::Object(result)
            }
            serde_json::Value::Array(arr) => serde_json::Value::Array(
                arr.iter()
                    .map(|item| resolve_ref(item, defs, &mut visited.clone()))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    let mut visited = std::collections::HashSet::new();
    resolve_ref(&schema, &defs, &mut visited)
}

#[derive(Debug, Clone)]
pub struct GeminiConfig {
    pub provider_id: String,
    pub provider_name: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub headers: serde_json::Map<String, serde_json::Value>,
    pub models: Vec<ModelInfo>,
    pub timeout_secs: u64,
    pub generate_kwargs: serde_json::Map<String, serde_json::Value>,
}

pub struct GeminiProvider {
    config: GeminiConfig,
    transport: Arc<HttpTransport>,
    info: ProviderInfo,
}

impl GeminiProvider {
    fn is_custom_endpoint(base_url: &str) -> bool {
        !base_url.is_empty() && !base_url.contains("googleapis.com")
    }

    pub fn new(config: GeminiConfig) -> Result<Self, LlmError> {
        let mut transport_config = TransportConfig::new(&config.base_url, &config.provider_id);

        if let Some(ref key) = config.api_key {
            if Self::is_custom_endpoint(&config.base_url) {
                transport_config = transport_config.with_api_key(key.clone());
            } else {
                transport_config = transport_config.with_header("x-goog-api-key", key);
            }
        }
        transport_config = transport_config
            .with_timeout(std::time::Duration::from_secs(config.timeout_secs))
            .with_headers_from_map(&config.headers);

        let transport = Arc::new(HttpTransport::new(transport_config)?);

        let info = ProviderInfo {
            id: config.provider_id.clone(),
            name: config.provider_name.clone(),
            provider_type: Some("GeminiChatModel".into()),
            is_local: false,
            support_model_discovery: true,
            support_connection_check: true,
        };

        Ok(Self {
            config,
            transport,
            info,
        })
    }

    pub fn build_request_body(&self, request: &ChatRequest, _model: &str) -> serde_json::Value {
        let messages = crate::request::normalize_tool_history(&request.messages);
        let provider_call_ids: HashMap<String, String> = messages
            .iter()
            .flat_map(|message| message.content.iter())
            .filter_map(|part| match part {
                ContentPart::ToolUse { call } => {
                    call.provider_call_id.as_ref().map(|provider_id| {
                        [
                            (call.id.clone(), provider_id.clone()),
                            (provider_id.clone(), provider_id.clone()),
                        ]
                    })
                }
                _ => None,
            })
            .flatten()
            .collect();
        let mut contents = Vec::new();

        for msg in &messages {
            if msg.role == Role::System {
                continue;
            }

            let role = match msg.role {
                Role::User | Role::Developer => "user",
                Role::Assistant => "model",
                Role::Tool => "user",
                _ => "user",
            };

            let parts: Vec<serde_json::Value> = msg
                .content
                .iter()
                .filter_map(|part| match part {
                    ContentPart::Text { text } => Some(serde_json::json!({"text": text})),
                    ContentPart::Thinking { text, signature } if msg.role == Role::Assistant => {
                        let mut thought = serde_json::json!({"text": text, "thought": true});
                        if let Some(sig) = signature {
                            thought["thoughtSignature"] = serde_json::json!(sig);
                        }
                        Some(thought)
                    }
                    ContentPart::ToolUse { call } if call.has_valid_identity() => {
                        let mut function_call = serde_json::json!({
                            "functionCall": {
                                "name": call.name,
                                "args": call.effective_arguments(),
                            }
                        });
                        if let Some(provider_call_id) = &call.provider_call_id {
                            function_call["functionCall"]["id"] =
                                serde_json::json!(provider_call_id);
                        }
                        if let Some(signature) = gemini_thought_signature(call) {
                            function_call["thoughtSignature"] = serde_json::json!(signature);
                        }
                        Some(function_call)
                    }
                    ContentPart::ToolResult { result } => {
                        if !result.has_valid_call_id() {
                            return None;
                        }
                        let mut response_parts = Vec::new();
                        for part in &result.content {
                            match part {
                                ContentPart::Text { text } => {
                                    response_parts.push(text.clone());
                                }
                                ContentPart::Image { url, .. } => {
                                    response_parts.push(format!("[Image: {}]", url));
                                }
                                ContentPart::File { url, name, .. } => {
                                    response_parts.push(format!("[File: {} at {}]", name, url));
                                }
                                ContentPart::Video { url, .. } => {
                                    response_parts.push(format!("[Video: {}]", url));
                                }
                                _ => {}
                            }
                        }
                        let result_text = response_parts.join("\n");
                        let mut function_response = serde_json::json!({
                            "functionResponse": {
                                "name": result.name.as_deref().unwrap_or(""),
                                "response": {"result": result_text},
                            }
                        });
                        if let Some(provider_call_id) = provider_call_ids.get(&result.call_id) {
                            function_response["functionResponse"]["id"] =
                                serde_json::json!(provider_call_id);
                        }
                        Some(function_response)
                    }
                    ContentPart::Image {
                        url, media_type, ..
                    } => convert_media_to_gemini_part(url, media_type.as_deref(), "image/png"),
                    ContentPart::Video {
                        url, media_type, ..
                    } => convert_media_to_gemini_part(url, media_type.as_deref(), "video/mp4"),
                    _ => None,
                })
                .collect();

            if !parts.is_empty() {
                contents.push(serde_json::json!({"role": role, "parts": parts}));
            }
        }

        let mut body = serde_json::json!({ "contents": contents });

        let system_text: Vec<String> = request
            .messages
            .iter()
            .filter(|m| m.role == Role::System)
            .map(|m| m.text_content())
            .collect();
        if !system_text.is_empty() {
            body["systemInstruction"] = serde_json::json!({
                "parts": [{"text": system_text.join("\n\n")}]
            });
        }

        let mut gen_config = serde_json::Map::new();
        if let Some(temp) = request.temperature {
            gen_config.insert("temperature".into(), serde_json::json!(temp));
        }
        if let Some(max_tokens) = request.max_tokens {
            gen_config.insert("maxOutputTokens".into(), serde_json::json!(max_tokens));
        }

        if let Some(ref so) = request.structured_output {
            gen_config.insert(
                "responseMimeType".into(),
                serde_json::json!("application/json"),
            );
            let mut schema = so.schema.clone();
            crate::request::remove_title_field_public(&mut schema);
            gen_config.insert("responseSchema".into(), schema);
        }

        super::reasoning::apply_defaults(&mut gen_config, &self.config.generate_kwargs);
        super::reasoning::merge_options(&mut gen_config, &request.provider_options);
        super::reasoning::flatten_extra_body(&mut gen_config);
        if request.reasoning.is_some() {
            super::reasoning::clear_controls(&mut gen_config);
        }
        translate_gemini_reasoning_options(&mut gen_config, request);
        apply_gemini_reasoning(&mut gen_config, request);
        if !gen_config.is_empty() {
            body["generationConfig"] = serde_json::Value::Object(gen_config);
        }

        if !request.tools.is_empty() {
            let functions: Vec<serde_json::Value> = request
                .tools
                .iter()
                .map(|t| {
                    let params = lower_gemini_tool_parameters(&flatten_json_schema(&t.parameters));
                    serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "parameters": params,
                    })
                })
                .collect();
            body["tools"] = serde_json::json!([{"functionDeclarations": functions}]);

            if let Some(ref tc) = request.tool_choice {
                let tool_config = match tc {
                    ToolChoice::Auto => serde_json::json!({
                        "function_calling_config": {"mode": "AUTO"}
                    }),
                    ToolChoice::None => serde_json::json!({
                        "function_calling_config": {"mode": "NONE"}
                    }),
                    ToolChoice::Required => serde_json::json!({
                        "function_calling_config": {"mode": "ANY"}
                    }),
                    ToolChoice::Function(name) => serde_json::json!({
                        "function_calling_config": {
                            "mode": "ANY",
                            "allowed_function_names": [name],
                        }
                    }),
                };
                body["toolConfig"] = tool_config;
            }
        }

        merge_function_response_contents(&mut body);
        body
    }

    pub fn get_endpoint(&self, model: &str, stream: bool) -> String {
        let key_param = if !Self::is_custom_endpoint(&self.config.base_url) {
            self.config
                .api_key
                .as_deref()
                .map(|k| format!("key={k}"))
                .unwrap_or_default()
        } else {
            String::new()
        };

        if stream {
            if key_param.is_empty() {
                format!("/v1beta/models/{model}:streamGenerateContent?alt=sse")
            } else {
                format!("/v1beta/models/{model}:streamGenerateContent?alt=sse&{key_param}")
            }
        } else if key_param.is_empty() {
            format!("/v1beta/models/{model}:generateContent")
        } else {
            format!("/v1beta/models/{model}:generateContent?{key_param}")
        }
    }
}

fn translate_gemini_reasoning_options(
    gen_config: &mut serde_json::Map<String, serde_json::Value>,
    request: &ChatRequest,
) {
    super::reasoning::flatten_extra_body(gen_config);
    if let Some(serde_json::Value::Object(mut config)) = gen_config.remove("thinking_config") {
        for (source, target) in [
            ("thinking_budget", "thinkingBudget"),
            ("thinking_level", "thinkingLevel"),
            ("include_thoughts", "includeThoughts"),
        ] {
            if let Some(value) = config.remove(source) {
                config.entry(target).or_insert(value);
            }
        }
        let target = gen_config
            .entry("thinkingConfig")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(target) = target.as_object_mut() {
            super::reasoning::merge_options(target, &config);
        }
    }
    let enabled = gen_config
        .remove("thinking_enabled")
        .and_then(|value| value.as_bool());
    let legacy_enabled = gen_config
        .remove("thinking_enable")
        .and_then(|value| value.as_bool());
    let compat_enabled = gen_config
        .remove("enable_thinking")
        .and_then(|value| value.as_bool());
    let disabled = gen_config
        .remove("disable_thinking")
        .and_then(|value| value.as_bool())
        == Some(true);
    let thinking_enabled = if disabled {
        Some(false)
    } else {
        enabled.or(legacy_enabled).or(compat_enabled)
    };
    let thinking_budget = gen_config.remove("thinking_budget").and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
    });
    let effort = gen_config.remove("reasoning_effort");

    if thinking_enabled.is_none() && thinking_budget.is_none() && effort.is_none() {
        return;
    }

    let mut thinking_config = gen_config
        .remove("thinkingConfig")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    if let Some(enabled) = thinking_enabled {
        thinking_config
            .entry("includeThoughts")
            .or_insert(serde_json::Value::Bool(enabled));
        if !enabled {
            thinking_config.remove("thinkingLevel");
            thinking_config.remove("thinkingBudget");
            if let Some(effort) = request.metadata.get("rua_reasoning_off_effort") {
                thinking_config.insert("thinkingLevel".into(), effort.clone());
            } else {
                let budget = request
                    .metadata
                    .get("rua_reasoning_off_budget")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0);
                thinking_config.insert("thinkingBudget".into(), serde_json::json!(budget));
            }
        }
    }
    if thinking_enabled != Some(false) {
        if let Some(effort) = effort {
            thinking_config.remove("thinkingBudget");
            thinking_config.insert("thinkingLevel".into(), effort);
        } else if let Some(budget) = thinking_budget {
            thinking_config
                .entry("thinkingBudget")
                .or_insert(serde_json::Value::Number(budget.into()));
        }
    }
    gen_config.insert(
        "thinkingConfig".into(),
        serde_json::Value::Object(thinking_config),
    );
}

fn apply_gemini_reasoning(
    generation_config: &mut serde_json::Map<String, serde_json::Value>,
    request: &ChatRequest,
) {
    let Some(reasoning) = request.reasoning.as_ref() else {
        return;
    };
    let mut thinking = generation_config
        .remove("thinkingConfig")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    thinking.remove("thinkingBudget");
    thinking.remove("thinkingLevel");
    thinking.insert(
        "includeThoughts".into(),
        serde_json::Value::Bool(reasoning.enabled),
    );
    if !reasoning.enabled {
        match reasoning.mode {
            ReasoningMode::Effort => {
                thinking.insert(
                    "thinkingLevel".into(),
                    serde_json::json!(super::reasoning::off_effort(request, "low")),
                );
            }
            ReasoningMode::Budget => {
                let budget = request
                    .metadata
                    .get("rua_reasoning_off_budget")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0);
                thinking.insert("thinkingBudget".into(), serde_json::json!(budget));
            }
        }
    } else {
        match reasoning.mode {
            ReasoningMode::Budget => {
                if let Some(budget) = reasoning.budget_tokens {
                    thinking.insert("thinkingBudget".into(), serde_json::json!(budget));
                }
            }
            ReasoningMode::Effort => {
                if let Some(effort) = &reasoning.effort {
                    thinking.insert("thinkingLevel".into(), serde_json::json!(effort));
                }
            }
        }
    }
    generation_config.insert("thinkingConfig".into(), serde_json::Value::Object(thinking));
}

#[async_trait]
impl Provider for GeminiProvider {
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
        super::reasoning::merge_options(&mut config.generate_kwargs, &info.generate_kwargs);
        Ok(Box::new(GeminiChatModel {
            model_info: info,
            transport: self.transport.clone(),
            config,
        }))
    }

    async fn fetch_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        let mut models = Vec::new();
        let mut seen = HashSet::new();
        let mut cursors = HashSet::new();
        let mut path = "/v1beta/models".to_string();
        loop {
            let body = self.transport.get(&path).await?;
            let value: serde_json::Value =
                serde_json::from_str(&body).map_err(|e| LlmError::InvalidResponse {
                    provider: self.config.provider_id.clone(),
                    message: format!("failed to parse Gemini models response: {e}"),
                })?;

            let page: Vec<ModelInfo> = value
                .get("models")
                .and_then(|models| models.as_array())
                .map(|rows| {
                    rows.iter()
                        .filter_map(|row| {
                            if row
                                .get("supportedGenerationMethods")
                                .and_then(|value| value.as_array())
                                .is_some_and(|methods| {
                                    !methods.iter().any(|method| method == "generateContent")
                                })
                            {
                                return None;
                            }
                            let raw_name = row.get("name")?.as_str()?.trim();
                            let id = raw_name
                                .strip_prefix("models/")
                                .unwrap_or(raw_name)
                                .trim()
                                .to_string();
                            if id.is_empty() || !seen.insert(id.clone()) {
                                return None;
                            }
                            let display_name = row
                                .get("displayName")
                                .or_else(|| row.get("display_name"))
                                .and_then(|name| name.as_str())
                                .map(str::trim)
                                .filter(|name| !name.is_empty())
                                .map(|name| name.strip_prefix("models/").unwrap_or(name))
                                .filter(|name| !name.is_empty())
                                .unwrap_or(&id)
                                .to_string();
                            Some(ModelInfo {
                                id: id.clone(),
                                name: display_name,
                                context_window: row
                                    .get("inputTokenLimit")
                                    .or_else(|| row.get("input_token_limit"))
                                    .and_then(|limit| limit.as_u64()),
                                max_output_tokens: row
                                    .get("outputTokenLimit")
                                    .or_else(|| row.get("output_token_limit"))
                                    .and_then(|limit| limit.as_u64()),
                                supports_multimodal: None,
                                supports_image: None,
                                supports_video: None,
                                probe_source: Some("api".into()),
                                generate_kwargs: serde_json::Map::new(),
                            })
                        })
                        .collect()
                })
                .ok_or_else(|| LlmError::InvalidResponse {
                    provider: self.config.provider_id.clone(),
                    message: "missing model list".into(),
                })?;
            models.extend(page);
            let Some(cursor) = value
                .get("nextPageToken")
                .and_then(serde_json::Value::as_str)
                .filter(|cursor| !cursor.is_empty())
            else {
                break;
            };
            if !cursors.insert(cursor.to_string()) {
                return Err(LlmError::InvalidResponse {
                    provider: self.config.provider_id.clone(),
                    message: "repeated model pagination cursor".into(),
                });
            }
            let mut url = reqwest::Url::parse("https://pagination.invalid/v1beta/models")
                .expect("constant URL");
            url.query_pairs_mut().append_pair("pageToken", cursor);
            path = format!("{}?{}", url.path(), url.query().unwrap_or_default());
        }

        Ok(models)
    }

    async fn check_connection(&self) -> Result<ConnectionStatus, LlmError> {
        match self.transport.get("/v1beta/models").await {
            Ok(_) => Ok(ConnectionStatus::Connected),
            Err(e) => Ok(ConnectionStatus::Failed(e.to_string())),
        }
    }
}

struct GeminiChatModel {
    model_info: ModelInfo,
    transport: Arc<HttpTransport>,
    config: GeminiConfig,
}

#[async_trait]
impl ChatModel for GeminiChatModel {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let model = request.model.as_deref().unwrap_or(&self.model_info.id);
        let provider = GeminiProvider {
            config: self.config.clone(),
            transport: self.transport.clone(),
            info: ProviderInfo {
                id: self.config.provider_id.clone(),
                name: self.config.provider_name.clone(),
                provider_type: Some("GeminiChatModel".into()),
                is_local: false,
                support_model_discovery: false,
                support_connection_check: false,
            },
        };

        let endpoint = provider.get_endpoint(model, false);
        let body = provider.build_request_body(&request, model);
        crate::debug_capture::record_provider_request(
            &request.metadata,
            &self.config.provider_id,
            "/v1beta/models/{model}:generateContent",
            &body,
        );
        let response_text = self.transport.post(&endpoint, &body).await?;

        let value: serde_json::Value =
            serde_json::from_str(&response_text).map_err(|e| LlmError::InvalidResponse {
                provider: self.config.provider_id.clone(),
                message: format!("failed to parse response: {e}"),
            })?;

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
                                    signature: part
                                        .get("thoughtSignature")
                                        .and_then(|s| s.as_str())
                                        .map(String::from),
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
                            if let Some(signature) =
                                part.get("thoughtSignature").and_then(|s| s.as_str())
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

    fn chat_stream(&self, request: ChatRequest) -> EventStream {
        let model = request
            .model
            .clone()
            .unwrap_or_else(|| self.model_info.id.clone());
        let provider = GeminiProvider {
            config: self.config.clone(),
            transport: self.transport.clone(),
            info: ProviderInfo {
                id: self.config.provider_id.clone(),
                name: self.config.provider_name.clone(),
                provider_type: Some("GeminiChatModel".into()),
                is_local: false,
                support_model_discovery: false,
                support_connection_check: false,
            },
        };

        let endpoint = provider.get_endpoint(&model, true);
        let body = provider.build_request_body(&request, &model);
        crate::debug_capture::record_provider_request(
            &request.metadata,
            &self.config.provider_id,
            "/v1beta/models/{model}:streamGenerateContent",
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

            let byte_stream = match transport.post_stream(&endpoint, &body).await {
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


                        let (lines, remaining) = crate::transport::parse_sse_lines(&buffer);
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
                                    warn!(provider = %provider_id, error = %e, "failed to parse Gemini chunk");
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

fn extract_data_uri_parts(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("data:")?;
    let semicolon = rest.find(';')?;
    let mime = rest[..semicolon].to_string();
    let comma = rest.find(',')?;
    let data = rest[comma + 1..].to_string();
    Some((mime, data))
}

fn guess_mime_from_url(url: &str, fallback: &str) -> String {
    let path = url.split('?').next().unwrap_or(url);
    if let Some(dot) = path.rfind('.') {
        match &path[dot..].to_ascii_lowercase()[..] {
            ".png" => return "image/png".into(),
            ".jpg" | ".jpeg" => return "image/jpeg".into(),
            ".gif" => return "image/gif".into(),
            ".webp" => return "image/webp".into(),
            ".bmp" => return "image/bmp".into(),
            ".mp4" => return "video/mp4".into(),
            ".webm" => return "video/webm".into(),
            ".mov" => return "video/quicktime".into(),
            ".avi" => return "video/x-msvideo".into(),
            _ => {}
        }
    }
    fallback.into()
}

fn convert_media_to_gemini_part(
    url: &str,
    media_type: Option<&str>,
    default_mime: &str,
) -> Option<serde_json::Value> {
    if url.starts_with("data:") {
        if let Some((extracted_mime, data)) = extract_data_uri_parts(url) {
            let mime = media_type.unwrap_or(&extracted_mime);
            return Some(serde_json::json!({
                "inlineData": {
                    "mimeType": mime,
                    "data": data,
                }
            }));
        }
        warn!("failed to parse data URI for Gemini request");
        return None;
    }

    if url.starts_with("http://") || url.starts_with("https://") {
        let mime = media_type
            .map(String::from)
            .unwrap_or_else(|| guess_mime_from_url(url, default_mime));
        return Some(serde_json::json!({
            "fileData": {
                "mimeType": mime,
                "fileUri": url,
            }
        }));
    }

    warn!(url = %url, "unsupported media URL scheme for Gemini, skipping");
    None
}

pub fn strip_function_response_ids(body: &mut serde_json::Value) {
    let contents = match body.get_mut("contents").and_then(|c| c.as_array_mut()) {
        Some(c) => c,
        None => return,
    };
    for msg in contents.iter_mut() {
        let parts = match msg.get_mut("parts").and_then(|p| p.as_array_mut()) {
            Some(p) => p,
            None => continue,
        };
        for part in parts.iter_mut() {
            if let Some(fr) = part.get_mut("functionResponse") {
                if let Some(obj) = fr.as_object_mut() {
                    obj.remove("id");
                }
            }
        }
    }
}

pub fn merge_function_response_contents(body: &mut serde_json::Value) {
    let Some(contents) = body.get_mut("contents").and_then(|c| c.as_array_mut()) else {
        return;
    };

    let mut index = 0;
    while index < contents.len() {
        if !is_function_response_only_content(&contents[index]) {
            index += 1;
            continue;
        }

        contents[index]["role"] = serde_json::json!("user");
        let mut end = index + 1;
        while end < contents.len() && is_function_response_only_content(&contents[end]) {
            end += 1;
        }
        if end > index + 1 {
            let mut merged_parts = Vec::new();
            for content in contents.iter().take(end).skip(index) {
                if let Some(parts) = content.get("parts").and_then(|parts| parts.as_array()) {
                    merged_parts.extend(parts.iter().cloned());
                }
            }
            contents[index]["parts"] = serde_json::Value::Array(merged_parts);
            contents.drain(index + 1..end);
        }
        index += 1;
    }
}

fn is_function_response_only_content(content: &serde_json::Value) -> bool {
    let role = content.get("role").and_then(|role| role.as_str());
    if !matches!(role, Some("user" | "function")) {
        return false;
    }
    let Some(parts) = content.get("parts").and_then(|parts| parts.as_array()) else {
        return false;
    };
    !parts.is_empty()
        && parts
            .iter()
            .all(|part| part.is_object() && part.get("functionResponse").is_some())
}
