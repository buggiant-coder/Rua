use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use rua_skills::{
    build_signature, reconcile_pool_manifest_with, reconcile_workspace_manifest,
    reconcile_workspace_manifest_with, SkillSource, POOL_SCHEMA_VERSION, WORKSPACE_SCHEMA_VERSION,
};
use serde_json::Value;
use tempfile::TempDir;

fn write_skill_md(dir: &Path, name: &str, description: &str) {
    fs::create_dir_all(dir).unwrap();
    let body = format!("---\nname: {name}\ndescription: {description}\n---\n# {name}\n");
    fs::write(dir.join("SKILL.md"), body).unwrap();
}

fn read_manifest_value(path: &Path) -> Value {
    let bytes = fs::read(path).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[test]
fn workspace_first_init_creates_default_manifest_and_dirs() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().join("ws-001");

    let manifest = reconcile_workspace_manifest(&workspace).unwrap();

    assert!(
        workspace.join("skill.json").exists(),
        "skill.json must be written"
    );
    assert!(
        workspace.join("skills").is_dir(),
        "skills/ directory must be created"
    );
    assert_eq!(manifest.schema_version, WORKSPACE_SCHEMA_VERSION);
    assert!(manifest.skills.is_empty());
    assert!(
        manifest.version > 0,
        "version must be bumped above the default zero"
    );
}

#[test]
fn workspace_reconcile_discovers_disk_skills() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    let skills = workspace.join("skills");
    write_skill_md(&skills.join("alpha"), "alpha", "Alpha skill");
    write_skill_md(&skills.join("beta"), "beta", "Beta skill");

    let manifest = reconcile_workspace_manifest(&workspace).unwrap();

    assert_eq!(manifest.skills.len(), 2);
    let alpha = manifest.skills.get("alpha").expect("alpha entry");
    assert!(!alpha.enabled);
    assert_eq!(alpha.channels, vec!["all".to_string()]);
    assert_eq!(alpha.metadata.description, "Alpha skill");
    assert_eq!(alpha.metadata.name, "alpha");
}

#[test]
fn workspace_reconcile_preserves_enabled_channels_and_config() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    let skills = workspace.join("skills");
    write_skill_md(&skills.join("alpha"), "alpha", "Alpha");
    reconcile_workspace_manifest(&workspace).unwrap();

    let mut value = read_manifest_value(&workspace.join("skill.json"));
    {
        let alpha = value
            .pointer_mut("/skills/alpha")
            .expect("alpha entry exists")
            .as_object_mut()
            .unwrap();
        alpha.insert("enabled".to_string(), Value::Bool(true));
        alpha.insert("channels".to_string(), serde_json::json!(["telegram"]));
        alpha.insert("config".to_string(), serde_json::json!({"token": "secret"}));
    }
    fs::write(
        workspace.join("skill.json"),
        serde_json::to_string_pretty(&value).unwrap(),
    )
    .unwrap();

    write_skill_md(&skills.join("beta"), "beta", "Beta");
    let manifest = reconcile_workspace_manifest(&workspace).unwrap();

    let alpha = manifest.skills.get("alpha").unwrap();
    assert!(alpha.enabled, "enabled flag must survive reconcile");
    assert_eq!(alpha.channels, vec!["telegram".to_string()]);
    assert_eq!(
        alpha.config.get("token").and_then(|v| v.as_str()),
        Some("secret")
    );

    let beta = manifest.skills.get("beta").unwrap();
    assert!(!beta.enabled);
    assert_eq!(beta.channels, vec!["all".to_string()]);
}

#[test]
fn workspace_reconcile_removes_orphans() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    let skills = workspace.join("skills");
    write_skill_md(&skills.join("alpha"), "alpha", "Alpha");
    write_skill_md(&skills.join("beta"), "beta", "Beta");
    reconcile_workspace_manifest(&workspace).unwrap();

    fs::remove_dir_all(skills.join("beta")).unwrap();
    let manifest = reconcile_workspace_manifest(&workspace).unwrap();

    assert!(manifest.skills.contains_key("alpha"));
    assert!(
        !manifest.skills.contains_key("beta"),
        "orphan beta must be cleared"
    );
}

