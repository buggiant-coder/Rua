use rua_provider::{migrate_legacy_providers, DefaultModel, ProviderConfig};

#[test]
fn no_legacy_file_is_noop() {
    let dir = tempfile::tempdir().unwrap();

    migrate_legacy_providers(dir.path()).unwrap();

    assert!(!dir.path().join("providers").exists());
}

#[test]
fn migrates_python_legacy_provider_file_to_rust_cells_layout() {
    let dir = tempfile::tempdir().unwrap();
    let legacy = serde_json::json!({
        "providers": {
            "dashscope": {
                "name": "DashScope",
                "base_url": "https://dashscope.aliyuncs.com/compatible-mode/v1",
                "api_key": "sk-test-legacy-secret",
                "extra_models": [
                    {"id": "qwen3-max", "name": "Qwen3 Max"}
                ],
                "chat_model": "OpenAIChatModel"
            },
            "ollama": {
                "name": "Ollama",
                "base_url": "http://myhost:11434",
                "api_key": "",
                "extra_models": [
                    {"id": "llama3.2", "name": "Llama 3.2"}
                ],
                "chat_model": "OllamaChatModel"
            }
        },
        "custom_providers": {
            "mydash": {
                "name": "My DashScope",
                "base_url": "https://dashscope.aliyuncs.com/compatible-mode/v1",
                "api_key": "sk-test-legacy-custom-secret",
                "models": [
                    {"id": "qwen3-max", "name": "Qwen3 Max"}
                ],
                "chat_model": "OpenAIChatModel"
            }
        },
        "default_llm": {
            "provider_id": "dashscope",
            "model": "qwen3-max"
        }
    });
    let legacy_path = dir.path().join("providers.json");
    std::fs::write(&legacy_path, serde_json::to_string_pretty(&legacy).unwrap()).unwrap();

    migrate_legacy_providers(dir.path()).unwrap();

    assert!(!legacy_path.exists());
    assert_eq!(backup_count(dir.path()), 1);

    let providers_dir = dir.path().join("providers").join("providers");
    let dashscope = read_provider(&providers_dir.join("dashscope.json"));
    assert_eq!(dashscope.id, "dashscope");
    assert_eq!(dashscope.name, "DashScope");
    assert_eq!(dashscope.api_key, "sk-test-legacy-secret");
    assert_eq!(dashscope.chat_model.as_deref(), Some("OpenAIChatModel"));
    assert!(!dashscope.is_custom);
    assert_eq!(dashscope.extra_models.len(), 1);
    assert_eq!(dashscope.extra_models[0].id, "qwen3-max");

    let mydash = read_provider(&providers_dir.join("mydash.json"));
    assert_eq!(mydash.id, "mydash");
    assert_eq!(mydash.name, "My DashScope");
    assert_eq!(mydash.api_key, "sk-test-legacy-custom-secret");
    assert!(mydash.is_custom);
    assert_eq!(mydash.chat_model.as_deref(), Some("OpenAIChatModel"));
    assert_eq!(mydash.models.len(), 0);
    assert_eq!(mydash.extra_models.len(), 1);
    assert_eq!(mydash.extra_models[0].id, "qwen3-max");

    let ollama = read_provider(&providers_dir.join("ollama.json"));
    assert_eq!(ollama.id, "ollama");
    assert_eq!(ollama.base_url, "http://myhost:11434");
    assert!(!ollama.is_custom);

    let active_path = dir.path().join("providers").join("default_model.json");
    let active: DefaultModel =
        serde_json::from_str(&std::fs::read_to_string(active_path).unwrap()).unwrap();
    assert_eq!(active.provider_id, "dashscope");
    assert_eq!(active.model, "qwen3-max");
}

#[test]
fn restores_backup_on_bad_json() {
    let dir = tempfile::tempdir().unwrap();
    let legacy_path = dir.path().join("providers.json");
    std::fs::write(&legacy_path, "not valid json").unwrap();

    let result = migrate_legacy_providers(dir.path());

    assert!(result.is_err());
    assert!(legacy_path.is_file());
    assert_eq!(
        std::fs::read_to_string(&legacy_path).unwrap(),
        "not valid json"
    );
    assert_eq!(backup_count(dir.path()), 1);
}

fn read_provider(path: &std::path::Path) -> ProviderConfig {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn backup_count(dir: &std::path::Path) -> usize {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("providers.json.bak."))
        })
        .count()
}
