use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};

use rua_project::{
    ProjectCatalog, ProjectCreateResourcesRequest, ProjectError, ProjectResourceDirectoryInput,
    ProjectResources,
};
use serde_json::json;

fn input(device: &str, path: impl AsRef<Path>) -> ProjectResourceDirectoryInput {
    ProjectResourceDirectoryInput {
        directory_id: None,
        device_id: device.into(),
        resource_root: path.as_ref().to_owned(),
        display_name: None,
        extra: Default::default(),
    }
}

#[test]
fn device_and_root_are_the_address_and_only_a_rename_keeps_the_id() {
    let pc = input("pc", "/photos");
    let phone = input("phone", "/photos");
    let mut resources = ProjectResources::create(vec![pc.clone(), phone.clone()]).unwrap();
    let id = resources.directories[1].directory_id.clone();
    resources.directories[1]
        .extra
        .insert("unknown".into(), json!(true));
    resources.extra.insert("collection_field".into(), json!(7));
    let mut renamed = phone;
    renamed.directory_id = Some(id.clone());
    renamed.display_name = Some("Camera".into());
    let updated = resources.edited(vec![renamed.clone(), pc]).unwrap();
    assert_eq!(updated.directory(&id).unwrap().extra["unknown"], true);
    assert_eq!(
        updated.directory(&id).unwrap().display_name.as_deref(),
        Some("Camera")
    );
    assert_eq!(updated.extra, resources.extra);
    assert_eq!(updated.revision, 2);
    let mut moved = renamed.clone();
    moved.resource_root = PathBuf::from("/archive");
    let replaced = updated.edited(vec![moved]).unwrap();
    assert!(replaced.directory(&id).is_err());
    assert_eq!(
        resources.directory(&id).unwrap().resource_root,
        Path::new("/photos")
    );
    renamed.device_id = "tablet".into();
    let moved_device = updated.edited(vec![renamed]).unwrap();
    assert!(moved_device.directory(&id).is_err());
    assert_eq!(moved_device.directories[0].device_id, "tablet");
}

#[test]
fn malformed_ids_duplicate_locations_and_relative_paths_are_rejected_atomically() {
    let a = input("pc", "/a");
    assert!(ProjectResources::create(vec![a.clone(), a.clone()]).is_err());
    for invalid in ["", "relative", "C:relative", r"\relative", r"\\server"] {
        assert!(ProjectResources::create(vec![input("pc", invalid)]).is_err());
    }
    for valid in [
        "/storage/emulated/0",
        r"C:\Users\me\Pictures",
        r"\\nas\photos",
    ] {
        assert!(ProjectResources::create(vec![input("device", valid)]).is_ok());
    }
    assert!(ProjectResources::create(vec![input("", "/a")]).is_err());
    let base = ProjectResources::create(vec![a.clone()]).unwrap();
    let mut b = input("pc", "/b");
    b.directory_id = Some("unknown".into());
    assert!(base.edited(vec![b.clone()]).is_err());
    b.directory_id = Some(base.directories[0].directory_id.clone());
    let mut c = b.clone();
    c.resource_root = "/c".into();
    assert!(base.edited(vec![b, c]).is_err());
    assert_eq!(base.revision, 1);
}

#[test]
fn empty_projects_and_offline_metadata_edits_survive_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("catalog");
    let catalog = ProjectCatalog::open(&root).unwrap();
    let project = catalog
        .create_resources(ProjectCreateResourcesRequest {
            execution_default: None,
            display_name: Some("Family Photos".into()),
            directories: vec![],
        })
        .unwrap()
        .project;
    let id = &project.project_id;
    assert_eq!(project.resources, ProjectResources::default());
    let offline = input("phone", "/device-only/dcim");
    let added = catalog
        .replace_resources(id, 1, vec![offline.clone()])
        .unwrap()
        .project;
    let mut renamed = offline;
    renamed.directory_id = Some(added.resources.directories[0].directory_id.clone());
    renamed.display_name = Some("DCIM".into());
    let admission = catalog.begin_admission(id).unwrap();
    let next = catalog
        .replace_resources(id, 2, vec![renamed])
        .unwrap()
        .project;
    assert_eq!(
        next.resources.directories[0].directory_id,
        added.resources.directories[0].directory_id
    );
    let removed = catalog.replace_resources(id, 3, vec![]).unwrap().project;
    assert!(removed.resources.directories.is_empty());
    assert_eq!(removed.resources.revision, 4);
    assert!(matches!(
        catalog.replace_resources(id, 3, vec![]),
        Err(ProjectError::ResourceRevisionConflict)
    ));
    drop(admission);
    drop(catalog);
    let reopened = ProjectCatalog::open(root).unwrap();
    assert_eq!(reopened.get(id).unwrap(), Some(removed));
}

