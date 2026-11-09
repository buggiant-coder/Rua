use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use agent_types::{ChatRequest, ModelDialogueItem, Role};
use llm_multiplex::{
    CapabilityBaseline, ChatModel, ConnectionStatus, LlmError, ModelInfo, MultimodalProber,
    OpenAiCompatibleConfig, OpenAiCompatibleProvider, ProbeResult, Provider, ProviderInfo,
};

use crate::config_loader::{
    protocol_thinking_param_style, DefaultModel, ProviderBaseUrlOption, ProviderConfig,
    ProviderConfigLoader, ProviderModelEntry,
};

static KNOWN_CONTEXT_WINDOWS: &[(&str, u64)] = &[
    ("claude-sonnet-5", 1_000_000),
    ("claude-fable-5", 1_000_000),
    ("claude-mythos-5", 1_000_000),
    ("claude-opus-4", 1_000_000),
    ("claude-sonnet-4", 1_000_000),
    ("claude-haiku-4", 1_000_000),
    ("claude-3.5", 200_000),
    ("claude-3", 200_000),
    ("gpt-5.6-luna", 400_000),
    ("gpt-5.6", 1_050_000),
    ("gpt-5.5", 1_050_000),
    ("gpt-5.4-mini", 400_000),
    ("gpt-5.4-nano", 400_000),
    ("gpt-5.4", 1_050_000),
    ("gpt-5", 1_047_576),
    ("gpt-4.1", 1_047_576),
    ("o3", 200_000),
    ("o4-mini", 200_000),
    ("gpt-4o", 128_000),
    ("gemini-3", 1_048_576),
    ("gemini-2.5", 1_048_576),
    ("gemini-2.0", 1_048_576),
    ("qwen3.7", 983_040),
    ("qwen3.6", 1_048_576),
    ("qwen3.5", 131_072),
    ("qwen3-235b", 131_072),
    ("qwen3", 32_768),
    ("qwen2.5", 131_072),
    ("kimi-k3", 1_048_576),
    ("kimi-k2.6", 256_000),
    ("kimi-k2.5", 256_000),
    ("glm-5.2", 1_048_576),
    ("glm-5.1", 128_000),
    ("glm-5", 202_000),
    ("glm-4", 128_000),
    ("deepseek-v4", 1_000_000),
    ("deepseek-v3.2", 1_000_000),
    ("deepseek-v3", 65_536),
    ("deepseek-r1", 65_536),
    ("minimax-m2", 196_608),
];

const DEFAULT_CONTEXT_WINDOW: u64 = 128 * 1024;

pub fn infer_context_window(model_id: &str) -> Option<u64> {
    let lower = model_id.to_lowercase();
    for &(prefix, ctx_win) in KNOWN_CONTEXT_WINDOWS {
        if lower.starts_with(prefix) {
            return Some(ctx_win);
        }
    }
    None
}

pub fn apply_model_catalog_defaults_for_new_model(model: &mut ProviderModelEntry) -> bool {
    let mut changed = false;
    if let Some(baseline) = CapabilityBaseline::lookup_fuzzy(&model.id) {
        if model.context_window.is_none() {
            model.context_window = baseline.context_window;
            changed |= model.context_window.is_some();
        }
        if model.max_output_tokens.is_none() {
            model.max_output_tokens = baseline.max_output_tokens;
            changed |= model.max_output_tokens.is_some();
        }
        if model.supports_multimodal.is_none() {
            model.supports_multimodal = baseline.supports_multimodal;
            changed |= model.supports_multimodal.is_some();
        }
        if model.supports_image.is_none() {
            model.supports_image = baseline.supports_image;
            changed |= model.supports_image.is_some();
        }
        if model.supports_video.is_none() {
            model.supports_video = baseline.supports_video;
            changed |= model.supports_video.is_some();
        }
    }
    if model.context_window.is_none() {
        model.context_window = infer_context_window(&model.id);
        changed |= model.context_window.is_some();
    }
    if model.context_window.is_none() {
        model.context_window = Some(DEFAULT_CONTEXT_WINDOW);
        changed = true;
    }
    changed
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProviderTemplate {
    pub id: &'static str,
    pub name: &'static str,
    pub base_url: &'static str,
    pub api_key_prefix: &'static str,
    pub require_api_key: bool,
    pub chat_model: &'static str,
    pub preset_models: &'static [&'static str],
    pub freeze_url: bool,
    pub support_connection_check: bool,
    pub is_local: bool,
}

impl ProviderTemplate {
    pub fn supports_oauth(&self) -> bool {
        self.catalog_flag("supports_oauth")
    }

    pub fn is_free_tier(&self) -> bool {
        self.catalog_flag("is_free_tier")
    }

    fn catalog_flag(&self, key: &str) -> bool {
        crate::catalog::provider(self.id)
            .and_then(|provider| provider.extra.get(key))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    }

    pub fn is_zero_config_free_tier(&self) -> bool {
        self.is_free_tier() && !self.require_api_key && !self.is_local
    }

    pub fn support_model_discovery(&self) -> bool {
        crate::catalog::provider(self.id).is_some_and(|provider| provider.support_model_discovery)
    }

