use rua_config::ToolGuardConfig;
use serde_json::json;

#[test]
fn removed_tool_list_fields_are_ignored_by_tool_guard_config() {
    let missing: ToolGuardConfig =
        serde_json::from_value(json!({})).expect("missing tool lists should deserialize");
    assert!(missing.extra.is_empty());

    let with_removed_fields: ToolGuardConfig = serde_json::from_value(json!({
        "guarded_tools": ["bash", "read"],
        "denied_tools": ["edit"],
        "custom_rules": [],
        "disabled_rules": []
    }))
    .expect("removed tool-list fields should be ignored by serde");
    assert!(with_removed_fields.custom_rules.is_empty());
    assert!(with_removed_fields.disabled_rules.is_empty());
    assert_eq!(
        with_removed_fields.extra["guarded_tools"],
        json!(["bash", "read"])
    );
    assert_eq!(with_removed_fields.extra["denied_tools"], json!(["edit"]));
}
