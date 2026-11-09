use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use rua_core::ProjectId;
use rua_project::{ProjectDescriptor, ProjectLifecycle, ProjectStores};
use rua_sync::hlc::HlcTimestamp;
use rua_sync::merge_store::MergeStore;
use rua_sync::project_adapter::{
    ProjectCatalogSyncAdapter, PROJECT_CATALOG_DOMAIN, PROJECT_DELETE_KIND, PROJECT_DESCRIPTOR_KIND,
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

fn descriptor(id: &str, name: &str, updated_ms: i64) -> ProjectDescriptor {
    ProjectDescriptor {
        project_id: ProjectId::new(id),
        display_name: name.to_string(),
        resource_device_id: "device-a".to_string(),
        display_path: format!("/repos/{name}"),
        lifecycle: ProjectLifecycle::Active,
        resources: None,
        created_at: Utc.timestamp_millis_opt(1_000).unwrap(),
        updated_at: Utc.timestamp_millis_opt(updated_ms).unwrap(),
        extra: BTreeMap::new(),
    }
}

#[tokio::test]
async fn export_contains_only_portable_descriptors() {
    const PROJECT_INSTRUCTIONS_MARKER: &str = "PROJECT_INSTRUCTIONS_MUST_NOT_SYNC_12D0";
    const PROJECT_SKILL_MARKER: &str = "PROJECT_SKILL_MUST_NOT_SYNC_13A1";
    let temp = tempfile::tempdir().unwrap();
    let project_dir = temp.path().join("repo");
    std::fs::create_dir(&project_dir).unwrap();
    std::fs::write(
        project_dir.join("AGENTS.md"),
        PROJECT_INSTRUCTIONS_MARKER.as_bytes(),
    )
    .unwrap();
    std::fs::create_dir_all(project_dir.join(".agents/skills/local-only")).unwrap();
    std::fs::write(
        project_dir.join(".agents/skills/local-only/SKILL.md"),
        PROJECT_SKILL_MARKER.as_bytes(),
    )
    .unwrap();
    let stores = ProjectStores::open(temp.path().join("projects")).unwrap();
    let created = stores
        .catalog
        .create_bound(rua_project::ProjectCreateRequest {
            resource_device_id: "device-a".to_string(),
            resource_root: project_dir.to_string_lossy().to_string(),
            display_name: Some("Repo".to_string()),
        })
        .unwrap();
    let adapter = ProjectCatalogSyncAdapter::new(Arc::clone(&stores.catalog), "device-a");

    let exported = adapter.export_records().await.unwrap();

    assert_eq!(exported.len(), 1);
    assert_eq!(exported[0].domain, PROJECT_CATALOG_DOMAIN);
    assert_eq!(exported[0].kind, PROJECT_DESCRIPTOR_KIND);
    assert_eq!(exported[0].record_id, created.project.project_id.as_str());
    assert!(exported[0].payload.get("resource_root").is_none());
    assert!(exported[0].payload.get("binding").is_none());
    assert!(!serde_json::to_string(&exported)
        .unwrap()
        .contains(PROJECT_INSTRUCTIONS_MARKER));
    assert!(!serde_json::to_string(&exported)
        .unwrap()
        .contains(PROJECT_SKILL_MARKER));
}

#[tokio::test]
async fn inbound_descriptor_preserves_local_binding_and_lww() {
    let temp = tempfile::tempdir().unwrap();
    let project_dir = temp.path().join("repo");
    std::fs::create_dir(&project_dir).unwrap();
    let stores = ProjectStores::open(temp.path().join("projects")).unwrap();
    let created = stores
        .catalog
        .create_bound(rua_project::ProjectCreateRequest {
            resource_device_id: "device-a".to_string(),
            resource_root: project_dir.to_string_lossy().to_string(),
            display_name: Some("Original".to_string()),
        })
        .unwrap();
    let merge_store = MergeStore::open(&temp.path().join("merge")).unwrap();
    let mut registry = SyncRegistry::new();
    registry
        .register(Arc::new(ProjectCatalogSyncAdapter::new(
            Arc::clone(&stores.catalog),
            "device-a",
        )))
        .unwrap();

    let mut fresh = created.project.clone();
    fresh.display_name = "Fresh".to_string();
    fresh.updated_at = Utc.timestamp_millis_opt(2_000).unwrap();
    let fresh_change =
        ProjectCatalogSyncAdapter::descriptor_change("fresh", fresh, "device-b", hlc(2_000));
    assert_eq!(
        registry
            .apply_change(&fresh_change, &merge_store)
            .await
            .unwrap()
            .status,
        RegistryApplyStatus::Applied
    );

    let stale_change = ProjectCatalogSyncAdapter::descriptor_change(
        "stale",
        descriptor(created.project.project_id.as_str(), "Stale", 1_000),
        "device-c",
        hlc(1_000),
    );
    assert_eq!(
        registry
            .apply_change(&stale_change, &merge_store)
            .await
            .unwrap()
            .status,
        RegistryApplyStatus::Skipped
    );
    assert_eq!(
        stores
            .catalog
            .get(&created.project.project_id)
            .unwrap()
            .unwrap()
            .display_name,
        "Fresh"
    );
    assert_eq!(
        stores
            .bindings
            .get(&created.project.project_id)
            .unwrap()
            .unwrap()
            .primary_root(),
        project_dir
    );
}

#[tokio::test]
async fn project_delete_tombstone_wins_over_stale_descriptor_and_exports() {
    let temp = tempfile::tempdir().unwrap();
    let stores = ProjectStores::open(temp.path().join("projects")).unwrap();
    let merge_store = MergeStore::open(&temp.path().join("merge")).unwrap();
    let mut registry = SyncRegistry::new();
    let adapter = Arc::new(ProjectCatalogSyncAdapter::new(
        Arc::clone(&stores.catalog),
        "device-b",
    ));
    registry.register(adapter.clone()).unwrap();
    let project = descriptor("project-deleted", "Deleted", 1_000);
    stores.catalog.apply_descriptor(project.clone()).unwrap();

    let delete = ProjectCatalogSyncAdapter::delete_change(
        "delete",
        project.project_id.to_string(),
        "device-a",
        hlc(2_000),
    );
    assert_eq!(delete.kind, PROJECT_DELETE_KIND);
    assert_eq!(
        registry
            .apply_change(&delete, &merge_store)
            .await
            .unwrap()
            .status,
        RegistryApplyStatus::Applied
    );
    assert!(stores.catalog.get(&project.project_id).unwrap().is_none());

    let stale = ProjectCatalogSyncAdapter::descriptor_change(
        "stale-after-delete",
        project,
        "device-c",
        hlc(1_500),
    );
    assert_eq!(
        registry
            .apply_change(&stale, &merge_store)
            .await
            .unwrap()
            .status,
        RegistryApplyStatus::Skipped
    );
    assert_eq!(
        merge_store
            .get_record_cursor("project_catalog:project-deleted")
            .unwrap()
            .unwrap()
            .state,
        RecordState::Tombstoned
    );

    let exported = adapter.export_records().await.unwrap();
    assert_eq!(exported.len(), 1);
    assert_eq!(exported[0].kind, PROJECT_DELETE_KIND);
    assert_eq!(exported[0].record_id, "project-deleted");
}
