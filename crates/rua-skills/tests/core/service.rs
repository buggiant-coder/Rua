use std::collections::{BTreeMap, HashMap};
use std::io::{Cursor, Write};

use rua_skills::{
    reconcile_workspace_manifest, ImportOptions, SaveOutcome, SkillError, SkillService,
};
use serde_json::{Map, Value};
use tempfile::TempDir;
use zip::write::{SimpleFileOptions, ZipWriter};
use zip::CompressionMethod;

const SAMPLE_SKILL: &str = "---
name: demo
description: demo skill for tests
tags: [demo]
emoji: \u{1f9ea}
---

# Demo body
";

fn workspace() -> (TempDir, SkillService) {
    let dir = tempfile::tempdir().expect("tempdir");
    let svc = SkillService::new(dir.path().to_path_buf()).expect("service");
    (dir, svc)
}

fn read_manifest(dir: &TempDir) -> Value {
    let bytes = std::fs::read(dir.path().join("skill.json")).expect("read manifest");
    serde_json::from_slice(&bytes).expect("parse manifest")
}

#[test]
fn create_then_lifecycle() {
    let (dir, svc) = workspace();

    let created = svc
        .create_skill("demo", SAMPLE_SKILL, None, false)
        .expect("create");
    assert_eq!(created.as_deref(), Some("demo"));

    let all = svc.list_all_skills().expect("list_all");
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].name, "demo");

    assert!(svc
        .list_available_skills()
        .expect("list_available")
        .is_empty());

    let enabled = svc.enable_skill("demo").expect("enable");
    assert!(enabled);
    let available = svc.list_available_skills().expect("list_available");
    assert_eq!(available.len(), 1);

    let err = svc.delete_skill("demo").unwrap_err();
    assert!(matches!(err, SkillError::Conflict(_)));

    let disabled = svc.disable_skill("demo").expect("disable");
    assert!(disabled);
    assert!(svc
        .list_available_skills()
        .expect("list_available")
        .is_empty());

    let removed = svc.delete_skill("demo").expect("delete");
    assert!(removed);
    assert!(svc.list_all_skills().expect("list_all").is_empty());

    let manifest = read_manifest(&dir);
    let skills = manifest.get("skills").and_then(Value::as_object).unwrap();
    assert!(!skills.contains_key("demo"));
}

#[test]
fn create_duplicate_returns_none() {
    let (_dir, svc) = workspace();
    svc.create_skill("demo", SAMPLE_SKILL, None, false)
        .expect("create");
    let dup = svc
        .create_skill("demo", SAMPLE_SKILL, None, false)
        .expect("create");
    assert!(dup.is_none(), "duplicate create should report None");
}

#[test]
fn create_skill_persists_android_runtime_scan() {
    let (dir, svc) = workspace();
    let content =
        "---\nname: linuxy\ndescription: needs apt\n---\n\nRun `apt-get install poppler-utils`.\n";

    svc.create_skill("linuxy", content, None, false)
        .expect("create");

    let manifest = read_manifest(&dir);
    assert_eq!(
        manifest
            .pointer("/skills/linuxy/metadata/android_runtime/recommendation")
            .and_then(Value::as_str),
        Some("requires_linux_compat")
    );
    assert_eq!(
        manifest
            .pointer("/skills/linuxy/metadata/android_runtime/signals/0/kind")
            .and_then(Value::as_str),
        Some("debian_package_manager")
    );
}

#[test]
fn create_rejects_missing_frontmatter() {
    let (_dir, svc) = workspace();
    let err = svc
        .create_skill("demo", "---\nname: ''\ndescription: ''\n---\n", None, false)
        .unwrap_err();
    assert!(matches!(err, SkillError::InvalidFrontmatter(_)));
}

#[test]
fn save_skill_in_place_updates_content() {
    let (_dir, svc) = workspace();
    svc.create_skill("demo", SAMPLE_SKILL, None, false)
        .expect("create");

    let new_content = "---\nname: demo\ndescription: revised\n---\n\nrevised body\n";
    let outcome = svc
        .save_skill("demo", new_content, None, None, false)
        .expect("save");
    assert!(matches!(outcome, SaveOutcome::Edit { .. }));

    let info = svc.get_skill("demo").expect("get").expect("present");
    assert!(info.content.contains("revised"));
    assert_eq!(info.description, "revised");
}

