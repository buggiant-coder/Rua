use rua_skills::{SkillInfo, SkillRequirements, SkillSource};

#[test]
fn skill_source_serializes_to_lowercase_strings() {
    assert_eq!(
        serde_json::to_value(SkillSource::Builtin).unwrap(),
        serde_json::Value::String("builtin".to_string())
    );
    assert_eq!(
        serde_json::to_value(SkillSource::Customized).unwrap(),
        serde_json::Value::String("customized".to_string())
    );
}

#[test]
fn skill_source_deserializes_from_python_strings() {
    let v: SkillSource = serde_json::from_str("\"builtin\"").unwrap();
    assert_eq!(v, SkillSource::Builtin);
    let v: SkillSource = serde_json::from_str("\"customized\"").unwrap();
    assert_eq!(v, SkillSource::Customized);
}

#[test]
fn skill_source_default_is_customized() {
    assert_eq!(SkillSource::default(), SkillSource::Customized);
    assert_eq!(SkillSource::Builtin.as_str(), "builtin");
    assert_eq!(SkillSource::Customized.as_str(), "customized");
}

#[test]
fn skill_requirements_default_is_empty() {
    let req = SkillRequirements::default();
    assert!(req.require_bins.is_empty());
    assert!(req.require_envs.is_empty());
}

#[test]
fn skill_requirements_round_trip() {
    let req = SkillRequirements {
        require_bins: vec!["git".into(), "node".into()],
        require_envs: vec!["GITHUB_TOKEN".into()],
    };
    let json = serde_json::to_string(&req).unwrap();
    let back: SkillRequirements = serde_json::from_str(&json).unwrap();
    assert_eq!(back, req);
}

#[test]
fn skill_requirements_deserializes_with_missing_fields() {
    let req: SkillRequirements = serde_json::from_str("{}").unwrap();
    assert!(req.require_bins.is_empty());
    assert!(req.require_envs.is_empty());

    let req: SkillRequirements = serde_json::from_str(r#"{"require_bins": ["bash"]}"#).unwrap();
    assert_eq!(req.require_bins, vec!["bash"]);
    assert!(req.require_envs.is_empty());
}

#[test]
fn skill_info_serializes_with_all_fields() {
    let info = SkillInfo {
        name: "demo".into(),
        description: "d".into(),
        version_text: "1.0".into(),
        content: "body".into(),
        source: SkillSource::Builtin,
        references: serde_json::json!({}),
        scripts: serde_json::json!({}),
        tags: vec!["foo".into()],
        emoji: "🌟".into(),
    };
    let v = serde_json::to_value(&info).unwrap();
    assert_eq!(v["name"], "demo");
    assert_eq!(v["source"], "builtin");
    assert_eq!(v["tags"], serde_json::json!(["foo"]));
    assert_eq!(v["emoji"], "🌟");
}
