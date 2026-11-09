use std::path::{Path, PathBuf};

use rua_config::encrypted_io::EncryptedFileIO;
use rua_config::serde_helpers::deserialize_null_default;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use llm_multiplex::LlmError;

use crate::redaction::redact_provider_secret;
use crate::secret_field_io::{llm_config_error, read_provider_config, write_provider_config};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefaultModel {
    pub provider_id: String,

    #[serde(alias = "model_id")]
    pub model: String,

    pub source: Option<String>,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderModelEntry {
    pub id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub name: String,
    #[serde(default)]
    pub supports_multimodal: Option<bool>,
    #[serde(default)]
    pub supports_image: Option<bool>,
    #[serde(default)]
    pub supports_video: Option<bool>,
    pub probe_source: Option<String>,
    pub context_window: Option<u64>,
    pub max_output_tokens: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub generate_kwargs: serde_json::Map<String, serde_json::Value>,
    #[serde(default, alias = "thinkingEnabled")]
    pub thinking_enabled: Option<bool>,
    #[serde(default, alias = "thinkingBudget")]
    pub thinking_budget: Option<i64>,
    #[serde(default, alias = "reasoningEffort")]
    pub reasoning_effort: Option<String>,
    #[serde(default, alias = "thinkingParamStyle")]
    pub thinking_param_style: Option<String>,
    #[serde(
        default,
        alias = "reasoningEffortOptions",
        deserialize_with = "deserialize_null_default"
    )]
    pub reasoning_effort_options: Vec<String>,
    #[serde(default, alias = "thinkingBudgetRange")]
    pub thinking_budget_range: Option<[i64; 2]>,
    #[serde(default, alias = "relayReasoning")]
    pub relay_reasoning: Option<bool>,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderBaseUrlOption {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub label: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub name: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub base_url: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub api_key: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub api_keys: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub api_key_prefixes: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub headers: serde_json::Map<String, serde_json::Value>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub custom_headers: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub chat_model: Option<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub models: Vec<ProviderModelEntry>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub extra_models: Vec<ProviderModelEntry>,
    #[serde(default)]
    pub timeout: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub generate_kwargs: serde_json::Map<String, serde_json::Value>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub support_model_discovery: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub support_connection_check: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub is_local: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub api_key_prefix: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub is_custom: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub is_plugin: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub freeze_url: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub require_api_key: bool,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub provider_group: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub provider_group_name: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub provider_variant: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub base_url_options: Vec<ProviderBaseUrlOption>,
    #[serde(default, alias = "thinkingParamStyle")]
    pub thinking_param_style: Option<String>,
    #[serde(
        default,
        alias = "reasoningEffortOptions",
        deserialize_with = "deserialize_null_default"
    )]
    pub reasoning_effort_options: Vec<String>,
    #[serde(default, alias = "thinkingBudgetRange")]
    pub thinking_budget_range: Option<[i64; 2]>,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

pub fn protocol_thinking_param_style(chat_model: Option<&str>) -> Option<&'static str> {
    match chat_model {
        Some(
            "OpenAIResponseModel"
            | "DashScopeResponseModel"
            | "OllamaChatModel"
            | "OpenRouterChatModel",
        ) => Some("effort"),
        Some("AnthropicChatModel" | "GeminiChatModel" | "DashScopeChatModel") => Some("budget"),
        _ => None,
    }
}

