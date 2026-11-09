use rua_project::ProjectResources;

#[test]
fn explicit_rebinding_preserves_identity_and_unknown_fields_after_directory_move() {
    let temp = tempfile::tempdir().unwrap();
    let old = temp.path().join("copal-rust");
    let new = temp.path().join("rua");
    std::fs::create_dir(&old).unwrap();
    std::fs::write(old.join("user-file"), "keep").unwrap();
    let mut resources = ProjectResources::create(std::slice::from_ref(&old), 0).unwrap();
    resources.directories[0]
        .extra
        .insert("future".into(), serde_json::json!(true));
    std::fs::rename(&old, &new).unwrap();
    let updated = resources
        .edited_with_directory_ids(std::slice::from_ref(&new), 0, &[Some("initial".into())])
        .unwrap();
    assert_eq!(updated.primary_directory_id, "initial");
    assert_eq!(updated.directory("initial").unwrap().resource_root, new);
    assert_eq!(updated.directories[0].extra, resources.directories[0].extra);
    assert_eq!(updated.revision, 2);
    assert_eq!(
        std::fs::read_to_string(new.join("user-file")).unwrap(),
        "keep"
    );
    assert!(!old.exists());
    assert!(resources
        .edited_with_directory_ids(&[old], 0, &[Some("initial".into())])
        .is_err());
    assert!(resources
        .edited_with_directory_ids(std::slice::from_ref(&new), 0, &[Some("unknown".into())])
        .is_err());
    assert!(resources
        .edited_with_directory_ids(std::slice::from_ref(&new), 0, &[None, None])
        .is_err());
    assert!(resources
        .edited_with_directory_ids(
            &[new, temp.path().to_path_buf()],
            0,
            &[Some("initial".into()), Some("initial".into())]
        )
        .is_err());
    assert_eq!(resources.revision, 1);
}

#[test]
fn directory_collection_preserves_individual_roots_and_primary_after_reload() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    let docs = temp.path().join("docs");
    std::fs::create_dir(&app).unwrap();
    std::fs::create_dir(&docs).unwrap();
    let resources = ProjectResources::create(&[app.clone(), docs.clone()], 1).unwrap();
    assert_eq!(resources.primary().unwrap().resource_root, docs);
    assert_eq!(resources.directories.len(), 2);
    assert_eq!(resources.directories[0].resource_root, app);
    let saved = serde_json::to_vec(&resources).unwrap();
    let restored: ProjectResources = serde_json::from_slice(&saved).unwrap();
    assert_eq!(resources, restored);
    assert_ne!(
        resources.directories[0].directory_id,
        resources.directories[1].directory_id
    );
    assert!(resources.directory("missing").is_err());
}

#[test]
fn duplicate_normalized_directories_and_invalid_primary_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    assert!(ProjectResources::create(&[root.clone(), root.join(".")], 0).is_err());
    assert!(ProjectResources::create(&[root], 1).is_err());
    assert!(ProjectResources::create(&[], 0).is_err());
}

#[test]
fn migration_preserves_offline_path_and_directory_identity() {
    let missing = std::env::temp_dir().join("rua-missing-resource-for-migration");
    let first = ProjectResources::from_legacy_root(missing.clone());
    let second = ProjectResources::from_legacy_root(missing.clone());
    assert_eq!(first.primary_directory_id, second.primary_directory_id);
    assert_eq!(first.primary().unwrap().resource_root, missing);
    first.validate().unwrap();
}

#[test]
fn resource_collection_preserves_unknown_fields() {
    let resources = ProjectResources::from_legacy_root(std::env::temp_dir());
    let mut value = serde_json::to_value(&resources).unwrap();
    value["future_policy"] = serde_json::json!({"enabled":true});
    value["directories"][0]["future_label"] = serde_json::json!("docs");
    let restored: ProjectResources = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(restored).unwrap(), value);
}

#[test]
fn multiple_directories_are_committed_and_reloaded_as_one_binding() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    let docs = temp.path().join("docs");
    std::fs::create_dir(&app).unwrap();
    std::fs::create_dir(&docs).unwrap();
    let store_root = temp.path().join("store");
    let stores = rua_project::ProjectStores::open(&store_root).unwrap();
    let created = stores
        .catalog
        .create_resources(rua_project::ProjectCreateResourcesRequest {
            resource_device_id: "local".into(),
            resource_roots: vec![app.clone(), docs.clone()],
            primary_index: 1,
            display_name: Some("Product".into()),
        })
        .unwrap();
    let id = created.project.project_id;
    drop(stores);
    let stores = rua_project::ProjectStores::open(&store_root).unwrap();
    let binding = stores.bindings.get(&id).unwrap().unwrap();
    assert_eq!(binding.primary_root(), docs);
    assert_eq!(binding.resources.directories.len(), 2);
    assert_eq!(binding.resources.directories[0].resource_root, app);
    assert!(serde_json::to_value(binding)
        .unwrap()
        .get("resource_root")
        .is_none());
}

