use std::sync::Arc;

use async_trait::async_trait;
use llm_multiplex::{ChatModel, ConnectionStatus, LlmError, ModelInfo, Provider, ProviderInfo};
use rua_provider::ProviderManager;

struct DiscoveryProvider {
    info: ProviderInfo,
    models: Vec<ModelInfo>,
}

#[async_trait]
impl Provider for DiscoveryProvider {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    fn supports_model(&self, _model: &str) -> bool {
        true
    }

    fn create_chat_model(&self, _model: &str) -> Result<Box<dyn ChatModel>, LlmError> {
        Err(LlmError::Config {
            message: "not used by discovery test".into(),
        })
    }

    async fn fetch_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        Ok(self.models.clone())
    }

    async fn check_connection(&self) -> Result<ConnectionStatus, LlmError> {
        Ok(ConnectionStatus::Connected)
    }
}

fn model(id: &str) -> ModelInfo {
    ModelInfo {
        id: id.into(),
        name: id.into(),
        context_window: None,
        max_output_tokens: None,
        supports_multimodal: None,
        supports_image: None,
        supports_video: None,
        probe_source: Some("api".into()),
        generate_kwargs: serde_json::Map::new(),
    }
}

#[tokio::test]
async fn custom_gateway_discovery_does_not_add_private_presets() {
    let manager = ProviderManager::new();
    manager.register(Arc::new(DiscoveryProvider {
        info: ProviderInfo {
            id: "idealab-responses".into(),
            name: "Idealab Responses".into(),
            provider_type: Some("OpenAIResponseModel".into()),
            is_local: false,
            support_model_discovery: true,
            support_connection_check: true,
        },
        models: vec![model("qwen3-coder-plus")],
    }));

    let models = manager
        .discover_models("idealab-responses")
        .await
        .expect("Idealab discovery");
    let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();

    assert_eq!(ids, ["qwen3-coder-plus"]);
}

#[tokio::test]
async fn custom_protocol_adapters_do_not_invent_preset_models() {
    for (provider_id, provider_type) in [
        ("idealab-claude", "IdealabAnthropic"),
        ("idealab-gemini", "IdealabVertex"),
    ] {
        let manager = ProviderManager::new();
        manager.register(Arc::new(DiscoveryProvider {
            info: ProviderInfo {
                id: provider_id.into(),
                name: provider_id.into(),
                provider_type: Some(provider_type.into()),
                is_local: false,
                support_model_discovery: false,
                support_connection_check: true,
            },
            models: vec![],
        }));
        assert!(manager
            .discover_models(provider_id)
            .await
            .unwrap()
            .is_empty());
    }
}

#[tokio::test]
async fn discovery_merges_upstream_catalog_without_overwriting_api_metadata() {
    for (provider_id, known_id) in [
        ("deepseek", "deepseek-v4-pro"),
        ("kimi-cn", "kimi-k3"),
        ("kimi-intl", "kimi-k3"),
    ] {
        let manager = ProviderManager::new();
        let mut reported = model(known_id);
        reported.context_window = Some(123_456);
        manager.register(Arc::new(DiscoveryProvider {
            info: ProviderInfo {
                id: provider_id.into(),
                name: provider_id.into(),
                provider_type: Some("OpenAIChatModel".into()),
                is_local: false,
                support_model_discovery: true,
                support_connection_check: true,
            },
            models: vec![reported, model("custom-deployment")],
        }));
        let models = manager.discover_models(provider_id).await.unwrap();
        let known = models
            .iter()
            .filter(|model| model.id == known_id)
            .collect::<Vec<_>>();
        assert_eq!(known.len(), 1);
        assert_eq!(known[0].context_window, Some(123_456));
        assert!(models.iter().any(|model| model.id == "custom-deployment"));
        for preset in manager
            .list_templates()
            .iter()
            .find(|template| template.id == provider_id)
            .unwrap()
            .preset_models
        {
            assert!(
                models.iter().any(|model| model.id == *preset),
                "{provider_id}: {preset}"
            );
        }
    }
}

#[tokio::test]
async fn local_discovery_does_not_infer_cloud_context_limits() {
    let manager = ProviderManager::new();
    manager.register(Arc::new(DiscoveryProvider {
        info: ProviderInfo {
            id: "ollama".into(),
            name: "Ollama".into(),
            provider_type: Some("OllamaChatModel".into()),
            is_local: true,
            support_model_discovery: true,
            support_connection_check: true,
        },
        models: vec![model("qwen3.5")],
    }));
    let models = manager.discover_models("ollama").await.unwrap();
    assert_eq!(models[0].context_window, None);
}