    pub fn api_key_prefixes(&self) -> Vec<&'static str> {
        crate::catalog::provider(self.id)
            .map(|provider| {
                provider
                    .api_key_prefixes
                    .iter()
                    .map(String::as_str)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn provider_group(&self) -> &'static str {
        crate::catalog::provider(self.id)
            .map(|provider| provider.provider_group.as_str())
            .unwrap_or("")
    }

    pub fn provider_group_name(&self) -> &'static str {
        crate::catalog::provider(self.id)
            .map(|provider| provider.provider_group_name.as_str())
            .unwrap_or("")
    }

    pub fn provider_variant(&self) -> &'static str {
        crate::catalog::provider(self.id)
            .map(|provider| provider.provider_variant.as_str())
            .unwrap_or("")
    }

    pub fn provider_adapter(&self) -> Option<&'static str> {
        match self.id {
            "idealab-claude" => Some("idealab-anthropic"),
            "idealab-gemini" => Some("idealab-vertex"),
            _ => None,
        }
    }

    pub fn thinking_param_style(&self) -> Option<&'static str> {
        protocol_thinking_param_style(Some(self.chat_model))
    }

    pub fn reasoning_effort_options(&self) -> Vec<&'static str> {
        match self.thinking_param_style() {
            Some("effort") => vec!["none", "minimal", "low", "medium", "high", "xhigh"],
            _ => Vec::new(),
        }
    }

    pub fn thinking_budget_range(&self) -> Option<[i64; 2]> {
        match self.thinking_param_style() {
            Some("budget") => Some([1, 81920]),
            _ => None,
        }
    }

    pub fn base_url_options(&self) -> Vec<ProviderBaseUrlOption> {
        crate::catalog::provider(self.id)
            .map(|provider| provider.base_url_options.clone())
            .unwrap_or_default()
    }

    pub fn preset_model_entries(&self) -> Vec<crate::config_loader::ProviderModelEntry> {
        self.preset_models
            .iter()
            .map(|id| self.preset_model_entry(id))
            .collect()
    }

    pub fn preset_model_entry(&self, id: &str) -> crate::config_loader::ProviderModelEntry {
        if let Some(mut model) = crate::catalog::provider(self.id)
            .and_then(|provider| provider.models.iter().find(|model| model.id == id))
            .cloned()
        {
            crate::model_knowledge::apply_missing_defaults(self.id, &mut model);
            return model;
        }
        let mut extra = serde_json::Map::new();
        if self.preset_model_is_free(id) {
            extra.insert("is_free".into(), serde_json::Value::Bool(true));
        }
        let mut model = crate::config_loader::ProviderModelEntry {
            id: id.to_string(),
            name: self.preset_model_name(id).unwrap_or(id).to_string(),
            supports_multimodal: None,
            supports_image: None,
            supports_video: None,
            probe_source: None,
            context_window: infer_context_window(id),
            max_output_tokens: None,
            generate_kwargs: Default::default(),
            thinking_enabled: None,
            thinking_budget: None,
            reasoning_effort: None,
            thinking_param_style: None,
            reasoning_effort_options: Vec::new(),
            thinking_budget_range: None,
            relay_reasoning: None,
            extra,
        };
        crate::model_knowledge::apply_missing_defaults(self.id, &mut model);
        model
    }

    pub fn preset_model_is_free(&self, id: &str) -> bool {
        if let Some(is_free) = crate::catalog::provider(self.id)
            .and_then(|provider| provider.models.iter().find(|model| model.id == id))
            .and_then(|model| model.extra.get("is_free"))
            .and_then(serde_json::Value::as_bool)
        {
            return is_free;
        }
        if self.id == "opencode" {
            return id.ends_with("-free");
        }
        if self.id == "kilo" {
            return id == "kilo-auto/free" || id.ends_with(":free");
        }
        matches!(
            (self.id, id),
            ("github-models", "openai/gpt-4o-mini") | ("github-models", "openai/gpt-4o")
        )
    }

    pub fn preset_model_name(&self, id: &str) -> Option<&'static str> {
        crate::catalog::provider(self.id)?
            .models
            .iter()
            .find(|model| model.id == id)
            .map(|model| model.name.as_str())
    }

    pub fn to_config(&self) -> ProviderConfig {
        let upstream = crate::catalog::provider(self.id);
        let mut extra = upstream
            .map(|provider| provider.extra.clone())
            .unwrap_or_default();
        if self.supports_oauth() {
            extra.insert("supports_oauth".into(), serde_json::Value::Bool(true));
            extra.insert("oauth_connected".into(), serde_json::Value::Bool(false));
        }
        if self.is_free_tier() {
            extra.insert("is_free_tier".into(), serde_json::Value::Bool(true));
        }
        if let Some(adapter) = self.provider_adapter() {
            extra.insert(
                "provider_adapter".into(),
                serde_json::Value::String(adapter.into()),
            );
        }
        ProviderConfig {
            id: self.id.into(),
            name: self.name.into(),
            base_url: self.base_url.into(),
            api_key: String::new(),
            api_keys: Vec::new(),
            api_key_prefixes: self
                .api_key_prefixes()
                .into_iter()
                .map(String::from)
                .collect(),
            headers: serde_json::Map::new(),
            custom_headers: serde_json::Map::new(),
            chat_model: Some(self.chat_model.into()),
            models: self.preset_model_entries(),
            extra_models: Vec::new(),
            timeout: None,
            generate_kwargs: upstream
                .map(|provider| provider.generate_kwargs.clone())
                .unwrap_or_default(),
            support_model_discovery: self.support_model_discovery(),
            support_connection_check: self.support_connection_check,
            is_local: self.is_local,
            api_key_prefix: self.api_key_prefix.into(),
            is_custom: false,
            is_plugin: false,
            freeze_url: self.freeze_url,
            require_api_key: self.require_api_key,
            provider_group: self.provider_group().into(),
            provider_group_name: self.provider_group_name().into(),
            provider_variant: self.provider_variant().into(),
            base_url_options: self.base_url_options(),
            thinking_param_style: self.thinking_param_style().map(String::from),
            reasoning_effort_options: self
                .reasoning_effort_options()
                .into_iter()
                .map(String::from)
                .collect(),
            thinking_budget_range: self.thinking_budget_range(),
            extra,
        }
    }
}

