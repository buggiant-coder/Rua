use std::collections::BTreeMap;

use rua_skills::{
    reconcile_workspace_manifest, suggest_pool_conflict_name, InstallMeta, SkillBundleWrite,
    SkillPoolService, SkillService, SkillSource, SyncStatus,
};
use serde_json::{json, Map, Value};
use tempfile::TempDir;

const SAMPLE_SKILL: &str = "---
name: demo
description: demo skill body
tags: [demo]
emoji: \u{1f9ea}
---

# Demo body
";

const SAMPLE_SKILL_V2: &str = "---
name: demo
description: demo skill body — second revision
tags: [demo, v2]
emoji: \u{1f9ea}
---

# Demo body v2
";

fn pool() -> (TempDir, SkillPoolService) {
    let dir = tempfile::tempdir().expect("tempdir");
    let svc = SkillPoolService::new_in(dir.path().to_path_buf()).expect("service");
    (dir, svc)
}

fn read_pool_manifest(dir: &TempDir) -> Value {
    let bytes =
        std::fs::read(dir.path().join("skill.json")).expect("read pool skill.json after init");
    serde_json::from_slice(&bytes).expect("parse pool skill.json")
}

fn workspace() -> (TempDir, SkillService) {
    let dir = tempfile::tempdir().expect("tempdir");
    let svc = SkillService::new(dir.path().to_path_buf()).expect("workspace svc");
    (dir, svc)
}

#[test]
fn ensure_initialized_extracts_packaged_builtins() {
    let (dir, _svc) = pool();
    let manifest = read_pool_manifest(&dir);
    let names: Vec<String> = manifest
        .get("skills")
        .and_then(|v| v.as_object())
        .expect("skills map")
        .keys()
        .cloned()
        .collect();

    assert!(
        names.len() >= 8,
        "expected >= 8 builtin skills extracted, got {}: {:?}",
        names.len(),
        names
    );
    let cron = manifest
        .get("skills")
        .and_then(|v| v.get("cron"))
        .expect("cron entry should be present");
    assert_eq!(cron.get("source").and_then(|v| v.as_str()), Some("builtin"));
}

#[test]
fn create_skill_writes_pool_entry_and_returns_name() {
    let (dir, svc) = pool();
    let created = svc
        .create_skill("custom_one", SAMPLE_SKILL, None, SkillSource::Customized)
        .expect("create");
    assert_eq!(created.as_deref(), Some("custom_one"));
    assert!(dir.path().join("custom_one/SKILL.md").exists());
    let manifest = read_pool_manifest(&dir);
    let entry = manifest
        .get("skills")
        .and_then(|v| v.get("custom_one"))
        .expect("manifest entry");
    assert_eq!(
        entry.get("source").and_then(|v| v.as_str()),
        Some("customized")
    );
    assert_eq!(
        entry
            .pointer("/android_runtime/recommendation")
            .and_then(Value::as_str),
        Some("requires_linux_compat")
    );
}

#[test]
fn create_skill_persists_linux_compat_android_runtime_scan() {
    let (dir, svc) = pool();
    let content =
        "---\nname: linuxy\ndescription: needs apt\n---\n\nRun `apt install imagemagick`.\n";

    svc.create_skill("linuxy", content, None, SkillSource::Customized)
        .expect("create");

    let manifest = read_pool_manifest(&dir);
    let entry = manifest.pointer("/skills/linuxy").expect("manifest entry");
    assert_eq!(
        entry
            .pointer("/android_runtime/recommendation")
            .and_then(Value::as_str),
        Some("requires_linux_compat")
    );
    assert_eq!(
        entry
            .pointer("/android_runtime/signals/0/kind")
            .and_then(Value::as_str),
        Some("debian_package_manager")
    );
}

#[test]
fn create_skill_returns_none_on_dir_collision() {
    let (_dir, svc) = pool();
    svc.create_skill("dup", SAMPLE_SKILL, None, SkillSource::Customized)
        .expect("first");
    let second = svc
        .create_skill("dup", SAMPLE_SKILL, None, SkillSource::Customized)
        .expect("second");
    assert_eq!(second, None);
}

#[test]
fn delete_skill_removes_dir_and_manifest_entry() {
    let (dir, svc) = pool();
    svc.create_skill("transient", SAMPLE_SKILL, None, SkillSource::Customized)
        .expect("create");
    assert!(dir.path().join("transient/SKILL.md").exists());
    let removed = svc.delete_skill("transient").expect("delete");
    assert!(removed);
    assert!(!dir.path().join("transient").exists());
    let manifest = read_pool_manifest(&dir);
    assert!(manifest
        .get("skills")
        .and_then(|v| v.get("transient"))
        .is_none());
}

