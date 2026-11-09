use std::sync::{Arc, Barrier};

use rua_project::{
    ProjectCreateRequest, ProjectDeletePreparation, ProjectError, ProjectLifecycle,
    ProjectOverlapRelation, ProjectStores,
};

fn create_request(path: &std::path::Path) -> ProjectCreateRequest {
    ProjectCreateRequest {
        resource_device_id: "device-a".to_string(),
        resource_root: path.to_string_lossy().to_string(),
        display_name: None,
    }
}

#[test]
fn creation_commits_descriptor_and_binding_together() {
    let temp = tempfile::tempdir().unwrap();
    let project_dir = temp.path().join("repo");
    std::fs::create_dir(&project_dir).unwrap();
    let stores = ProjectStores::open(temp.path().join("data")).unwrap();

    let outcome = stores
        .catalog
        .create_bound(create_request(&project_dir))
        .unwrap();
    let descriptor = stores
        .catalog
        .get(&outcome.project.project_id)
        .unwrap()
        .unwrap();
    let binding = stores
        .bindings
        .get(&outcome.project.project_id)
        .unwrap()
        .unwrap();

    assert_eq!(descriptor.project_id, binding.project_id);
    assert_eq!(binding.primary_root().to_path_buf(), project_dir);
    assert_eq!(descriptor.display_name, "repo");
    let serialized = serde_json::to_value(descriptor).unwrap();
    assert!(serialized.get("resource_root").is_none());
}

#[test]
fn shared_directory_reports_archived_project_without_reusing_identity() {
    let temp = tempfile::tempdir().unwrap();
    let project_dir = temp.path().join("repo");
    std::fs::create_dir(&project_dir).unwrap();
    let stores = ProjectStores::open(temp.path().join("data")).unwrap();
    let created = stores
        .catalog
        .create_bound(create_request(&project_dir))
        .unwrap();
    let mut archived = created.project.clone();
    archived.lifecycle = ProjectLifecycle::Archived;
    stores.catalog.apply_descriptor(archived.clone()).unwrap();

    let reused = stores
        .catalog
        .create_bound(create_request(&project_dir))
        .unwrap();
    assert_ne!(reused.project.project_id, archived.project_id);
    assert_eq!(reused.overlap_warnings.len(), 1);
    assert_eq!(reused.overlap_warnings[0].project_id, archived.project_id);
    assert_eq!(
        reused.overlap_warnings[0].relation,
        ProjectOverlapRelation::SharedDirectory
    );
}

#[test]
fn permanent_delete_hides_first_and_releases_binding_only_after_finalize() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    let project_dir = temp.path().join("repo");
    std::fs::create_dir(&project_dir).unwrap();
    let stores = ProjectStores::open(&data_dir).unwrap();
    let created = stores
        .catalog
        .create_bound(create_request(&project_dir))
        .unwrap();
    let project_id = created.project.project_id.clone();

    assert!(matches!(
        stores.catalog.begin_permanent_delete(&project_id),
        Err(ProjectError::PermanentDeleteRequiresArchive(_))
    ));
    stores
        .catalog
        .set_lifecycle(&project_id, ProjectLifecycle::Archived)
        .unwrap();

    let prepared = stores.catalog.begin_permanent_delete(&project_id).unwrap();
    assert!(matches!(
        prepared,
        ProjectDeletePreparation::Pending {
            newly_tombstoned: true,
            ..
        }
    ));
    assert!(stores.catalog.get(&project_id).unwrap().is_none());
    assert!(stores.catalog.is_deleted(&project_id).unwrap());
    assert!(stores.bindings.get(&project_id).unwrap().is_some());

    assert!(stores
        .catalog
        .finalize_permanent_delete(&project_id)
        .unwrap());
    assert!(stores.bindings.get(&project_id).unwrap().is_none());
    let replacement = stores
        .catalog
        .create_bound(create_request(&project_dir))
        .unwrap();
    assert_ne!(replacement.project.project_id, project_id);

    drop(stores);
    let reopened = ProjectStores::open(&data_dir).unwrap();
    assert!(reopened.catalog.get(&project_id).unwrap().is_none());
    assert!(reopened.catalog.is_deleted(&project_id).unwrap());
    assert_eq!(reopened.catalog.list().unwrap(), vec![replacement.project]);
}

