use rua_provider::ProviderConfigLoader;

fn provider_payload(id: &str, api_key: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "name": format!("{id} provider"),
        "base_url": "https://provider.example/v1",
        "api_key": api_key,
        "chat_model": "OpenAIChatModel",
        "models": [
            {"id": "model-a", "name": "Model A"}
        ],
        "x-rust-preserved": true
    })
}

#[tokio::test]
async fn load_all_providers_skips_invalid_json_and_non_json_files() {
    let dir = tempfile::tempdir().unwrap();
    let providers_dir = dir.path().join("providers/providers");
    std::fs::create_dir_all(&providers_dir).unwrap();
    std::fs::write(providers_dir.join("bad-provider.json"), "{invalid-json").unwrap();
    std::fs::write(providers_dir.join("notes.txt"), "not a provider").unwrap();
    std::fs::write(
        providers_dir.join("good-provider.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "id": "good-provider",
            "name": "Good Provider",
            "base_url": "https://good.example/v1",
            "api_key": "sk-good",
            "chat_model": "OpenAIChatModel",
            "models": [
                {"id": "good-model", "name": "Good Model"}
            ],
            "x-rust-preserved": true
        }))
        .unwrap(),
    )
    .unwrap();

    let loader = ProviderConfigLoader::new(dir.path());

    let providers = loader.load_all_providers().await.unwrap();

    assert_eq!(providers.len(), 1);
    let provider = &providers[0];
    assert_eq!(provider.id, "good-provider");
    assert_eq!(provider.api_key, "sk-good");
    assert_eq!(provider.models.len(), 1);
    assert_eq!(provider.models[0].id, "good-model");
    assert_eq!(
        provider.extra.get("x-rust-preserved"),
        Some(&serde_json::Value::Bool(true))
    );
}

#[tokio::test]
async fn save_provider_with_key_encrypts_api_key_but_keeps_provider_json_editable() {
    let dir = tempfile::tempdir().unwrap();
    let loader = ProviderConfigLoader::with_key(dir.path(), [3_u8; 32]);
    let mut provider: rua_provider::config_loader::ProviderConfig =
        serde_json::from_value(provider_payload("encrypted", "sk-provider-secret")).unwrap();

    provider
        .extra
        .insert("future_field".into(), serde_json::json!({"kept": true}));
    loader.save_provider(&provider).await.unwrap();

    let path = dir.path().join("providers/providers/encrypted.json");
    let raw = std::fs::read(&path).unwrap();
    let on_disk: serde_json::Value = serde_json::from_slice(&raw).unwrap();

    assert_eq!(on_disk["id"], "encrypted");
    assert_eq!(on_disk["name"], "encrypted provider");
    assert_eq!(on_disk["base_url"], "https://provider.example/v1");
    assert_eq!(on_disk["future_field"], serde_json::json!({"kept": true}));
    assert_eq!(on_disk["api_key"]["rua_encrypted"], true);
    assert_eq!(on_disk["api_key"]["field"], "api_key");
    assert!(
        !String::from_utf8_lossy(&raw).contains("sk-provider-secret"),
        "provider config must stay JSON-editable while api_key is encrypted at rest"
    );

    let loaded = loader.load_provider("encrypted").await.unwrap();
    assert_eq!(loaded.api_key, "sk-provider-secret");
    assert_eq!(
        loaded.extra.get("future_field"),
        Some(&serde_json::json!({"kept": true}))
    );
}