include!("provider_templates.rs");

#[derive(Debug, Clone)]
pub struct PluginProviderSpec {
    pub id: String,
    pub label: String,
    pub base_url: String,
    pub chat_model: Option<String>,
    pub preset_models: Vec<String>,
    pub require_api_key: bool,
}

pub struct ProviderManager {
    providers: DashMap<String, Arc<dyn Provider>>,
    default_model: RwLock<Option<DefaultModel>>,
    config_loader: Option<ProviderConfigLoader>,
}

impl ProviderManager {
    pub fn new() -> Self {
        Self {
            providers: DashMap::new(),
            default_model: RwLock::new(None),
            config_loader: None,
        }
    }

    pub fn with_config_loader(loader: ProviderConfigLoader) -> Self {
        Self {
            providers: DashMap::new(),
            default_model: RwLock::new(None),
            config_loader: Some(loader),
        }
    }

    pub fn register(&self, provider: Arc<dyn Provider>) {
        let id = provider.info().id.clone();
        info!(provider_id = %id, name = %provider.info().name, "registering provider");
        self.providers.insert(id, provider);
    }

    pub fn remove(&self, provider_id: &str) -> Option<Arc<dyn Provider>> {
        let removed = self.providers.remove(provider_id).map(|(_, v)| v);
        if removed.is_some() {
            info!(provider_id, "removed provider");
        }
        removed
    }

    pub fn get(&self, provider_id: &str) -> Option<Arc<dyn Provider>> {
        self.providers.get(provider_id).map(|r| r.value().clone())
    }

    async fn get_or_load_provider(&self, provider_id: &str) -> Result<Arc<dyn Provider>, LlmError> {
        if let Some(provider) = self.get(provider_id) {
            return Ok(provider);
        }

        let loader = self
            .config_loader
            .as_ref()
            .ok_or_else(|| LlmError::Config {
                message: format!("provider '{provider_id}' not registered"),
            })?;
        let config = loader.load_provider(provider_id).await?;
        let provider = create_provider_from_config(&config)?;
        self.register(Arc::clone(&provider));
        Ok(provider)
    }

    pub fn list(&self) -> Vec<ProviderInfo> {
        self.providers
            .iter()
            .map(|r| r.value().info().clone())
            .collect()
    }

    pub fn provider_ids(&self) -> Vec<String> {
        self.providers.iter().map(|r| r.key().clone()).collect()
    }

    pub async fn default_model(&self) -> Option<DefaultModel> {
        self.default_model.read().await.clone()
    }

    pub async fn store_default_model(&self, active: DefaultModel) {
        info!(
            provider_id = %active.provider_id,
            model = %active.model,
            "setting active model"
        );
        *self.default_model.write().await = Some(active);
    }

    pub async fn default_chat_model(&self) -> Result<Box<dyn ChatModel>, LlmError> {
        let active = self
            .default_model
            .read()
            .await
            .clone()
            .ok_or_else(|| LlmError::Config {
                message: "no active model configured".into(),
            })?;

        let provider = self.get_or_load_provider(&active.provider_id).await?;

        provider.create_chat_model(&active.model)
    }

    pub fn chat_model(
        &self,
        provider_id: &str,
        model: &str,
    ) -> Result<Box<dyn ChatModel>, LlmError> {
        let provider = self.get(provider_id).ok_or_else(|| LlmError::Config {
            message: format!("provider '{}' not registered", provider_id),
        })?;

        provider.create_chat_model(model)
    }

    pub async fn load_from_config(&self) -> Result<(), LlmError> {
        let loader = self
            .config_loader
            .as_ref()
            .ok_or_else(|| LlmError::Config {
                message: "no config loader configured".into(),
            })?;

        if let Err(e) = crate::migrate::migrate_legacy_providers(loader.secret_dir()) {
            return Err(LlmError::Config {
                message: format!("provider migration failed: {e}"),
            });
        }

        match loader.load_default_model().await {
            Ok(active) => {
                self.store_default_model(active).await;
            }
            Err(e) => {
                warn!(error = %e, "failed to load default_model.json, continuing without");
            }
        }

        let configs = loader.load_all_providers().await?;
        for config in &configs {
            match create_provider_from_config(config) {
                Ok(provider) => {
                    self.register(provider);
                }
                Err(e) => {
                    warn!(
                        provider_id = %config.id,
                        error = %e,
                        "failed to create provider, skipping"
                    );
                }
            }
        }

        let working_dir = rua_config::dirs::rua_root();
        let resumed = crate::local_model_resume::resume_local_models(&working_dir);
        for entry in &resumed {
            info!(
                model_id = %entry.model_id,
                pid = entry.pid,
                status = ?entry.status,
                "resumed local model"
            );
        }

        Ok(())
    }

