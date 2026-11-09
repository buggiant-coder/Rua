use std::sync::Arc;

use cell_store::FileSystemCellStore;
use rua_provider::manager::ProviderManager;
use rua_provider::{ProviderCells, ProviderConfig, ProviderModelEntry};
use rua_server::local_model_resume::{sync_local_provider_url, LOCAL_PROVIDER_ID};

#[tokio::test]
async fn sync_local_provider_url_updates_disk_and_runtime_without_losing_model_metadata() {
    let harness = Harness::new();
    let mut config = local_provider_config();
    config.base_url = "http://127.0.0.1:9000/v1".into();
    config.extra_models = vec![ProviderModelEntry {
        id: "AgentScope/QwenPaw-Flash-2B-Q4_K_M".into(),
        name: "QwenPaw Flash 2B".into(),
        supports_multimodal: Some(true),
        supports_image: Some(true),
        supports_video: Some(true),
        probe_source: Some("documentation".into()),
        context_window: Some(32_768),
        max_output_tokens: Some(4096),
        generate_kwargs: serde_json::Map::new(),
        thinking_enabled: None,
        thinking_budget: None,
        reasoning_effort: None,
        thinking_param_style: None,
        reasoning_effort_options: Vec::new(),
        thinking_budget_range: None,
        relay_reasoning: None,
        extra: serde_json::Map::new(),
    }];
    harness
        .cells
        .cell(LOCAL_PROVIDER_ID)
        .write(config)
        .await
        .expect("seed local provider");

    sync_local_provider_url(
        Arc::clone(&harness.manager),
        Arc::clone(&harness.cells),
        43111,
    )
    .await;

    let updated = harness
        .cells
        .cell(LOCAL_PROVIDER_ID)
        .read()
        .await
        .expect("read local provider")
        .expect("provider exists");
    assert_eq!(updated.base_url, "http://127.0.0.1:43111/v1");
    assert_eq!(updated.extra_models.len(), 1);
    assert_eq!(
        updated.extra_models[0].id,
        "AgentScope/QwenPaw-Flash-2B-Q4_K_M"
    );
    assert_eq!(updated.extra_models[0].supports_multimodal, Some(true));
    assert_eq!(updated.extra_models[0].supports_image, Some(true));
    assert_eq!(updated.extra_models[0].supports_video, Some(true));
    assert_eq!(
        updated.extra_models[0].probe_source.as_deref(),
        Some("documentation")
    );

    let runtime_provider = harness
        .manager
        .get(LOCAL_PROVIDER_ID)
        .expect("runtime provider re-registered");
    assert!(runtime_provider.supports_model("AgentScope/QwenPaw-Flash-2B-Q4_K_M"));
}

#[tokio::test]
async fn sync_local_provider_url_is_noop_when_local_provider_is_absent() {
    let harness = Harness::new();

    sync_local_provider_url(
        Arc::clone(&harness.manager),
        Arc::clone(&harness.cells),
        43111,
    )
    .await;

    let missing = harness
        .cells
        .cell(LOCAL_PROVIDER_ID)
        .read()
        .await
        .expect("read missing provider");
    assert!(missing.is_none());
    assert!(harness.manager.get(LOCAL_PROVIDER_ID).is_none());
}

struct Harness {
    _tmp: tempfile::TempDir,
    manager: Arc<ProviderManager>,
    cells: Arc<ProviderCells>,
}

impl Harness {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base_dir = tmp.path().join("providers/providers");
        let store = Arc::new(FileSystemCellStore::new());
        let manager = Arc::new(ProviderManager::new());
        let cells = Arc::new(ProviderCells::new(store, Arc::clone(&manager), base_dir));
        Self {
            _tmp: tmp,
            manager,
            cells,
        }
    }
}

fn local_provider_config() -> ProviderConfig {
    ProviderConfig {
        id: LOCAL_PROVIDER_ID.into(),
        name: "Rua Local".into(),
        base_url: String::new(),
        api_key: "local-key".into(),
        api_keys: Vec::new(),
        api_key_prefixes: Vec::new(),
        headers: serde_json::Map::new(),
        custom_headers: serde_json::Map::new(),
        chat_model: Some("OpenAIChatModel".into()),
        models: Vec::new(),
        extra_models: Vec::new(),
        timeout: None,
        generate_kwargs: serde_json::Map::new(),
        support_model_discovery: false,
        support_connection_check: false,
        is_local: true,
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
