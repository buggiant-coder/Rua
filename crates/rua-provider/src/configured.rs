use std::sync::Arc;

use agent_types::{ChatRequest, ChatResponse, ReasoningConfig, ReasoningMode};
use async_trait::async_trait;
use llm_multiplex::{
    ChatModel, ConnectionStatus, EventStream, LlmError, ModelInfo, Provider, ProviderInfo,
};

use crate::config_loader::ProviderConfig;

/// Applies persisted model defaults uniformly across all protocol adapters.
pub(crate) struct ConfiguredProvider {
    inner: Arc<dyn Provider>,
    config: ProviderConfig,
    info: ProviderInfo,
}

impl ConfiguredProvider {
    pub fn new(inner: Arc<dyn Provider>, config: ProviderConfig) -> Self {
        let mut info = inner.info().clone();
        info.support_model_discovery = config.support_model_discovery;
        info.support_connection_check = config.support_connection_check;
        Self {
            inner,
            config,
            info,
        }
    }
}

#[async_trait]
impl Provider for ConfiguredProvider {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    fn supports_model(&self, model: &str) -> bool {
        self.inner.supports_model(model)
    }

    fn create_chat_model(&self, model_id: &str) -> Result<Box<dyn ChatModel>, LlmError> {
        let model = self
            .config
            .all_models()
            .into_iter()
            .find(|model| model.id == model_id);
        let mut defaults = self.config.generate_kwargs.clone();
        if let Some(model) = model {
            crate::manager::deep_merge_maps(&mut defaults, &model.generate_kwargs);
        }
        let reasoning = model.and_then(|model| {
            let mode = match model.thinking_param_style.as_deref() {
                Some("effort") => ReasoningMode::Effort,
                Some("budget") => ReasoningMode::Budget,
                _ => return None,
            };
            Some(ReasoningConfig {
                mode,
                enabled: model.thinking_enabled?,
                effort: model.reasoning_effort.clone(),
                budget_tokens: model.thinking_budget,
                summary: None,
            })
        });
        Ok(Box::new(ConfiguredChatModel {
            inner: self.inner.create_chat_model(model_id)?,
            defaults_have_reasoning: has_reasoning_options(&defaults),
            reasoning,
            reasoning_format: model
                .and_then(|model| model.extra.get("reasoning_format"))
                .cloned(),
            off_effort: model
                .and_then(|model| model.extra.get("reasoning_off_effort"))
                .cloned(),
            off_budget: model
                .and_then(|model| model.extra.get("reasoning_off_budget"))
                .cloned(),
            effort_options: model
                .map(|model| model.reasoning_effort_options.clone())
                .unwrap_or_default(),
            budget_range: model.and_then(|model| model.thinking_budget_range),
        }))
    }

    async fn fetch_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        if !self.config.support_model_discovery {
            return Ok(self.config.to_model_infos());
        }
        let mut models = self.inner.fetch_models().await?;
        let template_id = self.config.template_id();
        models.retain(|model| crate::catalog::supports_chat(template_id, &model.id));
        if self
            .config
            .extra
            .get("merge_with_catalog")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        {
            if let Some(template) = crate::manager::PROVIDER_TEMPLATES
                .iter()
                .find(|template| template.id == template_id)
            {
                for known in template.to_config().to_model_infos() {
                    if !models.iter().any(|model| model.id == known.id) {
                        models.push(known);
                    }
                }
            }
        }
        Ok(models)
    }

    async fn check_connection(&self) -> Result<ConnectionStatus, LlmError> {
        if !self.config.support_connection_check {
            return Ok(ConnectionStatus::Connected);
        }
        if self.config.template_id() == "github-models" {
            let model = self
                .config
                .models
                .first()
                .map(|model| model.id.as_str())
                .unwrap_or("openai/gpt-4o-mini");
            let request = ChatRequest::new(vec![agent_types::ModelDialogueItem::text(
                agent_types::Role::User,
                "ping",
            )])
            .with_max_tokens(5);
            return Ok(match self.create_chat_model(model)?.chat(request).await {
                Ok(_) => ConnectionStatus::Connected,
                Err(error) => ConnectionStatus::Failed(error.to_string()),
            });
        }
        self.inner.check_connection().await
    }
}

struct ConfiguredChatModel {
    inner: Box<dyn ChatModel>,
    defaults_have_reasoning: bool,
    reasoning: Option<ReasoningConfig>,
    reasoning_format: Option<serde_json::Value>,
    off_effort: Option<serde_json::Value>,
    off_budget: Option<serde_json::Value>,
    effort_options: Vec<String>,
    budget_range: Option<[i64; 2]>,
}

