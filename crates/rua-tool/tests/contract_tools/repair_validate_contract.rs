use rua_tool::{repair_and_validate, ToolError};
use serde_json::json;

#[test]
fn empty_raw_args_are_repaired_to_empty_object_when_schema_allows_it() {
    let schema = json!({
        "type": "object",
        "properties": {}
    });

    let (value, outcome) = repair_and_validate("noop", "", &schema).expect("empty args allowed");

    assert_eq!(value, json!({}));
    assert_eq!(outcome.value, json!({}));
}

#[test]
fn repaired_json_still_must_satisfy_required_fields() {
    let schema = json!({
        "type": "object",
        "properties": {
            "file_path": {"type": "string"}
        },
        "required": ["file_path"]
    });

    let err = repair_and_validate("read", "{}", &schema).expect_err("file_path is required");

    assert!(matches!(err, ToolError::SchemaValidation { .. }));
    assert!(
        err.to_string()
            .contains("missing required field: file_path"),
        "unexpected error: {err}"
    );
}

#[test]
fn non_object_args_are_rejected_for_object_schema() {
    let schema = json!({
        "type": "object",
        "properties": {}
    });

    let err = repair_and_validate("read", r#""not an object""#, &schema)
        .expect_err("object schema rejects string args");

    assert!(matches!(err, ToolError::SchemaValidation { .. }));
    assert!(
        err.to_string().contains("expected object, got string"),
        "unexpected error: {err}"
    );
}

#[test]
fn property_types_are_validated_recursively() {
    let schema = json!({
        "type": "object",
        "properties": {
            "config": {
                "type": "object",
                "properties": {"enabled": {"type": "boolean"}},
                "required": ["enabled"]
            }
        },
        "required": ["config"]
    });

    let err = repair_and_validate("configure", r#"{"config":{"enabled":"yes"}}"#, &schema)
        .expect_err("nested property type must be enforced");

    assert!(err.to_string().contains("$.config.enabled"));
    assert!(err.to_string().contains("expected boolean, got string"));
}

#[test]
fn additional_properties_false_rejects_unknown_fields() {
    let schema = json!({
        "type": "object",
        "properties": {"value": {"type": "string"}},
        "additionalProperties": false
    });

    let err = repair_and_validate("echo", r#"{"value":"ok","extra":1}"#, &schema)
        .expect_err("unknown field must be rejected");

    assert!(err.to_string().contains("unknown field at $.extra"));
}
