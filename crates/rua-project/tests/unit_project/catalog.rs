use std::sync::{Arc, Barrier};

use rua_project::{
    ProjectCatalog, ProjectCreateRequest, ProjectDeletePreparation, ProjectError, ProjectLifecycle,
    ProjectOverlapRelation,
};

fn create_request(path: &std::path::Path) -> ProjectCreateRequest {
    ProjectCreateRequest {
        resource_device_id: "device-a".to_string(),
        resource_root: path.to_string_lossy().to_string(),
        display_name: None,
    }
}

#[test]
fn creation_commits_named_project_and_device_location_together() {
    let temp = tempfile::tempdir().unwrap();
    let project_dir = temp.path().join("repo");
    std::fs::create_dir(&project_dir).unwrap();
    let catalog = ProjectCatalog::open(temp.path().join("data")).unwrap();

    let outcome = catalog.create_bound(create_request(&project_dir)).unwrap();
    let descriptor = catalog.get(&outcome.project.project_id).unwrap().unwrap();
    assert_eq!(
        descriptor.resources.directories[0].resource_root,
        project_dir
    );
    assert_eq!(descriptor.resources.directories[0].device_id, "device-a");
    assert_eq!(descriptor.display_name, "repo");
    let serialized = serde_json::to_value(descriptor).unwrap();
    assert!(serialized.get("resource_root").is_none());
}

#[test]
fn shared_directory_reports_archived_project_without_reusing_identity() {
    let temp = tempfile::tempdir().unwrap();
    let project_dir = temp.path().join("repo");
    std::fs::create_dir(&project_dir).unwrap();
    let catalog = ProjectCatalog::open(temp.path().join("data")).unwrap();
    let created = catalog.create_bound(create_request(&project_dir)).unwrap();
    let mut archived = created.project.clone();
    archived.lifecycle = ProjectLifecycle::Archived;
    catalog.apply_descriptor(archived.clone()).unwrap();

    let reused = catalog.create_bound(create_request(&project_dir)).unwrap();
    assert_ne!(reused.project.project_id, archived.project_id);
    assert_eq!(reused.overlap_warnings.len(), 1);
    assert_eq!(reused.overlap_warnings[0].project_id, archived.project_id);
    assert_eq!(
        reused.overlap_warnings[0].relation,
        ProjectOverlapRelation::SharedDirectory
    );
}

#[test]
fn permanent_delete_hides_first_and_removes_the_record_on_finalize() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    let project_dir = temp.path().join("repo");
    std::fs::create_dir(&project_dir).unwrap();
    let catalog = ProjectCatalog::open(&data_dir).unwrap();
    let created = catalog.create_bound(create_request(&project_dir)).unwrap();
    let project_id = created.project.project_id.clone();

    assert!(matches!(
        catalog.begin_permanent_delete(&project_id),
        Err(ProjectError::PermanentDeleteRequiresArchive(_))
    ));
    catalog
        .set_lifecycle(&project_id, ProjectLifecycle::Archived)
        .unwrap();

    let prepared = catalog.begin_permanent_delete(&project_id).unwrap();
    assert!(matches!(
        prepared,
        ProjectDeletePreparation::Pending {
            newly_tombstoned: true,
            ..
        }
    ));
    assert!(catalog.get(&project_id).unwrap().is_none());
    assert!(catalog.is_deleted(&project_id).unwrap());
    assert!(project_dir.is_dir());

    assert!(catalog.finalize_permanent_delete(&project_id).unwrap());
    assert!(catalog.get(&project_id).unwrap().is_none());
    let replacement = catalog.create_bound(create_request(&project_dir)).unwrap();
    assert_ne!(replacement.project.project_id, project_id);

    drop(catalog);
    let reopened = ProjectCatalog::open(&data_dir).unwrap();
    assert!(reopened.get(&project_id).unwrap().is_none());
    assert!(reopened.is_deleted(&project_id).unwrap());
    assert_eq!(reopened.list().unwrap(), vec![replacement.project]);
}