    pub async fn check_connection(&self, provider_id: &str) -> Result<ConnectionStatus, LlmError> {
        let provider = self.get_or_load_provider(provider_id).await?;

        provider.check_connection().await
    }

    pub async fn fetch_models(&self, provider_id: &str) -> Result<Vec<ModelInfo>, LlmError> {
        let provider = self.get_or_load_provider(provider_id).await?;

        provider.fetch_models().await
    }

    pub async fn resolve_default_model(
        &self,
    ) -> Result<crate::resolver::ResolvedModel, crate::resolver::Unavailable> {
        let loader = self.config_loader.as_ref().ok_or_else(|| {
            crate::resolver::Unavailable::NoDefaultModel {
                detail: "no config loader configured".into(),
            }
        })?;
        crate::resolver::resolve_default_model(loader).await
    }

    pub async fn debug_summary(&self) -> String {
        let active = self.default_model.read().await;
        let provider_count = self.providers.len();
        let active_str = match active.as_ref() {
            Some(a) => format!("{}:{}", a.provider_id, a.model),
            None => "none".into(),
        };
        format!(
            "ProviderManager(providers={}, active={})",
            provider_count, active_str
        )
    }

    pub async fn register_plugin_provider(
        &self,
        spec: &PluginProviderSpec,
    ) -> Result<(), LlmError> {
        let models: Vec<crate::config_loader::ProviderModelEntry> = spec
            .preset_models
            .iter()
            .map(|id| crate::config_loader::ProviderModelEntry {
                id: id.clone(),
                name: id.clone(),
                supports_multimodal: None,
                supports_image: None,
                supports_video: None,
                probe_source: None,
                context_window: None,
                max_output_tokens: None,
                generate_kwargs: Default::default(),
                thinking_enabled: None,
                thinking_budget: None,
                reasoning_effort: None,
                thinking_param_style: None,
                reasoning_effort_options: Vec::new(),
                thinking_budget_range: None,
                relay_reasoning: None,
                extra: Default::default(),
            })
            .map(|mut model| {
                apply_model_catalog_defaults_for_new_model(&mut model);
                model
            })
            .collect();

        let mut config = ProviderConfig {
            id: spec.id.clone(),
            name: if spec.label.is_empty() {
                spec.id.clone()
            } else {
                spec.label.clone()
            },
            base_url: spec.base_url.clone(),
            api_key: String::new(),
            api_keys: Vec::new(),
            api_key_prefixes: Vec::new(),
            headers: Default::default(),
            custom_headers: Default::default(),
            chat_model: spec.chat_model.clone(),
            models,
            extra_models: Vec::new(),
            timeout: None,
            generate_kwargs: Default::default(),
            support_model_discovery: false,
            support_connection_check: true,
            is_local: false,
            api_key_prefix: String::new(),
            is_custom: false,
            is_plugin: true,
            freeze_url: !spec.base_url.is_empty(),
            require_api_key: spec.require_api_key,
            provider_group: String::new(),
            provider_group_name: String::new(),
            provider_variant: String::new(),
            base_url_options: Vec::new(),
            thinking_param_style: None,
            reasoning_effort_options: Vec::new(),
            thinking_budget_range: None,
            extra: Default::default(),
        };

        if let Some(loader) = &self.config_loader {
            if let Ok(existing) = loader.load_provider(&spec.id).await {
                if !existing.api_key.is_empty() {
                    config.api_key = existing.api_key;
                }
                if !existing.api_keys.is_empty() {
                    config.api_keys = existing.api_keys;
                }
                if !existing.extra_models.is_empty() {
                    config.extra_models = existing.extra_models;
                }
            }
        }

        let provider = create_provider_from_config(&config)?;
        self.register(provider);

        if let Some(loader) = &self.config_loader {
            if let Err(e) = loader.save_provider(&config).await {
                warn!(provider_id = %spec.id, error = %e, "failed to persist plugin provider config");
            }
        }

        info!(provider_id = %spec.id, "registered plugin provider");
        Ok(())
    }

    pub fn unregister_plugin_provider(&self, provider_id: &str) {
        if self.providers.remove(provider_id).is_some() {
            info!(
                provider_id,
                "unregistered plugin provider (disk config preserved)"
            );
        }
    }

    fn require_config_loader(&self) -> Result<&ProviderConfigLoader, LlmError> {
        self.config_loader.as_ref().ok_or_else(|| LlmError::Config {
            message: "no config loader configured".into(),
        })
    }

    pub async fn reload_provider(&self, provider_id: &str) -> Result<(), LlmError> {
        let loader = self.require_config_loader()?;
        let config = loader.load_provider(provider_id).await?;
        let provider = create_provider_from_config(&config)?;
        self.register(provider);
        info!(provider_id, "reloaded provider from disk");
        Ok(())
    }