#[test]
fn workspace_reconcile_strips_legacy_sync_fields() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    let skills = workspace.join("skills");
    write_skill_md(&skills.join("alpha"), "alpha", "Alpha");
    reconcile_workspace_manifest(&workspace).unwrap();

    let mut value = read_manifest_value(&workspace.join("skill.json"));
    {
        let alpha = value
            .pointer_mut("/skills/alpha")
            .unwrap()
            .as_object_mut()
            .unwrap();
        alpha.insert("sync_to_hub".to_string(), Value::Bool(true));
        alpha.insert("sync_to_pool".to_string(), Value::Bool(false));
    }
    fs::write(
        workspace.join("skill.json"),
        serde_json::to_string_pretty(&value).unwrap(),
    )
    .unwrap();

    reconcile_workspace_manifest(&workspace).unwrap();

    let after = read_manifest_value(&workspace.join("skill.json"));
    let entry = after
        .pointer("/skills/alpha")
        .and_then(|v| v.as_object())
        .unwrap();
    assert!(
        !entry.contains_key("sync_to_hub"),
        "sync_to_hub must be stripped"
    );
    assert!(
        !entry.contains_key("sync_to_pool"),
        "sync_to_pool must be stripped"
    );
}

#[test]
fn workspace_reconcile_version_is_strictly_monotonic() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    write_skill_md(&workspace.join("skills").join("alpha"), "alpha", "A");

    let v1 = reconcile_workspace_manifest(&workspace).unwrap().version;
    let v2 = reconcile_workspace_manifest(&workspace).unwrap().version;
    let v3 = reconcile_workspace_manifest(&workspace).unwrap().version;

    assert!(v2 > v1, "v2={v2} must exceed v1={v1}");
    assert!(v3 > v2, "v3={v3} must exceed v2={v2}");
}

#[test]
fn workspace_reconcile_assigns_builtin_source_for_known_names() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    write_skill_md(&workspace.join("skills").join("docx"), "docx", "Docx skill");
    write_skill_md(
        &workspace.join("skills").join("custom"),
        "custom",
        "Custom skill",
    );

    let mut sigs = BTreeMap::new();
    sigs.insert("docx".to_string(), "deadbeef".to_string());

    let manifest = reconcile_workspace_manifest_with(&workspace, &sigs).unwrap();
    assert_eq!(manifest.skills["docx"].source, SkillSource::Builtin);
    assert_eq!(manifest.skills["custom"].source, SkillSource::Customized);
}

#[test]
fn workspace_reconcile_does_not_panic_on_invalid_skill_md() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();

    let broken_dir = workspace.join("skills").join("broken");
    fs::create_dir_all(&broken_dir).unwrap();
    fs::write(
        broken_dir.join("SKILL.md"),
        "---\n: not: valid\n  - foo\n---\nbody\n",
    )
    .unwrap();

    write_skill_md(&workspace.join("skills").join("good"), "good", "Good");

    let manifest = reconcile_workspace_manifest(&workspace).unwrap();
    assert!(manifest.skills.contains_key("broken"));
    assert!(manifest.skills.contains_key("good"));
    assert_eq!(manifest.skills["good"].metadata.description, "Good");

    assert!(manifest.skills["broken"].metadata.description.is_empty());
}

#[test]
fn workspace_reconcile_no_legacy_fields_in_default_init() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    write_skill_md(&workspace.join("skills").join("alpha"), "alpha", "Alpha");
    reconcile_workspace_manifest(&workspace).unwrap();

    let after = read_manifest_value(&workspace.join("skill.json"));
    let entry = after
        .pointer("/skills/alpha")
        .and_then(|v| v.as_object())
        .unwrap();
    assert!(!entry.contains_key("sync_to_hub"));
    assert!(!entry.contains_key("sync_to_pool"));
}