impl ProviderConfig {
    pub fn protocol_thinking_param_style(&self) -> Option<&'static str> {
        protocol_thinking_param_style(self.chat_model.as_deref())
    }

    pub fn effective_headers(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut headers = self.headers.clone();
        for (key, value) in &self.custom_headers {
            headers.insert(key.clone(), value.clone());
        }
        headers
    }

    pub fn all_models(&self) -> Vec<&ProviderModelEntry> {
        self.models.iter().chain(self.extra_models.iter()).collect()
    }

    pub fn to_model_infos(&self) -> Vec<llm_multiplex::ModelInfo> {
        self.all_models()
            .into_iter()
            .map(|m| {
                let generate_kwargs = self.project_model_generate_kwargs(m);
                llm_multiplex::ModelInfo {
                    id: m.id.clone(),
                    name: m.name.clone(),
                    context_window: m.context_window,
                    max_output_tokens: m.max_output_tokens,
                    supports_multimodal: m.supports_multimodal,
                    supports_image: m.supports_image,
                    supports_video: m.supports_video,
                    probe_source: m.probe_source.clone(),
                    generate_kwargs,
                }
            })
            .collect()
    }

    pub fn project_model_generate_kwargs(
        &self,
        model: &ProviderModelEntry,
    ) -> serde_json::Map<String, serde_json::Value> {
        model.generate_kwargs.clone()
    }

    /// Fill model knowledge when configuration is created/imported, never at request time.
    pub fn apply_model_defaults(&mut self) -> bool {
        let mut changed = false;
        let template_id = self.template_id().to_string();
        let protocol_style = self
            .thinking_param_style
            .clone()
            .or_else(|| self.protocol_thinking_param_style().map(str::to_string));
        for model in self.models.iter_mut().chain(self.extra_models.iter_mut()) {
            changed |= crate::model_knowledge::apply_missing_defaults(&template_id, model);
            // Explicit controls on a custom model are evidence of support;
            // a protocol alone is not evidence for an unconfigured model.
            if model.thinking_param_style.is_none() {
                if model.thinking_budget.is_some() {
                    model.thinking_param_style = Some("budget".into());
                } else if model.reasoning_effort.is_some() {
                    model.thinking_param_style = Some("effort".into());
                } else if model.thinking_enabled.is_some() {
                    model.thinking_param_style =
                        protocol_style.clone().or_else(|| Some("effort".into()));
                }
                if model.thinking_param_style.is_some() {
                    changed = true;
                    crate::model_knowledge::apply_missing_defaults(&template_id, model);
                }
            }
        }
        changed
    }

    pub fn template_id(&self) -> &str {
        self.extra
            .get("template_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(&self.id)
    }

    pub fn redacted_summary(&self) -> String {
        format!(
            "Provider(id={}, name={}, base_url={}, api_key={}, models={})",
            self.id,
            self.name,
            self.base_url,
            redact_provider_secret(&self.api_key),
            self.all_models().len()
        )
    }
}

pub struct ProviderConfigLoader {
    secret_dir: PathBuf,

    key: Option<[u8; 32]>,
}

impl ProviderConfigLoader {
    pub fn new(secret_dir: impl Into<PathBuf>) -> Self {
        Self {
            secret_dir: secret_dir.into(),
            key: None,
        }
    }

    pub fn with_key(secret_dir: impl Into<PathBuf>, key: [u8; 32]) -> Self {
        Self {
            secret_dir: secret_dir.into(),
            key: Some(key),
        }
    }

    pub fn dev_profile() -> Self {
        Self::new(rua_config::dirs::resolve_secret_dir())
    }

    pub fn prod_profile() -> Self {
        let home = dirs_path();
        Self::new(home.join(".rua.secret"))
    }

    pub async fn load_default_model(&self) -> Result<DefaultModel, LlmError> {
        let path = self.secret_dir.join("providers").join("default_model.json");
        debug!(path = %path.display(), "loading default_model");

        let active: DefaultModel =
            EncryptedFileIO::read_json(&path, self.key.as_ref()).map_err(|e| LlmError::Config {
                message: format!(
                    "failed to read default_model.json at {}: {e}",
                    path.display()
                ),
            })?;

        debug!(
            provider_id = %active.provider_id,
            model = %active.model,
            "loaded active model"
        );

        Ok(active)
    }