#[tokio::test]
async fn provider_parity_metadata_roundtrips_as_first_class_fields() {
    let dir = tempfile::tempdir().unwrap();
    let loader = ProviderConfigLoader::new(dir.path());
    let mut provider: rua_provider::config_loader::ProviderConfig =
        serde_json::from_value(provider_payload("parity", "sk-provider-secret")).unwrap();
    provider.api_key_prefixes = vec!["ghp_".into(), "github_pat_".into()];
    provider.custom_headers.insert(
        "X-Trace-Provider".into(),
        serde_json::Value::String("rua".into()),
    );
    provider.provider_group = "github".into();
    provider.provider_group_name = "GitHub".into();
    provider.provider_variant = "models".into();
    provider.base_url_options = vec![
        rua_provider::config_loader::ProviderBaseUrlOption {
            label: "GitHub Models".into(),
            value: "https://models.github.ai/inference".into(),
        },
        rua_provider::config_loader::ProviderBaseUrlOption {
            label: "GitHub Models Preview".into(),
            value: "https://models.github.ai/preview".into(),
        },
    ];
    provider.thinking_param_style = Some("effort".into());
    provider.reasoning_effort_options = vec!["low".into(), "medium".into(), "high".into()];
    provider.thinking_budget_range = Some([1, 81920]);

    loader.save_provider(&provider).await.unwrap();
    let loaded = loader.load_provider("parity").await.unwrap();

    assert_eq!(loaded.api_key_prefixes, ["ghp_", "github_pat_"]);
    assert_eq!(
        loaded.custom_headers.get("X-Trace-Provider"),
        Some(&serde_json::Value::String("rua".into()))
    );
    assert_eq!(loaded.provider_group, "github");
    assert_eq!(loaded.provider_group_name, "GitHub");
    assert_eq!(loaded.provider_variant, "models");
    assert_eq!(
        loaded
            .base_url_options
            .iter()
            .map(|option| (option.label.as_str(), option.value.as_str()))
            .collect::<Vec<_>>(),
        [
            ("GitHub Models", "https://models.github.ai/inference"),
            ("GitHub Models Preview", "https://models.github.ai/preview"),
        ]
    );
    assert_eq!(loaded.thinking_param_style.as_deref(), Some("effort"));
    assert_eq!(loaded.reasoning_effort_options, ["low", "medium", "high"]);
    assert_eq!(loaded.thinking_budget_range, Some([1, 81920]));
}

#[tokio::test]
async fn save_provider_with_key_encrypts_api_keys_and_secret_custom_headers() {
    let dir = tempfile::tempdir().unwrap();
    let loader = ProviderConfigLoader::with_key(dir.path(), [9_u8; 32]);
    let mut provider: rua_provider::config_loader::ProviderConfig =
        serde_json::from_value(provider_payload("multi-secret", "sk-primary-secret")).unwrap();
    provider.api_keys = vec!["sk-secondary-secret".into(), "sk-tertiary-secret".into()];
    provider.custom_headers.insert(
        "Authorization".into(),
        serde_json::Value::String("Bearer header-secret".into()),
    );
    provider.custom_headers.insert(
        "X-Provider-Trace".into(),
        serde_json::Value::String("visible-trace".into()),
    );

    loader.save_provider(&provider).await.unwrap();

    let path = dir.path().join("providers/providers/multi-secret.json");
    let raw = std::fs::read_to_string(&path).unwrap();
    let on_disk: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(on_disk["api_key"]["rua_encrypted"], true);
    assert_eq!(on_disk["api_keys"]["rua_encrypted"], true);
    assert_eq!(
        on_disk["custom_headers"]["Authorization"]["rua_encrypted"],
        true
    );
    assert_eq!(
        on_disk["custom_headers"]["X-Provider-Trace"],
        "visible-trace"
    );
    assert!(!raw.contains("sk-primary-secret"));
    assert!(!raw.contains("sk-secondary-secret"));
    assert!(!raw.contains("sk-tertiary-secret"));
    assert!(!raw.contains("header-secret"));

    let loaded = loader.load_provider("multi-secret").await.unwrap();
    assert_eq!(loaded.api_key, "sk-primary-secret");
    assert_eq!(
        loaded.api_keys,
        ["sk-secondary-secret", "sk-tertiary-secret"]
    );
    assert_eq!(
        loaded.custom_headers.get("Authorization"),
        Some(&serde_json::Value::String("Bearer header-secret".into()))
    );
    assert_eq!(
        loaded.custom_headers.get("X-Provider-Trace"),
        Some(&serde_json::Value::String("visible-trace".into()))
    );
}

