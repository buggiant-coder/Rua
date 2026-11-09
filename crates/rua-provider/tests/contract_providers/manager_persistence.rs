use rua_provider::config_loader::{
    DefaultModel, ProviderConfig, ProviderConfigLoader, ProviderModelEntry,
};
use rua_provider::manager::{create_provider_from_config, ProviderManager};

fn model(id: &str) -> ProviderModelEntry {
    ProviderModelEntry {
        id: id.to_string(),
        name: id.to_string(),
        supports_multimodal: None,
        supports_image: None,
        supports_video: None,
        probe_source: None,
        context_window: None,
        max_output_tokens: None,
        generate_kwargs: serde_json::Map::new(),
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

fn provider_config(id: &str, name: &str, models: Vec<&str>) -> ProviderConfig {
    ProviderConfig {
        id: id.to_string(),
        name: name.to_string(),
        base_url: "https://custom.example/v1".into(),
        api_key: "sk-custom".into(),
        api_keys: Vec::new(),
        api_key_prefixes: Vec::new(),
        headers: serde_json::Map::new(),
        custom_headers: serde_json::Map::new(),
        chat_model: Some("OpenAIChatModel".into()),
        models: models.into_iter().map(model).collect(),
        extra_models: Vec::new(),
        timeout: Some(1),
        generate_kwargs: serde_json::Map::new(),
        support_model_discovery: false,
        support_connection_check: false,
        is_local: false,
        api_key_prefix: "sk-".into(),
        is_custom: true,
        is_plugin: false,
        freeze_url: false,
        require_api_key: false,
        provider_group: String::new(),
        provider_group_name: String::new(),
        provider_variant: String::new(),
        base_url_options: Vec::new(),
        thinking_param_style: None,
        reasoning_effort_options: Vec::new(),
        thinking_budget_range: None,
        extra: serde_json::Map::new(),
    }
}

async fn manager_with_registered_provider(
    loader: ProviderConfigLoader,
    config: &ProviderConfig,
) -> ProviderManager {
    let manager = ProviderManager::with_config_loader(loader);
    let provider = create_provider_from_config(config).expect("provider");
    manager.register(provider);
    manager
}

#[tokio::test]
async fn set_default_model_persists_default_model_and_reload_restores_state() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let loader = ProviderConfigLoader::new(tmp.path());
    let config = provider_config("custom-openai", "Custom OpenAI", vec!["custom-model"]);
    loader.save_provider(&config).await.expect("save provider");

    let manager =
        manager_with_registered_provider(ProviderConfigLoader::new(tmp.path()), &config).await;
    manager
        .set_default_model(DefaultModel {
            provider_id: "custom-openai".into(),
            model: "custom-model".into(),
            source: Some("contract-test".into()),
            extra: serde_json::Map::from_iter([(
                "reason".into(),
                serde_json::Value::String("qwenpaw-parity".into()),
            )]),
        })
        .await
        .expect("set default model");

    let loaded_active = loader.load_default_model().await.expect("load active");
    assert_eq!(loaded_active.provider_id, "custom-openai");
    assert_eq!(loaded_active.model, "custom-model");
    assert_eq!(loaded_active.source.as_deref(), Some("contract-test"));
    assert_eq!(loaded_active.extra["reason"], "qwenpaw-parity");

    let reloaded = ProviderManager::with_config_loader(ProviderConfigLoader::new(tmp.path()));
    reloaded.load_from_config().await.expect("load from config");

    let active = reloaded.default_model().await.expect("active model");
    assert_eq!(active.provider_id, "custom-openai");
    assert_eq!(active.model, "custom-model");

    let provider = reloaded.get("custom-openai").expect("provider loaded");
    assert_eq!(provider.info().name, "Custom OpenAI");
    assert!(provider.supports_model("custom-model"));
}

#[tokio::test]
async fn set_default_model_validates_provider_and_model_before_persisting() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config = provider_config("custom-openai", "Custom OpenAI", vec!["custom-model"]);
    let manager =
        manager_with_registered_provider(ProviderConfigLoader::new(tmp.path()), &config).await;

    let missing_provider = manager
        .set_default_model(DefaultModel {
            provider_id: "missing-provider".into(),
            model: "custom-model".into(),
            source: None,
            extra: serde_json::Map::new(),
        })
        .await
        .expect_err("missing provider should fail");
    assert!(missing_provider
        .to_string()
        .contains("provider 'missing-provider' not found"));

    let missing_model = manager
        .set_default_model(DefaultModel {
            provider_id: "custom-openai".into(),
            model: "missing-model".into(),
            source: None,
            extra: serde_json::Map::new(),
        })
        .await
        .expect_err("missing model should fail");
    assert!(missing_model
        .to_string()
        .contains("model 'missing-model' not found in provider 'custom-openai'"));

    let loader = ProviderConfigLoader::new(tmp.path());
    assert!(
        loader.load_default_model().await.is_err(),
        "invalid activation must not persist default_model.json"
    );
    assert!(
        manager.default_model().await.is_none(),
        "invalid activation must not update memory"
    );
}

#[tokio::test]
async fn reload_provider_replaces_registered_runtime_from_disk() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let loader = ProviderConfigLoader::new(tmp.path());
    let initial = provider_config("custom-openai", "Custom OpenAI", vec!["old-model"]);
    loader.save_provider(&initial).await.expect("save initial");

    let manager = ProviderManager::with_config_loader(ProviderConfigLoader::new(tmp.path()));
    manager.load_from_config().await.expect("initial load");

    let provider = manager.get("custom-openai").expect("initial provider");
    assert_eq!(provider.info().name, "Custom OpenAI");
    assert!(provider.supports_model("old-model"));
    assert!(!provider.supports_model("new-model"));

    let updated = provider_config("custom-openai", "Updated OpenAI", vec!["new-model"]);
    loader.save_provider(&updated).await.expect("save updated");
    manager
        .reload_provider("custom-openai")
        .await
        .expect("reload provider");

    let provider = manager.get("custom-openai").expect("updated provider");
    assert_eq!(provider.info().name, "Updated OpenAI");
    assert!(!provider.supports_model("old-model"));
    assert!(provider.supports_model("new-model"));
}

#[tokio::test]
async fn clear_default_model_removes_disk_and_memory_state() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let loader = ProviderConfigLoader::new(tmp.path());
    let config = provider_config("custom-openai", "Custom OpenAI", vec!["custom-model"]);
    loader.save_provider(&config).await.expect("save provider");

    let manager =
        manager_with_registered_provider(ProviderConfigLoader::new(tmp.path()), &config).await;
    manager
        .set_default_model(DefaultModel {
            provider_id: "custom-openai".into(),
            model: "custom-model".into(),
            source: None,
            extra: serde_json::Map::new(),
        })
        .await
        .expect("set default model");
    assert!(ProviderConfigLoader::new(tmp.path())
        .load_default_model()
        .await
        .is_ok());

    manager
        .clear_default_model()
        .await
        .expect("clear active model");

    assert!(manager.default_model().await.is_none());
    assert!(ProviderConfigLoader::new(tmp.path())
        .load_default_model()
        .await
        .is_err());
}
