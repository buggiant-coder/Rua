use std::sync::Arc;

use rua_skills_hub::HubClient;

use crate::provider::MarketProvider;
use crate::providers::{AliyunProvider, ClawHubProvider, ModelScopeProvider, SkillHubProvider};

#[derive(Clone, Default)]
pub struct ProviderRegistry {
    providers: Vec<Arc<dyn MarketProvider>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    pub fn register(&mut self, provider: Arc<dyn MarketProvider>) {
        if let Some(slot) = self
            .providers
            .iter_mut()
            .find(|p| p.key() == provider.key())
        {
            *slot = provider;
        } else {
            self.providers.push(provider);
        }
    }

    pub fn get(&self, key: &str) -> Option<&Arc<dyn MarketProvider>> {
        self.providers.iter().find(|p| p.key() == key)
    }

    pub fn contains(&self, key: &str) -> bool {
        self.providers.iter().any(|p| p.key() == key)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn MarketProvider>> {
        self.providers.iter()
    }
}

pub fn build_default_registry(client: Arc<HubClient>) -> ProviderRegistry {
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(ClawHubProvider::new(client.clone())));
    registry.register(Arc::new(ModelScopeProvider::new(client.clone())));
    registry.register(Arc::new(AliyunProvider::new()));
    registry.register(Arc::new(SkillHubProvider::new(client)));
    registry
}