#[tokio::test]
async fn load_all_with_key_skips_wrong_key_encrypted_provider_without_dropping_valid_entries() {
    let dir = tempfile::tempdir().unwrap();
    let good_loader = ProviderConfigLoader::with_key(dir.path(), [5_u8; 32]);
    let wrong_key_loader = ProviderConfigLoader::with_key(dir.path(), [8_u8; 32]);

    let good: rua_provider::config_loader::ProviderConfig =
        serde_json::from_value(provider_payload("good", "sk-good-secret")).unwrap();
    let bad: rua_provider::config_loader::ProviderConfig =
        serde_json::from_value(provider_payload("bad", "sk-bad-secret")).unwrap();

    good_loader.save_provider(&good).await.unwrap();
    wrong_key_loader.save_provider(&bad).await.unwrap();

    let providers = good_loader.load_all_providers().await.unwrap();
    let ids: Vec<_> = providers
        .iter()
        .map(|provider| provider.id.as_str())
        .collect();

    assert_eq!(ids, vec!["good"]);
    assert_eq!(providers[0].api_key, "sk-good-secret");
}

#[tokio::test]
async fn load_provider_defaults_null_python_fields_and_preserves_unknown_fields() {
    let dir = tempfile::tempdir().unwrap();
    let providers_dir = dir.path().join("providers/providers");
    std::fs::create_dir_all(&providers_dir).unwrap();
    std::fs::write(
        providers_dir.join("nullable.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "id": "nullable",
            "name": null,
            "base_url": null,
            "api_key": null,
            "api_keys": null,
            "headers": null,
            "models": null,
            "extra_models": null,
            "generate_kwargs": null,
            "support_model_discovery": null,
            "support_connection_check": null,
            "is_local": null,
            "api_key_prefix": null,
            "is_custom": null,
            "is_plugin": null,
            "freeze_url": null,
            "require_api_key": null,
            "python_future_field": {"kept": true}
        }))
        .unwrap(),
    )
    .unwrap();

    let loader = ProviderConfigLoader::new(dir.path());

    let provider = loader.load_provider("nullable").await.unwrap();

    assert_eq!(provider.id, "nullable");
    assert_eq!(provider.name, "");
    assert_eq!(provider.base_url, "");
    assert_eq!(provider.api_key, "");
    assert!(provider.api_keys.is_empty());
    assert!(provider.headers.is_empty());
    assert!(provider.models.is_empty());
    assert!(provider.extra_models.is_empty());
    assert!(provider.generate_kwargs.is_empty());
    assert!(!provider.support_model_discovery);
    assert!(!provider.support_connection_check);
    assert!(!provider.is_local);
    assert_eq!(provider.api_key_prefix, "");
    assert!(!provider.is_custom);
    assert!(!provider.is_plugin);
    assert!(!provider.freeze_url);
    assert!(!provider.require_api_key);
    assert_eq!(
        provider.extra.get("python_future_field"),
        Some(&serde_json::json!({"kept": true}))
    );
}

