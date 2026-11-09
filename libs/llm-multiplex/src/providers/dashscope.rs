use std::collections::HashMap;
use std::pin::Pin;

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use tracing::{info, warn};

use agent_types::{
    AgentEvent, ChatRequest, ChatResponse, ContentPart, FinishReason, ModelDialogueItem, Role,
    TokenUsage, ToolCall, ToolChoice,
};

use crate::error::LlmError;
use crate::providers::openai_compatible::{OpenAiCompatibleConfig, OpenAiCompatibleProvider};
use crate::traits::{ChatModel, ConnectionStatus, EventStream, ModelInfo, Provider, ProviderInfo};

pub const DASHSCOPE_BASE_URL: &str = "https://dashscope.aliyuncs.com/compatible-mode/v1";

pub struct DashScopeProvider {
    inner: OpenAiCompatibleProvider,
    info: ProviderInfo,
    generate_kwargs: serde_json::Map<String, serde_json::Value>,
}

impl DashScopeProvider {
    pub fn from_config(mut config: OpenAiCompatibleConfig) -> Result<Self, LlmError> {
        inject_dashscope_defaults(&mut config.generate_kwargs);
        let mut headers = load_dashscope_headers();
        headers.extend(config.headers);
        config.headers = headers;
        let generate_kwargs = config.generate_kwargs.clone();
        Ok(Self::new(
            OpenAiCompatibleProvider::new(config)?,
            generate_kwargs,
        ))
    }

    fn new(
        inner: OpenAiCompatibleProvider,
        generate_kwargs: serde_json::Map<String, serde_json::Value>,
    ) -> Self {
        let mut info = inner.info().clone();
        info.provider_type = Some("DashScopeChatModel".into());
        Self {
            inner,
            info,
            generate_kwargs,
        }
    }
}

#[async_trait]
impl Provider for DashScopeProvider {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    fn supports_model(&self, model: &str) -> bool {
        self.inner.supports_model(model)
    }

    fn create_chat_model(&self, model: &str) -> Result<Box<dyn ChatModel>, LlmError> {
        let inner_model = self.inner.create_chat_model(model)?;
        let mut generate_kwargs = self.generate_kwargs.clone();
        super::reasoning::merge_options(
            &mut generate_kwargs,
            &inner_model.model_info().generate_kwargs,
        );
        Ok(Box::new(DashScopeChatModel {
            inner: inner_model,
            generate_kwargs,
        }))
    }

    async fn fetch_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        self.inner.fetch_models().await
    }

    async fn check_connection(&self) -> Result<ConnectionStatus, LlmError> {
        self.inner.check_connection().await
    }
}

struct DashScopeChatModel {
    inner: Box<dyn ChatModel>,
    generate_kwargs: serde_json::Map<String, serde_json::Value>,
}

impl DashScopeChatModel {
    fn apply_dashscope_adjustments(&self, request: &mut ChatRequest) {
        if let Some(ref tc) = request.tool_choice {
            if matches!(tc, ToolChoice::Required) {
                warn!("DashScope does not support tool_choice='required', downgrading to 'auto'");
                request.tool_choice = Some(ToolChoice::Auto);
            }
        }
    }
}

#[async_trait]
impl ChatModel for DashScopeChatModel {
    async fn chat(&self, mut request: ChatRequest) -> Result<ChatResponse, LlmError> {
        self.apply_dashscope_adjustments(&mut request);

        let mut options = serde_json::Map::new();
        super::reasoning::apply_defaults(&mut options, &self.generate_kwargs);
        super::reasoning::merge_options(&mut options, &request.provider_options);
        super::reasoning::flatten_extra_body(&mut options);
        let enable_thinking = request
            .reasoning
            .as_ref()
            .map(|reasoning| reasoning.enabled)
            .unwrap_or_else(|| {
                options
                    .get("disable_thinking")
                    .and_then(serde_json::Value::as_bool)
                    != Some(true)
                    && ["enable_thinking", "thinking_enabled", "thinking_enable"]
                        .iter()
                        .find_map(|key| options.get(*key).and_then(serde_json::Value::as_bool))
                        .unwrap_or_else(|| {
                            options
                                .get("thinking")
                                .and_then(|thinking| thinking.get("type"))
                                .and_then(serde_json::Value::as_str)
                                == Some("enabled")
                        })
            });
        if enable_thinking {
            info!("DashScope: stream must be true when enable_thinking is true, using streaming");
            let stream = self.inner.chat_stream(request);
            return collect_stream_to_response(stream).await;
        }

        self.inner.chat(request).await
    }

    fn chat_stream(&self, mut request: ChatRequest) -> EventStream {
        self.apply_dashscope_adjustments(&mut request);
        self.inner.chat_stream(request)
    }

    fn model_info(&self) -> &ModelInfo {
        self.inner.model_info()
    }
}

