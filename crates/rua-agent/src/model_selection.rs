use serde::{Deserialize, Serialize};

use crate::error::AgentError;
use crate::profile::AgentProfile;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelSelectionSource {
    RequestOverride,

    AgentProfile,

    DefaultModel,

    DefaultProvider,
}

impl std::fmt::Display for ModelSelectionSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::RequestOverride => "request_override",
            Self::AgentProfile => "agent_profile",
            Self::DefaultModel => "default_model",
            Self::DefaultProvider => "default_provider",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSelection {
    pub provider_id: String,

    pub model_id: String,

    pub source: ModelSelectionSource,
}

impl ModelSelection {
    pub fn new(
        provider_id: impl Into<String>,
        model_id: impl Into<String>,
        source: ModelSelectionSource,
    ) -> Self {
        Self {
            provider_id: provider_id.into(),
            model_id: model_id.into(),
            source,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DefaultModelConfig {
    #[serde(default)]
    pub provider_id: String,

    #[serde(default)]
    pub model: String,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default)]
pub struct DefaultProviderInfo {
    pub provider_id: String,

    pub default_model: String,
}

pub struct ModelSelector;

impl ModelSelector {
    pub fn select(
        request_override: Option<&str>,
        profile: &AgentProfile,
        default_model: Option<&DefaultModelConfig>,
        default_provider: Option<&DefaultProviderInfo>,
    ) -> Result<ModelSelection, AgentError> {
        if let Some(model_str) = request_override {
            if !model_str.is_empty() {
                if let Some(ref slot) = profile.model_slot {
                    if !slot.provider_id.is_empty() {
                        return Ok(ModelSelection::new(
                            &slot.provider_id,
                            model_str,
                            ModelSelectionSource::RequestOverride,
                        ));
                    }
                }
                if let Some(am) = default_model {
                    if !am.provider_id.is_empty() {
                        return Ok(ModelSelection::new(
                            &am.provider_id,
                            model_str,
                            ModelSelectionSource::RequestOverride,
                        ));
                    }
                }
                if let Some(dp) = default_provider {
                    if !dp.provider_id.is_empty() {
                        return Ok(ModelSelection::new(
                            &dp.provider_id,
                            model_str,
                            ModelSelectionSource::RequestOverride,
                        ));
                    }
                }
            }
        }

        if let Some(ref slot) = profile.model_slot {
            if !slot.provider_id.is_empty() && !slot.model.is_empty() {
                return Ok(ModelSelection::new(
                    &slot.provider_id,
                    &slot.model,
                    ModelSelectionSource::AgentProfile,
                ));
            }
        }

        if let Some(am) = default_model {
            if !am.provider_id.is_empty() && !am.model.is_empty() {
                return Ok(ModelSelection::new(
                    &am.provider_id,
                    &am.model,
                    ModelSelectionSource::DefaultModel,
                ));
            }
        }

        if let Some(dp) = default_provider {
            if !dp.provider_id.is_empty() && !dp.default_model.is_empty() {
                return Ok(ModelSelection::new(
                    &dp.provider_id,
                    &dp.default_model,
                    ModelSelectionSource::DefaultProvider,
                ));
            }
        }

        Err(AgentError::provider_selection(
            "No model available: no request override, agent profile model slot, \
             default_model, or default provider configured",
        ))
    }
}
