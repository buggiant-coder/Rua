use llm_multiplex::ModelInfo;
use rua_provider::config_loader::{ProviderConfig, ProviderConfigLoader, ProviderModelEntry};
use rua_provider::manager::{
    apply_model_catalog_defaults_for_new_model, create_provider_from_config, PluginProviderSpec,
};
use rua_provider::ProviderManager;

fn make_model_entry(id: &str) -> ProviderModelEntry {
    ProviderModelEntry {
        id: id.into(),
        name: String::new(),
        supports_multimodal: None,
        supports_image: None,
        supports_video: None,
        probe_source: None,
        context_window: None,
        max_output_tokens: None,
        generate_kwargs: Default::default(),
        thinking_enabled: None,
        thinking_budget: None,
        reasoning_effort: None,
        thinking_param_style: None,
        reasoning_effort_options: Vec::new(),
        thinking_budget_range: None,
        relay_reasoning: None,
        extra: Default::default(),
    }
}

fn make_provider_config(models: Vec<ProviderModelEntry>) -> ProviderConfig {
    ProviderConfig {
        id: "test-provider".into(),
        name: "Test".into(),
        base_url: "http://localhost".into(),
        api_key: String::new(),
        api_keys: Vec::new(),
        api_key_prefixes: Vec::new(),
        headers: Default::default(),
        custom_headers: Default::default(),
        chat_model: Some("OpenAIChatModel".into()),
        models,
        extra_models: vec![],
        timeout: None,
        generate_kwargs: Default::default(),
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
        extra: Default::default(),
    }
}

#[test]
fn provider_protocol_supplies_default_reasoning_style() {
    let mut config = make_provider_config(Vec::new());
    for (chat_model, expected) in [
        ("OpenAIResponseModel", Some("effort")),
        ("DashScopeResponseModel", Some("effort")),
        ("OllamaChatModel", Some("effort")),
        ("OpenRouterChatModel", Some("effort")),
        ("AnthropicChatModel", Some("budget")),
        ("GeminiChatModel", Some("budget")),
        ("DashScopeChatModel", Some("budget")),
        ("OpenAIChatModel", None),
    ] {
        config.chat_model = Some(chat_model.to_string());
        assert_eq!(
            config.protocol_thinking_param_style(),
            expected,
            "{chat_model}"
        );
    }
}

async fn load_models_through_manager(config: ProviderConfig) -> Vec<ModelInfo> {
    let tmp = tempfile::tempdir().unwrap();
    let loader = ProviderConfigLoader::new(tmp.path());
    loader.save_provider(&config).await.unwrap();

    let manager = ProviderManager::with_config_loader(ProviderConfigLoader::new(tmp.path()));
    manager.load_from_config().await.unwrap();
    let provider = manager.get(&config.id).unwrap();
    provider.fetch_models().await.unwrap()
}

#[test]
fn new_model_catalog_defaults_fill_known_model_capabilities() {
    let mut model = make_model_entry("gemini-2.0-flash");

    assert!(apply_model_catalog_defaults_for_new_model(&mut model));

    assert_eq!(model.context_window, Some(1_048_576));
    assert_eq!(model.max_output_tokens, Some(8_192));
    assert_eq!(model.supports_image, Some(true));
    assert_eq!(model.supports_video, Some(true));
    assert_eq!(model.supports_multimodal, Some(true));
}

#[test]
fn provider_factory_enriches_old_model_capabilities_without_overriding_config() {
    let mut config = make_provider_config(vec![make_model_entry("deepseek-v4-flash-free")]);
    config.id = "opencode".into();

    let provider = create_provider_from_config(&config).unwrap();
    let model = provider
        .create_chat_model("deepseek-v4-flash-free")
        .unwrap();
    assert_eq!(model.model_info().supports_multimodal, Some(false));
    assert_eq!(model.model_info().supports_image, Some(false));
    assert_eq!(model.model_info().supports_video, Some(false));

    config.models[0].supports_multimodal = Some(true);
    config.models[0].supports_image = Some(true);
    let provider = create_provider_from_config(&config).unwrap();
    let model = provider
        .create_chat_model("deepseek-v4-flash-free")
        .unwrap();
    assert_eq!(model.model_info().supports_multimodal, Some(true));
    assert_eq!(model.model_info().supports_image, Some(true));
    assert_eq!(model.model_info().supports_video, Some(false));
}

