use rua_desktop_lib::bundle::{sha256, validate_receipt};
use rua_desktop_lib::update_bundle::{extract_macos, extract_windows_installer, DirectorySwap};
use serde_json::json;
use std::io::{Cursor, Write};
use std::path::Path;

fn file(root: &Path, path: &str, data: &[u8]) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut temporary = tempfile::NamedTempFile::new_in(path.parent().unwrap()).unwrap();
    temporary.write_all(data).unwrap();
    temporary.as_file().sync_all().unwrap();
    temporary.persist(&path).unwrap();
    #[cfg(unix)]
    std::fs::File::open(path.parent().unwrap())
        .unwrap()
        .sync_all()
        .unwrap();
}
fn set(root: &Path, version: &str) {
    for name in ["shell", "rua", "agent-browser", "cua-driver"] {
        file(root, name, format!("{version}:{name}").as_bytes());
    }
    file(root, "automation/receipt", version.as_bytes());
}
fn assert_set(root: &Path, version: &str) {
    for name in ["shell", "rua", "agent-browser", "cua-driver"] {
        assert_eq!(
            std::fs::read(root.join(name)).unwrap(),
            format!("{version}:{name}").as_bytes()
        );
    }
    assert_eq!(
        std::fs::read(root.join("automation/receipt")).unwrap(),
        version.as_bytes()
    );
}
#[test]
fn interrupted_update_restores_whole_set_and_keeps_profile_outside_bundle() {
    let temp = tempfile::tempdir().unwrap();
    let installed = temp.path().join("Rua.app");
    let candidate = temp.path().join("candidate");
    set(&installed, "old");
    set(&candidate, "new");
    file(temp.path(), "data/profile/cookies", b"login");
    {
        let _transaction = DirectorySwap::install(&installed, &candidate).unwrap();
        assert_set(&installed, "new");
        // A verifier failure/early return drops the uncommitted transaction.
    }
    assert_set(&installed, "old");
    assert_eq!(
        std::fs::read(temp.path().join("data/profile/cookies")).unwrap(),
        b"login"
    );
    set(&candidate, "new");
    DirectorySwap::install(&installed, &candidate)
        .unwrap()
        .commit()
        .unwrap();
    assert_set(&installed, "new");
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 2);
}
#[test]
fn failed_replacement_restores_files_before_reporting_failure() {
    let temp = tempfile::tempdir().unwrap();
    let installed = temp.path().join("app");
    set(&installed, "old");
    let result = DirectorySwap::install(&installed, &temp.path().join("missing"));
    assert!(result.is_err());
    assert_set(&installed, "old");
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
}
#[test]
fn release_receipt_rejects_missing_component_and_different_product_target_or_manifest() {
    let manifest = include_bytes!("../automation/manifest.json");
    let pinned: serde_json::Value = serde_json::from_slice(manifest).unwrap();
    let mut value = json!({"format":1,"target":"test-target","product_version":"1.0.0","manifest_sha256":sha256(manifest),"components":{
        "rua":{"version":"1.0.0","sha256":"a".repeat(64)},
        "agent-browser":{"version":pinned["agent_browser"]["version"],"sha256":"b".repeat(64)},
        "cua-driver":{"version":pinned["cua_driver"]["version"],"sha256":"c".repeat(64)}
    }});
    let valid = serde_json::to_vec(&value).unwrap();
    validate_receipt(&valid, manifest, "1.0.0", "test-target").unwrap();
    assert!(validate_receipt(&valid, manifest, "2.0.0", "test-target").is_err());
    assert!(validate_receipt(&valid, manifest, "1.0.0", "other-target").is_err());
    value["components"]
        .as_object_mut()
        .unwrap()
        .remove("cua-driver");
    assert!(validate_receipt(
        &serde_json::to_vec(&value).unwrap(),
        manifest,
        "1.0.0",
        "test-target"
    )
    .is_err());
}
fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    archive_with_mode(entries, 0o755)
}
fn archive_with_mode(entries: &[(&str, &[u8])], mode: u32) -> Vec<u8> {
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut tar = tar::Builder::new(encoder);
    for (name, data) in entries {
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(mode);
        h.set_cksum();
        tar.append_data(&mut h, name, *data).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap()
}
#[test]
fn archive_extraction_preserves_single_bundle_and_refuses_mixed_roots() {
    let temp = tempfile::tempdir().unwrap();
    let bytes = archive(&[("Rua.app/Contents/MacOS/rua", b"native")]);
    let root = extract_macos(&bytes, temp.path()).unwrap();
    assert_eq!(
        std::fs::read(root.join("Contents/MacOS/rua")).unwrap(),
        b"native"
    );
    let mixed = archive(&[("Rua.app/one", b"1"), ("Other.app/two", b"2")]);
    assert!(extract_macos(&mixed, temp.path()).is_err());
    assert!(extract_macos(&archive(&[]), temp.path()).is_err());
}

#[test]
fn archive_extraction_preserves_read_only_file_bytes_and_permissions() {
    let temp = tempfile::tempdir().unwrap();
    let bytes = archive_with_mode(
        &[(
            "Rua.app/Contents/Resources/read-only.txt",
            b"sealed resource",
        )],
        0o444,
    );
    let root = extract_macos(&bytes, temp.path()).unwrap();
    let path = root.join("Contents/Resources/read-only.txt");
    assert_eq!(std::fs::read(&path).unwrap(), b"sealed resource");
    assert!(std::fs::metadata(&path).unwrap().permissions().readonly());
    #[cfg(windows)]
    {
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        // Test cleanup clears the Windows file attribute, never Unix permission bits.
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        std::fs::set_permissions(path, permissions).unwrap();
    }
}
#[test]
fn windows_update_archive_accepts_one_installer_and_rejects_extra_executables() {
    let temp = tempfile::tempdir().unwrap();
    let create = |names: &[&str]| {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for name in names {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"MZ native fixture").unwrap();
        }
        zip.finish().unwrap().into_inner()
    };
    let installer = extract_windows_installer(&create(&["Rua-setup.exe"]), temp.path()).unwrap();
    assert_eq!(std::fs::read(installer).unwrap(), b"MZ native fixture");
    for names in [
        vec!["one.exe", "two.exe"],
        vec!["../outside.exe"],
        vec!["nested/setup.exe"],
    ] {
        assert!(extract_windows_installer(&create(&names), temp.path()).is_err());
    }
}

#[test]
fn signed_pe_retains_the_release_identity_but_changed_code_does_not() {
    use rua_desktop_lib::bundle::pe_image_sha256;
    let mut unsigned = vec![0u8; 512];
    unsigned[..2].copy_from_slice(b"MZ");
    unsigned[0x3c..0x40].copy_from_slice(&64u32.to_le_bytes());
    unsigned[64..68].copy_from_slice(b"PE\0\0");
    unsigned[88..90].copy_from_slice(&[0x0b, 0x02]);
    unsigned[400..405].copy_from_slice(b"code!");
    let original = pe_image_sha256(&unsigned).unwrap();
    let mut signed = unsigned.clone();
    signed[152..156].copy_from_slice(&123u32.to_le_bytes());
    signed[232..236].copy_from_slice(&512u32.to_le_bytes());
    signed[236..240].copy_from_slice(&16u32.to_le_bytes());
    signed.extend_from_slice(&[11u8; 16]);
    assert_eq!(original, pe_image_sha256(&signed).unwrap());
    signed[400] ^= 1;
    assert_ne!(original, pe_image_sha256(&signed).unwrap());
    signed.pop();
    assert!(pe_image_sha256(&signed).is_err());
    assert!(pe_image_sha256(b"MZ").is_err());
}