impl ConfiguredChatModel {
    fn prepare(&self, request: &mut ChatRequest) -> Result<(), LlmError> {
        if let Some(format) = &self.reasoning_format {
            request
                .metadata
                .insert("rua_reasoning_format".into(), format.clone());
        }
        if let Some(effort) = &self.off_effort {
            request
                .metadata
                .insert("rua_reasoning_off_effort".into(), effort.clone());
        }
        if let Some(budget) = &self.off_budget {
            request
                .metadata
                .insert("rua_reasoning_off_budget".into(), budget.clone());
        }
        let auto = request.reasoning.as_ref().is_some_and(|reasoning| {
            reasoning.enabled && reasoning.effort.is_none() && reasoning.budget_tokens.is_none()
        });
        if auto {
            let summary = request
                .reasoning
                .as_ref()
                .and_then(|reasoning| reasoning.summary);
            request.reasoning = if self.defaults_have_reasoning
                || has_reasoning_options(&request.provider_options)
            {
                None
            } else {
                self.reasoning.clone()
            };
            if let Some(reasoning) = &mut request.reasoning {
                reasoning.summary = summary;
            }
        }
        if request
            .reasoning
            .as_ref()
            .is_some_and(|reasoning| !reasoning.enabled)
            && self.reasoning.is_none()
            && !self.defaults_have_reasoning
            && !has_reasoning_options(&request.provider_options)
        {
            request.reasoning = None;
        }
        match (&mut request.reasoning, &self.reasoning) {
            (Some(request), Some(defaults)) if !request.enabled => request.mode = defaults.mode,
            (Some(request), Some(defaults)) if request.enabled && request.mode == defaults.mode => {
                if request.effort.is_none() {
                    request.effort = defaults.effort.clone();
                }
                if request.budget_tokens.is_none() {
                    request.budget_tokens = defaults.budget_tokens;
                }
            }
            (None, defaults)
                if !self.defaults_have_reasoning
                    && !has_reasoning_options(&request.provider_options) =>
            {
                request.reasoning = defaults.clone()
            }
            _ => {}
        }
        if let Some(reasoning) = request
            .reasoning
            .as_ref()
            .filter(|reasoning| reasoning.enabled)
        {
            if self
                .reasoning
                .as_ref()
                .is_some_and(|defaults| defaults.mode != reasoning.mode)
            {
                return Err(LlmError::Config {
                    message: format!(
                        "model '{}' does not support the requested reasoning mode",
                        self.inner.model_info().id
                    ),
                });
            }
            if let Some(effort) = reasoning
                .effort
                .as_ref()
                .filter(|_| reasoning.mode == ReasoningMode::Effort)
            {
                if !self.effort_options.is_empty() && !self.effort_options.contains(effort) {
                    return Err(LlmError::Config { message: format!("model '{}' does not support reasoning effort '{effort}'; supported: {}", self.inner.model_info().id, self.effort_options.join(", ")) });
                }
            }
            if reasoning.mode == ReasoningMode::Budget {
                if let (Some(budget), Some([min, max])) =
                    (reasoning.budget_tokens, self.budget_range)
                {
                    if !(min..=max).contains(&budget) {
                        return Err(LlmError::Config {
                            message: format!(
                                "model '{}' thinking budget must be between {min} and {max}",
                                self.inner.model_info().id
                            ),
                        });
                    }
                }
            }
        }
        Ok(())
    }
}

fn has_reasoning_options(options: &serde_json::Map<String, serde_json::Value>) -> bool {
    [
        "think",
        "thinking",
        "reasoning",
        "thinkingConfig",
        "thinking_config",
        "thinking_enabled",
        "thinking_enable",
        "enable_thinking",
        "disable_thinking",
        "thinking_budget",
        "reasoning_effort",
    ]
    .iter()
    .any(|key| options.contains_key(*key))
        || options
            .get("extra_body")
            .and_then(serde_json::Value::as_object)
            .is_some_and(has_reasoning_options)
        || options
            .get("output_config")
            .is_some_and(|config| config.get("effort").is_some())
}

#[async_trait]
impl ChatModel for ConfiguredChatModel {
    async fn chat(&self, mut request: ChatRequest) -> Result<ChatResponse, LlmError> {
        self.prepare(&mut request)?;
        self.inner.chat(request).await
    }

    fn chat_stream(&self, mut request: ChatRequest) -> EventStream {
        if let Err(error) = self.prepare(&mut request) {
            return Box::pin(futures::stream::once(async { Err(error) }));
        }
        self.inner.chat_stream(request)
    }

    fn model_info(&self) -> &ModelInfo {
        self.inner.model_info()
    }
}
