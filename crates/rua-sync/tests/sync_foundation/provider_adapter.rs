use std::sync::Arc;

use rua_provider::manager::ProviderManager;
use rua_sync::hlc::HlcTimestamp;
use rua_sync::merge_store::MergeStore;
use rua_sync::provider_adapter::{
    ProviderSecretSyncAdapter, PROVIDER_SECRET_DOMAIN, PROVIDER_SECRET_KIND_DELETE,
    PROVIDER_SECRET_KIND_SNAPSHOT, PROVIDER_SECRET_KIND_UPSERT,
};
use rua_sync::sidecar::RecordState;
use rua_sync::sync_registry::{DomainSyncAdapter, RegistryApplyStatus, SyncRegistry};

fn hlc(wall_ms: u64) -> HlcTimestamp {
    HlcTimestamp {
        wall_ms,
        counter: 0,
        device_id: [0; 8],
    }
}

fn provider_path(secret_dir: &std::path::Path, provider_id: &str) -> std::path::PathBuf {
    secret_dir
        .join("providers")
        .join("providers")
        .join(format!("{provider_id}.json"))
}

fn provider_config(id: &str, name: &str, api_key: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "name": name,
        "base_url": "https://example.test/v1",
        "api_key": api_key,
        "api_keys": [],
        "headers": {},
        "models": [{"id": "chat", "name": "Chat"}],
        "extra_models": [],
        "generate_kwargs": {},
        "support_model_discovery": false,
        "support_connection_check": true,
        "is_local": false,
        "api_key_prefix": "",
        "is_custom": true,
        "is_plugin": false,
        "freeze_url": false,
        "require_api_key": true
    })
}

fn provider_config_with_parity_fields(id: &str, name: &str, api_key: &str) -> serde_json::Value {
    let mut value = provider_config(id, name, api_key);
    let obj = value.as_object_mut().unwrap();
    obj.insert(
        "api_key_prefixes".into(),
        serde_json::json!(["ghp_", "github_pat_"]),
    );
    obj.insert(
        "custom_headers".into(),
        serde_json::json!({
            "Authorization": "Bearer synced-header-secret",
            "X-Provider-Trace": "visible-trace"
        }),
    );
    obj.insert("provider_group".into(), serde_json::json!("github"));
    obj.insert("provider_group_name".into(), serde_json::json!("GitHub"));
    obj.insert("provider_variant".into(), serde_json::json!("models"));
    obj.insert(
        "base_url_options".into(),
        serde_json::json!([
            {"label": "GitHub Models", "value": "https://models.github.ai/inference"},
            {"label": "GitHub Models Preview", "value": "https://models.github.ai/preview"}
        ]),
    );
    obj.insert("thinking_param_style".into(), serde_json::json!("effort"));
    obj.insert(
        "reasoning_effort_options".into(),
        serde_json::json!(["low", "medium", "high"]),
    );
    obj.insert(
        "thinking_budget_range".into(),
        serde_json::json!([1, 81920]),
    );
    value
}

async fn registry_with_adapter(
    secret_dir: &std::path::Path,
    manager: Option<Arc<ProviderManager>>,
) -> (SyncRegistry, Arc<ProviderSecretSyncAdapter>) {
    registry_with_adapter_and_key(secret_dir, manager, None).await
}

async fn registry_with_adapter_and_key(
    secret_dir: &std::path::Path,
    manager: Option<Arc<ProviderManager>>,
    key: Option<[u8; 32]>,
) -> (SyncRegistry, Arc<ProviderSecretSyncAdapter>) {
    let adapter = ProviderSecretSyncAdapter::new(secret_dir.to_path_buf(), "device-b");
    let adapter = if let Some(key) = key {
        adapter.with_local_secret_key(key)
    } else {
        adapter
    };
    let adapter = if let Some(manager) = manager {
        adapter.with_provider_manager(manager)
    } else {
        adapter
    };
    let adapter = Arc::new(adapter);
    let mut registry = SyncRegistry::new();
    registry.register(adapter.clone()).unwrap();
    (registry, adapter)
}

#[tokio::test]
async fn inbound_provider_config_uses_lww_and_preserves_device_local_fields() {
    let secret = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let (registry, _adapter) = registry_with_adapter(secret.path(), None).await;

    let path = provider_path(secret.path(), "openai");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let local = serde_json::json!({
        "id": "openai",
        "name": "Local Name",
        "base_url": "https://local.test/v1",
        "api_key": "local-secret",
        "workspace_dir": "/local/device/path"
    });
    std::fs::write(&path, serde_json::to_vec_pretty(&local).unwrap()).unwrap();
    let change = ProviderSecretSyncAdapter::upsert_change(
        "op-provider-merge",
        "openai",
        serde_json::json!({
            "id": "openai",
            "name": "Remote Name",
            "base_url": "https://base.test/v1",
            "api_key": "base-secret",
            "workspace_dir": "/remote/path"
        }),
        "device-a",
        hlc(120),
    );

    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    let merged: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(merged["name"], "Remote Name");
    assert_eq!(merged["base_url"], "https://base.test/v1");
    assert_eq!(merged["api_key"], "base-secret");
    assert_eq!(merged["workspace_dir"], "/local/device/path");
    let cursor = merge_store
        .get_record_cursor("provider_secret:openai")
        .unwrap()
        .expect("provider cursor");
    assert_eq!(cursor.base_blob_hash, None);
}