#[test]
fn new_model_catalog_defaults_do_not_generate_thinking_fields() {
    let mut model = make_model_entry("qwen3.7-plus");

    assert!(apply_model_catalog_defaults_for_new_model(&mut model));

    assert_eq!(model.thinking_enabled, None);
    assert_eq!(model.thinking_param_style, None);
    assert_eq!(model.thinking_budget, None);
    assert_eq!(model.thinking_budget_range, None);
    assert_eq!(model.reasoning_effort, None);
    assert!(model.reasoning_effort_options.is_empty());
}

#[test]
fn new_model_catalog_defaults_match_alias_without_rewriting_model_id() {
    let mut model = make_model_entry("opus-4.8");

    assert!(apply_model_catalog_defaults_for_new_model(&mut model));

    assert_eq!(model.id, "opus-4.8");
    assert_eq!(model.context_window, Some(1_000_000));
    assert_eq!(model.supports_image, Some(true));
    assert_eq!(model.supports_video, Some(false));
    assert_eq!(model.supports_multimodal, Some(true));
}

#[test]
fn new_model_catalog_defaults_use_safe_context_fallback_for_unknown_models() {
    let mut model = make_model_entry("my-custom-model-v1");

    assert!(apply_model_catalog_defaults_for_new_model(&mut model));

    assert_eq!(model.context_window, Some(128 * 1024));
    assert_eq!(model.supports_image, None);
    assert_eq!(model.supports_video, None);
    assert_eq!(model.supports_multimodal, None);
    assert_eq!(model.thinking_param_style, None);
}

#[tokio::test]
async fn load_from_config_keeps_size_limits_but_enriches_runtime_capabilities() {
    let config = make_provider_config(vec![make_model_entry("gemini-2.0-flash")]);

    let models = load_models_through_manager(config).await;
    let model = &models[0];
    assert_eq!(model.context_window, None);
    assert_eq!(model.max_output_tokens, None);
    assert_eq!(model.supports_image, Some(true));
    assert_eq!(model.supports_video, Some(true));
    assert_eq!(model.supports_multimodal, Some(true));
}

#[test]
fn openai_response_model_uses_responses_provider_not_chat_completions_fallback() {
    let mut config = make_provider_config(vec![make_model_entry("gpt-5")]);
    config.id = "openai-response".into();
    config.name = "OpenAI (Responses API)".into();
    config.base_url = "https://api.openai.com/v1".into();
    config.chat_model = Some("OpenAIResponseModel".into());

    let provider = create_provider_from_config(&config).expect("responses provider");

    assert_eq!(
        provider.info().provider_type.as_deref(),
        Some("OpenAIResponseModel"),
        "OpenAIResponseModel must not fall back to OpenAIChatModel"
    );
}

#[test]
fn dashscope_response_model_uses_responses_provider() {
    let mut config = make_provider_config(vec![make_model_entry("qwen3.8-max")]);
    config.id = "dashscope".into();
    config.name = "DashScope".into();
    config.base_url = "https://workspace.cn-beijing.maas.aliyuncs.com/compatible-mode/v1".into();
    config.api_key = "sk-test".into();
    config.chat_model = Some("DashScopeResponseModel".into());

    let provider = create_provider_from_config(&config).expect("DashScope Responses provider");

    assert_eq!(
        provider.info().provider_type.as_deref(),
        Some("OpenAIResponseModel")
    );
}

