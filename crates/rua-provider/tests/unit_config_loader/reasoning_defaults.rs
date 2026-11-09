use rua_provider::{config_loader::ProviderConfigLoader, manager::ProviderManager};

#[test]
fn anthropic_presets_materialize_model_reasoning_capabilities() {
    let template = ProviderManager::new()
        .list_templates()
        .iter()
        .find(|entry| entry.id == "anthropic")
        .expect("template");
    let model = template.preset_model_entry("claude-opus-4-7");
    assert_eq!(model.thinking_param_style.as_deref(), Some("effort"));
    assert!(model
        .reasoning_effort_options
        .iter()
        .any(|value| value == "high"));
    assert_eq!(model.thinking_budget_range, None);
}

#[tokio::test]
async fn loading_provider_fills_missing_model_reasoning_without_overwriting_explicit_values() {
    let dir = tempfile::tempdir().unwrap();
    let loader = ProviderConfigLoader::new(dir.path());
    let template = ProviderManager::new()
        .list_templates()
        .iter()
        .find(|entry| entry.id == "anthropic")
        .expect("template");
    let mut config = template.to_config();
    config.models = vec![template.preset_model_entry("claude-opus-4-7")];
    config.models[0].thinking_param_style = None;
    config.models[0].reasoning_effort_options.clear();
    let mut explicit = config.models[0].clone();
    explicit.id = "claude-fable-5".into();
    explicit.thinking_param_style = Some("budget".into());
    explicit.thinking_budget = Some(2048);
    config.extra_models = vec![explicit];
    loader.save_provider(&config).await.unwrap();

    let loaded = loader.load_provider("anthropic").await.unwrap();
    assert_eq!(
        loaded.models[0].thinking_param_style.as_deref(),
        Some("effort")
    );
    assert!(loaded.models[0]
        .reasoning_effort_options
        .iter()
        .any(|value| value == "high"));
    assert_eq!(
        loaded.extra_models[0].thinking_param_style.as_deref(),
        Some("budget")
    );
    assert_eq!(loaded.extra_models[0].thinking_budget, Some(2048));
}