#[test]
fn failed_delete_can_roll_back_before_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let project_dir = temp.path().join("repo");
    std::fs::create_dir(&project_dir).unwrap();
    let catalog = ProjectCatalog::open(temp.path().join("data")).unwrap();
    let created = catalog.create_bound(create_request(&project_dir)).unwrap();
    let project_id = created.project.project_id.clone();
    catalog
        .set_lifecycle(&project_id, ProjectLifecycle::Archived)
        .unwrap();

    catalog.begin_permanent_delete(&project_id).unwrap();
    assert!(catalog.rollback_permanent_delete(&project_id).unwrap());

    assert_eq!(
        catalog.get(&project_id).unwrap().unwrap().lifecycle,
        ProjectLifecycle::Archived
    );
    assert!(project_dir.is_dir());
    assert!(!catalog.is_deleted(&project_id).unwrap());
}

#[test]
fn interrupted_delete_reopens_hidden_until_retry() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    let project_dir = temp.path().join("repo");
    std::fs::create_dir(&project_dir).unwrap();
    let catalog = ProjectCatalog::open(&data_dir).unwrap();
    let created = catalog.create_bound(create_request(&project_dir)).unwrap();
    let project_id = created.project.project_id.clone();
    catalog
        .set_lifecycle(&project_id, ProjectLifecycle::Archived)
        .unwrap();
    catalog.begin_permanent_delete(&project_id).unwrap();
    drop(catalog);

    let reopened = ProjectCatalog::open(&data_dir).unwrap();
    assert!(reopened.get(&project_id).unwrap().is_none());
    assert!(project_dir.is_dir());
    assert!(matches!(
        reopened.begin_permanent_delete(&project_id).unwrap(),
        ProjectDeletePreparation::Pending {
            newly_tombstoned: false,
            ..
        }
    ));
    assert!(reopened.finalize_permanent_delete(&project_id).unwrap());
    assert!(reopened.get(&project_id).unwrap().is_none());
}