#[tokio::test]
async fn load_provider_accepts_model_dicts_and_preserves_model_unknown_fields() {
    let dir = tempfile::tempdir().unwrap();
    let providers_dir = dir.path().join("providers/providers");
    std::fs::create_dir_all(&providers_dir).unwrap();
    std::fs::write(
        providers_dir.join("model-dicts.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "id": "model-dicts",
            "chat_model": "OpenAIChatModel",
            "models": [
                {
                    "id": "base-model",
                    "name": null,
                    "context_window": 131072,
                    "thinking_enabled": true,
                    "thinking_param_style": "effort",
                    "reasoning_effort": "high",
                    "reasoning_effort_options": ["low", "medium", "high"],
                    "thinking_budget_range": [64, 4096],
                    "future_model_field": {"kept": true}
                }
            ],
            "extra_models": [
                {
                    "id": "extra-model",
                    "name": "Extra Model",
                    "supports_image": true,
                    "generate_kwargs": {"temperature": 0.2}
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    let loader = ProviderConfigLoader::new(dir.path());

    let provider = loader.load_provider("model-dicts").await.unwrap();
    assert_eq!(provider.models[0].name, "");
    assert_eq!(
        provider.models[0].extra.get("future_model_field"),
        Some(&serde_json::json!({"kept": true}))
    );
    assert_eq!(provider.models[0].thinking_budget_range, Some([64, 4096]));

    let projected = provider.to_model_infos();
    assert_eq!(
        projected
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        ["base-model", "extra-model"]
    );
    assert_eq!(projected[0].context_window, Some(131_072));
    assert!(projected[0]
        .generate_kwargs
        .get("reasoning_effort")
        .is_none());
    assert_eq!(projected[1].supports_image, Some(true));
    assert_eq!(projected[1].generate_kwargs["temperature"], 0.2);
}

#[tokio::test]
async fn default_model_accepts_python_model_id_alias_and_preserves_unknown_fields() {
    let dir = tempfile::tempdir().unwrap();
    let providers_dir = dir.path().join("providers");
    std::fs::create_dir_all(&providers_dir).unwrap();
    std::fs::write(
        providers_dir.join("default_model.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "provider_id": "dashscope",
            "model_id": "qwen3-max",
            "source": "providers.default_llm",
            "python_future_field": ["kept"]
        }))
        .unwrap(),
    )
    .unwrap();

    let loader = ProviderConfigLoader::new(dir.path());

    let active = loader.load_default_model().await.unwrap();

    assert_eq!(active.provider_id, "dashscope");
    assert_eq!(active.model, "qwen3-max");
    assert_eq!(active.source.as_deref(), Some("providers.default_llm"));
    assert_eq!(
        active.extra.get("python_future_field"),
        Some(&serde_json::json!(["kept"]))
    );
}

#[tokio::test]
async fn load_provider_accepts_camel_case_thinking_fields_without_projection() {
    let dir = tempfile::tempdir().unwrap();
    let providers_dir = dir.path().join("providers/providers");
    std::fs::create_dir_all(&providers_dir).unwrap();
    std::fs::write(
        providers_dir.join("gemini-thinking.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "id": "gemini-thinking",
            "chat_model": "GeminiChatModel",
            "models": [
                {
                    "id": "gemini-2.5-pro",
                    "thinkingEnabled": true,
                    "thinkingBudget": 2048
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    let loader = ProviderConfigLoader::new(dir.path());
    let provider = loader.load_provider("gemini-thinking").await.unwrap();
    let projected = provider.to_model_infos();

    assert_eq!(provider.models[0].thinking_enabled, Some(true));
    assert!(projected[0].generate_kwargs.get("thinkingConfig").is_none());
}

#[tokio::test]
async fn dashscope_effort_thinking_fields_do_not_project_to_extra_body() {
    let dir = tempfile::tempdir().unwrap();
    let providers_dir = dir.path().join("providers/providers");
    std::fs::create_dir_all(&providers_dir).unwrap();
    std::fs::write(
        providers_dir.join("dashscope-effort.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "id": "dashscope-effort",
            "chat_model": "DashScopeChatModel",
            "thinking_param_style": "budget",
            "models": [
                {
                    "id": "glm-5.2",
                    "thinking_enabled": true,
                    "thinking_param_style": "effort",
                    "reasoning_effort": "high"
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    let loader = ProviderConfigLoader::new(dir.path());
    let provider = loader.load_provider("dashscope-effort").await.unwrap();
    let projected = provider.to_model_infos();

    assert!(projected[0].generate_kwargs.get("extra_body").is_none());
    assert!(projected[0]
        .generate_kwargs
        .get("enable_thinking")
        .is_none());
}