#[test]
fn store_migrates_legacy_binding_atomically_even_when_directory_is_missing() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let root = temp.path().join("data");
    let stores = rua_project::ProjectStores::open(&root).unwrap();
    let created = stores
        .catalog
        .create_bound(rua_project::ProjectCreateRequest {
            resource_device_id: "device".into(),
            resource_root: repo.to_string_lossy().into(),
            display_name: None,
        })
        .unwrap();
    drop(stores);
    let record_path = std::fs::read_dir(root.join("records"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
    record["schema_version"] = serde_json::json!(2);
    record["binding"]
        .as_object_mut()
        .unwrap()
        .remove("resources");
    record["binding"]["resource_root"] = serde_json::json!(repo);
    record["binding"]["future_binding"] = serde_json::json!("keep");
    record["descriptor"]
        .as_object_mut()
        .unwrap()
        .remove("resources");
    rua_config::atomic_write(&record_path, &serde_json::to_vec(&record).unwrap()).unwrap();
    std::fs::remove_dir(&repo).unwrap();
    let stores = rua_project::ProjectStores::open(&root).unwrap();
    let id = created.project.project_id;
    let binding = stores.bindings.get(&id).unwrap().unwrap();
    assert_eq!(binding.primary_root(), repo);
    assert_eq!(
        binding.resources.primary_directory_id,
        rua_project::INITIAL_PROJECT_DIRECTORY_ID
    );
    assert_eq!(binding.extra["future_binding"], "keep");
    drop(stores);
    let after: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
    assert_eq!(after["schema_version"], 3);
    assert!(after["binding"].get("resource_root").is_none());
    let stores = rua_project::ProjectStores::open(&root).unwrap();
    assert_eq!(stores.bindings.get(&id).unwrap().unwrap(), binding);
}

#[test]
fn unsupported_future_schema_is_rejected_without_rewriting_it() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    std::fs::create_dir_all(root.join("records")).unwrap();
    let record_path = root.join("records/future.json");
    let bytes = br#"{"schema_version":99,"future_record":"keep"}"#;
    rua_config::atomic_write(&record_path, bytes).unwrap();
    assert!(rua_project::ProjectStores::open(&root).is_err());
    assert_eq!(std::fs::read(record_path).unwrap(), bytes);
}

#[test]
fn admissions_and_resource_transitions_exclude_each_other_until_guard_drops() {
    let temp = tempfile::tempdir().unwrap();
    let stores = rua_project::ProjectStores::open(temp.path().join("data")).unwrap();
    let project = stores
        .catalog
        .create_bound(rua_project::ProjectCreateRequest {
            resource_device_id: "device".into(),
            resource_root: temp.path().to_string_lossy().into(),
            display_name: None,
        })
        .unwrap()
        .project;
    let admission = stores.catalog.begin_admission(&project.project_id).unwrap();
    assert!(stores
        .catalog
        .begin_lifecycle_transition(&project.project_id)
        .is_err());
    drop(admission);
    stores
        .catalog
        .begin_lifecycle_transition(&project.project_id)
        .unwrap();
    assert!(stores.catalog.begin_admission(&project.project_id).is_err());
    stores.catalog.end_lifecycle_transition(&project.project_id);
    assert!(stores.catalog.begin_admission(&project.project_id).is_ok());
}

#[test]
fn editing_preserves_ids_and_unknown_fields_and_never_reuses_initial_identity() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    let c = temp.path().join("c");
    for path in [&a, &b, &c] {
        std::fs::create_dir(path).unwrap();
    }
    let mut resources = ProjectResources::create(&[a.clone(), b.clone()], 0).unwrap();
    resources.directories[1]
        .extra
        .insert("future".into(), serde_json::json!(true));
    let updated = resources.edited(&[c.clone(), b.clone()], 1).unwrap();
    assert_eq!(updated.directories[1], resources.directories[1]);
    assert_ne!(updated.directories[0].directory_id, "initial");
    assert_eq!(updated.revision, 2);
    assert_eq!(
        updated.primary_directory_id,
        resources.directories[1].directory_id
    );
    assert!(resources.edited(&[], 0).is_err());
    assert!(resources.edited(&[a.clone(), a], 0).is_err());
}

#[test]
fn guarded_commit_preserves_checked_directory_ids_and_releases_failed_edits() {
    let temp = tempfile::tempdir().unwrap();
    let stores = rua_project::ProjectStores::open(temp.path().join("data")).unwrap();
    let first = temp.path().join("first");
    let second = temp.path().join("second");
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    let project = stores
        .catalog
        .create_bound(rua_project::ProjectCreateRequest {
            resource_device_id: "device".into(),
            resource_root: first.to_string_lossy().into(),
            display_name: None,
        })
        .unwrap()
        .project;
    let id = &project.project_id;
    let binding = stores.bindings.get(id).unwrap().unwrap();
    let candidate = binding.resources.edited(&[first, second], 1).unwrap();
    let guard = stores.catalog.begin_resource_edit(id).unwrap();
    assert!(stores.catalog.begin_admission(id).is_err());
    guard.commit_resources(1, candidate.clone()).unwrap();
    assert_eq!(
        stores.bindings.get(id).unwrap().unwrap().resources,
        candidate
    );
    drop(stores.catalog.begin_admission(id).unwrap());
    let stale = stores.catalog.begin_resource_edit(id).unwrap();
    assert!(stale.commit_resources(1, candidate).is_err());
    drop(stores.catalog.begin_admission(id).unwrap());
    drop(stores.catalog.begin_resource_edit(id).unwrap());
    assert!(stores.catalog.begin_admission(id).is_ok());
    let reopened = rua_project::ProjectStores::open(temp.path().join("data")).unwrap();
    assert_eq!(
        reopened.bindings.get(id).unwrap(),
        stores.bindings.get(id).unwrap()
    );
}