#[test]
fn upload_from_workspace_copies_to_pool() {
    let (pool_dir, pool_svc) = pool();
    let (ws_dir, ws_svc) = workspace();
    ws_svc
        .create_skill("ws_skill", SAMPLE_SKILL, None, true)
        .expect("create workspace skill");

    let outcome = pool_svc
        .upload_from_workspace(ws_dir.path(), "ws_skill", None, false)
        .expect("upload");
    assert_eq!(outcome.name, "ws_skill");
    assert_eq!(outcome.source, SkillSource::Customized);
    assert!(!outcome.signature.is_empty());
    assert!(pool_dir.path().join("ws_skill/SKILL.md").exists());
    let manifest = read_pool_manifest(&pool_dir);
    let entry = manifest
        .get("skills")
        .and_then(|v| v.get("ws_skill"))
        .expect("ws_skill in pool");
    assert_eq!(
        entry.get("source").and_then(|v| v.as_str()),
        Some("customized")
    );
}

#[test]
fn upload_same_name_as_builtin_is_conflict() {
    let (_pool_dir, pool_svc) = pool();
    let (ws_dir, ws_svc) = workspace();
    ws_svc
        .create_skill("cron", SAMPLE_SKILL, None, true)
        .expect("create workspace skill named cron");

    let result = pool_svc.upload_from_workspace(ws_dir.path(), "cron", None, false);
    let err = result.expect_err("upload should conflict with builtin cron");
    let msg = err.to_string();
    assert!(
        msg.contains("builtin"),
        "expected builtin conflict message, got: {msg}"
    );

    let renamed = pool_svc
        .upload_from_workspace(ws_dir.path(), "cron", Some("my_cron"), false)
        .expect("upload to free target name");
    assert_eq!(renamed.name, "my_cron");
}

#[test]
fn upload_overwrite_replaces_customized_entry() {
    let (_pool_dir, pool_svc) = pool();
    let (ws_dir, ws_svc) = workspace();
    ws_svc
        .create_skill("shared", SAMPLE_SKILL, None, true)
        .expect("workspace v1");
    pool_svc
        .upload_from_workspace(ws_dir.path(), "shared", None, false)
        .expect("first upload");

    std::fs::write(
        ws_dir.path().join("skills/shared/SKILL.md"),
        SAMPLE_SKILL_V2,
    )
    .expect("write v2");
    let outcome = pool_svc
        .upload_from_workspace(ws_dir.path(), "shared", None, true)
        .expect("overwrite");
    assert_eq!(outcome.name, "shared");

    let pool_md =
        std::fs::read_to_string(pool_svc.pool_dir().join("shared/SKILL.md")).expect("read v2");
    assert!(pool_md.contains("v2"));
}

#[test]
fn download_to_workspace_creates_skill_via_skill_service() {
    let (_pool_dir, pool_svc) = pool();
    pool_svc
        .create_skill("share_me", SAMPLE_SKILL, None, SkillSource::Customized)
        .expect("create pool skill");
    let (ws_dir, _ws_svc) = workspace();

    let outcome = pool_svc
        .download_to_workspace(ws_dir.path(), "share_me", None, false)
        .expect("download");
    assert_eq!(outcome.name, "share_me");
    assert_eq!(outcome.source, SkillSource::Customized);

    let workspace_md_path = ws_dir.path().join("skills/share_me/SKILL.md");
    assert!(workspace_md_path.exists());

    let manifest_path = ws_dir.path().join("skill.json");
    let manifest: Value = serde_json::from_slice(&std::fs::read(&manifest_path).expect("read"))
        .expect("parse workspace manifest");
    let entry = manifest
        .get("skills")
        .and_then(|v| v.get("share_me"))
        .expect("workspace entry");
    assert_eq!(
        entry.get("source").and_then(|v| v.as_str()),
        Some("customized")
    );
}

