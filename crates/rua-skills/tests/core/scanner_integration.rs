use std::io::{Cursor, Write};
use std::sync::Arc;

use rua_skills::scanner_integration::InMemoryScannerHandle;
use rua_skills::{SkillError, SkillPoolService, SkillService};
use tempfile::TempDir;
use zip::write::{SimpleFileOptions, ZipWriter};
use zip::CompressionMethod;

const SAMPLE_SKILL: &str = "---
name: demo
description: demo skill for scanner integration tests
---

# Body
";

fn allow_handle() -> Arc<InMemoryScannerHandle> {
    Arc::new(InMemoryScannerHandle::allow())
}

fn block_handle() -> Arc<InMemoryScannerHandle> {
    Arc::new(InMemoryScannerHandle::block_with(
        "CRITICAL",
        "test-induced block for demo skill",
        vec!["[CRITICAL] dummy-finding".to_string()],
    ))
}

fn workspace() -> TempDir {
    tempfile::tempdir().expect("workspace tempdir")
}

fn build_zip_with(skill_name: &str, content: &str) -> Vec<u8> {
    let buf: Vec<u8> = Vec::new();
    let mut zw = ZipWriter::new(Cursor::new(buf));
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    zw.start_file(format!("{skill_name}/SKILL.md"), opts)
        .unwrap();
    zw.write_all(content.as_bytes()).unwrap();
    let cursor = zw.finish().unwrap();
    cursor.into_inner()
}

#[test]
fn service_create_skill_blocks_when_scanner_rejects() {
    let dir = workspace();
    let scanner = block_handle();
    let svc = SkillService::new(dir.path().to_path_buf())
        .unwrap()
        .with_scanner(scanner.clone() as Arc<_>);

    let err = svc
        .create_skill("demo", SAMPLE_SKILL, None, false)
        .expect_err("scanner block must surface as Err");

    match &err {
        SkillError::ScanBlocked {
            skill_name,
            findings,
            max_severity,
            ..
        } => {
            assert_eq!(skill_name, "demo");
            assert_eq!(max_severity, "CRITICAL");
            assert!(!findings.is_empty());
        }
        other => panic!("expected ScanBlocked, got {other:?}"),
    }

    let installed = dir.path().join("skills").join("demo");
    assert!(!installed.exists(), "blocked skill leaked at {installed:?}");

    let calls = scanner.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].skill_name, "demo");
}

#[test]
fn service_create_skill_allows_when_scanner_accepts() {
    let dir = workspace();
    let scanner = allow_handle();
    let svc = SkillService::new(dir.path().to_path_buf())
        .unwrap()
        .with_scanner(scanner.clone() as Arc<_>);

    let created = svc
        .create_skill("demo", SAMPLE_SKILL, None, true)
        .expect("create_skill must succeed when scanner allows");
    assert_eq!(created.as_deref(), Some("demo"));

    let installed = dir.path().join("skills").join("demo").join("SKILL.md");
    assert!(installed.exists());

    let calls = scanner.calls();
    assert_eq!(calls.len(), 1);
}

#[test]
fn service_save_skill_in_place_blocks_on_scanner() {
    let dir = workspace();

    let svc_setup = SkillService::new(dir.path().to_path_buf()).unwrap();
    svc_setup
        .create_skill("demo", SAMPLE_SKILL, None, false)
        .expect("bootstrap create");

    let scanner = block_handle();
    let svc = SkillService::new(dir.path().to_path_buf())
        .unwrap()
        .with_scanner(scanner.clone() as Arc<_>);

    let new_md = format!("{SAMPLE_SKILL}\n## new section\n");
    let err = svc
        .save_skill("demo", &new_md, None, None, false)
        .expect_err("save_skill must propagate scanner block");
    assert!(matches!(err, SkillError::ScanBlocked { .. }));

    let on_disk =
        std::fs::read_to_string(dir.path().join("skills").join("demo").join("SKILL.md")).unwrap();
    assert_eq!(on_disk, SAMPLE_SKILL);
    assert_eq!(scanner.calls().len(), 1);
}