    pub async fn clear_default_model(&self) -> Result<(), LlmError> {
        if let Some(loader) = &self.config_loader {
            loader.delete_default_model_file().await?;
        }
        *self.default_model.write().await = None;
        info!("cleared active model");
        Ok(())
    }

    pub async fn set_default_model(&self, active: DefaultModel) -> Result<(), LlmError> {
        let provider = self
            .get(&active.provider_id)
            .ok_or_else(|| LlmError::Config {
                message: format!("provider '{}' not found", active.provider_id),
            })?;

        if !provider.supports_model(&active.model) {
            return Err(LlmError::Config {
                message: format!(
                    "model '{}' not found in provider '{}'",
                    active.model, active.provider_id
                ),
            });
        }

        if let Some(loader) = &self.config_loader {
            loader.save_default_model(&active).await?;
        }

        let provider_id = active.provider_id.clone();
        let model_id = active.model.clone();

        self.store_default_model(active).await;

        self.maybe_spawn_multimodal_probe(&provider_id, &model_id, &provider)
            .await;

        Ok(())
    }

    pub async fn openrouter(
        &self,
    ) -> Result<crate::openrouter::OpenRouterExtendedClient, LlmError> {
        let loader = self.require_config_loader()?;
        let config = loader
            .load_provider("openrouter")
            .await
            .map_err(|e| LlmError::Config {
                message: format!("openrouter provider not configured: {e}"),
            })?;
        Ok(crate::openrouter::OpenRouterExtendedClient::new(
            config.base_url,
            config.api_key,
        ))
    }