#[tokio::test]
async fn inbound_provider_config_same_field_uses_lww() {
    let secret = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let (registry, _adapter) = registry_with_adapter(secret.path(), None).await;

    let path = provider_path(secret.path(), "openai");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&provider_config("openai", "Local Name", "local-secret"))
            .unwrap(),
    )
    .unwrap();
    let change = ProviderSecretSyncAdapter::upsert_change(
        "op-provider-lww",
        "openai",
        provider_config("openai", "Remote Name", "remote-secret"),
        "device-a",
        hlc(120),
    );

    registry.apply_change(&change, &merge_store).await.unwrap();

    let merged: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(merged["name"], "Remote Name");
    assert_eq!(merged["api_key"], "remote-secret");
}

#[tokio::test]
async fn inbound_provider_secret_roundtrips_and_reloads_live_provider() {
    let secret = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let manager = Arc::new(ProviderManager::new());
    let (registry, _adapter) = registry_with_adapter(secret.path(), Some(manager.clone())).await;

    let change = ProviderSecretSyncAdapter::upsert_change(
        "op-provider-secret",
        "custom",
        provider_config("custom", "Custom", "sk-secret"),
        "device-a",
        hlc(100),
    );

    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    let written: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(provider_path(secret.path(), "custom")).unwrap(),
    )
    .unwrap();
    assert_eq!(written["api_key"], "sk-secret");
    assert!(manager.get("custom").is_some());
}

#[tokio::test]
async fn delete_provider_removes_file_reloads_manager_and_commits_tombstone() {
    let secret = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let manager = Arc::new(ProviderManager::new());
    let (registry, _adapter) = registry_with_adapter(secret.path(), Some(manager.clone())).await;

    let path = provider_path(secret.path(), "custom");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&provider_config("custom", "Custom", "sk-secret")).unwrap(),
    )
    .unwrap();
    let provider = rua_provider::manager::create_provider_from_config(
        &serde_json::from_value(provider_config("custom", "Custom", "sk-secret")).unwrap(),
    )
    .unwrap();
    manager.register(provider);

    let change =
        ProviderSecretSyncAdapter::delete_change("op-provider-delete", "custom", "a", hlc(200));
    let result = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    assert!(!path.exists());
    assert!(manager.get("custom").is_none());
    let cursor = merge_store
        .get_record_cursor("provider_secret:custom")
        .unwrap()
        .unwrap();
    assert_eq!(cursor.state, RecordState::Tombstoned);
}

#[tokio::test]
async fn stale_provider_delete_does_not_remove_newer_provider_config() {
    let secret = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let manager = Arc::new(ProviderManager::new());
    let (registry, _adapter) = registry_with_adapter(secret.path(), Some(manager.clone())).await;

    let path = provider_path(secret.path(), "custom");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&provider_config("custom", "Local", "local-secret")).unwrap(),
    )
    .unwrap();
    let newer_upsert = ProviderSecretSyncAdapter::upsert_change(
        "op-newer-upsert",
        "custom",
        provider_config("custom", "Newer", "newer-secret"),
        "device-b",
        hlc(200),
    );
    registry
        .apply_change(&newer_upsert, &merge_store)
        .await
        .unwrap();
    let stale_delete =
        ProviderSecretSyncAdapter::delete_change("op-stale-delete", "custom", "device-a", hlc(100));

    let result = registry
        .apply_change(&stale_delete, &merge_store)
        .await
        .unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Skipped);
    assert!(path.exists());
    let stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(stored["name"], "Newer");
    assert!(manager.get("custom").is_some());
}

#[tokio::test]
async fn duplicate_provider_delete_is_idempotent() {
    let secret = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let (registry, _adapter) = registry_with_adapter(secret.path(), None).await;

    let path = provider_path(secret.path(), "custom");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&provider_config("custom", "Custom", "sk-secret")).unwrap(),
    )
    .unwrap();
    let change =
        ProviderSecretSyncAdapter::delete_change("op-delete-same", "custom", "device-a", hlc(100));

    let first = registry.apply_change(&change, &merge_store).await.unwrap();
    let second = registry.apply_change(&change, &merge_store).await.unwrap();

    assert_eq!(first.status, RegistryApplyStatus::Applied);
    assert_eq!(second.status, RegistryApplyStatus::AlreadyApplied);
    assert!(!path.exists());
}