#[test]
fn synchronized_tombstone_blocks_same_project_id_resurrection() {
    let temp = tempfile::tempdir().unwrap();
    let catalog = ProjectCatalog::open(temp.path().join("data")).unwrap();
    let descriptor = rua_project::ProjectDescriptor {
        execution_default: None,
        project_id: rua_core::ProjectId::new("deleted-project"),
        display_name: "Deleted".to_string(),
        lifecycle: ProjectLifecycle::Archived,
        resources: Default::default(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        extra: Default::default(),
    };

    assert!(catalog
        .apply_tombstone(descriptor.project_id.clone(), chrono::Utc::now())
        .unwrap());
    assert!(matches!(
        catalog.apply_descriptor(descriptor),
        Err(ProjectError::PermanentlyDeleted(_))
    ));
}

#[test]
fn lifecycle_transition_is_idempotent_and_preserves_resource_associations() {
    let temp = tempfile::tempdir().unwrap();
    let project_dir = temp.path().join("repo");
    std::fs::create_dir(&project_dir).unwrap();
    let catalog = ProjectCatalog::open(temp.path().join("data")).unwrap();
    let created = catalog.create_bound(create_request(&project_dir)).unwrap();
    let archived = catalog
        .set_lifecycle(&created.project.project_id, ProjectLifecycle::Archived)
        .unwrap();
    let archived_again = catalog
        .set_lifecycle(&created.project.project_id, ProjectLifecycle::Archived)
        .unwrap();
    let reopened = catalog
        .set_lifecycle(&created.project.project_id, ProjectLifecycle::Active)
        .unwrap();

    assert_eq!(archived.lifecycle, ProjectLifecycle::Archived);
    assert_eq!(archived_again.updated_at, archived.updated_at);
    assert_eq!(reopened.lifecycle, ProjectLifecycle::Active);
    assert_eq!(created.project.resources, reopened.resources);
}

#[test]
fn lifecycle_transition_gate_blocks_duplicate_coordinators() {
    let temp = tempfile::tempdir().unwrap();
    let project_dir = temp.path().join("repo");
    std::fs::create_dir(&project_dir).unwrap();
    let catalog = ProjectCatalog::open(temp.path().join("data")).unwrap();
    let created = catalog.create_bound(create_request(&project_dir)).unwrap();

    catalog
        .begin_lifecycle_transition(&created.project.project_id)
        .unwrap();
    assert!(catalog.lifecycle_transition_in_progress(&created.project.project_id));
    assert!(matches!(
        catalog.begin_lifecycle_transition(&created.project.project_id),
        Err(ProjectError::LifecycleTransitionInProgress(_))
    ));
    catalog.end_lifecycle_transition(&created.project.project_id);
    assert!(!catalog.lifecycle_transition_in_progress(&created.project.project_id));
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
    let catalog = ProjectCatalog::open(temp.path().join("data")).unwrap();

    let root = catalog.create_bound(create_request(&repo)).unwrap();
    let child = catalog.create_bound(create_request(&package)).unwrap();
    let alias_project = catalog.create_bound(create_request(&alias)).unwrap();

    assert!(root.overlap_warnings.is_empty());
    assert_eq!(child.overlap_warnings.len(), 1);
    assert_eq!(
        child.overlap_warnings[0].relation,
        ProjectOverlapRelation::ExistingAncestor
    );
    assert!(alias_project.overlap_warnings.is_empty());
    assert_eq!(catalog.list_active().unwrap().len(), 3);
}

#[test]
fn project_display_names_need_not_be_unique() {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("first");
    let second = temp.path().join("second");
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    let catalog = ProjectCatalog::open(temp.path().join("data")).unwrap();

    for path in [&first, &second] {
        catalog
            .create_bound(ProjectCreateRequest {
                resource_device_id: "device-a".to_string(),
                resource_root: path.to_string_lossy().to_string(),
                display_name: Some("Same name".to_string()),
            })
            .unwrap();
    }

    let projects = catalog.list().unwrap();
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
    let catalog = ProjectCatalog::open(temp.path().join("data")).unwrap();
    let created = catalog.create_bound(create_request(&project_dir)).unwrap();
    let mut changed = created.project.clone();
    changed.created_at += chrono::Duration::seconds(1);
    assert!(matches!(
        catalog.apply_descriptor(changed),
        Err(ProjectError::ImmutableIdentityChange(_))
    ));
    assert_eq!(
        catalog.get(&created.project.project_id).unwrap().unwrap(),
        created.project
    );
}

#[test]
fn concurrent_shared_directory_creation_preserves_distinct_project_identities() {
    let temp = tempfile::tempdir().unwrap();
    let project_dir = temp.path().join("repo");
    std::fs::create_dir(&project_dir).unwrap();
    let catalog = Arc::new(ProjectCatalog::open(temp.path().join("data")).unwrap());
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
fn failed_atomic_write_does_not_publish_a_project() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    let project_dir = temp.path().join("repo");
    std::fs::create_dir(&project_dir).unwrap();
    let catalog = ProjectCatalog::open(&data_dir).unwrap();
    let records_dir = data_dir.join("records");
    std::fs::remove_dir(&records_dir).unwrap();
    std::fs::write(&records_dir, b"not a directory").unwrap();

    assert!(catalog.create_bound(create_request(&project_dir)).is_err());
    assert!(catalog.list().unwrap().is_empty());
}

#[test]
fn rename_preserves_unknown_descriptor_and_directory_fields() {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    let project_dir = temp.path().join("repo");
    std::fs::create_dir(&project_dir).unwrap();
    let catalog = ProjectCatalog::open(&data_dir).unwrap();
    let created = catalog.create_bound(create_request(&project_dir)).unwrap();
    drop(catalog);

    let record_path = std::fs::read_dir(data_dir.join("records"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
    raw["descriptor"]["future_descriptor"] = serde_json::json!({"enabled": true});
    raw["descriptor"]["resources"]["directories"][0]["future_field"] = serde_json::json!("kept");
    raw["future_record"] = serde_json::json!(7);
    rua_config::atomic_write(&record_path, &serde_json::to_vec_pretty(&raw).unwrap()).unwrap();

    let catalog = ProjectCatalog::open(&data_dir).unwrap();
    catalog
        .rename(&created.project.project_id, "Renamed")
        .unwrap();
    let after: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
    assert_eq!(after["descriptor"]["future_descriptor"]["enabled"], true);
    assert_eq!(
        after["descriptor"]["resources"]["directories"][0]["future_field"],
        "kept"
    );
    assert_eq!(after["future_record"], 7);
}

#[test]
fn apply_descriptor_drops_legacy_project_running_config() {
    let temp = tempfile::tempdir().unwrap();
    let project_dir = temp.path().join("repo");
    std::fs::create_dir(&project_dir).unwrap();
    let catalog = ProjectCatalog::open(temp.path().join("data")).unwrap();
    let mut descriptor = catalog
        .create_bound(create_request(&project_dir))
        .unwrap()
        .project;
    descriptor.extra.insert(
        "running_config".to_string(),
        serde_json::json!({"max_iters": 9}),
    );

    let applied = catalog.apply_descriptor(descriptor).unwrap();

    assert!(!applied.extra.contains_key("running_config"));
}

#[test]
fn execution_default_persists_separately_from_resources_and_can_be_cleared() {
    use rua_core::ExecutionDirectorySelection;
    use rua_project::{ProjectCreateResourcesRequest, ProjectResourceDirectoryInput};
    use serde_json::json;

    let temp = tempfile::tempdir().unwrap();
    let catalog = ProjectCatalog::open(temp.path()).unwrap();
    let original: ExecutionDirectorySelection = serde_json::from_value(json!({
        "device_id": "phone", "path": "/srv/photos", "extension": 7,
    }))
    .unwrap();
    let project = catalog
        .create_resources(ProjectCreateResourcesRequest {
            display_name: Some("Photos".into()),
            directories: vec![ProjectResourceDirectoryInput::new("laptop", "/srv/photos")],
            execution_default: Some(original.clone()),
        })
        .unwrap()
        .project;
    assert_eq!(project.execution_default.as_ref(), Some(&original));
    let id = &project.project_id;
    let initial_resources = project.resources.clone();
    catalog.rename(id, "Family album").unwrap();
    drop(catalog);
    let catalog = ProjectCatalog::open(temp.path()).unwrap();
    assert_eq!(
        catalog.get(id).unwrap().unwrap().execution_default,
        Some(original.clone())
    );

    let mut invalid = original.clone();
    invalid.path = Some("relative".into());
    assert!(matches!(
        catalog.set_execution_default(id, Some(invalid)),
        Err(ProjectError::InvalidExecutionDefault(_))
    ));
    assert_eq!(
        catalog.get(id).unwrap().unwrap().execution_default,
        Some(original)
    );
    let windows: ExecutionDirectorySelection = serde_json::from_value(json!({
        "device_id": "desktop", "path": r"D:\Photos"
    }))
    .unwrap();
    let changed = catalog
        .set_execution_default(id, Some(windows.clone()))
        .unwrap();
    assert_eq!(changed.resources, initial_resources);
    assert_eq!(changed.execution_default, Some(windows));
    let managed: ExecutionDirectorySelection =
        serde_json::from_value(json!({"device_id":"phone"})).unwrap();
    assert_eq!(
        catalog
            .set_execution_default(id, Some(managed.clone()))
            .unwrap()
            .execution_default,
        Some(managed)
    );
    catalog.set_execution_default(id, None).unwrap();
    drop(catalog);
    let cleared = ProjectCatalog::open(temp.path())
        .unwrap()
        .get(id)
        .unwrap()
        .unwrap();
    assert_eq!(cleared.resources, initial_resources);
    assert_eq!(cleared.display_name, "Family album");
    assert!(cleared.execution_default.is_none());
}