#[test]
fn service_enable_skill_blocks_on_scanner() {
    let dir = workspace();
    let svc_setup = SkillService::new(dir.path().to_path_buf()).unwrap();
    svc_setup
        .create_skill("demo", SAMPLE_SKILL, None, false)
        .expect("bootstrap create");

    let scanner = block_handle();
    let svc = SkillService::new(dir.path().to_path_buf())
        .unwrap()
        .with_scanner(scanner.clone() as Arc<_>);

    let err = svc
        .enable_skill("demo")
        .expect_err("enable_skill must re-scan and propagate block");
    assert!(matches!(err, SkillError::ScanBlocked { .. }));
    assert_eq!(scanner.calls().len(), 1);
}

#[test]
fn service_import_from_zip_blocks_on_scanner() {
    let dir = workspace();
    let scanner = block_handle();
    let svc = SkillService::new(dir.path().to_path_buf())
        .unwrap()
        .with_scanner(scanner.clone() as Arc<_>);

    let zip_bytes = build_zip_with("demo", SAMPLE_SKILL);
    let err = svc
        .import_from_zip(&zip_bytes, false)
        .expect_err("import_from_zip must propagate block");
    assert!(matches!(err, SkillError::ScanBlocked { .. }));
    assert!(!dir.path().join("skills").join("demo").exists());
    assert_eq!(scanner.calls().len(), 1);
}

#[test]
fn pool_create_skill_blocks_on_scanner() {
    let pool = workspace();
    let scanner = block_handle();
    let svc = SkillPoolService::new_in(pool.path().to_path_buf())
        .unwrap()
        .with_scanner(scanner.clone() as Arc<_>);

    let err = svc
        .create_skill(
            "demo",
            SAMPLE_SKILL,
            None,
            rua_skills::SkillSource::Customized,
        )
        .expect_err("pool create must propagate block");
    assert!(matches!(err, SkillError::ScanBlocked { .. }));

    assert!(!pool.path().join("demo").exists());
    assert_eq!(scanner.calls().len(), 1);
}

#[test]
fn pool_upload_blocks_on_scanner() {
    let workspace_dir = workspace();
    let pool = workspace();

    let ws = SkillService::new(workspace_dir.path().to_path_buf()).unwrap();
    ws.create_skill("demo", SAMPLE_SKILL, None, false).unwrap();

    let scanner = block_handle();
    let pool_svc = SkillPoolService::new_in(pool.path().to_path_buf())
        .unwrap()
        .with_scanner(scanner.clone() as Arc<_>);
    let err = pool_svc
        .upload_from_workspace(workspace_dir.path(), "demo", None, false)
        .expect_err("pool upload must propagate block");
    assert!(matches!(err, SkillError::ScanBlocked { .. }));
    assert!(!pool.path().join("demo").exists());
    assert_eq!(scanner.calls().len(), 1);
}

#[test]
fn pool_download_blocks_on_scanner_and_keeps_workspace_clean() {
    let workspace_dir = workspace();
    let pool = workspace();

    let pool_setup = SkillPoolService::new_in(pool.path().to_path_buf()).unwrap();
    pool_setup
        .create_skill(
            "demo",
            SAMPLE_SKILL,
            None,
            rua_skills::SkillSource::Customized,
        )
        .expect("seed pool skill");

    let scanner = block_handle();
    let pool_svc = SkillPoolService::new_in(pool.path().to_path_buf())
        .unwrap()
        .with_scanner(scanner.clone() as Arc<_>);

    let err = pool_svc
        .download_to_workspace(workspace_dir.path(), "demo", None, false)
        .expect_err("download must propagate scanner block");
    assert!(matches!(err, SkillError::ScanBlocked { .. }));

    let installed = workspace_dir.path().join("skills").join("demo");
    assert!(
        !installed.exists(),
        "blocked download leaked at {installed:?}"
    );

    assert_eq!(scanner.calls().len(), 1);
}

#[test]
fn no_scanner_means_no_calls_and_no_blocks() {
    let dir = workspace();
    let svc = SkillService::new(dir.path().to_path_buf()).unwrap();
    let created = svc.create_skill("demo", SAMPLE_SKILL, None, true).unwrap();
    assert_eq!(created.as_deref(), Some("demo"));
    assert!(dir
        .path()
        .join("skills")
        .join("demo")
        .join("SKILL.md")
        .exists());
}
