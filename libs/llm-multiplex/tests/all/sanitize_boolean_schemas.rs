use llm_multiplex::request::sanitize_boolean_schemas;
use serde_json::json;

#[test]
fn converts_additional_properties_true_to_empty_object() {
    let mut schema = json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"}
        },
        "additionalProperties": true
    });
    sanitize_boolean_schemas(&mut schema);
    assert_eq!(schema["additionalProperties"], json!({}));
}

#[test]
fn preserves_additional_properties_false() {
    let mut schema = json!({
        "type": "object",
        "additionalProperties": false
    });
    sanitize_boolean_schemas(&mut schema);
    assert_eq!(schema["additionalProperties"], json!(false));
}

#[test]
fn preserves_additional_properties_object() {
    let mut schema = json!({
        "type": "object",
        "additionalProperties": {"type": "string"}
    });
    sanitize_boolean_schemas(&mut schema);
    assert_eq!(schema["additionalProperties"], json!({"type": "string"}));
}

#[test]
fn converts_items_true_to_empty_object() {
    let mut schema = json!({
        "type": "array",
        "items": true
    });
    sanitize_boolean_schemas(&mut schema);
    assert_eq!(schema["items"], json!({}));
}

#[test]
fn preserves_items_object() {
    let mut schema = json!({
        "type": "array",
        "items": {"type": "number"}
    });
    sanitize_boolean_schemas(&mut schema);
    assert_eq!(schema["items"], json!({"type": "number"}));
}

#[test]
fn removes_boolean_required() {
    let mut schema = json!({
        "type": "object",
        "properties": {
            "x": {"type": "string", "required": true}
        },
        "required": true
    });
    sanitize_boolean_schemas(&mut schema);
    assert!(schema.get("required").is_none());
    assert!(schema["properties"]["x"].get("required").is_none());
}

#[test]
fn preserves_array_required() {
    let mut schema = json!({
        "type": "object",
        "properties": {
            "x": {"type": "string"}
        },
        "required": ["x"]
    });
    sanitize_boolean_schemas(&mut schema);
    assert_eq!(schema["required"], json!(["x"]));
}

#[test]
fn handles_deeply_nested_schemas() {
    let mut schema = json!({
        "type": "object",
        "properties": {
            "config": {
                "type": "object",
                "properties": {
                    "items": {
                        "type": "array",
                        "items": true
                    }
                },
                "additionalProperties": true
            }
        }
    });
    sanitize_boolean_schemas(&mut schema);
    assert_eq!(
        schema["properties"]["config"]["additionalProperties"],
        json!({})
    );
    assert_eq!(
        schema["properties"]["config"]["properties"]["items"]["items"],
        json!({})
    );
}

#[test]
fn handles_null_and_primitives_without_panic() {
    let mut schema = json!(null);
    sanitize_boolean_schemas(&mut schema);
    assert_eq!(schema, json!(null));

    let mut schema = json!("string");
    sanitize_boolean_schemas(&mut schema);
    assert_eq!(schema, json!("string"));

    let mut schema = json!(42);
    sanitize_boolean_schemas(&mut schema);
    assert_eq!(schema, json!(42));
}
