use rua_skills::types::SkillSource;
use rua_skills::{build_plugin_skill_info, parse_plugin_skill_id, plugin_skill_id};

#[test]
fn plugin_skill_id_format() {
    assert_eq!(
        plugin_skill_id("my-plugin", "greeting"),
        "plugin/my-plugin/greeting"
    );
}

#[test]
fn parse_valid_plugin_skill_id() {
    let (pid, name) = parse_plugin_skill_id("plugin/my-plugin/greeting").unwrap();
    assert_eq!(pid, "my-plugin");
    assert_eq!(name, "greeting");
}

#[test]
fn parse_missing_prefix() {
    assert!(parse_plugin_skill_id("my-plugin/greeting").is_none());
}

#[test]
fn parse_empty_plugin_id() {
    assert!(parse_plugin_skill_id("plugin//greeting").is_none());
}

#[test]
fn parse_empty_skill_name() {
    assert!(parse_plugin_skill_id("plugin/my-plugin/").is_none());
}

#[test]
fn parse_no_slash_after_prefix() {
    assert!(parse_plugin_skill_id("plugin/").is_none());
}

#[test]
fn build_plugin_skill_info_fields() {
    let info = build_plugin_skill_info("p1", "search", "Search things", "body content");
    assert_eq!(info.name, "plugin/p1/search");
    assert_eq!(info.description, "Search things");
    assert_eq!(info.content, "body content");
    assert_eq!(info.source, SkillSource::Plugin("p1".to_string()));
    assert!(info.tags.contains(&"plugin".to_string()));
}