#[test]
fn failed_delete_can_roll_back_before_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let project_dir = temp.path().join("repo");
    std::fs::create_dir(&project_dir).unwrap();
    let stores = ProjectStores::open(temp.path().join("data")).unwrap();
    let created = stores
        .catalog
        .create_bound(create_request(&project_dir))
        .unwrap();
    let project_id = created.project.project_id.clone();
    stores
        .catalog
        .set_lifecycle(&project_id, ProjectLifecycle::Archived)
        .unwrap();

    stores.catalog.begin_permanent_delete(&project_id).unwrap();
    assert!(stores
        .catalog
        .rollback_permanent_delete(&project_id)
        .unwrap());

    assert_eq!(
        stores.catalog.get(&project_id).unwrap().unwrap().lifecycle,
        ProjectLifecycle::Archived
    );
    assert!(stores.bindings.get(&project_id).unwrap().is_some());
    assert!(!stores.catalog.is_deleted(&project_id).unwrap());
}

#[test]
fn interrupted_delete_reopens_hidden_with_binding_retained_until_retry() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    let project_dir = temp.path().join("repo");
    std::fs::create_dir(&project_dir).unwrap();
    let stores = ProjectStores::open(&data_dir).unwrap();
    let created = stores
        .catalog
        .create_bound(create_request(&project_dir))
        .unwrap();
    let project_id = created.project.project_id.clone();
    stores
        .catalog
        .set_lifecycle(&project_id, ProjectLifecycle::Archived)
        .unwrap();
    stores.catalog.begin_permanent_delete(&project_id).unwrap();
    drop(stores);

    let reopened = ProjectStores::open(&data_dir).unwrap();
    assert!(reopened.catalog.get(&project_id).unwrap().is_none());
    assert!(reopened.bindings.get(&project_id).unwrap().is_some());
    assert!(matches!(
        reopened
            .catalog
            .begin_permanent_delete(&project_id)
            .unwrap(),
        ProjectDeletePreparation::Pending {
            newly_tombstoned: false,
            ..
        }
    ));
    assert!(reopened
        .catalog
        .finalize_permanent_delete(&project_id)
        .unwrap());
    assert!(reopened.bindings.get(&project_id).unwrap().is_none());
}

#[test]
fn synchronized_tombstone_blocks_same_project_id_resurrection() {
    let temp = tempfile::tempdir().unwrap();
    let stores = ProjectStores::open(temp.path().join("data")).unwrap();
    let descriptor = rua_project::ProjectDescriptor {
        project_id: rua_core::ProjectId::new("deleted-project"),
        display_name: "Deleted".to_string(),
        resource_device_id: "device-a".to_string(),
        display_path: "/repos/deleted".to_string(),
        lifecycle: ProjectLifecycle::Archived,
        resources: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        extra: Default::default(),
    };

    assert!(stores
        .catalog
        .apply_tombstone(descriptor.project_id.clone(), chrono::Utc::now())
        .unwrap());
    assert!(matches!(
        stores.catalog.apply_descriptor(descriptor),
        Err(ProjectError::PermanentlyDeleted(_))
    ));
}

#[test]
fn lifecycle_transition_is_idempotent_and_preserves_binding_identity() {
    let temp = tempfile::tempdir().unwrap();
    let project_dir = temp.path().join("repo");
    std::fs::create_dir(&project_dir).unwrap();
    let stores = ProjectStores::open(temp.path().join("data")).unwrap();
    let created = stores
        .catalog
        .create_bound(create_request(&project_dir))
        .unwrap();
    let binding = stores
        .bindings
        .get(&created.project.project_id)
        .unwrap()
        .unwrap();

    let archived = stores
        .catalog
        .set_lifecycle(&created.project.project_id, ProjectLifecycle::Archived)
        .unwrap();
    let archived_again = stores
        .catalog
        .set_lifecycle(&created.project.project_id, ProjectLifecycle::Archived)
        .unwrap();
    let reopened = stores
        .catalog
        .set_lifecycle(&created.project.project_id, ProjectLifecycle::Active)
        .unwrap();

    assert_eq!(archived.lifecycle, ProjectLifecycle::Archived);
    assert_eq!(archived_again.updated_at, archived.updated_at);
    assert_eq!(reopened.lifecycle, ProjectLifecycle::Active);
    assert_eq!(
        stores
            .bindings
            .get(&created.project.project_id)
            .unwrap()
            .unwrap(),
        binding
    );
}

