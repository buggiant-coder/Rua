use rua_provider::config_loader::{ProviderConfig, ProviderModelEntry};
use rua_provider::manager::{create_provider_from_config, PROVIDER_TEMPLATES};

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

fn model_with_capabilities(id: &str) -> ProviderModelEntry {
    let mut entry = model(id);
    entry.name = format!("{id} Display");
    entry.supports_multimodal = Some(true);
    entry.supports_image = Some(true);
    entry.supports_video = Some(false);
    entry.probe_source = Some("config".to_string());
    entry.context_window = Some(131_072);
    entry.max_output_tokens = Some(8192);
    entry.generate_kwargs.insert(
        "temperature".to_string(),
        serde_json::Value::Number(serde_json::Number::from_f64(0.2).unwrap()),
    );
    entry
}

fn provider_config(
    id: &str,
    name: &str,
    base_url: &str,
    chat_model: &str,
    models: Vec<ProviderModelEntry>,
) -> ProviderConfig {
    ProviderConfig {
        id: id.to_string(),
        name: name.to_string(),
        base_url: base_url.to_string(),
        api_key: "test-key".to_string(),
        api_keys: Vec::new(),
        api_key_prefixes: Vec::new(),
        headers: serde_json::Map::new(),
        custom_headers: serde_json::Map::new(),
        chat_model: Some(chat_model.to_string()),
        models,
        extra_models: Vec::new(),
        timeout: Some(1),
        generate_kwargs: serde_json::Map::new(),
        support_model_discovery: false,
        support_connection_check: false,
        is_local: false,
        api_key_prefix: String::new(),
        is_custom: false,
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

#[test]
fn builtin_provider_templates_expose_required_contract_fields() {
    assert!(
        !PROVIDER_TEMPLATES.is_empty(),
        "provider template contract must not be empty"
    );

    for template in PROVIDER_TEMPLATES {
        assert!(!template.id.trim().is_empty(), "template id is required");
        assert!(
            !template.name.trim().is_empty(),
            "template name is required for {}",
            template.id
        );
        assert!(
            !template.chat_model.trim().is_empty(),
            "chat_model is required for {}",
            template.id
        );
        assert!(
            template
                .preset_models
                .iter()
                .all(|model| !model.trim().is_empty()),
            "default models cannot contain blanks for {}",
            template.id
        );
    }
}

#[tokio::test]
async fn openai_compatible_provider_satisfies_provider_contract() {
    let config = provider_config(
        "contract-openai",
        "Contract OpenAI",
        "https://example.invalid/v1",
        "OpenAIChatModel",
        vec![model("gpt-contract")],
    );

    let provider = create_provider_from_config(&config).expect("provider");
    let info = provider.info();
    assert_eq!(info.id, "contract-openai");
    assert_eq!(info.name, "Contract OpenAI");
    assert_eq!(info.provider_type.as_deref(), Some("OpenAIChatModel"));
    assert!(!info.is_local);
    assert!(!info.support_model_discovery);
    assert!(!info.support_connection_check);

    assert!(provider.supports_model("gpt-contract"));
    assert!(!provider.supports_model("missing-model"));
    provider
        .create_chat_model("gpt-contract")
        .expect("configured model creates chat model");

    let models = provider.fetch_models().await.expect("configured models");
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "gpt-contract");
}

#[tokio::test]
async fn provider_config_model_rows_survive_provider_projection() {
    let mut config = provider_config(
        "external-model-rows",
        "External Model Rows",
        "https://example.invalid/v1",
        "OpenAIChatModel",
        vec![model("base-model")],
    );
    config.extra_models = vec![model_with_capabilities("extra-model")];

    let provider = create_provider_from_config(&config).expect("provider");

    assert!(provider.supports_model("base-model"));
    assert!(provider.supports_model("extra-model"));
    provider
        .create_chat_model("extra-model")
        .expect("extra model from external row creates chat model");

    let models = provider.fetch_models().await.expect("configured models");
    assert_eq!(
        models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        ["base-model", "extra-model"],
        "Rust provider projection must preserve configured model row order"
    );

    let extra = models
        .iter()
        .find(|model| model.id == "extra-model")
        .expect("extra model row projected");
    assert_eq!(extra.name, "extra-model Display");
    assert_eq!(extra.supports_multimodal, Some(true));
    assert_eq!(extra.supports_image, Some(true));
    assert_eq!(extra.supports_video, Some(false));
    assert_eq!(extra.probe_source.as_deref(), Some("config"));
    assert_eq!(extra.context_window, Some(131_072));
    assert_eq!(extra.max_output_tokens, Some(8192));
    assert_eq!(extra.generate_kwargs["temperature"], 0.2);
}

#[tokio::test]
async fn anthropic_and_gemini_providers_return_configured_models_without_network() {
    let cases = [
        (
            "contract-anthropic",
            "Anthropic Contract",
            "https://anthropic.invalid/v1",
            "AnthropicChatModel",
            "claude-contract",
        ),
        (
            "contract-gemini",
            "Gemini Contract",
            "https://gemini.invalid/v1beta",
            "GeminiChatModel",
            "gemini-contract",
        ),
    ];

    for (id, name, base_url, chat_model, model_id) in cases {
        let config = provider_config(id, name, base_url, chat_model, vec![model(model_id)]);
        let provider = create_provider_from_config(&config).expect("provider");

        assert_eq!(provider.info().id, id);
        assert_eq!(provider.info().name, name);
        assert_eq!(provider.info().provider_type.as_deref(), Some(chat_model));
        assert!(provider.supports_model(model_id));
        provider
            .create_chat_model(model_id)
            .unwrap_or_else(|err| panic!("{id} did not create chat model: {err}"));

        let models = provider
            .fetch_models()
            .await
            .unwrap_or_else(|err| panic!("{id} did not return configured models: {err}"));
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, model_id);
    }
}