#[test]
fn save_skill_rename_moves_directory_and_manifest() {
    let (dir, svc) = workspace();
    svc.create_skill("demo", SAMPLE_SKILL, None, false)
        .expect("create");

    let new_content = "---\nname: renamed\ndescription: now renamed\n---\n";
    let outcome = svc
        .save_skill("demo", new_content, Some("renamed"), None, false)
        .expect("save rename");
    assert!(matches!(outcome, SaveOutcome::Rename { .. }));

    let manifest = read_manifest(&dir);
    let skills = manifest.get("skills").and_then(Value::as_object).unwrap();
    assert!(skills.contains_key("renamed"));
    assert!(!skills.contains_key("demo"));

    let skills_root = dir.path().join("skills");
    assert!(skills_root.join("renamed").exists());
    assert!(!skills_root.join("demo").exists());
}

#[test]
fn save_skill_rename_conflict_returns_suggestion() {
    let (_dir, svc) = workspace();
    svc.create_skill("demo", SAMPLE_SKILL, None, false)
        .expect("create");
    svc.create_skill("other", SAMPLE_SKILL, None, false)
        .expect("create other");

    let outcome = svc
        .save_skill("demo", SAMPLE_SKILL, Some("other"), None, false)
        .expect("save");
    match outcome {
        SaveOutcome::Conflict { suggested_name } => {
            assert!(suggested_name.starts_with("other-"));
        }
        other => panic!("expected Conflict, got {other:?}"),
    }
}

#[test]
fn set_channels_validates_channels() {
    let (_dir, svc) = workspace();
    svc.create_skill("demo", SAMPLE_SKILL, None, false)
        .expect("create");

    let updated = svc
        .set_skill_channels("demo", vec!["console".to_string(), "discord".to_string()])
        .expect("set valid");
    assert!(updated);

    svc.set_skill_channels("demo", Vec::new()).expect("empty");

    let err = svc
        .set_skill_channels("demo", vec!["bogus-channel".to_string()])
        .unwrap_err();
    assert!(matches!(err, SkillError::InvalidSkillName(_)));
}

#[test]
fn set_config_persists_into_manifest() {
    let (dir, svc) = workspace();
    svc.create_skill("demo", SAMPLE_SKILL, None, false)
        .expect("create");

    let mut config = Map::new();
    config.insert("api_key".to_string(), Value::String("xyz".to_string()));
    let updated = svc.set_skill_config("demo", config).expect("set");
    assert!(updated);

    let manifest = read_manifest(&dir);
    let api_key = manifest
        .pointer("/skills/demo/config/api_key")
        .and_then(Value::as_str);
    assert_eq!(api_key, Some("xyz"));
}

#[test]
fn import_zip_lands_skill_and_manifest() {
    let (dir, svc) = workspace();
    let zip_bytes = make_skill_zip(&[
        ("alpha/SKILL.md", make_skill_md("alpha", "alpha skill")),
        ("alpha/scripts/run.sh", "echo alpha".to_string()),
        ("beta/SKILL.md", make_skill_md("beta", "beta skill")),
    ]);

    let outcome = svc.import_from_zip(&zip_bytes, true).expect("import");
    assert!(outcome.conflicts.is_empty(), "no conflicts expected");
    let mut imported = outcome.imported.clone();
    imported.sort();
    assert_eq!(imported, vec!["alpha".to_string(), "beta".to_string()]);
    assert!(outcome.enabled);

    let manifest = read_manifest(&dir);
    let skills = manifest.get("skills").and_then(Value::as_object).unwrap();
    assert!(skills.contains_key("alpha"));
    assert!(skills.contains_key("beta"));
    assert_eq!(
        skills
            .get("alpha")
            .and_then(|e| e.get("enabled"))
            .and_then(Value::as_bool),
        Some(true)
    );

    let manifest = reconcile_workspace_manifest(dir.path()).expect("reconcile");
    assert!(manifest.skills.contains_key("alpha"));
    assert!(manifest.skills.contains_key("beta"));
}