#[tokio::test]
async fn export_reads_local_provider_json_files() {
    let secret = tempfile::tempdir().unwrap();
    let (_registry, adapter) = registry_with_adapter(secret.path(), None).await;
    let path = provider_path(secret.path(), "openai");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&provider_config("openai", "OpenAI", "sk-secret")).unwrap(),
    )
    .unwrap();

    let exported = adapter.export_records().await.unwrap();

    assert_eq!(exported.len(), 1);
    assert_eq!(exported[0].domain, PROVIDER_SECRET_DOMAIN);
    assert_eq!(exported[0].record_id, "openai");
    assert_eq!(exported[0].kind, PROVIDER_SECRET_KIND_SNAPSHOT);
    assert_eq!(exported[0].payload["api_key"], "sk-secret");
}

#[tokio::test]
async fn export_decrypts_local_api_key_field_before_syncing_payload() {
    let secret = tempfile::tempdir().unwrap();
    let key = [42_u8; 32];
    let (_registry, adapter) = registry_with_adapter_and_key(secret.path(), None, Some(key)).await;
    let config: rua_provider::ProviderConfig =
        serde_json::from_value(provider_config("openai", "OpenAI", "sk-secret")).unwrap();
    rua_provider::ProviderConfigLoader::with_key(secret.path(), key)
        .save_provider(&config)
        .await
        .unwrap();

    let path_text = std::fs::read_to_string(provider_path(secret.path(), "openai")).unwrap();
    assert!(path_text.contains("rua_encrypted"));
    assert!(!path_text.contains("sk-secret"));

    let exported = adapter.export_records().await.unwrap();

    assert_eq!(exported.len(), 1);
    assert_eq!(exported[0].payload["api_key"], "sk-secret");
    assert!(!exported[0].payload["api_key"].is_object());
}

#[tokio::test]
async fn inbound_provider_upsert_reencrypts_api_key_for_local_disk() {
    let secret = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let key = [24_u8; 32];
    let (registry, _adapter) = registry_with_adapter_and_key(secret.path(), None, Some(key)).await;
    let change = ProviderSecretSyncAdapter::upsert_change(
        "op-provider-encrypted-local",
        "custom",
        provider_config("custom", "Custom", "sk-remote-secret"),
        "device-a",
        hlc(100),
    );

    registry.apply_change(&change, &merge_store).await.unwrap();

    let path = provider_path(secret.path(), "custom");
    let on_disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(on_disk["id"], "custom");
    assert_eq!(on_disk["name"], "Custom");
    assert_eq!(on_disk["api_key"]["rua_encrypted"], true);
    assert!(!std::fs::read_to_string(&path)
        .unwrap()
        .contains("sk-remote-secret"));

    let loaded = rua_provider::ProviderConfigLoader::with_key(secret.path(), key)
        .load_provider("custom")
        .await
        .unwrap();
    assert_eq!(loaded.api_key, "sk-remote-secret");
}

#[tokio::test]
async fn provider_api_key_update_syncs_between_devices_with_local_reencryption() {
    let secret_a = tempfile::tempdir().unwrap();
    let secret_b = tempfile::tempdir().unwrap();
    let merge_dir_b = tempfile::tempdir().unwrap();
    let merge_store_b = MergeStore::open(merge_dir_b.path()).unwrap();
    let key_a = [11_u8; 32];
    let key_b = [22_u8; 32];

    let initial: rua_provider::ProviderConfig =
        serde_json::from_value(provider_config("dashscope", "DashScope", "sk-old-key")).unwrap();
    rua_provider::ProviderConfigLoader::with_key(secret_a.path(), key_a)
        .save_provider(&initial)
        .await
        .unwrap();
    rua_provider::ProviderConfigLoader::with_key(secret_b.path(), key_b)
        .save_provider(&initial)
        .await
        .unwrap();

    let mut updated = initial.clone();
    updated.api_key = "sk-new-key-from-device-a".to_string();
    rua_provider::ProviderConfigLoader::with_key(secret_a.path(), key_a)
        .save_provider(&updated)
        .await
        .unwrap();

    let (_registry_a, adapter_a) =
        registry_with_adapter_and_key(secret_a.path(), None, Some(key_a)).await;
    let (registry_b, _adapter_b) =
        registry_with_adapter_and_key(secret_b.path(), None, Some(key_b)).await;
    let exported = adapter_a.export_records().await.unwrap();
    let change = exported
        .iter()
        .find(|change| change.record_id == "dashscope")
        .expect("dashscope export");
    assert_eq!(change.payload["api_key"], "sk-new-key-from-device-a");

    let result = registry_b
        .apply_change(change, &merge_store_b)
        .await
        .unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    let loaded_b = rua_provider::ProviderConfigLoader::with_key(secret_b.path(), key_b)
        .load_provider("dashscope")
        .await
        .unwrap();
    assert_eq!(loaded_b.api_key, "sk-new-key-from-device-a");

    let disk_b = std::fs::read_to_string(provider_path(secret_b.path(), "dashscope")).unwrap();
    assert!(disk_b.contains("rua_encrypted"));
    assert!(!disk_b.contains("sk-new-key-from-device-a"));
    assert!(!disk_b.contains("sk-old-key"));
}