#[test]
fn lifecycle_transition_gate_blocks_duplicate_coordinators() {
    let temp = tempfile::tempdir().unwrap();
    let project_dir = temp.path().join("repo");
    std::fs::create_dir(&project_dir).unwrap();
    let stores = ProjectStores::open(temp.path().join("data")).unwrap();
    let created = stores
        .catalog
        .create_bound(create_request(&project_dir))
        .unwrap();

    stores
        .catalog
        .begin_lifecycle_transition(&created.project.project_id)
        .unwrap();
    assert!(stores
        .catalog
        .lifecycle_transition_in_progress(&created.project.project_id));
    assert!(matches!(
        stores
            .catalog
            .begin_lifecycle_transition(&created.project.project_id),
        Err(ProjectError::LifecycleTransitionInProgress(_))
    ));
    stores
        .catalog
        .end_lifecycle_transition(&created.project.project_id);
    assert!(!stores
        .catalog
        .lifecycle_transition_in_progress(&created.project.project_id));
}

#[test]
fn aliases_are_distinct_and_ancestor_overlap_only_warns() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    let package = repo.join("package");
    std::fs::create_dir_all(&package).unwrap();
    let alias = temp.path().join("repo-alias");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&repo, &alias).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&repo, &alias).unwrap();
    let stores = ProjectStores::open(temp.path().join("data")).unwrap();

    let root = stores.catalog.create_bound(create_request(&repo)).unwrap();
    let child = stores
        .catalog
        .create_bound(create_request(&package))
        .unwrap();
    let alias_project = stores.catalog.create_bound(create_request(&alias)).unwrap();

    assert!(root.overlap_warnings.is_empty());
    assert_eq!(child.overlap_warnings.len(), 1);
    assert_eq!(
        child.overlap_warnings[0].relation,
        ProjectOverlapRelation::ExistingAncestor
    );
    assert!(alias_project.overlap_warnings.is_empty());
    assert_eq!(stores.catalog.list_active().unwrap().len(), 3);
}

#[test]
fn project_display_names_need_not_be_unique() {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("first");
    let second = temp.path().join("second");
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    let stores = ProjectStores::open(temp.path().join("data")).unwrap();

    for path in [&first, &second] {
        stores
            .catalog
            .create_bound(ProjectCreateRequest {
                resource_device_id: "device-a".to_string(),
                resource_root: path.to_string_lossy().to_string(),
                display_name: Some("Same name".to_string()),
            })
            .unwrap();
    }

    let projects = stores.catalog.list().unwrap();
    assert_eq!(projects.len(), 2);
    assert!(projects
        .iter()
        .all(|project| project.display_name == "Same name"));
}

#[test]
fn synchronized_descriptor_cannot_rebind_project_identity() {
    let temp = tempfile::tempdir().unwrap();
    let project_dir = temp.path().join("repo");
    std::fs::create_dir(&project_dir).unwrap();
    let stores = ProjectStores::open(temp.path().join("data")).unwrap();
    let created = stores
        .catalog
        .create_bound(create_request(&project_dir))
        .unwrap();
    let mut changed = created.project.clone();
    changed.resource_device_id = "device-b".to_string();

    assert!(matches!(
        stores.catalog.apply_descriptor(changed),
        Err(ProjectError::ImmutableIdentityChange(_))
    ));
    assert_eq!(
        stores
            .bindings
            .get(&created.project.project_id)
            .unwrap()
            .unwrap()
            .resource_device_id,
        "device-a"
    );
}

#[test]
fn concurrent_shared_directory_creation_preserves_distinct_project_identities() {
    let temp = tempfile::tempdir().unwrap();
    let project_dir = temp.path().join("repo");
    std::fs::create_dir(&project_dir).unwrap();
    let stores = ProjectStores::open(temp.path().join("data")).unwrap();
    let catalog = Arc::clone(&stores.catalog);
    let barrier = Arc::new(Barrier::new(8));

    let handles = (0..8)
        .map(|_| {
            let catalog = Arc::clone(&catalog);
            let barrier = Arc::clone(&barrier);
            let project_dir = project_dir.clone();
            std::thread::spawn(move || {
                barrier.wait();
                catalog.create_bound(create_request(&project_dir))
            })
        })
        .collect::<Vec<_>>();
    let results = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();

    assert!(results.iter().all(Result::is_ok));
    let projects = catalog.list().unwrap();
    assert_eq!(projects.len(), 8);
    let ids = projects
        .iter()
        .map(|project| &project.project_id)
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(ids.len(), 8);
}

