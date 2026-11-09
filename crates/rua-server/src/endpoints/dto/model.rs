use rua_provider::config_loader::{
    ProviderBaseUrlOption, ProviderConfig, ProviderModelEntry as ConfigModelEntry,
};
use rua_provider::manager::ProviderTemplate;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderDto {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub chat_model: String,
    #[serde(default)]
    pub models: Vec<ModelDto>,
    #[serde(default)]
    pub extra_models: Vec<ModelDto>,
    #[serde(default)]
    pub api_key_prefix: String,
    #[serde(default)]
    pub api_key_prefixes: Vec<String>,
    #[serde(default)]
    pub custom_headers: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub is_local: Option<bool>,
    #[serde(default)]
    pub freeze_url: Option<bool>,
    #[serde(default)]
    pub require_api_key: Option<bool>,
    #[serde(default)]
    pub is_custom: Option<bool>,
    #[serde(default)]
    pub support_model_discovery: Option<bool>,
    #[serde(default)]
    pub support_connection_check: Option<bool>,
    #[serde(default)]
    pub supports_oauth: Option<bool>,
    #[serde(default)]
    pub oauth_connected: Option<bool>,
    #[serde(default)]
    pub is_free_tier: Option<bool>,
    #[serde(default)]
    pub generate_kwargs: Option<serde_json::Value>,
    #[serde(default)]
    pub provider_group: String,
    #[serde(default)]
    pub provider_group_name: String,
    #[serde(default)]
    pub provider_variant: String,
    #[serde(default)]
    pub base_url_options: Vec<ProviderBaseUrlOption>,
    #[serde(default)]
    pub thinking_param_style: Option<String>,
    #[serde(default)]
    pub reasoning_effort_options: Vec<String>,
    #[serde(default)]
    pub thinking_budget_range: Option<[i64; 2]>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelDto {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub supports_multimodal: Option<bool>,
    #[serde(default)]
    pub supports_image: Option<bool>,
    #[serde(default)]
    pub supports_video: Option<bool>,
    #[serde(default)]
    pub probe_source: Option<String>,
    #[serde(default)]
    pub context_window: Option<u64>,
    #[serde(default)]
    pub max_output_tokens: Option<u64>,
    #[serde(default)]
    pub is_free: Option<bool>,
    #[serde(default)]
    pub generate_kwargs: Option<serde_json::Value>,
    #[serde(default)]
    pub thinking_enabled: Option<bool>,
    #[serde(default)]
    pub thinking_budget: Option<i64>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub thinking_param_style: Option<String>,
    #[serde(default)]
    pub reasoning_effort_options: Vec<String>,
    #[serde(default)]
    pub thinking_budget_range: Option<[i64; 2]>,
    #[serde(default)]
    pub relay_reasoning: Option<bool>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefaultModelDto {
    pub provider_id: String,
    pub model: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefaultModelResponseDto {
    pub default_llm: Option<DefaultModelDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeRequestDto {
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeResultDto {
    pub success: bool,
    #[serde(default)]
    pub latency_ms: Option<u64>,
    #[serde(default)]
    pub models: Vec<ModelDto>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderUpdateDto {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub api_key_prefixes: Option<Vec<String>>,
    #[serde(default)]
    pub chat_model: Option<String>,
    #[serde(default)]
    pub custom_headers: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(default)]
    pub models: Option<Vec<ModelDto>>,
    #[serde(default)]
    pub generate_kwargs: Option<serde_json::Value>,
    #[serde(default)]
    pub provider_group: Option<String>,
    #[serde(default)]
    pub provider_group_name: Option<String>,
    #[serde(default)]
    pub provider_variant: Option<String>,
    #[serde(default)]
    pub base_url_options: Option<Vec<ProviderBaseUrlOption>>,
    #[serde(default)]
    pub thinking_param_style: Option<String>,
    #[serde(default)]
    pub reasoning_effort_options: Option<Vec<String>>,
    #[serde(default)]
    pub thinking_budget_range: Option<[i64; 2]>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomProviderCreateDto {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub api_key_prefixes: Vec<String>,
    #[serde(default)]
    pub custom_headers: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub models: Vec<ModelDto>,
    #[serde(default)]
    pub provider_group: String,
    #[serde(default)]
    pub provider_group_name: String,
    #[serde(default)]
    pub provider_variant: String,
    #[serde(default)]
    pub base_url_options: Vec<ProviderBaseUrlOption>,
    #[serde(default)]
    pub thinking_param_style: Option<String>,
    #[serde(default)]
    pub reasoning_effort_options: Vec<String>,
    #[serde(default)]
    pub thinking_budget_range: Option<[i64; 2]>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderTemplateInfoDto {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key_prefix: String,
    #[serde(default)]
    pub api_key_prefixes: Vec<String>,
    #[serde(default)]
    pub require_api_key: bool,
    #[serde(default)]
    pub chat_model: String,
    #[serde(default)]
    pub preset_models: Vec<String>,
    #[serde(default)]
    pub models: Vec<ModelDto>,
    #[serde(default)]
    pub freeze_url: bool,
    #[serde(default)]
    pub support_connection_check: bool,
    #[serde(default)]
    pub is_local: bool,
    #[serde(default)]
    pub supports_oauth: bool,
    #[serde(default)]
    pub is_free_tier: bool,
    #[serde(default)]
    pub provider_group: String,
    #[serde(default)]
    pub provider_group_name: String,
    #[serde(default)]
    pub provider_variant: String,
    #[serde(default)]
    pub base_url_options: Vec<ProviderBaseUrlOption>,
    #[serde(default)]
    pub thinking_param_style: Option<String>,
    #[serde(default)]
    pub reasoning_effort_options: Vec<String>,
    #[serde(default)]
    pub thinking_budget_range: Option<[i64; 2]>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl From<&ConfigModelEntry> for ModelDto {
    fn from(m: &ConfigModelEntry) -> Self {
        let generate_kwargs = if m.generate_kwargs.is_empty() {
            None
        } else {
            Some(serde_json::Value::Object(m.generate_kwargs.clone()))
        };
        ModelDto {
            id: m.id.clone(),
            name: m.name.clone(),
            supports_multimodal: m.supports_multimodal,
            supports_image: m.supports_image,
            supports_video: m.supports_video,
            probe_source: m.probe_source.clone(),
            context_window: m.context_window,
            max_output_tokens: m.max_output_tokens,
            is_free: model_is_free(&m.extra),
            generate_kwargs,
            thinking_enabled: m.thinking_enabled,
            thinking_budget: m.thinking_budget,
            reasoning_effort: m.reasoning_effort.clone(),
            thinking_param_style: m.thinking_param_style.clone(),
            reasoning_effort_options: m.reasoning_effort_options.clone(),
            thinking_budget_range: m.thinking_budget_range,
            relay_reasoning: m.relay_reasoning,
            extra: m.extra.clone(),
        }
    }
}

impl From<llm_multiplex::ModelInfo> for ModelDto {
    fn from(m: llm_multiplex::ModelInfo) -> Self {
        let generate_kwargs = if m.generate_kwargs.is_empty() {
            None
        } else {
            Some(serde_json::Value::Object(m.generate_kwargs))
        };
        ModelDto {
            id: m.id,
            name: m.name,
            supports_multimodal: m.supports_multimodal,
            supports_image: m.supports_image,
            supports_video: m.supports_video,
            probe_source: m.probe_source,
            context_window: m.context_window,
            max_output_tokens: m.max_output_tokens,
            is_free: None,
            generate_kwargs,
            thinking_enabled: None,
            thinking_budget: None,
            reasoning_effort: None,
            thinking_param_style: None,
            reasoning_effort_options: Vec::new(),
            thinking_budget_range: None,
            relay_reasoning: None,
            extra: serde_json::Map::new(),
        }
    }
}

impl From<&ProviderConfig> for ProviderDto {
    fn from(p: &ProviderConfig) -> Self {
        let generate_kwargs = if p.generate_kwargs.is_empty() {
            None
        } else {
            Some(serde_json::Value::Object(p.generate_kwargs.clone()))
        };
        ProviderDto {
            id: p.id.clone(),
            name: p.name.clone(),
            base_url: p.base_url.clone(),
            api_key: rua_config::secrets::redact_api_key(&p.api_key),
            chat_model: p.chat_model.clone().unwrap_or_default(),
            models: p
                .models
                .iter()
                .map(|model| model_dto_from_config_model(&p.id, model))
                .collect(),
            extra_models: p
                .extra_models
                .iter()
                .map(|model| model_dto_from_config_model(&p.id, model))
                .collect(),
            api_key_prefix: p.api_key_prefix.clone(),
            api_key_prefixes: p.api_key_prefixes.clone(),
            custom_headers: p.custom_headers.clone(),
            is_local: Some(p.is_local),
            freeze_url: Some(p.freeze_url),
            require_api_key: Some(p.require_api_key),
            is_custom: Some(p.is_custom),
            support_model_discovery: Some(p.support_model_discovery),
            support_connection_check: Some(p.support_connection_check),
            supports_oauth: Some(provider_supports_oauth(&p.id, &p.extra)),
            oauth_connected: Some(provider_oauth_connected(&p.id, &p.api_key, &p.extra)),
            is_free_tier: Some(provider_is_free_tier(&p.id, &p.extra)),
            generate_kwargs,
            provider_group: p.provider_group.clone(),
            provider_group_name: p.provider_group_name.clone(),
            provider_variant: p.provider_variant.clone(),
            base_url_options: p.base_url_options.clone(),
            thinking_param_style: p
                .thinking_param_style
                .clone()
                .or_else(|| p.protocol_thinking_param_style().map(str::to_string)),
            reasoning_effort_options: p.reasoning_effort_options.clone(),
            thinking_budget_range: p.thinking_budget_range,
            extra: p.extra.clone(),
        }
    }
}

fn model_dto_from_config_model(provider_id: &str, m: &ConfigModelEntry) -> ModelDto {
    let mut dto = ModelDto::from(m);
    if let Some(baseline) =
        llm_multiplex::CapabilityBaseline::lookup_fuzzy_for_provider(provider_id, &m.id)
    {
        dto.supports_multimodal = dto.supports_multimodal.or(baseline.supports_multimodal);
        dto.supports_image = dto.supports_image.or(baseline.supports_image);
        dto.supports_video = dto.supports_video.or(baseline.supports_video);
        dto.probe_source = dto.probe_source.or(baseline.probe_source);
    }
    if dto.is_free.is_none() {
        dto.is_free = rua_provider::manager::PROVIDER_TEMPLATES
            .iter()
            .find(|template| template.id == provider_id)
            .and_then(|template| template.preset_model_is_free(&m.id).then_some(true));
    }
    dto
}

fn provider_supports_oauth(
    provider_id: &str,
    extra: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    extra
        .get("supports_oauth")
        .and_then(|v| v.as_bool())
        .unwrap_or_else(|| {
            rua_provider::manager::PROVIDER_TEMPLATES
                .iter()
                .find(|template| template.id == provider_id)
                .is_some_and(|template| template.supports_oauth())
        })
}

fn provider_oauth_connected(
    provider_id: &str,
    api_key: &str,
    extra: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    extra
        .get("oauth_connected")
        .and_then(|v| v.as_bool())
        .unwrap_or_else(|| provider_supports_oauth(provider_id, extra) && !api_key.is_empty())
}

fn provider_is_free_tier(
    provider_id: &str,
    extra: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    extra
        .get("is_free_tier")
        .and_then(|v| v.as_bool())
        .unwrap_or_else(|| {
            rua_provider::manager::PROVIDER_TEMPLATES
                .iter()
                .find(|template| template.id == provider_id)
                .is_some_and(|template| template.is_free_tier())
        })
}

impl From<&ProviderTemplate> for ProviderTemplateInfoDto {
    fn from(t: &ProviderTemplate) -> Self {
        let config = t.to_config();
        let mut extra = config.extra;
        for key in ["supports_oauth", "oauth_connected", "is_free_tier"] {
            extra.remove(key);
        }
        extra.insert(
            "support_model_discovery".into(),
            serde_json::json!(t.support_model_discovery()),
        );
        extra.insert(
            "generate_kwargs".into(),
            serde_json::json!(config.generate_kwargs),
        );
        if let Some(adapter) = t.provider_adapter() {
            extra.insert(
                "provider_adapter".into(),
                serde_json::Value::String(adapter.into()),
            );
        }
        ProviderTemplateInfoDto {
            id: t.id.to_string(),
            name: t.name.to_string(),
            base_url: t.base_url.to_string(),
            api_key_prefix: t.api_key_prefix.to_string(),
            api_key_prefixes: t.api_key_prefixes().into_iter().map(String::from).collect(),
            require_api_key: t.require_api_key,
            chat_model: t.chat_model.to_string(),
            preset_models: t.preset_models.iter().map(|s| s.to_string()).collect(),
            models: t
                .preset_model_entries()
                .iter()
                .map(ModelDto::from)
                .collect(),
            freeze_url: t.freeze_url,
            support_connection_check: t.support_connection_check,
            is_local: t.is_local,
            supports_oauth: t.supports_oauth(),
            is_free_tier: t.is_free_tier(),
            provider_group: t.provider_group().into(),
            provider_group_name: t.provider_group_name().into(),
            provider_variant: t.provider_variant().into(),
            base_url_options: t.base_url_options(),
            thinking_param_style: t.thinking_param_style().map(String::from),
            reasoning_effort_options: t
                .reasoning_effort_options()
                .into_iter()
                .map(String::from)
                .collect(),
            thinking_budget_range: t.thinking_budget_range(),
            extra,
        }
    }
}

fn model_is_free(extra: &serde_json::Map<String, serde_json::Value>) -> Option<bool> {
    extra.get("is_free").and_then(|v| v.as_bool())
}