#[test]
fn provider_adapter_selects_idealab_compatibility_implementations() {
    let mut anthropic = make_provider_config(vec![make_model_entry("claude-opus-4-6")]);
    anthropic.chat_model = Some("AnthropicChatModel".into());
    anthropic.extra.insert(
        "provider_adapter".into(),
        serde_json::Value::String("idealab-anthropic".into()),
    );
    let provider = create_provider_from_config(&anthropic).expect("idealab anthropic provider");
    assert_eq!(
        provider.info().provider_type.as_deref(),
        Some("IdealabAnthropic")
    );

    let mut vertex = make_provider_config(vec![make_model_entry("gemini-3.1-pro-preview")]);
    vertex.chat_model = Some("GeminiChatModel".into());
    vertex.extra.insert(
        "provider_adapter".into(),
        serde_json::Value::String("idealab-vertex".into()),
    );
    let provider = create_provider_from_config(&vertex).expect("idealab vertex provider");
    assert_eq!(
        provider.info().provider_type.as_deref(),
        Some("IdealabVertex")
    );
}

#[tokio::test]
async fn new_model_catalog_defaults_fill_kimi_coding_plan_text_only_capabilities() {
    let mut model = make_model_entry("kimi-for-coding");
    apply_model_catalog_defaults_for_new_model(&mut model);

    assert_eq!(model.id, "kimi-for-coding");
    assert_eq!(model.context_window, Some(256_000));
    assert_eq!(model.max_output_tokens, Some(16_384));
    assert_eq!(model.supports_image, Some(false));
    assert_eq!(model.supports_video, Some(false));
    assert_eq!(model.supports_multimodal, Some(false));
}

#[tokio::test]
async fn new_model_catalog_defaults_fill_mimo_token_plan_model_capabilities() {
    let mut models = vec![
        make_model_entry("mimo-v2.5-pro"),
        make_model_entry("mimo-v2.5"),
    ];
    for model in &mut models {
        apply_model_catalog_defaults_for_new_model(model);
    }
    let pro = &models[0];
    assert_eq!(pro.id, "mimo-v2.5-pro");
    assert_eq!(pro.supports_image, Some(false));
    assert_eq!(pro.supports_video, Some(false));
    assert_eq!(pro.supports_multimodal, Some(false));

    let standard = &models[1];
    assert_eq!(standard.id, "mimo-v2.5");
    assert_eq!(standard.context_window, Some(131_072));
    assert_eq!(standard.supports_image, Some(true));
    assert_eq!(standard.supports_video, Some(true));
    assert_eq!(standard.supports_multimodal, Some(true));
}

#[tokio::test]
async fn new_model_catalog_defaults_fill_opencode_free_tier_model_capabilities() {
    let mut models = vec![
        make_model_entry("deepseek-v4-flash-free"),
        make_model_entry("mimo-v2.5-free"),
        make_model_entry("nemotron-3-ultra-free"),
        make_model_entry("nemotron-3-super-free"),
    ];
    for model in &mut models {
        apply_model_catalog_defaults_for_new_model(model);
    }

    assert_eq!(
        models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        vec![
            "deepseek-v4-flash-free",
            "mimo-v2.5-free",
            "nemotron-3-ultra-free",
            "nemotron-3-super-free"
        ]
    );
    for model in models {
        assert_eq!(model.context_window, Some(131_072), "{}", model.id);
        assert_eq!(model.max_output_tokens, Some(8_192), "{}", model.id);
        assert_eq!(model.supports_image, Some(false), "{}", model.id);
        assert_eq!(model.supports_video, Some(false), "{}", model.id);
        assert_eq!(model.supports_multimodal, Some(false), "{}", model.id);
    }
}

