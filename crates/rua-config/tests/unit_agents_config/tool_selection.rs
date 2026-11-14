use rua_config::types::tools::{ToolIdentity, ToolSelectionPolicy};
use serde_json::json;

#[test]
fn default_enabled_grows_but_restricted_keeps_exact_identities() {
    let added = ToolIdentity::builtin("new_product_tool");
    assert!(ToolSelectionPolicy::default_enabled().selects(&added, None));
    let restricted = ToolSelectionPolicy::restricted(vec![ToolIdentity::builtin("read")]);
    assert!(!restricted.selects(&added, None));
    assert!(restricted.selects(&ToolIdentity::builtin("read"), None));
    assert!(!restricted.selects(&ToolIdentity::mcp("external", "read"), None));
}

#[test]
fn disables_match_stable_identity_and_survive_unknown_fields() {
    let policy: ToolSelectionPolicy = serde_json::from_value(json!({
        "mode":"default_enabled",
        "disabled_tools":[{"kind":"mcp","source":"external","name":"click","future":true}],
        "disabled_capabilities":["phone_use"],
        "future_policy":{"version":2}
    }))
    .unwrap();
    policy.validate().unwrap();
    assert!(!policy.selects(&ToolIdentity::mcp("external", "click"), None));
    assert!(policy.selects(&ToolIdentity::mcp("another", "click"), None));
    assert!(!policy.selects(
        &ToolIdentity::builtin("new_phone_action"),
        Some("phone_use")
    ));
    assert!(serde_json::to_value(policy).unwrap()["future_policy"].is_object());
}

#[test]
fn incomplete_policy_is_not_an_empty_default_enabled_policy() {
    for value in [
        json!(null),
        json!({}),
        json!({"mode":"default_enabled"}),
        json!({"mode":"restricted"}),
        json!({"mode":"unknown"}),
    ] {
        assert!(serde_json::from_value::<ToolSelectionPolicy>(value).is_err());
    }
    assert!(!ToolSelectionPolicy::restricted(vec![]).selects(&ToolIdentity::builtin("read"), None));
}

#[test]
fn identity_validation_rejects_missing_or_inconsistent_sources() {
    for value in [
        json!({"kind":"mcp","name":"read"}),
        json!({"kind":"builtin","source":"external","name":"read"}),
        json!({"kind":"builtin","name":""}),
    ] {
        let identity: ToolIdentity = serde_json::from_value(value).unwrap();
        assert!(identity.validate().is_err());
    }
}

#[test]
fn conflicting_known_mode_fields_are_invalid_while_extensions_survive() {
    for value in [
        json!({"mode":"default_enabled","disabled_tools":[],"disabled_capabilities":[],"allowed_tools":[]}),
        json!({"mode":"restricted","allowed_tools":[],"disabled_tools":[]}),
    ] {
        let policy: ToolSelectionPolicy = serde_json::from_value(value).unwrap();
        assert!(policy.validate().is_err());
    }
}

#[test]
fn idempotent_tool_selection_preserves_identity_extensions() {
    let mut policy:ToolSelectionPolicy=serde_json::from_value(json!({"mode":"restricted","allowed_tools":[{"kind":"builtin","name":"read","future":true}]})).unwrap();
    policy.set_tool_enabled(ToolIdentity::builtin("read"), true);
    assert_eq!(
        serde_json::to_value(policy).unwrap()["allowed_tools"][0]["future"],
        true
    );
}
