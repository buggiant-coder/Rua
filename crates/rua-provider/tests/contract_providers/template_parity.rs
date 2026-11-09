use rua_provider::config_loader::{ProviderConfig, ProviderModelEntry};
use rua_provider::manager::{create_provider_from_config, ProviderManager, ProviderTemplate};

fn template(id: &str) -> &'static ProviderTemplate {
    ProviderManager::new()
        .list_templates()
        .iter()
        .find(|template| template.id == id)
        .unwrap_or_else(|| panic!("missing provider template {id}"))
}

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

fn config_from_template(template: &ProviderTemplate) -> ProviderConfig {
    let mut config = template.to_config();
    config.api_key = "test-key".to_string();
    config.timeout = Some(1);
    config.support_model_discovery = false;
    config
}

#[test]
fn every_qwenpaw_provider_matches_the_pinned_catalog_with_explicit_rua_overrides() {
    let snapshot: serde_json::Value =
        serde_json::from_str(include_str!("../../data/qwenpaw.json")).unwrap();
    let upstream = snapshot["providers"].as_array().unwrap();
    let aliases = [("qwenpaw-local", "rua-local"), ("gemini", "google")];
    let local: serde_json::Value =
        serde_json::from_str(include_str!("../../data/local_templates.json")).unwrap();
    assert_eq!(
        ProviderManager::new().list_templates().len(),
        upstream.len() + local["providers"].as_array().unwrap().len()
    );
    for source in upstream {
        let upstream_id = source["id"].as_str().unwrap();
        let id = aliases
            .iter()
            .find(|(name, _)| *name == upstream_id)
            .map(|(_, id)| *id)
            .unwrap_or(upstream_id);
        let template = template(id);
        let config = template.to_config();
        let actual = serde_json::to_value(&config).unwrap();
        for key in [
            "require_api_key",
            "freeze_url",
            "support_connection_check",
            "support_model_discovery",
            "is_local",
            "api_key_prefix",
            "provider_group",
            "provider_group_name",
            "provider_variant",
            "generate_kwargs",
            "supports_oauth",
            "is_free_tier",
        ] {
            assert_eq!(actual[key], source[key], "{id}.{key}");
        }
        if id != "ollama" {
            assert_eq!(
                config.base_url,
                source["base_url"].as_str().unwrap(),
                "{id}"
            );
            assert_eq!(actual["chat_model"], source["chat_model"], "{id}");
        }
        let source_models = source["models"].as_array().unwrap();
        assert_eq!(config.models.len(), source_models.len(), "{id}");
        for (model, source) in config.models.iter().zip(source_models) {
            assert_eq!(model.id, source["id"].as_str().unwrap(), "{id}");
            let model = serde_json::to_value(model).unwrap();
            for key in [
                "name",
                "context_window",
                "max_output_tokens",
                "supports_image",
                "supports_video",
                "relay_reasoning",
                "is_free",
                "is_recommended",
            ] {
                assert_eq!(model[key], source[key], "{id}.{}.{key}", source["id"]);
            }
        }
    }
    assert_eq!(template("mimo").base_url, "https://api.xiaomimimo.com/v1");
    assert!(template("mimo").support_model_discovery());
    assert!(!template("volcengine-cn-agentplan").support_model_discovery());
    assert!(!template("opencode")
        .preset_models
        .contains(&"deepseek-v4-flash-free"));
    assert!(!template("opencode")
        .preset_models
        .contains(&"nemotron-3-super-free"));
}