#[tokio::test]
async fn new_model_catalog_defaults_fill_kilo_free_tier_model_capabilities() {
    let mut models = vec![
        make_model_entry("kilo-auto/free"),
        make_model_entry("nvidia/nemotron-3-ultra-550b-a55b:free"),
        make_model_entry("nvidia/nemotron-3-super-120b-a12b:free"),
        make_model_entry("poolside/laguna-m.1:free"),
        make_model_entry("poolside/laguna-xs.2:free"),
        make_model_entry("stepfun/step-3.7-flash:free"),
        make_model_entry("nex-agi/nex-n2-pro:free"),
    ];
    for model in &mut models {
        apply_model_catalog_defaults_for_new_model(model);
    }

    assert_eq!(
        models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        vec![
            "kilo-auto/free",
            "nvidia/nemotron-3-ultra-550b-a55b:free",
            "nvidia/nemotron-3-super-120b-a12b:free",
            "poolside/laguna-m.1:free",
            "poolside/laguna-xs.2:free",
            "stepfun/step-3.7-flash:free",
            "nex-agi/nex-n2-pro:free",
        ]
    );
    for model in models {
        assert_eq!(model.context_window, Some(131_072), "{}", model.id);
        assert_eq!(model.max_output_tokens, Some(8_192), "{}", model.id);
        assert_eq!(model.supports_image, Some(false), "{}", model.id);
        assert_eq!(model.supports_video, Some(false), "{}", model.id);
        assert_eq!(model.supports_multimodal, Some(false), "{}", model.id);
    }
}

#[tokio::test]
async fn new_model_catalog_defaults_fill_zhipu_glm_model_capabilities() {
    for provider_id in [
        "zhipu-cn",
        "zhipu-cn-codingplan",
        "zhipu-intl",
        "zhipu-intl-codingplan",
    ] {
        let mut models = vec![make_model_entry("glm-5"), make_model_entry("glm-5v-turbo")];
        for model in &mut models {
            apply_model_catalog_defaults_for_new_model(model);
        }

        let text = &models[0];
        assert_eq!(text.id, "glm-5", "{provider_id}");
        assert_eq!(text.context_window, Some(128_000), "{provider_id}");
        assert_eq!(text.max_output_tokens, Some(8_192), "{provider_id}");
        assert_eq!(text.supports_image, Some(false), "{provider_id}");
        assert_eq!(text.supports_video, Some(false), "{provider_id}");
        assert_eq!(text.supports_multimodal, Some(false), "{provider_id}");

        let vision = &models[1];
        assert_eq!(vision.id, "glm-5v-turbo", "{provider_id}");
        assert_eq!(vision.context_window, Some(128_000), "{provider_id}");
        assert_eq!(vision.max_output_tokens, Some(8_192), "{provider_id}");
        assert_eq!(vision.supports_image, Some(true), "{provider_id}");
        assert_eq!(vision.supports_video, Some(false), "{provider_id}");
        assert_eq!(vision.supports_multimodal, Some(true), "{provider_id}");
    }
}

#[tokio::test]
async fn new_model_catalog_defaults_use_model_id_without_provider_scope() {
    let mut models = vec![
        make_model_entry("kimi-k2.6"),
        make_model_entry("doubao-seed-2-0-pro-260215"),
    ];
    for model in &mut models {
        apply_model_catalog_defaults_for_new_model(model);
    }

    let kimi = &models[0];
    assert_eq!(kimi.id, "kimi-k2.6");
    assert_eq!(kimi.supports_image, Some(true));
    assert_eq!(kimi.supports_video, Some(true));
    assert_eq!(kimi.supports_multimodal, Some(true));

    let doubao = &models[1];
    assert_eq!(doubao.id, "doubao-seed-2-0-pro-260215");
    assert_eq!(doubao.supports_image, Some(true));
    assert_eq!(doubao.supports_video, Some(true));
    assert_eq!(doubao.supports_multimodal, Some(true));
}

#[tokio::test]
async fn new_model_catalog_defaults_preserve_explicit_capabilities() {
    let mut entry = make_model_entry("gemini-2.0-flash");
    entry.supports_image = Some(false);
    entry.context_window = Some(999);

    apply_model_catalog_defaults_for_new_model(&mut entry);

    assert_eq!(entry.supports_image, Some(false));
    assert_eq!(entry.context_window, Some(999));
    assert_eq!(entry.supports_video, Some(true));
    assert_eq!(entry.max_output_tokens, Some(8_192));
}

#[tokio::test]
async fn load_from_config_does_not_default_unknown_existing_models() {
    let config = make_provider_config(vec![make_model_entry("my-custom-model-v1")]);

    let models = load_models_through_manager(config).await;
    let model = &models[0];
    assert_eq!(model.context_window, None);
    assert_eq!(model.supports_image, None);
}

