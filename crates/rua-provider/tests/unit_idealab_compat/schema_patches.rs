use rua_provider::idealab::strip_anyof_null;
use serde_json::json;

#[test]
fn strip_anyof_null_simplifies_single_nullable_branch() {
    let mut schema = json!({
        "anyOf": [{"type": "integer"}, {"type": "null"}]
    });

    strip_anyof_null(&mut schema);

    assert_eq!(schema, json!({"type": "integer"}));
}

#[test]
fn strip_anyof_null_recurses_into_properties() {
    let mut schema = json!({
        "type": "object",
        "properties": {
            "count": {
                "anyOf": [{"type": "integer"}, {"type": "null"}]
            },
            "name": {"type": "string"}
        }
    });

    strip_anyof_null(&mut schema);

    assert_eq!(schema["properties"]["count"], json!({"type": "integer"}));
    assert_eq!(schema["properties"]["name"], json!({"type": "string"}));
}

#[test]
fn strip_anyof_null_recurses_into_array_items() {
    let mut schema = json!({
        "type": "array",
        "items": {
            "anyOf": [{"type": "string"}, {"type": "null"}]
        }
    });

    strip_anyof_null(&mut schema);

    assert_eq!(schema["items"], json!({"type": "string"}));
}