#[test]
fn download_to_workspace_preserves_pool_install_meta() {
    let (_pool_dir, pool_svc) = pool();
    let (ws_dir, _ws_svc) = workspace();

    let meta = InstallMeta {
        installed_from: "skillhub".to_string(),
        source_url: "https://example.test/bundle.zip".to_string(),
        installed_at: "2026-06-20T00:00:00Z".to_string(),
        updated_at: None,
        installed_version: "1.0.0".to_string(),
        content_hash: "abc".to_string(),
        market_meta: Some(json!({"name": "Market Skill"})),
    };
    pool_svc
        .create_skill_bundle(SkillBundleWrite {
            name: "hub_share",
            content: SAMPLE_SKILL,
            references: &Map::new(),
            scripts: &Map::new(),
            extra_files: &Map::new(),
            install_meta: Some(&meta),
        })
        .expect("create hub pool skill");
    pool_svc
        .download_to_workspace(ws_dir.path(), "hub_share", None, false)
        .expect("download hub skill");

    let manifest_path = ws_dir.path().join("skill.json");
    let manifest: Value = serde_json::from_slice(&std::fs::read(&manifest_path).expect("read"))
        .expect("parse workspace manifest");
    let hub_entry = manifest
        .get("skills")
        .and_then(|v| v.get("hub_share"))
        .expect("hub workspace entry");
    assert_eq!(
        hub_entry
            .get("install_meta")
            .and_then(|v| v.get("installed_from"))
            .and_then(|v| v.as_str()),
        Some("skillhub")
    );
}

#[test]
fn download_then_upload_round_trip_preserves_signature() {
    let (_pool_dir, pool_svc) = pool();
    pool_svc
        .create_skill("round_trip", SAMPLE_SKILL, None, SkillSource::Customized)
        .expect("create pool skill");
    let (ws_dir, _ws_svc) = workspace();

    let download = pool_svc
        .download_to_workspace(ws_dir.path(), "round_trip", None, false)
        .expect("download");

    let upload = pool_svc
        .upload_from_workspace(ws_dir.path(), "round_trip", Some("round_trip_copy"), false)
        .expect("upload back");

    assert_eq!(
        download.signature, upload.signature,
        "round-trip should preserve signatures"
    );
    let original = std::fs::read_to_string(pool_svc.pool_dir().join("round_trip/SKILL.md"))
        .expect("read original");
    let copy = std::fs::read_to_string(pool_svc.pool_dir().join("round_trip_copy/SKILL.md"))
        .expect("read copy");
    assert_eq!(original, copy);
}

#[test]
fn download_overwrite_replaces_existing_workspace_skill() {
    let (_pool_dir, pool_svc) = pool();
    pool_svc
        .create_skill("dl_overwrite", SAMPLE_SKILL, None, SkillSource::Customized)
        .expect("v1");
    let (ws_dir, _ws_svc) = workspace();

    pool_svc
        .download_to_workspace(ws_dir.path(), "dl_overwrite", None, false)
        .expect("first download");

    std::fs::write(
        pool_svc.pool_dir().join("dl_overwrite/SKILL.md"),
        SAMPLE_SKILL_V2,
    )
    .expect("write v2");

    pool_svc
        .download_to_workspace(ws_dir.path(), "dl_overwrite", None, true)
        .expect("download with overwrite");

    let ws_md = std::fs::read_to_string(ws_dir.path().join("skills/dl_overwrite/SKILL.md"))
        .expect("workspace v2");
    assert!(ws_md.contains("v2"));
}

#[test]
fn download_without_overwrite_blocks_existing_workspace_skill() {
    let (_pool_dir, pool_svc) = pool();
    pool_svc
        .create_skill("blocked", SAMPLE_SKILL, None, SkillSource::Customized)
        .expect("create");
    let (ws_dir, ws_svc) = workspace();
    ws_svc
        .create_skill("blocked", SAMPLE_SKILL_V2, None, false)
        .expect("workspace already has it");
    let err = pool_svc
        .download_to_workspace(ws_dir.path(), "blocked", None, false)
        .expect_err("should conflict");
    assert!(err.to_string().contains("already exists"));
}

#[test]
fn update_pool_skill_tags_keeps_builtin_source() {
    let (pool_dir, svc) = pool();
    svc.update_pool_skill_tags("cron", vec!["pinned".to_string(), "fav".to_string()])
        .expect("update tags");

    let manifest = read_pool_manifest(&pool_dir);
    let entry = manifest
        .get("skills")
        .and_then(|v| v.get("cron"))
        .expect("cron entry");
    let tags: Vec<String> = entry
        .get("tags")
        .and_then(|v| v.as_array())
        .expect("tags array")
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();
    assert_eq!(tags, vec!["pinned".to_string(), "fav".to_string()]);
    assert_eq!(
        entry.get("source").and_then(|v| v.as_str()),
        Some("builtin"),
        "tag updates must not flip a builtin to customized"
    );
}