#[test]
fn multi_key_old_schema_normalizes_to_api_keys() {
    let mut config = make_provider_config(vec![make_model_entry("gpt-4")]);
    config.api_key = "sk-old-key".into();
    config.api_keys = Vec::new();

    let provider = create_provider_from_config(&config).unwrap();
    assert_eq!(provider.info().id, "test-provider");
}

#[test]
fn multi_key_new_schema_loads() {
    let mut config = make_provider_config(vec![make_model_entry("gpt-4")]);
    config.api_key = String::new();
    config.api_keys = vec!["k1".into(), "k2".into(), "k3".into()];

    let provider = create_provider_from_config(&config).unwrap();
    assert_eq!(provider.info().id, "test-provider");
}

#[test]
fn multi_key_both_fields_set_prepends_api_key() {
    let mut config = make_provider_config(vec![make_model_entry("gpt-4")]);
    config.api_key = "sk-legacy".into();
    config.api_keys = vec!["k1".into(), "k2".into()];

    let provider = create_provider_from_config(&config).unwrap();
    assert_eq!(provider.info().id, "test-provider");
}

#[test]
fn config_deserialization_old_schema_api_keys_absent() {
    let json = r#"{
        "id": "test",
        "name": "Test",
        "base_url": "http://localhost",
        "api_key": "sk-abc",
        "chat_model": "OpenAIChatModel",
        "models": []
    }"#;
    let config: ProviderConfig = serde_json::from_str(json).unwrap();
    assert!(config.api_keys.is_empty());
    assert_eq!(config.api_key, "sk-abc");
}

#[test]
fn config_deserialization_new_schema_api_keys_present() {
    let json = r#"{
        "id": "test",
        "name": "Test",
        "base_url": "http://localhost",
        "api_key": "",
        "api_keys": ["k1", "k2", "k3"],
        "chat_model": "OpenAIChatModel",
        "models": []
    }"#;
    let config: ProviderConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.api_keys, vec!["k1", "k2", "k3"]);
}

#[tokio::test]
async fn register_plugin_provider_creates_and_registers() {
    let manager = ProviderManager::new();
    let spec = PluginProviderSpec {
        id: "plugin-llm".into(),
        label: "Plugin LLM".into(),
        base_url: "https://api.plugin.example/v1".into(),
        chat_model: Some("OpenAIChatModel".into()),
        preset_models: vec!["model-x".into(), "model-y".into()],
        require_api_key: true,
    };

    manager.register_plugin_provider(&spec).await.unwrap();

    let provider = manager.get("plugin-llm").unwrap();
    assert_eq!(provider.info().id, "plugin-llm");
    assert_eq!(provider.info().name, "Plugin LLM");
    assert!(provider.supports_model("model-x"));
    assert!(provider.supports_model("model-y"));
}

#[tokio::test]
async fn unregister_plugin_provider_removes_from_memory() {
    let manager = ProviderManager::new();
    let spec = PluginProviderSpec {
        id: "plugin-llm".into(),
        label: "Plugin LLM".into(),
        base_url: "https://api.plugin.example/v1".into(),
        chat_model: Some("OpenAIChatModel".into()),
        preset_models: vec!["model-x".into()],
        require_api_key: false,
    };

    manager.register_plugin_provider(&spec).await.unwrap();
    assert!(manager.get("plugin-llm").is_some());

    manager.unregister_plugin_provider("plugin-llm");
    assert!(manager.get("plugin-llm").is_none());
}

#[tokio::test]
async fn register_plugin_provider_uses_id_when_label_is_empty() {
    let manager = ProviderManager::new();
    let spec = PluginProviderSpec {
        id: "no-label".into(),
        label: String::new(),
        base_url: "http://localhost:1234".into(),
        chat_model: None,
        preset_models: vec![],
        require_api_key: false,
    };

    manager.register_plugin_provider(&spec).await.unwrap();
    let provider = manager.get("no-label").unwrap();
    assert_eq!(provider.info().name, "no-label");
}
