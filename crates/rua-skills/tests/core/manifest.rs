use std::collections::BTreeMap;

use rua_skills::{
    bump_manifest_version, next_manifest_version, write_json_atomic, write_manifest_atomic,
    PoolManifest, PoolSkillEntry, SkillMetadata, SkillRequirements, SkillSource, WorkspaceManifest,
    WorkspaceSkillEntry, POOL_SCHEMA_VERSION, WORKSPACE_SCHEMA_VERSION,
};
use serde_json::{json, Value};

#[test]
fn workspace_manifest_default_payload_matches_python_defaults() {
    let payload = WorkspaceManifest::default_payload();
    assert_eq!(
        payload.get("schema_version").unwrap().as_str().unwrap(),
        WORKSPACE_SCHEMA_VERSION
    );
    assert_eq!(payload.get("version").unwrap().as_u64().unwrap(), 0);
    assert!(payload
        .get("skills")
        .unwrap()
        .as_object()
        .unwrap()
        .is_empty());
}

#[test]
fn pool_manifest_default_payload_includes_builtin_names() {
    let payload = PoolManifest::default_payload();
    assert_eq!(
        payload.get("schema_version").unwrap().as_str().unwrap(),
        POOL_SCHEMA_VERSION
    );
    assert_eq!(payload.get("version").unwrap().as_u64().unwrap(), 0);
    assert!(payload
        .get("builtin_skill_names")
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
    assert!(payload
        .get("skills")
        .unwrap()
        .as_object()
        .unwrap()
        .is_empty());
}

#[test]
fn workspace_manifest_round_trip() {
    let mut entries = BTreeMap::new();
    entries.insert(
        "demo".into(),
        WorkspaceSkillEntry {
            enabled: true,
            channels: vec!["console".into(), "discord".into()],
            source: SkillSource::Customized,
            config: serde_json::Map::new(),
            metadata: SkillMetadata {
                name: "demo".into(),
                description: "d".into(),
                version_text: "1.0".into(),
                commit_text: "".into(),
                signature: "".into(),
                source: SkillSource::Customized,
                protected: false,
                requirements: SkillRequirements::default(),
                updated_at: "2026-05-10T00:00:00Z".into(),
                tags: vec![],
                emoji: "".into(),
                extra: serde_json::Map::new(),
            },
            requirements: SkillRequirements::default(),
            updated_at: "2026-05-10T00:00:00Z".into(),
            extra: serde_json::Map::new(),
        },
    );
    let m = WorkspaceManifest {
        schema_version: WORKSPACE_SCHEMA_VERSION.into(),
        version: 7,
        skills: entries,
        extra: serde_json::Map::new(),
    };
    let s = serde_json::to_string(&m).unwrap();
    let back: WorkspaceManifest = serde_json::from_str(&s).unwrap();
    assert_eq!(back.version, 7);
    assert_eq!(back.skills.len(), 1);
    assert!(back.skills.get("demo").unwrap().enabled);
    assert_eq!(
        back.skills.get("demo").unwrap().channels,
        vec!["console".to_string(), "discord".to_string()]
    );
}

#[test]
fn workspace_manifest_preserves_unknown_fields_via_extra() {
    let raw = r#"{
        "schema_version": "workspace-skill-manifest.v1",
        "version": 5,
        "skills": {},
        "future_field_xyz": {"a": 1}
    }"#;
    let m: WorkspaceManifest = serde_json::from_str(raw).unwrap();
    assert!(m.extra.contains_key("future_field_xyz"));
    let s = serde_json::to_value(&m).unwrap();
    assert_eq!(s["future_field_xyz"], json!({"a": 1}));
}