#[test]
fn aliyun_models_select_reasoning_format_per_model() {
    let dashscope = template("dashscope");
    let qwen = dashscope.preset_model_entry("qwen3.8-max");
    assert_eq!(qwen.thinking_param_style.as_deref(), Some("budget"));
    assert_eq!(qwen.thinking_enabled, Some(true));
    assert_eq!(qwen.thinking_budget, Some(8192));
    assert_eq!(qwen.extra["reasoning_format"], "dashscope");
    for id in ["dashscope", "aliyun-tokenplan"] {
        let glm = template(id).preset_model_entry("glm-5.2");
        assert_eq!(glm.thinking_param_style.as_deref(), Some("effort"));
        assert_eq!(glm.reasoning_effort_options, ["high", "max"]);
        assert_eq!(glm.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(glm.extra["reasoning_format"], "dashscope");
    }
}

#[test]
fn qwenpaw_provider_template_metadata_is_exposed_for_ui_and_sync() {
    let openai_response = template("openai-response");
    assert_eq!(openai_response.name, "OpenAI (Responses API)");
    assert_eq!(openai_response.base_url, "https://api.openai.com/v1");
    assert_eq!(openai_response.chat_model, "OpenAIResponseModel");
    assert_eq!(openai_response.api_key_prefix, "sk-");
    assert!(openai_response.freeze_url);
    assert_eq!(openai_response.thinking_param_style(), Some("effort"));

    let dashscope = template("dashscope");
    assert_eq!(dashscope.provider_group(), "aliyun");
    assert_eq!(dashscope.provider_group_name(), "Aliyun");
    assert_eq!(dashscope.provider_variant(), "dashscope");
    assert_eq!(dashscope.thinking_param_style(), Some("budget"));
    assert_eq!(
        dashscope
            .base_url_options()
            .iter()
            .map(|option| (option.label.as_str(), option.value.as_str()))
            .collect::<Vec<_>>(),
        [
            (
                "China (Beijing)",
                "https://dashscope.aliyuncs.com/compatible-mode/v1",
            ),
            (
                "International (Singapore)",
                "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
            ),
            (
                "US (Virginia)",
                "https://dashscope-us.aliyuncs.com/compatible-mode/v1",
            ),
        ]
    );

    for (id, group, name, variant) in [
        ("aliyun-codingplan", "aliyun", "Aliyun", "coding_plan_cn"),
        (
            "aliyun-codingplan-intl",
            "aliyun",
            "Aliyun",
            "coding_plan_intl",
        ),
        ("aliyun-tokenplan", "aliyun", "Aliyun", "token_plan"),
        (
            "aliyun-tokenplan-intl",
            "aliyun",
            "Aliyun",
            "token_plan_intl",
        ),
        ("kimi-cn", "kimi", "Kimi", "open_platform_cn"),
        ("kimi-intl", "kimi", "Kimi", "open_platform_intl"),
        ("kimi-codingplan", "kimi", "Kimi", "coding_plan"),
        ("siliconflow-cn", "siliconflow", "SiliconFlow", "china"),
        (
            "siliconflow-intl",
            "siliconflow",
            "SiliconFlow",
            "international",
        ),
        ("volcengine-cn", "volcengine", "Volcengine", "open_platform"),
        (
            "volcengine-cn-codingplan",
            "volcengine",
            "Volcengine",
            "coding_plan",
        ),
        ("minimax", "minimax", "MiniMax", "open_platform_intl"),
        ("minimax-cn", "minimax", "MiniMax", "open_platform_cn"),
    ] {
        let template = template(id);
        assert_eq!(template.provider_group(), group, "{id} group");
        assert_eq!(template.provider_group_name(), name, "{id} group name");
        assert_eq!(template.provider_variant(), variant, "{id} variant");
    }
}

#[test]
fn public_templates_exclude_private_gateway_presets() {
    for template in ProviderManager::new().list_templates() {
        assert!(!template.id.starts_with("idealab"));
    }
}

#[tokio::test]
async fn rust_local_provider_templates_do_not_require_api_credentials_to_be_configured() {
    for (id, base_url, preset_models) in [
        ("ollama", "http://localhost:11434", &[] as &[&str]),
        ("lmstudio", "http://localhost:1234/v1", &[][..]),
        ("rua-local", "", &[][..]),
    ] {
        let template = template(id);
        assert!(
            template.is_local,
            "{id} should stay in the local provider group"
        );
        assert!(
            !template.require_api_key,
            "{id} should not need API credentials before the UI treats it as configured"
        );
        assert_eq!(template.api_key_prefix, "");
        assert_eq!(template.base_url, base_url);
        assert!(
            !template.freeze_url,
            "{id} endpoint should remain editable or runtime-managed"
        );
        assert!(template.support_connection_check);
        assert_eq!(template.preset_models, preset_models);

        if id == "rua-local" {
            continue;
        }

        let config = config_from_template(template);
        let provider = create_provider_from_config(&config)
            .unwrap_or_else(|err| panic!("failed to create local provider {id}: {err}"));
        let info = provider.info();
        assert_eq!(info.id, id);
        assert!(
            info.is_local,
            "{id} ProviderInfo should advertise local=true"
        );
        assert_eq!(
            info.support_connection_check,
            template.support_connection_check
        );

        for model_id in preset_models {
            assert!(
                provider.supports_model(model_id),
                "{id} should support seeded local model {model_id}"
            );
        }
        if preset_models.is_empty() {
            assert!(
                !provider.supports_model("unseeded-local-model"),
                "{id} should not invent model ids before runtime discovery or download state"
            );
        }
    }
}

#[tokio::test]
async fn openai_compatible_templates_project_to_provider_info_and_models() {
    for id in [
        "kimi-cn",
        "kimi-codingplan",
        "volcengine-cn",
        "volcengine-cn-codingplan",
        "opencode",
        "kilo",
        "github-models",
        "mimo-tokenplan",
    ] {
        let template = template(id);
        let config = config_from_template(template);
        let provider = create_provider_from_config(&config)
            .unwrap_or_else(|err| panic!("failed to create provider from {id}: {err}"));

        let info = provider.info();
        assert_eq!(info.id, template.id);
        assert_eq!(info.name, template.name);
        assert_eq!(info.provider_type.as_deref(), Some("OpenAIChatModel"));
        assert_eq!(
            info.support_connection_check, template.support_connection_check,
            "{id} provider info should preserve connection-check flag"
        );
        assert!(!info.support_model_discovery);

        let models = provider
            .fetch_models()
            .await
            .unwrap_or_else(|err| panic!("{id} should return configured models: {err}"));
        let model_ids: Vec<_> = models.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(
            model_ids, template.preset_models,
            "{id} model order changed"
        );
        for model_id in template.preset_models {
            assert!(
                provider.supports_model(model_id),
                "{id} should support default model {model_id}"
            );
        }
    }
}

#[tokio::test]
async fn minimax_templates_project_to_anthropic_provider_info_and_models() {
    for (id, support_connection_check) in [("minimax", false), ("minimax-cn", false)] {
        let template = template(id);
        let config = config_from_template(template);
        let provider = create_provider_from_config(&config)
            .unwrap_or_else(|err| panic!("failed to create MiniMax provider {id}: {err}"));

        let info = provider.info();
        assert_eq!(info.id, template.id);
        assert_eq!(info.name, template.name);
        assert_eq!(info.provider_type.as_deref(), Some("AnthropicChatModel"));
        assert_eq!(info.support_connection_check, support_connection_check);
        assert!(!info.is_local);
        assert!(
            !info.support_model_discovery,
            "MiniMax uses catalog discovery"
        );

        let models = provider
            .fetch_models()
            .await
            .unwrap_or_else(|err| panic!("{id} should return configured models: {err}"));
        let model_ids: Vec<_> = models.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(model_ids, template.preset_models);
        assert!(provider.supports_model("MiniMax-M2.5"));
        provider
            .create_chat_model("MiniMax-M2.5")
            .unwrap_or_else(|err| panic!("{id} should create MiniMax chat model: {err}"));
    }
}

#[tokio::test]
async fn dashscope_template_projects_to_dashscope_provider_info_and_models() {
    let template = template("dashscope");
    assert_eq!(template.name, "DashScope");
    assert_eq!(
        template.base_url,
        "https://dashscope.aliyuncs.com/compatible-mode/v1"
    );
    assert_eq!(
        template.chat_model, "DashScopeChatModel",
        "DashScope must stay on Rust's DashScope provider variant, not generic OpenAI-compatible"
    );
    assert!(!template.freeze_url);
    assert!(template.support_connection_check);
    assert!(template.preset_models.contains(&"qwen3.8-max"));

    let config = config_from_template(template);
    let provider = create_provider_from_config(&config).expect("dashscope provider");
    let info = provider.info();
    assert_eq!(info.id, "dashscope");
    assert_eq!(info.name, "DashScope");
    assert_eq!(info.provider_type.as_deref(), Some("DashScopeChatModel"));
    assert!(!info.is_local);
    assert!(info.support_connection_check);
    assert!(
        !info.support_model_discovery,
        "configuration disables discovery"
    );
    for model_id in template.preset_models {
        assert!(
            provider.supports_model(model_id),
            "DashScope should support default model {model_id}"
        );
    }
    assert!(provider.supports_model("qwen3.8-max"));
}

#[test]
fn ollama_template_projects_to_local_provider_without_api_key_requirement() {
    let template = template("ollama");
    let config = config_from_template(template);
    let provider = create_provider_from_config(&config).expect("ollama provider");

    let info = provider.info();
    assert_eq!(info.id, "ollama");
    assert_eq!(info.name, "Ollama (Local)");
    assert_eq!(info.provider_type.as_deref(), Some("OllamaChatModel"));
    assert!(info.is_local);
    assert!(info.support_connection_check);
    assert!(
        !info.support_model_discovery,
        "configuration disables discovery"
    );

    for model_id in template.preset_models {
        assert!(
            provider.supports_model(model_id),
            "Ollama template should expose configured local model {model_id}"
        );
    }
    assert!(
        provider.create_chat_model("ad-hoc-local-model").is_ok(),
        "native Ollama can create a chat model for a user-provided local tag"
    );
}

#[tokio::test]
async fn empty_model_templates_remain_valid_openai_compatible_providers() {
    for id in ["siliconflow-cn", "siliconflow-intl"] {
        let template = template(id);
        let config = config_from_template(template);
        let provider = create_provider_from_config(&config)
            .unwrap_or_else(|err| panic!("failed to create empty-model provider {id}: {err}"));

        assert_eq!(provider.info().id, id);
        assert_eq!(
            provider.info().provider_type.as_deref(),
            Some("OpenAIChatModel")
        );
        assert!(
            provider.fetch_models().await.expect("models").is_empty(),
            "{id} should not invent default models"
        );
        assert!(
            !provider.supports_model("invented-model"),
            "{id} should not claim unknown model support"
        );
    }

    let generic = template("openai-compatible");
    assert_eq!(generic.base_url, "");
    assert!(!generic.freeze_url);
    assert!(generic.preset_models.is_empty());
    assert_eq!(generic.chat_model, "OpenAIChatModel");
}

#[test]
fn provider_config_model_projection_preserves_models_then_extra_models() {
    let mut extra = model("extra-model");
    extra.supports_image = Some(true);
    extra.context_window = Some(32000);
    extra
        .generate_kwargs
        .insert("temperature".into(), 0.2.into());

    let config = ProviderConfig {
        id: "mixed-models".into(),
        name: "Mixed Models".into(),
        base_url: "https://example.invalid/v1".into(),
        api_key: String::new(),
        api_keys: Vec::new(),
        api_key_prefixes: Vec::new(),
        headers: serde_json::Map::new(),
        custom_headers: serde_json::Map::new(),
        chat_model: Some("OpenAIChatModel".into()),
        models: vec![model("base-model")],
        extra_models: vec![extra],
        timeout: None,
        generate_kwargs: serde_json::Map::new(),
        support_model_discovery: false,
        support_connection_check: true,
        is_local: false,
        api_key_prefix: String::new(),
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
    };

    let models = config.to_model_infos();
    assert_eq!(
        models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        ["base-model", "extra-model"]
    );
    assert_eq!(models[1].supports_image, Some(true));
    assert_eq!(models[1].context_window, Some(32000));
    assert_eq!(models[1].generate_kwargs["temperature"], 0.2);
}
