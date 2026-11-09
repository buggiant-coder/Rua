use rua_project::{
    create_project_directory, list_project_directory, normalize_project_root,
    ProjectDirectoryEntryKind, ProjectError,
};

#[cfg(unix)]
#[test]
fn absolute_directory_browsing_survives_deleted_process_working_directory() {
    const CHILD: &str = "RUA_TEST_DELETED_PROJECT_CWD";
    if let Some(root) = std::env::var_os(CHILD) {
        let root = std::path::PathBuf::from(root);
        let cwd = root.join("deleted");
        std::fs::create_dir(&cwd).unwrap();
        std::env::set_current_dir(&cwd).unwrap();
        std::fs::remove_dir(&cwd).unwrap();
        assert!(std::env::current_dir().is_err());
        assert_eq!(
            list_project_directory(&root.to_string_lossy())
                .unwrap()
                .path,
            root
        );
        assert!(list_project_directory("~").is_ok());
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "path::absolute_directory_browsing_survives_deleted_process_working_directory",
            "--nocapture",
        ])
        .env(CHILD, temp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn create_directory_selects_child_and_never_overwrites_existing_work() {
    let temp = tempfile::tempdir().unwrap();
    let listing = create_project_directory(&temp.path().to_string_lossy(), "新项目").unwrap();
    assert_eq!(listing.path, temp.path().join("新项目"));
    assert!(listing.entries.is_empty());
    let file = listing.path.join("notes.txt");
    std::fs::write(&file, "user work").unwrap();
    assert!(create_project_directory(&temp.path().to_string_lossy(), "新项目").is_err());
    assert_eq!(std::fs::read_to_string(file).unwrap(), "user work");
}

#[test]
fn create_directory_rejects_traversal_and_missing_parent() {
    let temp = tempfile::tempdir().unwrap();
    for name in [
        "",
        " ",
        ".",
        "..",
        "../escape",
        "/absolute",
        "nested/child",
        "nested\\child",
        "bad\0name",
    ] {
        assert!(matches!(
            create_project_directory(&temp.path().to_string_lossy(), name),
            Err(ProjectError::InvalidPath(_))
        ));
    }
    assert!(
        create_project_directory(&temp.path().join("missing").to_string_lossy(), "child").is_err()
    );
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn directory_listing_skips_broken_symlink_entries() {
    let temp = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(temp.path().join("missing"), temp.path().join("broken")).unwrap();
    std::fs::create_dir(temp.path().join("visible")).unwrap();
    let listing = list_project_directory(&temp.path().to_string_lossy()).unwrap();
    assert_eq!(listing.entries.len(), 1);
    assert_eq!(listing.entries[0].name, "visible");
}

#[test]
fn missing_and_file_paths_are_rejected_as_directories() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("file.txt");
    std::fs::write(&file, b"x").unwrap();

    assert!(matches!(
        list_project_directory(&file.to_string_lossy()),
        Err(ProjectError::DirectoryUnavailable { .. })
    ));
    assert!(matches!(
        list_project_directory(&temp.path().join("missing").to_string_lossy()),
        Err(ProjectError::DirectoryUnavailable { .. })
    ));
}

#[test]
fn directory_listing_follows_symlink_directories_without_canonicalizing_identity() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target");
    let child = target.join("child");
    std::fs::create_dir_all(&child).unwrap();
    let alias = temp.path().join("alias");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &alias).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&target, &alias).unwrap();

    let normalized = normalize_project_root(&alias.to_string_lossy()).unwrap();
    let listing = list_project_directory(&alias.to_string_lossy()).unwrap();

    assert_eq!(normalized, alias);
    assert_eq!(listing.path, alias);
    assert_eq!(listing.entries[0].name, "child");
    assert_eq!(
        listing.entries[0].kind,
        ProjectDirectoryEntryKind::Directory
    );
}

#[cfg(unix)]
#[test]
fn inaccessible_directory_is_rejected() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("private");
    std::fs::create_dir(&directory).unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o000)).unwrap();

    let result = list_project_directory(&directory.to_string_lossy());

    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(matches!(
        result,
        Err(ProjectError::DirectoryUnavailable { .. })
    ));
}