#[tokio::test]
async fn provider_parity_fields_sync_and_secret_fields_reencrypt_locally() {
    let secret_a = tempfile::tempdir().unwrap();
    let secret_b = tempfile::tempdir().unwrap();
    let merge_dir_b = tempfile::tempdir().unwrap();
    let merge_store_b = MergeStore::open(merge_dir_b.path()).unwrap();
    let key_a = [31_u8; 32];
    let key_b = [32_u8; 32];

    let initial: rua_provider::ProviderConfig = serde_json::from_value(
        provider_config_with_parity_fields("github-models", "GitHub Models", "ghp-old-secret"),
    )
    .unwrap();
    rua_provider::ProviderConfigLoader::with_key(secret_a.path(), key_a)
        .save_provider(&initial)
        .await
        .unwrap();

    let (_registry_a, adapter_a) =
        registry_with_adapter_and_key(secret_a.path(), None, Some(key_a)).await;
    let manager_b = Arc::new(ProviderManager::new());
    let (registry_b, _adapter_b) =
        registry_with_adapter_and_key(secret_b.path(), Some(manager_b.clone()), Some(key_b)).await;
    let exported = adapter_a.export_records().await.unwrap();
    let change = exported
        .iter()
        .find(|change| change.record_id == "github-models")
        .expect("github models export");

    assert_eq!(change.payload["provider_group"], "github");
    assert_eq!(change.payload["api_key"], "ghp-old-secret");
    assert_eq!(
        change.payload["custom_headers"]["Authorization"],
        "Bearer synced-header-secret"
    );

    let result = registry_b
        .apply_change(change, &merge_store_b)
        .await
        .unwrap();

    assert_eq!(result.status, RegistryApplyStatus::Applied);
    assert!(manager_b.get("github-models").is_some());
    let loaded_b = rua_provider::ProviderConfigLoader::with_key(secret_b.path(), key_b)
        .load_provider("github-models")
        .await
        .unwrap();
    assert_eq!(loaded_b.provider_group, "github");
    assert_eq!(loaded_b.provider_variant, "models");
    assert_eq!(loaded_b.api_key_prefixes, ["ghp_", "github_pat_"]);
    assert_eq!(
        loaded_b.custom_headers.get("Authorization"),
        Some(&serde_json::Value::String(
            "Bearer synced-header-secret".into()
        ))
    );

    let disk_b = std::fs::read_to_string(provider_path(secret_b.path(), "github-models")).unwrap();
    assert!(disk_b.contains("rua_encrypted"));
    assert!(!disk_b.contains("ghp-old-secret"));
    assert!(!disk_b.contains("synced-header-secret"));
    assert!(disk_b.contains("visible-trace"));
}

#[tokio::test]
async fn invalid_provider_id_is_rejected() {
    let secret = tempfile::tempdir().unwrap();
    let merge_dir = tempfile::tempdir().unwrap();
    let merge_store = MergeStore::open(merge_dir.path()).unwrap();
    let (registry, _adapter) = registry_with_adapter(secret.path(), None).await;
    let change = ProviderSecretSyncAdapter::upsert_change(
        "op-provider-bad",
        "../openai",
        provider_config("openai", "OpenAI", "sk-secret"),
        "device-a",
        hlc(100),
    );

    let err = registry
        .apply_change(&change, &merge_store)
        .await
        .unwrap_err();

    assert!(err.to_string().contains("invalid provider id"));
    assert!(!provider_path(secret.path(), "openai").exists());
}

#[test]
fn helper_changes_use_provider_domain_and_do_not_embed_provider_id_in_op_id() {
    let change = ProviderSecretSyncAdapter::upsert_change(
        ProviderSecretSyncAdapter::new_op_id(),
        "openai",
        provider_config("openai", "OpenAI", "sk-secret"),
        "device-a",
        hlc(10),
    );

    assert_eq!(change.domain, PROVIDER_SECRET_DOMAIN);
    assert_eq!(change.kind, PROVIDER_SECRET_KIND_UPSERT);
    assert_eq!(change.record_id, "openai");
    assert!(!change.op_id.contains("openai"));
    assert_eq!(
        ProviderSecretSyncAdapter::delete_change("op", "openai", "device-a", hlc(20)).kind,
        PROVIDER_SECRET_KIND_DELETE
    );
}
