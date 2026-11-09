use rua_skills::{SkillError, SkillPoolService, SkillSource};

const SKILL_A: &str = "---\nname: S1\ndescription: Skill one\n---\nBody A\n";
const SKILL_B: &str = "---\nname: S1\ndescription: Skill one variant\n---\nBody B\n";

fn pool() -> (tempfile::TempDir, SkillPoolService) {
    let dir = tempfile::TempDir::new().unwrap();
    let svc = SkillPoolService::new_in(dir.path()).unwrap();
    (dir, svc)
}

#[test]
fn plugin_source_serde_roundtrip() {
    let src = SkillSource::Plugin("my-plugin".to_string());
    let json = serde_json::to_string(&src).unwrap();
    assert_eq!(json, "\"plugin:my-plugin\"");
    let back: SkillSource = serde_json::from_str(&json).unwrap();
    assert_eq!(back, src);
}

#[test]
fn builtin_customized_serde_unchanged() {
    let b = serde_json::to_string(&SkillSource::Builtin).unwrap();
    assert_eq!(b, "\"builtin\"");
    let c = serde_json::to_string(&SkillSource::Customized).unwrap();
    assert_eq!(c, "\"customized\"");

    let b2: SkillSource = serde_json::from_str("\"builtin\"").unwrap();
    assert_eq!(b2, SkillSource::Builtin);
    let c2: SkillSource = serde_json::from_str("\"customized\"").unwrap();
    assert_eq!(c2, SkillSource::Customized);
}

#[test]
fn plugin_source_display() {
    assert_eq!(SkillSource::Plugin("foo".into()).to_string(), "plugin:foo");
    assert_eq!(SkillSource::Builtin.to_string(), "builtin");
    assert_eq!(SkillSource::Customized.to_string(), "customized");
}

#[test]
fn create_skill_with_plugin_source() {
    let (_dir, pool) = pool();
    let src = SkillSource::Plugin("plugA".to_string());
    let result = pool.create_skill("S1", SKILL_A, None, src.clone()).unwrap();
    assert_eq!(result, Some("S1".to_string()));

    let info = pool.get_skill("S1").unwrap().unwrap();
    assert_eq!(info.source, SkillSource::Plugin("plugA".to_string()));
}

#[test]
fn plugin_a_registers_then_customized_conflicts() {
    let (_dir, pool) = pool();
    pool.create_skill("S1", SKILL_A, None, SkillSource::Plugin("plugA".into()))
        .unwrap();

    let err = pool
        .create_skill("S1", SKILL_B, None, SkillSource::Customized)
        .unwrap_err();
    assert!(matches!(err, SkillError::SourceConflict { .. }));
}

#[test]
fn plugin_a_registers_then_same_plugin_idempotent() {
    let (_dir, pool) = pool();
    let src = SkillSource::Plugin("plugA".into());
    pool.create_skill("S1", SKILL_A, None, src.clone()).unwrap();

    let result = pool.create_skill("S1", SKILL_A, None, src).unwrap();
    assert_eq!(result, None);
}

#[test]
fn plugin_b_conflicts_with_plugin_a() {
    let (_dir, pool) = pool();
    pool.create_skill("S1", SKILL_A, None, SkillSource::Plugin("plugA".into()))
        .unwrap();

    let err = pool
        .create_skill("S1", SKILL_B, None, SkillSource::Plugin("plugB".into()))
        .unwrap_err();
    assert!(matches!(err, SkillError::SourceConflict { .. }));
}

#[test]
fn uninstall_only_removes_matching_source() {
    let (_dir, pool) = pool();
    pool.create_skill("S1", SKILL_A, None, SkillSource::Plugin("plugA".into()))
        .unwrap();
    pool.create_skill("S2", SKILL_B, None, SkillSource::Customized)
        .unwrap();

    let info = pool.get_skill("S1").unwrap().unwrap();
    assert_eq!(info.source, SkillSource::Plugin("plugA".into()));
    pool.delete_skill("S1").unwrap();
    assert!(pool.get_skill("S1").unwrap().is_none());

    assert!(pool.get_skill("S2").unwrap().is_some());
}

#[test]
fn customized_create_still_works() {
    let (_dir, pool) = pool();
    let result = pool
        .create_skill("myskill", SKILL_A, None, SkillSource::Customized)
        .unwrap();
    assert_eq!(result, Some("myskill".to_string()));

    let info = pool.get_skill("myskill").unwrap().unwrap();
    assert_eq!(info.source, SkillSource::Customized);
}

#[test]
fn manifest_roundtrip_plugin_source() {
    let (_dir, pool) = pool();
    pool.create_skill("S1", SKILL_A, None, SkillSource::Plugin("plugX".into()))
        .unwrap();

    let manifest_path = pool.pool_dir().join("skill.json");
    let content = std::fs::read_to_string(&manifest_path).unwrap();
    let val: serde_json::Value = serde_json::from_str(&content).unwrap();
    let source_val = val["skills"]["S1"]["source"].as_str().unwrap();
    assert_eq!(source_val, "plugin:plugX");
}

#[test]
fn deserialize_unknown_source_errors() {
    let err = serde_json::from_str::<SkillSource>("\"unknown_thing\"");
    assert!(err.is_err());
}

#[test]
fn deserialize_empty_plugin_id_errors() {
    let err = serde_json::from_str::<SkillSource>("\"plugin:\"");
    assert!(err.is_err());
}
