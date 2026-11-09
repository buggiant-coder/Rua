use rua_provider::config_loader::ProviderConfig;
use rua_provider::openrouter::create_openrouter_provider;

fn make_openrouter_config() -> ProviderConfig {
    ProviderConfig {
        id: "openrouter".into(),
        name: "OpenRouter".into(),
        base_url: "https://openrouter.ai/api/v1".into(),
        api_key: "sk-or-test".into(),
        api_keys: Vec::new(),
        api_key_prefixes: Vec::new(),
        headers: Default::default(),
        custom_headers: Default::default(),
        chat_model: Some("OpenRouterChatModel".into()),
        models: vec![],
        extra_models: vec![],
        timeout: None,
        generate_kwargs: Default::default(),
        support_model_discovery: true,
        support_connection_check: true,
        is_local: false,
        api_key_prefix: "sk-or-".into(),
        is_custom: false,
        is_plugin: false,
        freeze_url: false,
        require_api_key: true,
        provider_group: String::new(),
        provider_group_name: String::new(),
        provider_variant: String::new(),
        base_url_options: Vec::new(),
        thinking_param_style: None,
        reasoning_effort_options: Vec::new(),
        thinking_budget_range: None,
        extra: Default::default(),
    }
}

#[test]
fn openrouter_provider_constructs_with_default_headers() {
    let config = make_openrouter_config();
    let provider = create_openrouter_provider(&config).unwrap();
    let info = provider.info();

    assert_eq!(info.id, "openrouter");
    assert_eq!(info.name, "OpenRouter");
}

#[test]
fn openrouter_provider_preserves_custom_header_inputs() {
    let mut config = make_openrouter_config();
    config.headers.insert(
        "HTTP-Referer".into(),
        serde_json::Value::String("https://custom.app".into()),
    );
    config
        .headers
        .insert("X-Title".into(), serde_json::Value::String("my-app".into()));

    let provider = create_openrouter_provider(&config).unwrap();

    assert_eq!(provider.info().id, "openrouter");
}