    pub async fn load_provider(&self, provider_id: &str) -> Result<ProviderConfig, LlmError> {
        let path = self
            .secret_dir
            .join("providers")
            .join("providers")
            .join(format!("{provider_id}.json"));

        debug!(path = %path.display(), provider_id, "loading provider config");

        let read =
            read_provider_config(&path, self.key.as_ref()).map_err(|e| LlmError::Config {
                message: format!(
                    "failed to read provider config for '{}' at {}: {e}",
                    provider_id,
                    path.display()
                ),
            })?;
        if read.needs_rewrite {
            write_provider_config(&path, &read.config, self.key.as_ref())
                .map_err(|e| llm_config_error("migrate provider api_key", &path, e))?;
        }
        let config = read.config;

        debug!(summary = %config.redacted_summary(), "loaded provider config");

        Ok(config)
    }

    pub async fn load_all_providers(&self) -> Result<Vec<ProviderConfig>, LlmError> {
        let providers_dir = self.secret_dir.join("providers").join("providers");

        if !providers_dir.exists() {
            debug!(path = %providers_dir.display(), "providers directory not found");
            return Ok(Vec::new());
        }

        let mut configs = Vec::new();
        let mut entries =
            tokio::fs::read_dir(&providers_dir)
                .await
                .map_err(|e| LlmError::Config {
                    message: format!(
                        "failed to read providers directory {}: {e}",
                        providers_dir.display()
                    ),
                })?;

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }

            match read_provider_config(&path, self.key.as_ref()) {
                Ok(read) => {
                    if read.needs_rewrite {
                        if let Err(e) =
                            write_provider_config(&path, &read.config, self.key.as_ref())
                        {
                            warn!(path = %path.display(), error = %e, "failed to migrate provider api_key");
                        }
                    }
                    let config = read.config;
                    debug!(summary = %config.redacted_summary(), "loaded provider");
                    configs.push(config);
                }
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "skipping invalid provider config");
                }
            }
        }

        Ok(configs)
    }

    pub fn secret_dir(&self) -> &Path {
        &self.secret_dir
    }

    fn providers_dir(&self) -> PathBuf {
        self.secret_dir.join("providers").join("providers")
    }

    pub async fn save_provider(&self, config: &ProviderConfig) -> Result<(), LlmError> {
        let path = self.providers_dir().join(format!("{}.json", config.id));

        write_provider_config(&path, config, self.key.as_ref()).map_err(|e| LlmError::Config {
            message: format!(
                "failed to write provider config for '{}' at {}: {e}",
                config.id,
                path.display()
            ),
        })?;

        debug!(provider_id = %config.id, path = %path.display(), "saved provider config");
        Ok(())
    }

    pub async fn save_default_model(&self, active: &DefaultModel) -> Result<(), LlmError> {
        let path = self.secret_dir.join("providers").join("default_model.json");

        EncryptedFileIO::write_json(&path, active, None).map_err(|e| LlmError::Config {
            message: format!(
                "failed to write default_model.json at {}: {e}",
                path.display()
            ),
        })?;

        debug!(
            provider_id = %active.provider_id,
            model = %active.model,
            "saved active model"
        );
        Ok(())
    }

    pub async fn delete_default_model_file(&self) -> Result<(), LlmError> {
        let path = self.secret_dir.join("providers").join("default_model.json");

        if path.exists() {
            tokio::fs::remove_file(&path)
                .await
                .map_err(|e| LlmError::Config {
                    message: format!(
                        "failed to delete default_model.json at {}: {e}",
                        path.display()
                    ),
                })?;
            debug!(path = %path.display(), "deleted default_model.json");
        }

        Ok(())
    }

    pub async fn delete_provider_file(&self, provider_id: &str) -> Result<(), LlmError> {
        let path = self.providers_dir().join(format!("{provider_id}.json"));

        if path.exists() {
            tokio::fs::remove_file(&path)
                .await
                .map_err(|e| LlmError::Config {
                    message: format!(
                        "failed to delete provider config for '{}' at {}: {e}",
                        provider_id,
                        path.display()
                    ),
                })?;
            debug!(provider_id, path = %path.display(), "deleted provider config file");
        }

        Ok(())
    }
}

fn dirs_path() -> PathBuf {
    rua_config::dirs::home_dir()
}
