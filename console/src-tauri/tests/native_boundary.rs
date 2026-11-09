use rua_desktop::native::{atomic_write, validate_external_url, validate_suggested_name};

#[test]
fn external_link_validation_accepts_only_supported_absolute_schemes() {
    for url in [
        "https://rua.example/docs",
        "http://127.0.0.1:8765/console/",
        "mailto:support@example.com",
        "tel:+12025550123",
    ] {
        assert!(validate_external_url(url).is_ok(), "url={url}");
    }
}

#[test]
fn external_link_validation_rejects_malformed_or_unsafe_inputs() {
    for url in [
        "",
        " https://rua.example",
        "https://rua.example\n",
        "javascript:alert(1)",
        "data:text/html,boom",
        "file:///tmp/secret",
        "http:example.com",
        "https://user:secret@example.com",
        "mailto:",
        "tel:",
    ] {
        assert!(validate_external_url(url).is_err(), "url={url:?}");
    }
}

#[test]
fn suggested_export_name_cannot_smuggle_a_path() {
    for name in ["report.md", "export.json", "archive.zip"] {
        assert!(validate_suggested_name(name).is_ok(), "name={name}");
    }
    for name in ["", "../secret", "folder/file", " folder.txt", "bad\nname"] {
        assert!(validate_suggested_name(name).is_err(), "name={name:?}");
    }
}

#[test]
fn selected_file_write_atomically_replaces_existing_content() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("export.txt");
    std::fs::write(&path, "old").expect("seed file");

    atomic_write(&path, b"new content").expect("atomic write");

    assert_eq!(std::fs::read(&path).expect("read file"), b"new content");
    assert!(
        std::fs::read_dir(temp.path())
            .expect("read tempdir")
            .all(|entry| !entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .contains(".tmp.")),
        "temporary export file must be removed"
    );
}

#[test]
fn resource_directory_open_requires_an_existing_absolute_directory() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("file");
    std::fs::write(&file, "data").unwrap();
    assert!(rua_desktop::native::validate_project_resource_directory("relative").is_err());
    assert!(
        rua_desktop::native::validate_project_resource_directory(file.to_str().unwrap()).is_err()
    );
    assert_eq!(
        rua_desktop::native::validate_project_resource_directory(temp.path().to_str().unwrap())
            .unwrap(),
        temp.path().canonicalize().unwrap()
    );
}