#[test]
fn pool_skill_entry_round_trip_with_signature_and_config() {
    let entry = PoolSkillEntry {
        name: "himalaya".into(),
        description: "d".into(),
        version_text: "1.1".into(),
        commit_text: "".into(),
        signature: "deadbeef".into(),
        source: SkillSource::Builtin,
        protected: true,
        requirements: SkillRequirements {
            require_bins: vec!["himalaya".into()],
            require_envs: vec!["IMAP_PASSWORD".into()],
        },
        updated_at: "2026-05-10T00:00:00Z".into(),
        tags: vec!["email".into()],
        emoji: "📧".into(),
        config: Some({
            let mut m = serde_json::Map::new();
            m.insert("IMAP_PASSWORD".into(), json!("secret"));
            m
        }),
        extra: serde_json::Map::new(),
    };
    let s = serde_json::to_string(&entry).unwrap();
    let back: PoolSkillEntry = serde_json::from_str(&s).unwrap();
    assert_eq!(back.signature, "deadbeef");
    assert_eq!(back.source, SkillSource::Builtin);
    assert!(back.protected);
    assert_eq!(
        back.config.unwrap().get("IMAP_PASSWORD").unwrap(),
        &json!("secret")
    );
}

#[test]
fn pool_skill_entry_accepts_python_payload_without_config_key() {
    let raw = r#"{
        "name": "demo",
        "description": "",
        "version_text": "",
        "commit_text": "",
        "signature": "",
        "source": "customized",
        "protected": false,
        "requirements": {"require_bins": [], "require_envs": []},
        "updated_at": "",
        "tags": [],
        "emoji": ""
    }"#;
    let entry: PoolSkillEntry = serde_json::from_str(raw).unwrap();
    assert_eq!(entry.name, "demo");
    assert!(entry.config.is_none());
}

#[test]
fn next_manifest_version_is_at_least_now_ms() {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let v = next_manifest_version(0);
    assert!(v >= now_ms);
}

#[test]
fn next_manifest_version_is_strictly_greater_than_prev() {
    let prev = u64::MAX - 1;
    let v = next_manifest_version(prev);

    assert!(v >= prev);
}

#[test]
fn bump_manifest_version_overwrites_in_place() {
    let mut payload = serde_json::Map::new();
    payload.insert("version".into(), json!(3u64));
    payload.insert(
        "schema_version".into(),
        json!("workspace-skill-manifest.v1"),
    );
    bump_manifest_version(&mut payload);
    let bumped = payload.get("version").unwrap().as_u64().unwrap();
    assert!(bumped > 3, "expected bump to be > 3, got {bumped}");
}

#[test]
fn bump_manifest_version_treats_missing_version_as_zero() {
    let mut payload = serde_json::Map::new();
    payload.insert(
        "schema_version".into(),
        json!("workspace-skill-manifest.v1"),
    );
    bump_manifest_version(&mut payload);
    assert!(payload.get("version").is_some());
}

#[test]
fn write_json_atomic_writes_pretty_json_and_leaves_no_tmp_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skill.json");
    let mut payload = WorkspaceManifest::default_payload();
    write_manifest_atomic(&path, &mut payload).unwrap();

    let content = std::fs::read_to_string(&path).unwrap();

    assert!(content.contains("\n  "), "expected pretty-printed JSON");
    assert!(content.ends_with('\n'), "expected trailing newline");

    let v: Value = serde_json::from_str(&content).unwrap();
    assert!(v["version"].as_u64().unwrap() > 0);

    let entries: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(
        entries,
        vec!["skill.json".to_string()],
        "expected only skill.json, got {:?}",
        entries
    );
}

#[test]
fn write_json_atomic_overwrites_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skill.json");
    write_json_atomic(&path, &json!({"a": 1})).unwrap();
    write_json_atomic(&path, &json!({"a": 2})).unwrap();
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["a"], 2);

    let entries: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(entries, vec!["skill.json".to_string()]);
}

#[test]
fn write_manifest_atomic_round_trip_preserves_structure() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspace_skill.json");
    let mut payload = WorkspaceManifest::default_payload();
    payload.insert("custom_field".into(), json!("survives"));
    write_manifest_atomic(&path, &mut payload).unwrap();

    let raw = std::fs::read_to_string(&path).unwrap();
    let v: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(v["schema_version"], WORKSPACE_SCHEMA_VERSION);
    assert!(v["version"].as_u64().unwrap() > 0);
    assert_eq!(v["custom_field"], "survives");
}
