use std::path::{Path, PathBuf};

use rua_skills::{build_signature, is_ignored_skill_path, IGNORED_ARTIFACTS};

fn fixture_dir(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

#[test]
fn ignored_artifacts_list_is_complete() {
    for name in [
        "__pycache__",
        "__MACOSX",
        ".DS_Store",
        "Thumbs.db",
        "desktop.ini",
    ] {
        assert!(
            IGNORED_ARTIFACTS.contains(&name),
            "expected {name} to be ignored"
        );
    }
}

#[test]
fn is_ignored_skill_path_matches_any_component() {
    assert!(is_ignored_skill_path(Path::new("__pycache__")));
    assert!(is_ignored_skill_path(Path::new(
        "scripts/__pycache__/cache.pyc"
    )));
    assert!(is_ignored_skill_path(Path::new(".DS_Store")));
    assert!(is_ignored_skill_path(Path::new("nested/Thumbs.db")));

    assert!(!is_ignored_skill_path(Path::new("SKILL.md")));
    assert!(!is_ignored_skill_path(Path::new("scripts/run.py")));
}

#[test]
fn signature_is_deterministic_for_same_dir() {
    let dir = fixture_dir("office_skill");
    let s1 = build_signature(&dir).unwrap();
    let s2 = build_signature(&dir).unwrap();
    assert_eq!(s1, s2);

    assert_eq!(s1.len(), 64);
    assert!(s1.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn signature_changes_when_content_changes() {
    let original = fixture_dir("office_skill");
    let s_original = build_signature(&original).unwrap();

    let tmp = tempfile::tempdir().unwrap();
    copy_dir_all(&original, tmp.path()).unwrap();
    let mutated_md = tmp.path().join("SKILL.md");
    let mut content = std::fs::read_to_string(&mutated_md).unwrap();
    content.push_str("\nappended trailing line\n");
    std::fs::write(&mutated_md, content).unwrap();

    let s_mutated = build_signature(tmp.path()).unwrap();
    assert_ne!(s_original, s_mutated);
}

#[test]
fn signature_ignores_ds_store_and_pycache_artifacts() {
    let original = fixture_dir("office_skill");
    let s_original = build_signature(&original).unwrap();

    let tmp = tempfile::tempdir().unwrap();
    copy_dir_all(&original, tmp.path()).unwrap();

    std::fs::write(tmp.path().join(".DS_Store"), b"binary garbage").unwrap();

    let pyc_dir = tmp.path().join("scripts").join("__pycache__");
    std::fs::create_dir_all(&pyc_dir).unwrap();
    std::fs::write(pyc_dir.join("recalc.cpython-312.pyc"), b"compiled").unwrap();

    std::fs::write(tmp.path().join("Thumbs.db"), b"x").unwrap();
    std::fs::write(tmp.path().join("desktop.ini"), b"y").unwrap();

    let s_with_artifacts = build_signature(tmp.path()).unwrap();
    assert_eq!(
        s_original, s_with_artifacts,
        "signature must ignore OS artifacts"
    );
}

#[test]
fn signature_of_empty_dir_is_sha256_of_empty_input() {
    let tmp = tempfile::tempdir().unwrap();
    let s = build_signature(tmp.path()).unwrap();

    assert_eq!(
        s,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[test]
fn signature_orders_files_deterministically_regardless_of_creation_order() {
    let tmp_a = tempfile::tempdir().unwrap();
    std::fs::write(tmp_a.path().join("a.txt"), b"A").unwrap();
    std::fs::write(tmp_a.path().join("b.txt"), b"B").unwrap();
    let s_a = build_signature(tmp_a.path()).unwrap();

    let tmp_b = tempfile::tempdir().unwrap();
    std::fs::write(tmp_b.path().join("b.txt"), b"B").unwrap();
    std::fs::write(tmp_b.path().join("a.txt"), b"A").unwrap();
    let s_b = build_signature(tmp_b.path()).unwrap();

    assert_eq!(s_a, s_b);
}

#[test]
fn signature_distinguishes_path_from_content() {
    let tmp_a = tempfile::tempdir().unwrap();
    std::fs::write(tmp_a.path().join("a.txt"), b"hello").unwrap();
    let tmp_b = tempfile::tempdir().unwrap();
    std::fs::write(tmp_b.path().join("b.txt"), b"hello").unwrap();
    assert_ne!(
        build_signature(tmp_a.path()).unwrap(),
        build_signature(tmp_b.path()).unwrap()
    );
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}
