use rua_provider::idealab_vertex::apply_idealab_vertex_patches;
use serde_json::json;

#[test]
fn vertex_patches_strip_function_response_id() {
    let mut body = json!({
        "contents": [{
            "role": "user",
            "parts": [{
                "functionResponse": {
                    "name": "get_weather",
                    "id": "call_123",
                    "response": {"result": "sunny"}
                }
            }]
        }]
    });

    apply_idealab_vertex_patches(&mut body);

    let fr = &body["contents"][0]["parts"][0]["functionResponse"];
    assert!(fr.get("id").is_none());
    assert_eq!(fr["name"], "get_weather");
}

#[test]
fn vertex_patches_keep_valid_thought_signature() {
    let mut body = json!({
        "contents": [{
            "role": "model",
            "parts": [{"text": "thinking...", "thought": true, "thoughtSignature": "dGVzdA=="}]
        }]
    });

    apply_idealab_vertex_patches(&mut body);

    assert_eq!(
        body["contents"][0]["parts"][0]["thoughtSignature"],
        "dGVzdA=="
    );
}

#[test]
fn vertex_patches_keep_valid_function_call_thought_signature() {
    let mut body = json!({
        "contents": [{
            "role": "model",
            "parts": [{
                "functionCall": {
                    "name": "bash",
                    "args": {"command": "pwd"}
                },
                "thoughtSignature": "dGVzdC1zaWduYXR1cmU="
            }]
        }]
    });

    apply_idealab_vertex_patches(&mut body);

    assert_eq!(
        body["contents"][0]["parts"][0]["thoughtSignature"],
        "dGVzdC1zaWduYXR1cmU="
    );
}

#[test]
fn vertex_patches_remove_invalid_thought_signature() {
    let mut body = json!({
        "contents": [{
            "role": "model",
            "parts": [{"text": "thinking...", "thought": true, "thoughtSignature": "toolu_vrtx_01Fx"}]
        }]
    });

    apply_idealab_vertex_patches(&mut body);

    assert!(body["contents"][0]["parts"][0]
        .get("thoughtSignature")
        .is_none());
}

#[test]
fn vertex_patches_merge_adjacent_function_response_messages() {
    let mut body = json!({
        "contents": [
            {"role": "model", "parts": [{"text": "call tools"}]},
            {"role": "function", "parts": [{"functionResponse": {"name": "a", "response": {}}}]},
            {"role": "function", "parts": [{"functionResponse": {"name": "b", "response": {}}}]},
            {"role": "user", "parts": [{"text": "next question"}]}
        ]
    });

    apply_idealab_vertex_patches(&mut body);

    let contents = body["contents"].as_array().unwrap();
    assert_eq!(contents.len(), 3);
    assert_eq!(contents[1]["role"], "user");
    let merged_parts = contents[1]["parts"].as_array().unwrap();
    assert_eq!(merged_parts.len(), 2);
    assert_eq!(merged_parts[0]["functionResponse"]["name"], "a");
    assert_eq!(merged_parts[1]["functionResponse"]["name"], "b");
}

#[test]
fn vertex_patches_clean_null_function_call_fields() {
    let mut body = json!({
        "contents": [{
            "role": "model",
            "parts": [
                {"functionCall": {"name": "foo", "args": {}, "id": null}, "thoughtSignature": null},
                {"text": "hello"}
            ]
        }]
    });

    apply_idealab_vertex_patches(&mut body);

    let part = &body["contents"][0]["parts"][0];
    assert!(part["functionCall"].get("id").is_none());
    assert!(part.get("thoughtSignature").is_none());
}

#[test]
fn vertex_patches_strip_function_call_and_response_ids() {
    let mut body = json!({
        "contents": [
            {
                "role": "model",
                "parts": [{
                    "functionCall": {
                        "id": "provider-call-1",
                        "name": "bash",
                        "args": {"command": "pwd"}
                    }
                }]
            },
            {
                "role": "user",
                "parts": [{
                    "functionResponse": {
                        "id": "provider-call-1",
                        "name": "bash",
                        "response": {"result": "ok"}
                    }
                }]
            }
        ]
    });

    apply_idealab_vertex_patches(&mut body);

    assert!(body["contents"][0]["parts"][0]["functionCall"]
        .get("id")
        .is_none());
    assert!(body["contents"][1]["parts"][0]["functionResponse"]
        .get("id")
        .is_none());
}

#[test]
fn vertex_patches_strip_nullable_anyof_from_tool_schemas() {
    let mut body = json!({
        "tools": [{
            "functionDeclarations": [{
                "name": "test",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "count": {"anyOf": [{"type": "integer"}, {"type": "null"}]}
                    }
                }
            }]
        }]
    });

    apply_idealab_vertex_patches(&mut body);

    assert_eq!(
        body["tools"][0]["functionDeclarations"][0]["parameters"]["properties"]["count"],
        json!({"type": "integer"})
    );
}