#[test]
fn import_zip_conflict_aborts_without_writing() {
    let (dir, svc) = workspace();
    svc.create_skill("alpha", SAMPLE_SKILL, None, false)
        .expect("create alpha");

    let zip_bytes =
        make_skill_zip(&[("alpha/SKILL.md", make_skill_md("alpha", "should conflict"))]);
    let outcome = svc.import_from_zip(&zip_bytes, false).expect("import");
    assert!(outcome.imported.is_empty());
    assert_eq!(outcome.conflicts.len(), 1);
    assert_eq!(outcome.conflicts[0].skill_name, "alpha");
    assert!(outcome.conflicts[0].suggested_name.starts_with("alpha-"));

    assert!(dir.path().join("skills/alpha/SKILL.md").exists());
}

#[test]
fn import_zip_supports_target_name_and_explicit_overwrite() {
    let (dir, svc) = workspace();
    let first = make_skill_zip(&[("SKILL.md", make_skill_md("demo", "first"))]);
    let outcome = svc
        .import_from_zip_with_options(
            &first,
            ImportOptions {
                enable: true,
                target_name: Some("renamed".to_string()),
                rename_map: BTreeMap::new(),
                overwrite: false,
            },
        )
        .expect("initial import");
    assert_eq!(outcome.imported, vec!["renamed"]);

    let replacement = make_skill_zip(&[("SKILL.md", make_skill_md("demo", "replacement"))]);
    let outcome = svc
        .import_from_zip_with_options(
            &replacement,
            ImportOptions {
                enable: true,
                target_name: Some("renamed".to_string()),
                rename_map: BTreeMap::new(),
                overwrite: true,
            },
        )
        .expect("overwrite import");
    assert_eq!(outcome.imported, vec!["renamed"]);
    let content = std::fs::read_to_string(dir.path().join("skills/renamed/SKILL.md"))
        .expect("read replacement");
    assert!(content.contains("replacement"));
}

#[test]
fn export_zip_skips_artifacts() {
    let (dir, svc) = workspace();
    svc.create_skill("demo", SAMPLE_SKILL, None, false)
        .expect("create");

    let cache = dir.path().join("skills/demo/__pycache__");
    std::fs::create_dir_all(&cache).expect("mkdir cache");
    std::fs::write(cache.join("noise.pyc"), b"junk").expect("write cache");
    let mac = dir.path().join("skills/demo/.DS_Store");
    std::fs::write(&mac, b"meta").expect("write meta");

    let zip_bytes = svc.export_to_zip(&["demo"]).expect("export");
    let mut archive = zip::ZipArchive::new(Cursor::new(&zip_bytes)).expect("zip");
    let mut entries = Vec::new();
    for i in 0..archive.len() {
        let f = archive.by_index(i).expect("entry");
        entries.push(f.name().to_string());
    }
    assert!(entries.iter().any(|n| n == "demo/SKILL.md"));
    assert!(
        !entries.iter().any(|n| n.contains("__pycache__")),
        "exported zip leaked __pycache__: {entries:?}"
    );
    assert!(
        !entries.iter().any(|n| n.ends_with(".DS_Store")),
        "exported zip leaked .DS_Store: {entries:?}"
    );
}

#[test]
fn export_zip_unknown_skill_is_error() {
    let (_dir, svc) = workspace();
    let err = svc.export_to_zip(&["nope"]).unwrap_err();
    assert!(matches!(err, SkillError::InvalidSkillName(_)));
}

#[test]
fn enable_unknown_skill_is_noop() {
    let (_dir, svc) = workspace();
    let updated = svc.enable_skill("ghost").expect("enable ghost");
    assert!(!updated);
}

fn make_skill_md(name: &str, description: &str) -> String {
    format!("---\nname: {name}\ndescription: {description}\n---\n\nbody for {name}\n")
}

fn make_skill_zip(files: &[(&str, String)]) -> Vec<u8> {
    let buf: Vec<u8> = Vec::new();
    let mut writer = ZipWriter::new(Cursor::new(buf));
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let mut dirs = HashMap::new();
    for (path, content) in files {
        if let Some((dir, _)) = path.split_once('/') {
            dirs.insert(dir.to_string(), ());
        }
        writer.start_file(*path, opts).expect("start");
        writer.write_all(content.as_bytes()).expect("write");
    }
    let _ = dirs;
    let cursor = writer.finish().expect("finish");
    cursor.into_inner()
}
