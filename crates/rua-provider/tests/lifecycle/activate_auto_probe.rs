use std::sync::Arc;

use async_trait::async_trait;

use agent_types::{ChatRequest, ChatResponse, ContentPart, FinishReason, ModelDialogueItem, Role};
use llm_multiplex::{
    ChatModel, ConnectionStatus, EventStream, LlmError, ModelInfo, Provider, ProviderInfo,
};
use rua_provider::config_loader::{
    DefaultModel, ProviderConfig, ProviderConfigLoader, ProviderModelEntry,
};
use rua_provider::manager::ProviderManager;

fn make_model_entry(id: &str, supports_multimodal: Option<bool>) -> ProviderModelEntry {
    ProviderModelEntry {
        id: id.into(),
        name: String::new(),
        supports_multimodal,
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

fn make_provider_config(provider_id: &str, models: Vec<ProviderModelEntry>) -> ProviderConfig {
    ProviderConfig {
        id: provider_id.into(),
        name: "Test".into(),
        base_url: "http://127.0.0.1:1".into(),
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

async fn setup_manager(
    tmp: &tempfile::TempDir,
    provider_id: &str,
    models: Vec<ProviderModelEntry>,
) -> ProviderManager {
    let secret_dir = tmp.path();
    let providers_dir = secret_dir.join("providers").join("providers");
    tokio::fs::create_dir_all(&providers_dir).await.unwrap();

    let config = make_provider_config(provider_id, models);
    let loader = ProviderConfigLoader::new(secret_dir);
    loader.save_provider(&config).await.unwrap();

    let provider = rua_provider::manager::create_provider_from_config(&config).unwrap();
    let mgr = ProviderManager::with_config_loader(loader);
    mgr.register(provider);
    mgr
}

async fn load_model_field(
    tmp: &tempfile::TempDir,
    provider_id: &str,
    model_id: &str,
) -> Option<bool> {
    let loader = ProviderConfigLoader::new(tmp.path());
    let config = loader.load_provider(provider_id).await.unwrap();
    config
        .all_models()
        .into_iter()
        .find(|m| m.id == model_id)
        .and_then(|m| m.supports_multimodal)
}

#[tokio::test]
async fn known_model_skips_probe() {
    let tmp = tempfile::tempdir().unwrap();
    let mgr = setup_manager(
        &tmp,
        "test-prov",
        vec![make_model_entry("my-model", Some(true))],
    )
    .await;

    let active = DefaultModel {
        provider_id: "test-prov".into(),
        model: "my-model".into(),
        source: None,
        extra: Default::default(),
    };
    mgr.set_default_model(active).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let val = load_model_field(&tmp, "test-prov", "my-model").await;
    assert_eq!(val, Some(true), "already-known model should stay unchanged");
}

#[tokio::test]
async fn enriched_wellknown_model_skips_probe() {
    let tmp = tempfile::tempdir().unwrap();
    let mgr = setup_manager(
        &tmp,
        "google",
        vec![make_model_entry("gemini-2.0-flash", None)],
    )
    .await;

    let active = DefaultModel {
        provider_id: "google".into(),
        model: "gemini-2.0-flash".into(),
        source: None,
        extra: Default::default(),
    };
    mgr.set_default_model(active).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let val = load_model_field(&tmp, "google", "gemini-2.0-flash").await;
    assert_eq!(
        val, None,
        "disk should be unchanged since enrichment recognized the model"
    );
}

#[tokio::test]
async fn activate_succeeds_even_when_probe_would_fail() {
    let tmp = tempfile::tempdir().unwrap();
    let mgr = setup_manager(
        &tmp,
        "custom",
        vec![make_model_entry("totally-unknown-xyz", None)],
    )
    .await;

    let active = DefaultModel {
        provider_id: "custom".into(),
        model: "totally-unknown-xyz".into(),
        source: None,
        extra: Default::default(),
    };
    let result = mgr.set_default_model(active).await;
    assert!(result.is_ok(), "activate must succeed regardless of probe");
}

struct MockChatModel {
    info: ModelInfo,
}

#[async_trait]
impl ChatModel for MockChatModel {
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        Ok(ChatResponse {
            message: ModelDialogueItem {
                id: None,
                role: Role::Assistant,
                content: vec![ContentPart::Text {
                    text: "red blue".into(),
                }],
                name: None,
                created_at: None,
                metadata: Default::default(),
            },
            usage: None,
            model: None,
            finish_reason: FinishReason::Stop,
            metadata: Default::default(),
            extra: Default::default(),
        })
    }

    fn chat_stream(&self, _request: ChatRequest) -> EventStream {
        unimplemented!("not used by probe")
    }

    fn model_info(&self) -> &ModelInfo {
        &self.info
    }
}

struct MockProvider {
    info: ProviderInfo,
    model_id: String,
}

#[async_trait]
impl Provider for MockProvider {
    fn info(&self) -> &ProviderInfo {
        &self.info
    }

    fn supports_model(&self, model: &str) -> bool {
        model == self.model_id
    }

    fn create_chat_model(&self, model: &str) -> Result<Box<dyn ChatModel>, LlmError> {
        Ok(Box::new(MockChatModel {
            info: ModelInfo {
                id: model.to_string(),
                name: model.to_string(),
                context_window: None,
                max_output_tokens: None,
                supports_multimodal: None,
                supports_image: None,
                supports_video: None,
                probe_source: None,
                generate_kwargs: Default::default(),
            },
        }))
    }

    async fn fetch_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        Ok(vec![])
    }

    async fn check_connection(&self) -> Result<ConnectionStatus, LlmError> {
        Ok(ConnectionStatus::Connected)
    }
}

async fn setup_manager_with_mock_provider(
    tmp: &tempfile::TempDir,
    provider_id: &str,
    model_id: &str,
) -> ProviderManager {
    let secret_dir = tmp.path();
    let providers_dir = secret_dir.join("providers").join("providers");
    tokio::fs::create_dir_all(&providers_dir).await.unwrap();

    let config = make_provider_config(provider_id, vec![make_model_entry(model_id, None)]);
    let loader = ProviderConfigLoader::new(secret_dir);
    loader.save_provider(&config).await.unwrap();

    let mock_provider: Arc<dyn Provider> = Arc::new(MockProvider {
        info: ProviderInfo {
            id: provider_id.into(),
            name: "Mock".into(),
            provider_type: None,
            is_local: false,
            support_model_discovery: false,
            support_connection_check: false,
        },
        model_id: model_id.into(),
    });

    let mgr = ProviderManager::with_config_loader(loader);
    mgr.register(mock_provider);
    mgr
}

#[tokio::test]
async fn successful_probe_does_not_persist_to_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let mgr = setup_manager_with_mock_provider(&tmp, "mock-prov", "mock-model").await;

    let active = DefaultModel {
        provider_id: "mock-prov".into(),
        model: "mock-model".into(),
        source: None,
        extra: Default::default(),
    };

    assert_eq!(
        load_model_field(&tmp, "mock-prov", "mock-model").await,
        None,
        "precondition: supports_multimodal starts as None"
    );

    mgr.set_default_model(active).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    let val = load_model_field(&tmp, "mock-prov", "mock-model").await;
    assert_eq!(
        val, None,
        "auto-probe must not persist to disk (emit-only architecture)"
    );
}

#[tokio::test]
async fn manual_probe_does_not_persist_or_rebuild_provider() {
    let tmp = tempfile::tempdir().unwrap();
    let mgr = setup_manager_with_mock_provider(&tmp, "mock-prov", "mock-model").await;

    let result = mgr
        .probe_model_multimodal("mock-prov", "mock-model")
        .await
        .unwrap();

    assert!(result.supports_image);
    assert_eq!(
        load_model_field(&tmp, "mock-prov", "mock-model").await,
        None,
        "the endpoint-owned cell update is the only authority that may persist probe results"
    );
    assert_eq!(
        mgr.get("mock-prov").unwrap().info().name,
        "Mock",
        "manual probing must not replace the registered provider from legacy disk config"
    );
}

#[tokio::test]
async fn probe_failure_keeps_field_none() {
    let tmp = tempfile::tempdir().unwrap();
    let mgr = setup_manager(
        &tmp,
        "custom",
        vec![make_model_entry("totally-unknown-xyz", None)],
    )
    .await;

    let active = DefaultModel {
        provider_id: "custom".into(),
        model: "totally-unknown-xyz".into(),
        source: None,
        extra: Default::default(),
    };
    mgr.set_default_model(active).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let val = load_model_field(&tmp, "custom", "totally-unknown-xyz").await;
    assert_eq!(
        val, None,
        "probe failure should keep supports_multimodal as None for retry"
    );
}