    pub fn list_templates(&self) -> &'static [ProviderTemplate] {
        PROVIDER_TEMPLATES
    }

    pub async fn get_model_context_window(&self, provider_id: &str, model_id: &str) -> Option<u64> {
        self.get(provider_id)?;
        Some(
            self.get_known_model_context_window(provider_id, model_id)
                .await
                .unwrap_or(DEFAULT_CONTEXT_WINDOW),
        )
    }

    pub async fn get_known_model_context_window(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Option<u64> {
        self.get(provider_id)?;

        if let Some(loader) = &self.config_loader {
            if let Ok(config) = loader.load_provider(provider_id).await {
                for model in config.all_models() {
                    if model.id == model_id {
                        return model
                            .context_window
                            .or_else(|| infer_context_window(model_id));
                    }
                }
            }
        }

        infer_context_window(model_id)
    }

    pub fn update_model_config(
        &self,
        provider_id: &str,
        model_id: &str,
        config: &mut ProviderConfig,
        updates: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), LlmError> {
        let model = config
            .models
            .iter_mut()
            .chain(config.extra_models.iter_mut())
            .find(|m| m.id == model_id)
            .ok_or_else(|| LlmError::Config {
                message: format!(
                    "model '{}' not found in provider '{}'",
                    model_id, provider_id
                ),
            })?;

        if let Some(ctx) = updates.get("context_window") {
            model.context_window = ctx.as_u64();
        }
        if let Some(max_out) = updates.get("max_output_tokens") {
            model.max_output_tokens = max_out.as_u64();
        }
        if let Some(gk) = updates.get("generate_kwargs") {
            if let Some(obj) = gk.as_object() {
                deep_merge_maps(&mut model.generate_kwargs, obj);
            }
        }
        if let Some(v) = updates.get("supports_image") {
            model.supports_image = v.as_bool();
        }
        if let Some(v) = updates.get("supports_video") {
            model.supports_video = v.as_bool();
        }
        if let Some(v) = updates.get("supports_multimodal") {
            model.supports_multimodal = v.as_bool();
        }
        if updates.contains_key("thinking_enabled") {
            model.thinking_enabled = updates.get("thinking_enabled").and_then(|v| v.as_bool());
        }
        if updates.contains_key("thinking_budget") {
            model.thinking_budget = updates.get("thinking_budget").and_then(|v| v.as_i64());
        }
        if updates.contains_key("reasoning_effort") {
            model.reasoning_effort = updates
                .get("reasoning_effort")
                .and_then(|v| v.as_str())
                .map(String::from);
        }
        if updates.contains_key("thinking_param_style") {
            model.thinking_param_style = updates
                .get("thinking_param_style")
                .and_then(|v| v.as_str())
                .map(String::from);
        }
        if let Some(options) = updates
            .get("reasoning_effort_options")
            .and_then(|v| v.as_array())
        {
            model.reasoning_effort_options = options
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
        }
        if updates.contains_key("thinking_budget_range") {
            model.thinking_budget_range = updates.get("thinking_budget_range").and_then(|v| {
                let values = v.as_array()?;
                Some([values.first()?.as_i64()?, values.get(1)?.as_i64()?])
            });
        }
        if updates.contains_key("relay_reasoning") {
            model.relay_reasoning = updates.get("relay_reasoning").and_then(|v| v.as_bool());
        }

        let provider = create_provider_from_config(config)?;
        self.register(provider);
        info!(provider_id, model_id, "updated model config (in-memory)");
        Ok(())
    }

    pub async fn effective_generate_kwargs(
        &self,
        provider_id: &str,
        model_id: &str,
        request_kwargs: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<serde_json::Map<String, serde_json::Value>, LlmError> {
        let loader = self.require_config_loader()?;
        let config = loader
            .load_provider(provider_id)
            .await
            .map_err(|e| LlmError::Config {
                message: format!("provider '{}' not found: {e}", provider_id),
            })?;

        let mut result = config.generate_kwargs.clone();

        let model = config.all_models().into_iter().find(|m| m.id == model_id);
        if let Some(m) = model {
            let model_kwargs = config.project_model_generate_kwargs(m);
            deep_merge_maps(&mut result, &model_kwargs);
        }

        deep_merge_maps(&mut result, request_kwargs);

        Ok(result)
    }

    pub async fn discover_models(&self, provider_id: &str) -> Result<Vec<ModelInfo>, LlmError> {
        let mut models = self.fetch_models(provider_id).await?;
        if matches!(
            provider_id,
            "idealab" | "idealab-responses" | "idealab-claude" | "idealab-gemini"
        ) || crate::catalog::provider(provider_id)
            .and_then(|provider| provider.extra.get("merge_with_catalog"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            if let Some(template) = PROVIDER_TEMPLATES
                .iter()
                .find(|template| template.id == provider_id)
            {
                for known in template.to_config().to_model_infos() {
                    if !models.iter().any(|model| model.id == known.id) {
                        models.push(known);
                    }
                }
            }
        }
        for model in &mut models {
            if let Some(known) = crate::catalog::provider(provider_id)
                .and_then(|provider| provider.models.iter().find(|entry| entry.id == model.id))
            {
                model.context_window = model.context_window.or(known.context_window);
                model.max_output_tokens = model.max_output_tokens.or(known.max_output_tokens);
                model.supports_image = model.supports_image.or(known.supports_image);
                model.supports_video = model.supports_video.or(known.supports_video);
                model.supports_multimodal = model.supports_multimodal.or(known.supports_multimodal);
            }
            if model.context_window.is_none()
                && !self
                    .get(provider_id)
                    .is_some_and(|provider| provider.info().is_local)
            {
                model.context_window = infer_context_window(&model.id);
            }
        }
        Ok(models)
    }

    pub async fn check_model_connection(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Result<(bool, String), LlmError> {
        let model_id = model_id.trim();
        if model_id.is_empty() {
            return Ok((false, "Empty model ID".into()));
        }

        let provider = self.get(provider_id).ok_or_else(|| LlmError::Config {
            message: format!("provider '{}' not found", provider_id),
        })?;

        let chat_model = provider
            .create_chat_model(model_id)
            .map_err(|e| LlmError::Config {
                message: format!(
                    "failed to create chat model for {}/{}: {e}",
                    provider_id, model_id
                ),
            })?;

        let provider_info = provider.info();
        let max_tokens = if provider_info.provider_type.as_deref() == Some("OpenAIResponseModel")
            || matches!(provider_info.id.as_str(), "idealab" | "idealab-responses")
        {
            16
        } else {
            1
        };
        let message = ModelDialogueItem::text(Role::User, "hi");
        let request = ChatRequest::new(vec![message]).with_max_tokens(max_tokens);

        let result =
            tokio::time::timeout(std::time::Duration::from_secs(30), chat_model.chat(request))
                .await;

        let (ok, msg) = match result {
            Ok(Ok(_)) => (true, "connected".to_string()),
            Ok(Err(e)) => (false, format!("Chat completion failed: {e}")),
            Err(_) => (false, "Connection timed out (30s)".to_string()),
        };

        Ok((ok, msg))
    }

    pub async fn probe_model_multimodal(
        &self,
        provider_id: &str,
        model_id: &str,
    ) -> Result<ProbeResult, LlmError> {
        let provider = self.get(provider_id).ok_or_else(|| LlmError::Config {
            message: format!("provider '{}' not found", provider_id),
        })?;

        if !provider.supports_model(model_id) {
            return Err(LlmError::Config {
                message: format!(
                    "model '{}' not found in provider '{}'",
                    model_id, provider_id
                ),
            });
        }

        let chat_model = provider
            .create_chat_model(model_id)
            .map_err(|e| LlmError::Config {
                message: format!(
                    "failed to create chat model for {}/{}: {e}",
                    provider_id, model_id
                ),
            })?;

        let prober = MultimodalProber::with_defaults();
        let probe = prober.probe(&*chat_model).await;

        if let Some(error) = &probe.error {
            warn!(provider_id, model_id, %error, "multimodal probe failed; capabilities unchanged");
        }
        Ok(probe)
    }

    async fn maybe_spawn_multimodal_probe(
        &self,
        provider_id: &str,
        model_id: &str,
        provider: &Arc<dyn Provider>,
    ) {
        let Some(loader) = &self.config_loader else {
            return;
        };

        let config = match loader.load_provider(provider_id).await {
            Ok(c) => c,
            Err(_) => return,
        };

        let needs_probe = config
            .all_models()
            .iter()
            .find(|m| m.id == model_id)
            .is_some_and(|m| m.supports_multimodal.is_none());

        if !needs_probe {
            debug!(
                provider_id,
                model_id, "multimodal already known, skip auto-probe"
            );
            return;
        }

        let provider = provider.clone();
        let provider_id = provider_id.to_string();
        let model_id = model_id.to_string();

        tokio::spawn(async move {
            info!(
                provider_id = %provider_id,
                model_id = %model_id,
                "auto-probing multimodal capability"
            );

            let chat_model = match provider.create_chat_model(&model_id) {
                Ok(m) => m,
                Err(e) => {
                    warn!(
                        provider_id = %provider_id,
                        model_id = %model_id,
                        error = %e,
                        "auto-probe: failed to create chat model"
                    );
                    return;
                }
            };

            let prober = MultimodalProber::with_defaults();
            let result = prober.probe(&*chat_model).await;

            if result.error.is_some() {
                warn!(
                    provider_id = %provider_id,
                    model_id = %model_id,
                    error = ?result.error,
                    "auto-probe: transient failure, keeping None for retry"
                );
                return;
            }

            info!(
                provider_id = %provider_id,
                model_id = %model_id,
                supports_multimodal = result.supports_multimodal,
                supports_image = result.supports_image,
                "auto-probe: result (in-memory only, persisted via sync emit)"
            );
        });
    }
}

impl Default for ProviderManager {
    fn default() -> Self {
        Self::new()
    }
}

fn config_with_runtime_capabilities(config: &ProviderConfig) -> ProviderConfig {
    let mut enriched = config.clone();
    for model in enriched
        .models
        .iter_mut()
        .chain(enriched.extra_models.iter_mut())
    {
        let Some(baseline) = CapabilityBaseline::lookup_fuzzy_for_provider(&enriched.id, &model.id)
        else {
            continue;
        };
        model.supports_multimodal = model.supports_multimodal.or(baseline.supports_multimodal);
        model.supports_image = model.supports_image.or(baseline.supports_image);
        model.supports_video = model.supports_video.or(baseline.supports_video);
        if model.probe_source.is_none() {
            model.probe_source = baseline.probe_source;
        }
    }
    enriched
}

pub fn create_provider_from_config(config: &ProviderConfig) -> Result<Arc<dyn Provider>, LlmError> {
    let config = config_with_runtime_capabilities(config);
    let provider = create_protocol_provider(&config)?;
    Ok(Arc::new(crate::configured::ConfiguredProvider::new(
        provider, config,
    )))
}

fn create_protocol_provider(config: &ProviderConfig) -> Result<Arc<dyn Provider>, LlmError> {
    let chat_model_type = config.chat_model.as_deref().unwrap_or("OpenAIChatModel");
    let provider_adapter = config
        .extra
        .get("provider_adapter")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty());

    debug!(
        provider_id = %config.id,
        chat_model_type,
        api_key_present = !config.api_key.is_empty(),
        "creating provider from config"
    );

    match chat_model_type {
        "OpenAIChatModel" => {
            if let Some(adapter) = provider_adapter {
                return Err(LlmError::Config {
                    message: format!(
                        "provider adapter '{adapter}' is not valid for OpenAI Chat Completions"
                    ),
                });
            }

            if config.template_id() == "openrouter" {
                return crate::openrouter::create_openrouter_provider(config);
            }

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

            let mut headers = config.effective_headers();
            inject_dashscope_cdpl_header(&config.base_url, &mut headers);

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
                is_local: config.is_local,
            };
            let provider = OpenAiCompatibleProvider::new(provider_config)?;
            Ok(Arc::new(provider))
        }
        "OpenAIResponseModel" | "DashScopeResponseModel" => {
            if let Some(adapter) = provider_adapter {
                return Err(LlmError::Config {
                    message: format!(
                        "provider adapter '{adapter}' is not valid for OpenAI Responses"
                    ),
                });
            }
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

            let mut headers = config.effective_headers();
            if config.id == "dashscope" {
                for (key, value) in llm_multiplex::dashscope::load_dashscope_headers() {
                    headers.entry(key).or_insert(value);
                }
                headers.insert(
                    "x-dashscope-session-cache".into(),
                    serde_json::Value::String("enable".into()),
                );
            }

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
                is_local: config.is_local,
            };
            let provider = llm_multiplex::OpenAiResponsesProvider::new(provider_config)?;
            Ok(Arc::new(provider))
        }
        #[cfg(feature = "anthropic")]
        "AnthropicChatModel" => {
            let anthropic_config = llm_multiplex::AnthropicConfig {
                provider_id: config.id.clone(),
                provider_name: config.name.clone(),
                base_url: config.base_url.clone(),
                api_key: if config.api_key.is_empty() {
                    None
                } else {
                    Some(config.api_key.clone())
                },
                headers: config.effective_headers(),
                models: config.to_model_infos(),
                timeout_secs: config.timeout.unwrap_or(120),
                generate_kwargs: config.generate_kwargs.clone(),
                support_connection_check: config.support_connection_check,
                api_version: "2023-06-01".into(),
                auth_mode: match config
                    .extra
                    .get("auth_mode")
                    .and_then(|value| value.as_str())
                {
                    Some("auth_token") => llm_multiplex::providers::anthropic::AuthMode::AuthToken,
                    _ => llm_multiplex::providers::anthropic::AuthMode::ApiKey,
                },
            };
            match provider_adapter {
                None => {
                    let provider = llm_multiplex::AnthropicProvider::new(anthropic_config)?;
                    Ok(Arc::new(provider))
                }
                Some("idealab-anthropic") => {
                    let provider =
                        crate::idealab_anthropic::IdealabAnthropicProvider::new(anthropic_config)?;
                    Ok(Arc::new(provider))
                }
                Some(adapter) => Err(LlmError::Config {
                    message: format!(
                        "provider adapter '{adapter}' is not valid for Anthropic Messages"
                    ),
                }),
            }
        }
        #[cfg(feature = "gemini")]
        "GeminiChatModel" => {
            let gemini_config = llm_multiplex::GeminiConfig {
                provider_id: config.id.clone(),
                provider_name: config.name.clone(),
                base_url: config.base_url.clone(),
                api_key: if config.api_key.is_empty() {
                    None
                } else {
                    Some(config.api_key.clone())
                },
                headers: config.effective_headers(),
                models: config.to_model_infos(),
                timeout_secs: config.timeout.unwrap_or(60),
                generate_kwargs: config.generate_kwargs.clone(),
            };
            match provider_adapter {
                None => {
                    let provider = llm_multiplex::GeminiProvider::new(gemini_config)?;
                    Ok(Arc::new(provider))
                }
                Some("idealab-vertex") => {
                    let provider =
                        crate::idealab_vertex::IdealabVertexProvider::new(gemini_config)?;
                    Ok(Arc::new(provider))
                }
                Some(adapter) => Err(LlmError::Config {
                    message: format!(
                        "provider adapter '{adapter}' is not valid for Gemini GenerateContent"
                    ),
                }),
            }
        }
        "DashScopeChatModel" => {
            let mut api_keys = config.api_keys.clone();
            if !config.api_key.is_empty() && !api_keys.contains(&config.api_key) {
                api_keys.insert(0, config.api_key.clone());
            }
            let provider =
                llm_multiplex::dashscope::DashScopeProvider::from_config(OpenAiCompatibleConfig {
                    provider_id: config.id.clone(),
                    provider_name: config.name.clone(),
                    base_url: if config.base_url.is_empty() {
                        llm_multiplex::dashscope::DASHSCOPE_BASE_URL.to_string()
                    } else {
                        config.base_url.clone()
                    },
                    api_key: (!config.api_key.is_empty()).then(|| config.api_key.clone()),
                    api_keys,
                    headers: config.effective_headers(),
                    models: config.to_model_infos(),
                    timeout_secs: config.timeout.unwrap_or(60),
                    generate_kwargs: config.generate_kwargs.clone(),
                    support_model_discovery: config.support_model_discovery,
                    support_connection_check: config.support_connection_check,
                    is_local: config.is_local,
                })?;
            Ok(Arc::new(provider))
        }
        "OpenRouterChatModel" => crate::openrouter::create_openrouter_provider(config),
        #[cfg(feature = "ollama")]
        "OllamaChatModel" => {
            let ollama_config = llm_multiplex::OllamaConfig {
                provider_id: config.id.clone(),
                provider_name: config.name.clone(),
                base_url: if config.base_url.is_empty() {
                    llm_multiplex::providers::ollama::OLLAMA_DEFAULT_URL.to_string()
                } else {
                    config.base_url.clone()
                },
                models: config.to_model_infos(),
                timeout_secs: config.timeout.unwrap_or(120),
                generate_kwargs: config.generate_kwargs.clone(),
            };
            let provider = llm_multiplex::OllamaProvider::new(ollama_config)?;
            Ok(Arc::new(provider))
        }
        other => {
            warn!(
                provider_id = %config.id,
                chat_model_type = other,
                "unknown chat_model type, falling back to OpenAI-compatible"
            );
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
                headers: config.effective_headers(),
                models: config.to_model_infos(),
                timeout_secs: config.timeout.unwrap_or(60),
                generate_kwargs: config.generate_kwargs.clone(),
                support_model_discovery: config.support_model_discovery,
                support_connection_check: config.support_connection_check,
                is_local: config.is_local,
            };
            let provider = OpenAiCompatibleProvider::new(provider_config)?;
            Ok(Arc::new(provider))
        }
    }
}

pub fn deep_merge_maps(
    base: &mut serde_json::Map<String, serde_json::Value>,
    overlay: &serde_json::Map<String, serde_json::Value>,
) {
    for (key, overlay_val) in overlay {
        match (base.get_mut(key), overlay_val) {
            (Some(serde_json::Value::Object(base_obj)), serde_json::Value::Object(overlay_obj)) => {
                deep_merge_maps(base_obj, overlay_obj);
            }
            _ => {
                base.insert(key.clone(), overlay_val.clone());
            }
        }
    }
}

const DASHSCOPE_CDPL_BASE_URLS: &[&str] = &[
    "https://coding.dashscope.aliyuncs.com/v1",
    "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
];

pub fn inject_dashscope_cdpl_header(
    base_url: &str,
    headers: &mut serde_json::Map<String, serde_json::Value>,
) {
    if DASHSCOPE_CDPL_BASE_URLS.contains(&base_url) {
        let cdpl = serde_json::json!({
            "agentType": "rua",
            "deployType": "UnKnown",
            "moduleCode": "model",
            "agentCode": "UnKnown",
        });
        headers.insert(
            "X-DashScope-Cdpl".to_string(),
            serde_json::Value::String(cdpl.to_string()),
        );
    }
}