#[test]
fn failed_atomic_write_leaves_neither_descriptor_nor_binding() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    let project_dir = temp.path().join("repo");
    std::fs::create_dir(&project_dir).unwrap();
    let stores = ProjectStores::open(&data_dir).unwrap();
    let records_dir = data_dir.join("records");
    std::fs::remove_dir(&records_dir).unwrap();
    std::fs::write(&records_dir, b"not a directory").unwrap();

    assert!(stores
        .catalog
        .create_bound(create_request(&project_dir))
        .is_err());
    assert!(stores.catalog.list().unwrap().is_empty());
    assert!(stores.bindings.list().unwrap().is_empty());
}

#[test]
fn rename_preserves_unknown_descriptor_and_binding_fields() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    let project_dir = temp.path().join("repo");
    std::fs::create_dir(&project_dir).unwrap();
    let stores = ProjectStores::open(&data_dir).unwrap();
    let created = stores
        .catalog
        .create_bound(create_request(&project_dir))
        .unwrap();
    drop(stores);

    let record_path = std::fs::read_dir(data_dir.join("records"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
    raw["descriptor"]["future_descriptor"] = serde_json::json!({"enabled": true});
    raw["binding"]["future_binding"] = serde_json::json!("kept");
    raw["future_record"] = serde_json::json!(7);
    rua_config::atomic_write(&record_path, &serde_json::to_vec_pretty(&raw).unwrap()).unwrap();

    let stores = ProjectStores::open(&data_dir).unwrap();
    stores
        .catalog
        .rename(&created.project.project_id, "Renamed")
        .unwrap();
    let after: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
    assert_eq!(after["descriptor"]["future_descriptor"]["enabled"], true);
    assert_eq!(after["binding"]["future_binding"], "kept");
    assert_eq!(after["future_record"], 7);
}

#[test]
fn open_migrates_legacy_project_running_config_without_dropping_unknown_fields() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    let project_dir = temp.path().join("repo");
    std::fs::create_dir(&project_dir).unwrap();
    let stores = ProjectStores::open(&data_dir).unwrap();
    let created = stores
        .catalog
        .create_bound(create_request(&project_dir))
        .unwrap();
    drop(stores);

    let record_path = std::fs::read_dir(data_dir.join("records"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
    raw["schema_version"] = serde_json::json!(1);
    raw["descriptor"]["running_config"] = serde_json::json!({"max_iters": 9});
    raw["descriptor"]["future_descriptor"] = serde_json::json!({"enabled": true});
    rua_config::atomic_write(&record_path, &serde_json::to_vec_pretty(&raw).unwrap()).unwrap();

    let stores = ProjectStores::open(&data_dir).unwrap();
    let descriptor = stores
        .catalog
        .get(&created.project.project_id)
        .unwrap()
        .unwrap();
    let descriptor = serde_json::to_value(descriptor).unwrap();
    assert!(descriptor.get("running_config").is_none());
    assert_eq!(descriptor["future_descriptor"]["enabled"], true);

    let migrated: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
    assert_eq!(migrated["schema_version"], 3);
    assert!(migrated["descriptor"].get("running_config").is_none());
    assert_eq!(migrated["descriptor"]["future_descriptor"]["enabled"], true);
}

#[test]
fn apply_descriptor_drops_legacy_project_running_config() {
    let temp = tempfile::tempdir().unwrap();
    let project_dir = temp.path().join("repo");
    std::fs::create_dir(&project_dir).unwrap();
    let stores = ProjectStores::open(temp.path().join("data")).unwrap();
    let mut descriptor = stores
        .catalog
        .create_bound(create_request(&project_dir))
        .unwrap()
        .project;
    descriptor.extra.insert(
        "running_config".to_string(),
        serde_json::json!({"max_iters": 9}),
    );

    let applied = stores.catalog.apply_descriptor(descriptor).unwrap();

    assert!(!applied.extra.contains_key("running_config"));
}