#[test]
fn update_pool_skill_tags_unknown_skill_errors() {
    let (_dir, svc) = pool();
    let err = svc
        .update_pool_skill_tags("never_existed", vec!["x".to_string()])
        .expect_err("must error");
    assert!(err.to_string().contains("not found"));
}

#[test]
fn sync_status_covers_every_packaged_builtin() {
    let (_dir, svc) = pool();
    let map: BTreeMap<String, SyncStatus> =
        svc.get_pool_builtin_sync_status().expect("sync status");
    assert!(!map.is_empty(), "at least one builtin should be reported");

    assert!(
        map.values().all(|s| matches!(s, SyncStatus::Current)),
        "expected Current for all builtins post-init, got {:?}",
        map
    );
}

#[test]
fn sync_status_marks_outdated_after_signature_drift() {
    let (pool_dir, svc) = pool();

    let manifest_path = pool_dir.path().join("skill.json");
    let bytes = std::fs::read(&manifest_path).expect("read");
    let mut manifest: Value = serde_json::from_slice(&bytes).expect("parse");
    manifest["skills"]["cron"]["signature"] =
        Value::String("0000000000000000000000000000000000000000000000000000000000000000".into());
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .expect("write");

    let map = svc.get_pool_builtin_sync_status().expect("sync status");
    assert!(matches!(map.get("cron"), Some(SyncStatus::Outdated)));
}

#[test]
fn pool_builtin_notice_reports_no_changes_after_init() {
    let (_dir, svc) = pool();
    let notice = svc.get_pool_builtin_notice().expect("notice");
    assert!(
        !notice.has_updates,
        "post-init pool should match packaged builtins exactly: {notice:?}"
    );
    assert_eq!(notice.total_changes, 0);
    assert!(notice.added.is_empty());
    assert!(notice.missing.is_empty());
    assert!(notice.updated.is_empty());
    assert!(notice.removed.is_empty());
}

#[test]
fn pool_builtin_notice_marks_missing_after_delete() {
    let (_dir, svc) = pool();
    svc.delete_skill("cron").expect("delete cron");
    let notice = svc.get_pool_builtin_notice().expect("notice");
    assert!(notice.has_updates);
    let missing_names: Vec<String> = notice.missing.iter().map(|i| i.name.clone()).collect();
    assert!(
        missing_names.contains(&"cron".to_string()),
        "expected cron in missing list, got {missing_names:?}"
    );
    assert!(notice.actionable_skill_names.contains(&"cron".to_string()));
    assert!(!notice.fingerprint.is_empty());
}

#[test]
fn suggest_conflict_name_returns_first_free_index() {
    assert_eq!(suggest_pool_conflict_name("foo", &[]), "foo-1");
    assert_eq!(
        suggest_pool_conflict_name("foo", &["foo".into(), "foo-1".into()]),
        "foo-2"
    );
    assert_eq!(
        suggest_pool_conflict_name("foo", &["foo-1".into(), "foo-2".into(), "foo-3".into()]),
        "foo-4"
    );
}

#[test]
fn list_and_get_skills_match_after_create() {
    let (_dir, svc) = pool();
    svc.create_skill(
        "alpha",
        SAMPLE_SKILL,
        Some({
            let mut m = Map::new();
            m.insert("k".to_string(), json!("v"));
            m
        }),
        SkillSource::Customized,
    )
    .expect("create alpha");

    let list = svc.list_skills().expect("list");
    let names: Vec<String> = list.iter().map(|i| i.name.clone()).collect();
    assert!(names.contains(&"alpha".to_string()));
    let alpha = svc.get_skill("alpha").expect("get").expect("present");
    assert_eq!(alpha.source, SkillSource::Customized);
    assert!(alpha.content.contains("Demo body"));
}

#[test]
fn workspace_reconcile_after_download_keeps_skill_listed() {
    let (_pool_dir, pool_svc) = pool();
    pool_svc
        .create_skill(
            "after_reconcile",
            SAMPLE_SKILL,
            None,
            SkillSource::Customized,
        )
        .expect("create");
    let (ws_dir, _ws_svc) = workspace();
    pool_svc
        .download_to_workspace(ws_dir.path(), "after_reconcile", None, false)
        .expect("download");

    reconcile_workspace_manifest(ws_dir.path()).expect("reconcile");
    let manifest_path = ws_dir.path().join("skill.json");
    let manifest: Value = serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    assert!(manifest
        .get("skills")
        .and_then(|v| v.get("after_reconcile"))
        .is_some());
}
