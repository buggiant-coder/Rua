use rua_core::desktop_cli::DesktopCliInstallation;
use serde_json::json;

#[test]
fn installs_exact_unified_cli_preserves_future_metadata_and_rejects_unowned_files() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("packaged rua");
    std::fs::write(&source, b"unified-runtime-cli").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let install = DesktopCliInstallation::new(source.clone(), temp.path());
    let pending = install.status();
    assert!(!pending.installed && !pending.conflict);
    let ready = install.install().unwrap();
    assert!(ready.installed && !ready.conflict);
    assert_eq!(std::fs::read(&ready.path).unwrap(), b"unified-runtime-cli");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&ready.path).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }
    let marker = temp
        .path()
        .join(".local/share/rua/desktop-cli-install.json");
    std::fs::write(
        &marker,
        serde_json::to_vec(&json!({"version":1,"destination":ready.path,"future":{"keep":true}}))
            .unwrap(),
    )
    .unwrap();
    install.install().unwrap();
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&marker).unwrap()).unwrap();
    assert_eq!(saved["future"], json!({"keep":true}));
    std::fs::remove_file(&marker).unwrap();
    assert!(install.status().conflict);
    let foreign = temp.path().join("different-rua");
    std::fs::write(&foreign, b"must-not-overwrite").unwrap();
    let conflicting = DesktopCliInstallation::new(foreign, temp.path());
    assert!(conflicting.install().is_err());
    assert_eq!(std::fs::read(&ready.path).unwrap(), b"unified-runtime-cli");
}

#[cfg(unix)]
#[test]
fn a_dangling_unowned_cli_symlink_is_a_conflict() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    std::fs::write(&source, b"cli").unwrap();
    let install = DesktopCliInstallation::new(source, temp.path());
    let path = install.status().path;
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(temp.path().join("missing"), &path).unwrap();
    assert!(install.status().conflict);
    assert!(install.install().is_err());
    assert!(path.symlink_metadata().unwrap().file_type().is_symlink());
}

#[cfg(target_os = "macos")]
#[test]
fn macos_cli_keeps_bundled_drivers_beside_the_runtime() {
    let temp = tempfile::tempdir().unwrap();
    let bundle = temp.path().join("Rua.app/Contents/MacOS");
    std::fs::create_dir_all(&bundle).unwrap();
    let source = bundle.join("rua");
    std::fs::write(&source, b"unified-rua").unwrap();
    for name in ["agent-browser", "cua-driver"] {
        std::fs::write(bundle.join(name), name).unwrap();
    }
    let install = DesktopCliInstallation::new(source.clone(), &temp.path().join("user"));
    let status = install.install().unwrap();
    let actual = status.path.canonicalize().unwrap();
    assert_eq!(actual, source.canonicalize().unwrap());
    assert!(actual.with_file_name("agent-browser").is_file());
    assert!(actual.with_file_name("cua-driver").is_file());
    std::fs::remove_file(&source).unwrap();
    assert!(
        !install.status().installed,
        "a moved app must allow reinstalling the CLI link"
    );
    assert!(
        !install.status().conflict,
        "a broken Rua-owned link is repairable"
    );
}
