use std::fmt;

use tracing::{debug, warn};

use llm_multiplex::ChatModel;

use crate::config_loader::ProviderConfigLoader;
use crate::manager::create_provider_from_config;

pub struct ResolvedModel {
    pub provider_id: String,
    pub model_id: String,
    pub chat_model: Box<dyn ChatModel>,
}

impl std::fmt::Debug for ResolvedModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedModel")
            .field("provider_id", &self.provider_id)
            .field("model_id", &self.model_id)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub enum Unavailable {
    NoDefaultModel {
        detail: String,
    },

    ProviderNotFound {
        provider_id: String,
        detail: String,
    },

    ProviderInvalid {
        provider_id: String,
        detail: String,
    },

    ModelNotSupported {
        provider_id: String,
        model_id: String,
        available: Vec<String>,
    },

    ChatModelFailed {
        provider_id: String,
        model_id: String,
        detail: String,
    },
}

impl fmt::Display for Unavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDefaultModel { detail } => {
                write!(f, "no active model configured: {detail}")
            }
            Self::ProviderNotFound {
                provider_id,
                detail,
            } => {
                write!(f, "provider '{provider_id}' not found: {detail}")
            }
            Self::ProviderInvalid {
                provider_id,
                detail,
            } => {
                write!(f, "provider '{provider_id}' config invalid: {detail}")
            }
            Self::ModelNotSupported {
                provider_id,
                model_id,
                ..
            } => {
                write!(
                    f,
                    "model '{model_id}' not supported by provider '{provider_id}'"
                )
            }
            Self::ChatModelFailed {
                provider_id,
                model_id,
                detail,
            } => {
                write!(
                    f,
                    "ChatModel creation failed for {provider_id}/{model_id}: {detail}"
                )
            }
        }
    }
}

impl std::error::Error for Unavailable {}

pub async fn resolve_default_model(
    loader: &ProviderConfigLoader,
) -> Result<ResolvedModel, Unavailable> {
    let active = loader.load_default_model().await.map_err(|e| {
        warn!(error = %e, "active model unavailable");
        Unavailable::NoDefaultModel {
            detail: e.to_string(),
        }
    })?;

    debug!(
        provider_id = %active.provider_id,
        model = %active.model,
        "resolving active model"
    );

    let config = loader
        .load_provider(&active.provider_id)
        .await
        .map_err(|e| {
            warn!(provider_id = %active.provider_id, error = %e, "provider config unavailable");
            Unavailable::ProviderNotFound {
                provider_id: active.provider_id.clone(),
                detail: e.to_string(),
            }
        })?;

    let provider = create_provider_from_config(&config).map_err(|e| {
        warn!(provider_id = %active.provider_id, error = %e, "provider construction failed");
        Unavailable::ProviderInvalid {
            provider_id: active.provider_id.clone(),
            detail: e.to_string(),
        }
    })?;

    if !provider.supports_model(&active.model) {
        let available: Vec<String> = config.all_models().iter().map(|m| m.id.clone()).collect();
        warn!(
            provider_id = %active.provider_id,
            model = %active.model,
            ?available,
            "model not supported by provider"
        );
        return Err(Unavailable::ModelNotSupported {
            provider_id: active.provider_id,
            model_id: active.model,
            available,
        });
    }

    let chat_model = provider.create_chat_model(&active.model).map_err(|e| {
        warn!(
            provider_id = %active.provider_id,
            model = %active.model,
            error = %e,
            "ChatModel creation failed"
        );
        Unavailable::ChatModelFailed {
            provider_id: active.provider_id.clone(),
            model_id: active.model.clone(),
            detail: e.to_string(),
        }
    })?;

    debug!(
        provider_id = %active.provider_id,
        model = %active.model,
        "active model resolved"
    );

    Ok(ResolvedModel {
        provider_id: active.provider_id,
        model_id: active.model,
        chat_model,
    })
}
