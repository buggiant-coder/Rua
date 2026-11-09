use tracing::debug;

use llm_multiplex::{ChatModel, LlmError};

use crate::manager::ProviderManager;

#[derive(Debug, Clone, Default)]
pub struct RouteRequest {
    pub provider_id: Option<String>,

    pub model_id: Option<String>,

    pub agent_model_slot: Option<AgentModelSlot>,
}

#[derive(Debug, Clone)]
pub struct AgentModelSlot {
    pub provider_id: Option<String>,
    pub model_id: Option<String>,
}

pub struct Router<'a> {
    manager: &'a ProviderManager,
}

impl<'a> Router<'a> {
    pub fn new(manager: &'a ProviderManager) -> Self {
        Self { manager }
    }

    pub async fn resolve(&self, request: &RouteRequest) -> Result<Box<dyn ChatModel>, LlmError> {
        if let (Some(ref pid), Some(ref mid)) = (&request.provider_id, &request.model_id) {
            debug!(provider_id = %pid, model = %mid, "routing: explicit provider+model");
            return self.manager.chat_model(pid, mid);
        }

        if let Some(ref pid) = request.provider_id {
            if let Some(active) = self.manager.default_model().await {
                if active.provider_id == *pid {
                    debug!(provider_id = %pid, model = %active.model, "routing: explicit provider, active model");
                    return self.manager.chat_model(pid, &active.model);
                }
            }
            let provider = self.manager.get(pid).ok_or_else(|| LlmError::Config {
                message: format!("provider '{}' not registered", pid),
            })?;
            let models = provider.fetch_models().await?;
            if let Some(first) = models.first() {
                return self.manager.chat_model(pid, &first.id);
            }
            return Err(LlmError::Config {
                message: format!("provider '{}' has no models", pid),
            });
        }

        if let Some(ref mid) = request.model_id {
            debug!(model = %mid, "routing: explicit model, broadcast lookup");
            return self.broadcast_lookup(mid);
        }

        if let Some(ref slot) = request.agent_model_slot {
            if let (Some(ref pid), Some(ref mid)) = (&slot.provider_id, &slot.model_id) {
                debug!(provider_id = %pid, model = %mid, "routing: agent profile slot");
                return self.manager.chat_model(pid, mid);
            }
            if let Some(ref mid) = slot.model_id {
                debug!(model = %mid, "routing: agent slot model, broadcast lookup");
                return self.broadcast_lookup(mid);
            }
        }

        if let Some(active) = self.manager.default_model().await {
            debug!(
                provider_id = %active.provider_id,
                model = %active.model,
                "routing: global active model"
            );
            return self.manager.chat_model(&active.provider_id, &active.model);
        }

        Err(LlmError::Config {
            message: "no provider/model could be resolved: no active model and no explicit provider/model in request".into(),
        })
    }

    fn broadcast_lookup(&self, model: &str) -> Result<Box<dyn ChatModel>, LlmError> {
        for entry in self.manager.provider_ids() {
            if let Some(provider) = self.manager.get(&entry) {
                if provider.supports_model(model) {
                    debug!(
                        provider_id = %entry,
                        model = model,
                        "routing: broadcast lookup found"
                    );
                    return provider.create_chat_model(model);
                }
            }
        }

        Err(LlmError::ModelNotFound {
            provider: "broadcast".into(),
            model: model.into(),
        })
    }
}
