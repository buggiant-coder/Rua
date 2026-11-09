use std::collections::BTreeSet;

use rua_server::sync_domain_catalog::{
    build_apply_registry, build_export_registry, APPLY_DOMAIN_IDS, EXPORT_DOMAIN_IDS,
};

#[test]
fn task_runtime_is_not_a_sync_domain() {
    assert!(EXPORT_DOMAIN_IDS
        .iter()
        .chain(APPLY_DOMAIN_IDS.iter())
        .all(|domain| !domain.contains("task")));
}

#[test]
fn token_usage_is_device_local_and_not_a_sync_domain() {
    assert!(!EXPORT_DOMAIN_IDS.contains(&"usage"));
    assert!(!APPLY_DOMAIN_IDS.contains(&"usage"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn export_catalog_registers_all_full_export_domains() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let data_dir = tmp.path().to_path_buf();
    let secret_dir = data_dir.join("secret");
    let provider_manager =
        std::sync::Arc::new(rua_provider::manager::ProviderManager::with_config_loader(
            rua_provider::config_loader::ProviderConfigLoader::new(secret_dir.clone()),
        ));
    let device_key = [7u8; 32];
    let blobs: rua_sync::endpoint::BlobsApiStore =
        iroh_blobs::store::mem::MemStore::default().into();
    let sync_key = rua_sync::crypto::SyncKey::from_bytes([9u8; 32]);
    let project_catalog = rua_project::ProjectStores::open(data_dir.join("projects"))
        .expect("Project stores")
        .catalog;

    let registry = build_export_registry(
        data_dir,
        secret_dir,
        "device-1".to_string(),
        device_key,
        blobs,
        sync_key,
        provider_manager,
        project_catalog,
    )
    .expect("export registry");

    let expected = EXPORT_DOMAIN_IDS
        .iter()
        .map(|domain| domain.to_string())
        .collect::<BTreeSet<_>>();
    let actual = registry.domains().into_iter().collect::<BTreeSet<_>>();
    assert_eq!(actual, expected);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn apply_catalog_registers_all_inbound_apply_domains() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let data_dir = tmp.path().to_path_buf();
    let secret_dir = data_dir.join("secret");
    let provider_manager =
        std::sync::Arc::new(rua_provider::manager::ProviderManager::with_config_loader(
            rua_provider::config_loader::ProviderConfigLoader::new(secret_dir.clone()),
        ));
    let security = std::sync::Arc::new(rua_security::SecurityModule::new(
        std::sync::Arc::new(rua_security::SecurityConfigStore::new(data_dir.clone())),
        None,
    ));
    let device_key = [7u8; 32];
    let blobs: rua_sync::endpoint::BlobsApiStore =
        iroh_blobs::store::mem::MemStore::default().into();
    let sync_key = rua_sync::crypto::SyncKey::from_bytes([9u8; 32]);
    let project_catalog = rua_project::ProjectStores::open(data_dir.join("projects"))
        .expect("Project stores")
        .catalog;

    let registry = build_apply_registry(
        data_dir,
        secret_dir,
        "device-1".to_string(),
        device_key,
        provider_manager,
        security,
        Some((blobs, sync_key)),
        None,
        project_catalog,
    )
    .expect("apply registry");

    let expected = APPLY_DOMAIN_IDS
        .iter()
        .map(|domain| domain.to_string())
        .collect::<BTreeSet<_>>();
    let actual = registry.domains().into_iter().collect::<BTreeSet<_>>();
    assert_eq!(actual, expected);
    assert!(actual.contains("network_migration"));
}