async fn collect_stream_to_response(
    mut stream: Pin<Box<dyn Stream<Item = Result<AgentEvent, LlmError>> + Send>>,
) -> Result<ChatResponse, LlmError> {
    let mut text = String::new();
    let mut thinking = String::new();
    let mut tool_calls: HashMap<String, ToolCall> = HashMap::new();
    let mut usage: Option<TokenUsage> = None;
    let mut finish_reason = FinishReason::Stop;
    let mut model: Option<String> = None;

    while let Some(event) = stream.next().await {
        match event? {
            AgentEvent::Start { model: m, .. } => {
                model = m;
            }
            AgentEvent::TextDelta { delta, .. } => {
                text.push_str(&delta);
            }
            AgentEvent::ThinkingDelta { delta, .. } => {
                thinking.push_str(&delta);
            }
            AgentEvent::ToolCallDelta {
                id,
                name,
                arguments_delta,
                ..
            } => {
                let entry = tool_calls.entry(id.clone()).or_insert_with(|| ToolCall {
                    id: id.clone(),
                    name: String::new(),
                    arguments: serde_json::Value::Null,
                    raw_arguments: Some(String::new()),
                    provider_call_id: Some(id),
                    metadata: serde_json::Map::new(),
                });
                if let Some(n) = name {
                    entry.name.push_str(&n);
                }
                if let Some(ref mut raw) = entry.raw_arguments {
                    raw.push_str(&arguments_delta);
                }
            }
            AgentEvent::Usage { usage: u } => {
                usage = Some(u);
            }
            AgentEvent::End {
                finish_reason: fr,
                usage: u,
                ..
            } => {
                finish_reason = fr;
                if u.is_some() {
                    usage = u;
                }
            }
            AgentEvent::Error { message, .. } => {
                return Err(LlmError::Server {
                    provider: "dashscope".into(),
                    status: None,
                    message,
                });
            }
            _ => {}
        }
    }

    let mut content = Vec::new();
    if !thinking.is_empty() {
        content.push(ContentPart::Thinking {
            text: thinking,
            signature: None,
        });
    }
    if !text.is_empty() {
        content.push(ContentPart::Text { text });
    }
    for (_, mut tc) in tool_calls {
        if let Some(ref raw) = tc.raw_arguments {
            tc.arguments = serde_json::from_str(raw).unwrap_or_else(|_| {
                rua_json_repair::repair_json(raw)
                    .map(|outcome| outcome.value)
                    .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()))
            });
        }
        content.push(ContentPart::ToolUse { call: tc });
    }

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
        extra: HashMap::new(),
    })
}

pub fn create_dashscope_provider(
    api_key: &str,
    models: Vec<ModelInfo>,
    generate_kwargs: serde_json::Map<String, serde_json::Value>,
) -> Result<DashScopeProvider, LlmError> {
    create_dashscope_provider_inner(DASHSCOPE_BASE_URL, api_key, models, generate_kwargs)
}

pub fn create_dashscope_provider_with_url(
    base_url: &str,
    api_key: &str,
    models: Vec<ModelInfo>,
    generate_kwargs: serde_json::Map<String, serde_json::Value>,
) -> Result<DashScopeProvider, LlmError> {
    create_dashscope_provider_inner(base_url, api_key, models, generate_kwargs)
}

fn create_dashscope_provider_inner(
    base_url: &str,
    api_key: &str,
    models: Vec<ModelInfo>,
    generate_kwargs: serde_json::Map<String, serde_json::Value>,
) -> Result<DashScopeProvider, LlmError> {
    let config = OpenAiCompatibleConfig {
        provider_id: "dashscope".into(),
        provider_name: "DashScope".into(),
        base_url: base_url.to_string(),
        api_key: Some(api_key.to_string()),
        api_keys: vec![api_key.to_string()],
        headers: serde_json::Map::new(),
        models,
        timeout_secs: 60,
        generate_kwargs,
        support_model_discovery: true,
        support_connection_check: true,
        is_local: false,
    };

    DashScopeProvider::from_config(config)
}

fn inject_dashscope_defaults(kwargs: &mut serde_json::Map<String, serde_json::Value>) {
    kwargs
        .entry("incremental_output")
        .or_insert(serde_json::Value::Bool(true));
}

pub fn load_dashscope_headers() -> serde_json::Map<String, serde_json::Value> {
    let mut headers = serde_json::Map::new();

    if let Ok(env_headers) = std::env::var("DASHSCOPE_API_HEADERS") {
        match serde_json::from_str::<serde_json::Value>(&env_headers) {
            Ok(serde_json::Value::Object(parsed)) => {
                for (k, v) in parsed {
                    headers.insert(k, v);
                }
            }
            _ => {
                warn!(
                    "Failed to parse DASHSCOPE_API_HEADERS environment variable as JSON. \
                     It should be a JSON object."
                );
            }
        }
    }

    inject_agentapp_header(&mut headers);

    headers
}

fn inject_agentapp_header(headers: &mut serde_json::Map<String, serde_json::Value>) {
    let agentapp = serde_json::json!({
        "agentType": "rua",
        "deployType": "UnKnown",
        "moduleCode": "model",
        "agentCode": "UnKnown",
    });
    headers.insert(
        "x-dashscope-agentapp".to_string(),
        serde_json::Value::String(agentapp.to_string()),
    );
}
