use std::sync::Arc;

use llm_multiplex::{OpenAiCompatibleConfig, OpenAiCompatibleProvider, Provider};

use crate::config_loader::ProviderConfig;
use crate::openrouter_extended::{self, ExtendedModel, FilterCriteria, UpstreamProvider};
use llm_multiplex::LlmError;

const DEFAULT_REFERER: &str = "https://rua.dev";
const DEFAULT_TITLE: &str = "rua-rust";

pub fn create_openrouter_provider(config: &ProviderConfig) -> Result<Arc<dyn Provider>, LlmError> {
    let mut headers = config.effective_headers();

    if !headers.contains_key("HTTP-Referer") {
        headers.insert(
            "HTTP-Referer".into(),
            serde_json::Value::String(DEFAULT_REFERER.into()),
        );
    }
    if !headers.contains_key("X-Title") {
        headers.insert(
            "X-Title".into(),
            serde_json::Value::String(DEFAULT_TITLE.into()),
        );
    }
    headers
        .entry("X-OpenRouter-Title")
        .or_insert(serde_json::json!(DEFAULT_TITLE));
    headers
        .entry("X-OpenRouter-Categories")
        .or_insert(serde_json::json!("personal-agent,cli-agent"));

    let api_keys = if !config.api_keys.is_empty() {
        let mut keys = config.api_keys.clone();
        if !config.api_key.is_empty() && !keys.contains(&config.api_key) {
            keys.insert(0, config.api_key.clone());
        }
        keys
    } else if !config.api_key.is_empty() {
        vec![config.api_key.clone()]
    } else {
        vec![]
    };
    let provider_config = OpenAiCompatibleConfig {
        provider_id: config.id.clone(),
        provider_name: config.name.clone(),
        base_url: config.base_url.clone(),
        api_key: if config.api_key.is_empty() {
            None
        } else {
            Some(config.api_key.clone())
        },
        api_keys,
        headers,
        models: config.to_model_infos(),
        timeout_secs: config.timeout.unwrap_or(60),
        generate_kwargs: config.generate_kwargs.clone(),
        support_model_discovery: config.support_model_discovery,
        support_connection_check: config.support_connection_check,
        is_local: false,
    };

    let provider = OpenAiCompatibleProvider::new(provider_config)?;
    Ok(Arc::new(provider))
}

pub struct OpenRouterExtendedClient {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl OpenRouterExtendedClient {
    pub fn new(base_url: String, api_key: String) -> Self {
        Self {
            client: rua_http::default_client_or_default("rua-provider"),
            base_url,
            api_key,
        }
    }

    pub async fn fetch_extended_models(&self) -> Result<Vec<ExtendedModel>, reqwest::Error> {
        openrouter_extended::fetch_extended_models(&self.client, &self.base_url, &self.api_key)
            .await
    }

    pub fn filter_models(
        models: &[ExtendedModel],
        criteria: &FilterCriteria,
    ) -> Vec<ExtendedModel> {
        openrouter_extended::filter_models(models, criteria)
    }

    pub async fn get_available_providers(&self) -> Result<Vec<UpstreamProvider>, reqwest::Error> {
        openrouter_extended::get_available_providers(&self.client, &self.base_url, &self.api_key)
            .await
    }
}