#[test]
fn pool_reconcile_first_init_creates_default_manifest() {
    let tmp = TempDir::new().unwrap();
    let pool = tmp.path().join("skill_pool");
    let sigs = BTreeMap::new();

    let manifest = reconcile_pool_manifest_with(&pool, &sigs).unwrap();

    assert!(pool.join("skill.json").exists());
    assert_eq!(manifest.schema_version, POOL_SCHEMA_VERSION);
    assert!(manifest.version > 0);
    assert!(manifest.skills.is_empty());
    assert!(manifest.builtin_skill_names.is_empty());
}

#[test]
fn pool_reconcile_records_builtin_names_and_classifies_source() {
    let tmp = TempDir::new().unwrap();
    let pool = tmp.path().to_path_buf();
    write_skill_md(&pool.join("docx"), "docx", "Docx");
    write_skill_md(&pool.join("custom"), "custom", "Custom");

    let docx_sig = build_signature(&pool.join("docx")).unwrap();
    let mut sigs = BTreeMap::new();
    sigs.insert("docx".to_string(), docx_sig);

    let manifest = reconcile_pool_manifest_with(&pool, &sigs).unwrap();

    assert_eq!(manifest.builtin_skill_names, vec!["docx".to_string()]);
    let docx = manifest.skills.get("docx").unwrap();
    assert_eq!(docx.source, SkillSource::Builtin);
    assert!(
        !docx.signature.is_empty(),
        "builtin pool entry must carry a non-empty signature"
    );

    let custom = manifest.skills.get("custom").unwrap();
    assert_eq!(custom.source, SkillSource::Customized);
    assert!(
        custom.signature.is_empty(),
        "customized pool entry must leave signature empty"
    );
}

#[test]
fn pool_reconcile_treats_drifted_builtin_first_seen_as_customized() {
    let tmp = TempDir::new().unwrap();
    let pool = tmp.path().to_path_buf();
    write_skill_md(&pool.join("docx"), "docx", "Drifted local copy");

    let mut sigs = BTreeMap::new();
    sigs.insert("docx".to_string(), "different-signature".to_string());

    let manifest = reconcile_pool_manifest_with(&pool, &sigs).unwrap();
    assert_eq!(manifest.skills["docx"].source, SkillSource::Customized);
    assert!(manifest.skills["docx"].signature.is_empty());
}

#[test]
fn pool_reconcile_preserves_builtin_classification_across_runs() {
    let tmp = TempDir::new().unwrap();
    let pool = tmp.path().to_path_buf();
    write_skill_md(&pool.join("docx"), "docx", "Docx");

    let docx_sig = build_signature(&pool.join("docx")).unwrap();
    let mut sigs = BTreeMap::new();
    sigs.insert("docx".to_string(), docx_sig);

    reconcile_pool_manifest_with(&pool, &sigs).unwrap();

    fs::write(
        pool.join("docx").join("SKILL.md"),
        "---\nname: docx\ndescription: drifted\n---\nedited\n",
    )
    .unwrap();
    let manifest = reconcile_pool_manifest_with(&pool, &sigs).unwrap();

    assert_eq!(manifest.skills["docx"].source, SkillSource::Builtin);
}

#[test]
fn pool_reconcile_removes_orphans() {
    let tmp = TempDir::new().unwrap();
    let pool = tmp.path().to_path_buf();
    write_skill_md(&pool.join("alpha"), "alpha", "Alpha");
    write_skill_md(&pool.join("beta"), "beta", "Beta");
    let sigs = BTreeMap::new();
    reconcile_pool_manifest_with(&pool, &sigs).unwrap();

    fs::remove_dir_all(pool.join("beta")).unwrap();
    let manifest = reconcile_pool_manifest_with(&pool, &sigs).unwrap();

    assert!(manifest.skills.contains_key("alpha"));
    assert!(!manifest.skills.contains_key("beta"));
}
