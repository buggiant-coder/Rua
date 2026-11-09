use std::path::PathBuf;
use std::sync::Arc;

use cell_store::FileSystemCellStore;
use rua_provider::config_loader::{ProviderConfig, ProviderModelEntry};
use rua_provider::manager::ProviderManager;
use rua_provider::ProviderCells;
use tempfile::TempDir;

fn make_provider_config(id: &str) -> ProviderConfig {
    ProviderConfig {
        id: id.to_string(),
        name: format!("Test {id}"),
        base_url: "http://localhost".into(),
        api_key: format!("sk-{id}"),
        api_keys: Vec::new(),
        api_key_prefixes: Vec::new(),
        headers: serde_json::Map::new(),
        custom_headers: serde_json::Map::new(),
        chat_model: Some("OpenAIChatModel".into()),
        models: vec![ProviderModelEntry {
            id: "model-a".into(),
            name: String::new(),
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
        }],
        extra_models: Vec::new(),
        timeout: None,
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

struct Harness {
    _tmp: TempDir,
    base_dir: PathBuf,
    manager: Arc<ProviderManager>,
    cells: ProviderCells,
}

fn make_harness() -> Harness {
    let tmp = TempDir::new().expect("tempdir");
    let base_dir = tmp.path().join("providers/providers");
    let store = Arc::new(FileSystemCellStore::new());
    let manager = Arc::new(ProviderManager::new());
    let cells = ProviderCells::new(Arc::clone(&store), Arc::clone(&manager), base_dir.clone());
    Harness {
        _tmp: tmp,
        base_dir,
        manager,
        cells,
    }
}

fn make_encrypted_harness() -> Harness {
    let tmp = TempDir::new().expect("tempdir");
    let base_dir = tmp.path().join("providers/providers");
    let store = Arc::new(FileSystemCellStore::new());
    let manager = Arc::new(ProviderManager::new());
    let cells = ProviderCells::with_key(
        Arc::clone(&store),
        Arc::clone(&manager),
        base_dir.clone(),
        [7_u8; 32],
    );
    Harness {
        _tmp: tmp,
        base_dir,
        manager,
        cells,
    }
}

#[tokio::test]
async fn cell_read_returns_none_for_missing_provider() {
    let h = make_harness();
    let cell = h.cells.cell("missing");
    let result = cell.read().await.expect("read");
    assert!(result.is_none(), "missing provider must read None");
}

#[tokio::test]
async fn cell_write_persists_to_correct_path() {
    let h = make_harness();
    let config = make_provider_config("openai");
    h.cells
        .cell("openai")
        .write(config.clone())
        .await
        .expect("write");

    let expected_path = h.base_dir.join("openai.json");
    assert!(
        expected_path.exists(),
        "expected file at {}, but it does not exist",
        expected_path.display()
    );

    let on_disk: ProviderConfig =
        serde_json::from_str(&std::fs::read_to_string(&expected_path).expect("read file"))
            .expect("parse json");
    assert_eq!(on_disk.id, "openai");
    assert_eq!(on_disk.api_key, "sk-openai");
}

#[tokio::test]
async fn cell_write_registers_runtime_instance() {
    let h = make_harness();
    assert!(
        h.manager.get("openai").is_none(),
        "no instance should be registered before write"
    );
    let config = make_provider_config("openai");
    h.cells.cell("openai").write(config).await.expect("write");
    assert!(
        h.manager.get("openai").is_some(),
        "after write, ProviderManager.get must return the freshly registered instance"
    );
}

#[tokio::test]
async fn cell_delete_removes_file_and_unregisters() {
    let h = make_harness();
    h.cells
        .cell("openai")
        .write(make_provider_config("openai"))
        .await
        .expect("seed write");
    let path = h.base_dir.join("openai.json");
    assert!(path.exists(), "precondition: file must exist after write");
    assert!(
        h.manager.get("openai").is_some(),
        "precondition: instance registered"
    );

    h.cells.cell("openai").delete().await.expect("delete");
    assert!(
        !path.exists(),
        "expected {} to be removed by delete",
        path.display()
    );
    assert!(
        h.manager.get("openai").is_none(),
        "delete must unregister the runtime instance"
    );
}

#[tokio::test]
async fn cell_delete_unregisters_when_file_was_already_removed() {
    let h = make_harness();
    h.cells
        .cell("custom-to-remove")
        .write(make_provider_config("custom-to-remove"))
        .await
        .expect("seed write");
    let path = h.base_dir.join("custom-to-remove.json");
    assert!(path.exists(), "precondition: provider file must exist");
    assert!(
        h.manager.get("custom-to-remove").is_some(),
        "precondition: runtime instance registered"
    );

    std::fs::remove_file(&path).expect("external file removal");
    h.cells
        .cell("custom-to-remove")
        .delete()
        .await
        .expect("delete should tolerate missing file");

    assert!(!path.exists(), "provider file should remain absent");
    assert!(
        h.manager.get("custom-to-remove").is_none(),
        "missing-file delete must still unregister the runtime instance"
    );
}

#[tokio::test]
async fn cell_update_atomic_mutator() {
    let h = make_harness();
    h.cells
        .cell("openai")
        .write(make_provider_config("openai"))
        .await
        .expect("seed write");

    let new_config = h
        .cells
        .cell("openai")
        .update(|maybe_existing| {
            let mut cfg = maybe_existing.expect("seed config must be present");
            cfg.models.push(ProviderModelEntry {
                id: "gpt-4o".into(),
                name: "GPT-4o".into(),
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
            });
            cfg
        })
        .await
        .expect("update");

    let model_ids: Vec<_> = new_config.models.iter().map(|m| m.id.clone()).collect();
    assert!(model_ids.contains(&"model-a".to_string()));
    assert!(model_ids.contains(&"gpt-4o".to_string()));

    let read_back = h
        .cells
        .cell("openai")
        .read()
        .await
        .expect("read back")
        .expect("must exist on disk");
    let read_ids: Vec<_> = read_back.models.iter().map(|m| m.id.clone()).collect();
    assert!(
        read_ids.contains(&"gpt-4o".to_string()),
        "mutator's added model must be persisted on disk"
    );
}

#[tokio::test]
async fn list_returns_all_providers_in_base_dir() {
    let h = make_harness();
    for id in ["openai", "anthropic", "ollama"] {
        h.cells
            .cell(id)
            .write(make_provider_config(id))
            .await
            .expect("seed");
    }
    let mut ids: Vec<String> = h
        .cells
        .list()
        .await
        .expect("list")
        .into_iter()
        .map(|c| c.id)
        .collect();
    ids.sort();
    assert_eq!(ids, vec!["anthropic", "ollama", "openai"]);
}

#[tokio::test]
async fn list_returns_empty_when_base_dir_missing() {
    let h = make_harness();
    assert!(
        !h.base_dir.exists(),
        "precondition: base_dir must not exist yet"
    );
    let configs = h.cells.list().await.expect("list");
    assert!(
        configs.is_empty(),
        "list must return empty Vec for missing dir"
    );
}

#[tokio::test]
async fn list_skips_non_json_files() {
    let h = make_harness();
    h.cells
        .cell("openai")
        .write(make_provider_config("openai"))
        .await
        .expect("seed json");

    std::fs::write(h.base_dir.join("README.txt"), "not json").expect("write txt");
    let ids: Vec<String> = h
        .cells
        .list()
        .await
        .expect("list")
        .into_iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(ids, vec!["openai".to_string()]);
}

#[tokio::test]
async fn encrypted_cell_write_does_not_store_plaintext_secret() {
    let h = make_encrypted_harness();
    let config = make_provider_config("dashscope");
    h.cells
        .cell("dashscope")
        .write(config.clone())
        .await
        .expect("write encrypted");

    let path = h.base_dir.join("dashscope.json");
    let bytes = std::fs::read(&path).expect("read encrypted file");
    let on_disk: serde_json::Value = serde_json::from_slice(&bytes).expect("provider json");
    assert_eq!(on_disk["id"], "dashscope");
    assert_eq!(on_disk["name"], "Test dashscope");
    assert_eq!(on_disk["base_url"], "http://localhost");
    assert_eq!(on_disk["api_key"]["rua_encrypted"], true);
    assert_eq!(on_disk["api_key"]["field"], "api_key");
    assert!(on_disk["api_key"]["notice"]
        .as_str()
        .expect("notice")
        .contains("Other JSON fields may be edited normally"));
    assert!(
        !String::from_utf8_lossy(&bytes).contains("sk-dashscope"),
        "provider file must not contain the raw api key"
    );
    assert!(
        serde_json::from_slice::<ProviderConfig>(&bytes).is_err(),
        "provider file with encrypted api_key must not parse without decryption"
    );

    let read_back = h
        .cells
        .cell("dashscope")
        .read()
        .await
        .expect("read encrypted")
        .expect("provider exists");
    assert_eq!(read_back.id, config.id);
    assert_eq!(read_back.api_key, config.api_key);
}

#[tokio::test]
async fn encrypted_cell_read_migrates_legacy_plaintext_file() {
    let h = make_encrypted_harness();
    std::fs::create_dir_all(&h.base_dir).expect("create providers dir");
    let path = h.base_dir.join("dashscope.json");
    let mut config = make_provider_config("dashscope");
    config.api_key = "legacy-secret-key".into();
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&config).expect("serialize plaintext"),
    )
    .expect("seed plaintext provider");

    let read_plaintext = h
        .cells
        .cell("dashscope")
        .read()
        .await
        .expect("read legacy plaintext")
        .expect("provider exists");
    assert_eq!(read_plaintext.api_key, "legacy-secret-key");

    let bytes = std::fs::read(&path).expect("read migrated file");
    let on_disk: serde_json::Value = serde_json::from_slice(&bytes).expect("provider json");
    assert_eq!(on_disk["id"], "dashscope");
    assert_eq!(on_disk["name"], "Test dashscope");
    assert_eq!(on_disk["api_key"]["rua_encrypted"], true);
    assert!(
        !String::from_utf8_lossy(&bytes).contains("legacy-secret-key"),
        "migrated provider file must not contain the old plaintext key"
    );
}

#[tokio::test]
async fn encrypted_cell_read_does_not_rewrite_provider_without_secrets() {
    let h = make_encrypted_harness();
    std::fs::create_dir_all(&h.base_dir).expect("create providers dir");
    let path = h.base_dir.join("kilo.json");
    let mut config = make_provider_config("kilo");
    config.api_key.clear();
    let original = serde_json::to_vec(&config).expect("serialize provider");
    std::fs::write(&path, &original).expect("seed provider");

    let read_back = h
        .cells
        .cell("kilo")
        .read()
        .await
        .expect("read provider")
        .expect("provider exists");
    assert!(read_back.api_key.is_empty());

    let after_read = std::fs::read(&path).expect("read provider after cell read");
    assert_eq!(
        after_read, original,
        "reading a provider without secrets must not rewrite its file"
    );
}