#[test]
fn concurrent_edits_commit_once_and_loser_can_retry() {
    let temp = tempfile::tempdir().unwrap();
    let catalog = Arc::new(ProjectCatalog::open(temp.path()).unwrap());
    let project = catalog
        .create_resources(ProjectCreateResourcesRequest {
            execution_default: None,
            display_name: Some("Snapshots".into()),
            directories: vec![],
        })
        .unwrap()
        .project;
    let barrier = Arc::new(Barrier::new(2));
    let workers = ["a", "b"].map(|device| {
        let catalog = catalog.clone();
        let id = project.project_id.clone();
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            barrier.wait();
            catalog.replace_resources(&id, 1, vec![input(device, "/photos")])
        })
    });
    let results = workers.map(|worker| worker.join().unwrap());
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(ProjectError::ResourceRevisionConflict)))
            .count(),
        1
    );
    let saved = catalog.get(&project.project_id).unwrap().unwrap();
    assert_eq!(saved.resources.directories.len(), 1);
    catalog
        .replace_resources(&project.project_id, saved.resources.revision, vec![])
        .unwrap();
}

#[test]
fn legacy_and_future_records_are_left_intact_for_explicit_migration() {
    for schema in [2, 3, 99] {
        let temp = tempfile::tempdir().unwrap();
        let records = temp.path().join("records");
        std::fs::create_dir(&records).unwrap();
        let file = records.join("project.json");
        let bytes =
            serde_json::to_vec(&json!({"schema_version":schema,"binding":{"future_key":7}}))
                .unwrap();
        rua_config::atomic_write(&file, &bytes).unwrap();
        assert!(ProjectCatalog::open(temp.path()).is_err());
        assert_eq!(std::fs::read(file).unwrap(), bytes);
    }
}

#[test]
fn a_replica_cannot_redirect_an_existing_association_id() {
    let temp = tempfile::tempdir().unwrap();
    let catalog = ProjectCatalog::open(temp.path()).unwrap();
    let project = catalog
        .create_resources(ProjectCreateResourcesRequest {
            execution_default: None,
            display_name: Some("Family".into()),
            directories: vec![input("phone", "/Photos")],
        })
        .unwrap()
        .project;
    let mut changed = project.clone();
    changed.resources.revision += 1;
    changed.resources.directories[0].device_id = "desktop".into();
    assert!(matches!(
        catalog.apply_descriptor(changed),
        Err(ProjectError::InvalidResources(_))
    ));
    assert_eq!(catalog.get(&project.project_id).unwrap().unwrap(), project);
}

#[test]
fn overlap_uses_the_resource_device_and_its_path_component_boundaries() {
    let temp = tempfile::tempdir().unwrap();
    let catalog = ProjectCatalog::open(temp.path()).unwrap();
    let create = |device: &str, path: &str| {
        catalog
            .create_resources(ProjectCreateResourcesRequest {
                execution_default: None,
                display_name: Some("Photo collection".into()),
                directories: vec![input(device, path)],
            })
            .unwrap()
    };
    let parent = create("windows", r"C:\Users\photos");
    let child = create("windows", r"C:\Users\photos\2026");
    assert_eq!(child.overlap_warnings.len(), 1);
    assert_eq!(
        child.overlap_warnings[0].project_id,
        parent.project.project_id
    );
    assert!(create("second-pc", r"C:\Users\photos\2026")
        .overlap_warnings
        .is_empty());
    assert!(create("windows", r"C:\Users\photos-private")
        .overlap_warnings
        .is_empty());
}
